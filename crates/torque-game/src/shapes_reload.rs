//! A server's shape reload: the kept shapes follow their files while the server runs.
//!
//! **The model.** A development server ([`poll`] is called with its gate on: a debug build, or `$Pref::Dev` true, the
//! asset watcher's gate) looks at the change time of every kept shape's file once per interval (the asset watcher's, passed in
//! by the caller), on the simulation clock, so a test's ticks are its time. A file whose change time moved since it was read
//! is read and parsed again: the new [`KeptShape`] replaces the kept entry, every `ShapeBaseData` whose `shapeFile` is that
//! path takes it as its resource and is marked [preloaded](crate::scene::refresh::mark_preloaded), and the tick's drain
//! then runs the heavy refresh for the datablock's users (a new instance, object box and hulls, script threads kept). A file
//! that no longer parses, is gone, or is too large to read and parse within a reload's budget keeps the old shape, prints one
//! error line, and is tried again only when its change time moves again (the asset watcher's rule). An entry no datablock holds a resource for (an image's, an explosion's, a tire's shape, a
//! constructor's base) is replaced in the kept set too, without a mark: nothing is built from it until its next load.
//!
//! **What is never done.** With the gate off nothing reads a change time: the poll returns at the gate, and a load records
//! its file's time only after a poll ran with the gate on ([`Shapes::watching`](super::Shapes)). A release server with
//! `$Pref::Dev` unset does none of this work; a client never calls it.
//!
//! **Limits.** A poll reads at most one change time per kept shape ([`MAX_KEPT_SHAPES`](super::MAX_KEPT_SHAPES)), so the
//! watched table is bounded by the kept set (an entry whose shape was purged is dropped at the next poll); a poll reloads at
//! most [`MAX_RELOADS_PER_POLL`] files (the rest are seen changed again at the next poll), each under its own host work
//! budget of one tick's pass ([`RELOAD_WORK`]): a file the budget cannot pay for is given up until it changes again, with one
//! line. A reload is charged what a load is (the file's bytes, the parse, the facts). A
//! `TSShapeConstructor`'s imported sequences are not imported again into a reloaded base shape: declare the constructor again.

use std::collections::HashMap;
use std::rc::Rc;

use ts_vm::sim::ObjId;
use ts_vm::text::fold;
use ts_vm::vm::{Vm, VmError};

use super::{FileBytes, KeptShape, Loaded, Shapes, shapes_of};
use crate::scene::{ext, refresh};

/// The host work budget of one file's reload, in the units of `Vm::charge_work`: the work of one tick's pass (a `.dts` of a
/// few MB; a larger one is not reloaded live).
pub const RELOAD_WORK: u64 = crate::sim::pass::TICK_PASS_WORK;

/// The most files one poll reloads; the rest wait for the next poll.
pub const MAX_RELOADS_PER_POLL: usize = 64;

/// A watched shape file: how the loader spelled its path and the change time the file had when it was read (or looked at
/// first).
#[derive(Debug)]
pub(super) struct Watched {
    path: Vec<u8>,
    modified: Option<u64>,
}

/// What the polls did so far (the tests read them).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Polls that passed the gate and the interval.
    pub polls: u64,
    /// Change times read (at the polls, and at the loads while watching).
    pub looks: u64,
    /// Shapes read again and replaced.
    pub reloads: u64,
    /// Files whose new contents did not load (the old shape kept).
    pub failed: u64,
    /// Datablocks marked preloaded for a replaced shape.
    pub marked: u64,
}

/// A kept shape whose file changed: its key, its path and the change time now.
struct Changed {
    key: Vec<u8>,
    path: Vec<u8>,
    modified: Option<u64>,
}

impl Shapes {
    /// Turns the polls on or off whatever the gate the caller passes; `None` goes back to the gate.
    pub fn set_hot_reload(&self, on: Option<bool>) {
        self.hot_reload.set(on);
    }

    /// Whether a poll runs, for a caller whose gate says `gate`.
    pub fn hot_reload_on(&self, gate: bool) -> bool {
        self.hot_reload.get().unwrap_or(gate)
    }

    /// What the polls did so far.
    pub fn reload_stats(&self) -> Stats {
        self.reload_stats.get()
    }

    /// Lowers the work one file's reload may take (default [`RELOAD_WORK`]): the tests' way to have a shape too large to reload.
    pub fn set_reload_work(&self, units: u64) {
        self.reload_work.set(units);
    }

    fn count(&self, f: impl FnOnce(&mut Stats)) {
        let mut stats = self.reload_stats.get();
        f(&mut stats);
        self.reload_stats.set(stats);
    }

    /// Records the change time of the file a load kept under `key`.
    pub(super) fn watch_file(&self, vm: &Vm, key: Vec<u8>, path: &[u8]) {
        let modified = vm.file_host().modified(path);
        self.count(|s| s.looks += 1);
        self.watched.borrow_mut().insert(
            key,
            Watched {
                path: path.to_vec(),
                modified,
            },
        );
    }

    /// One look at the kept shapes' files: the ones whose change time moved, in key order. A kept shape not watched yet is
    /// watched from now (its time now is the baseline); a watched entry whose shape is no longer kept is dropped.
    fn look(&self, vm: &Vm) -> Vec<Changed> {
        let files = vm.file_host();
        let kept = self.kept.borrow();
        let mut watched = self.watched.borrow_mut();
        watched.retain(|key, _| kept.contains_key(key));
        let mut changed = Vec::new();
        let mut keys: Vec<&[u8]> = kept.keys().map(Vec::as_slice).collect();
        keys.sort_unstable();
        for key in keys {
            match watched.get(key) {
                Some(entry) => {
                    let modified = files.modified(&entry.path);
                    if modified != entry.modified {
                        changed.push(Changed {
                            key: key.to_vec(),
                            path: entry.path.clone(),
                            modified,
                        });
                    }
                }
                // The kept key is the path folded, which the hosts find (they match ignoring case).
                None => {
                    watched.insert(
                        key.to_vec(),
                        Watched {
                            path: key.to_vec(),
                            modified: files.modified(key),
                        },
                    );
                }
            }
        }
        let looks = u64::try_from(kept.len()).unwrap_or(u64::MAX);
        drop((kept, watched));
        self.count(|s| s.looks += looks);
        changed
    }

    /// Reads and parses the file of the kept shape at `key` again, replacing the entry: the new shape, or `None` when the old
    /// one stays (the file is gone or unread, which prints a line; it does not load, which prints the loader's line; or the
    /// new shape does not fit the kept set).
    ///
    /// # Errors
    /// The budget ran out: the entry is as it was.
    fn reload(
        &self,
        vm: &mut Vm,
        key: &[u8],
        path: &[u8],
    ) -> Result<Option<Rc<KeptShape>>, VmError> {
        let Some(old) = self.kept.borrow().get(key).cloned() else {
            return Ok(None);
        };
        let bytes = match self.read_file(vm, path)? {
            FileBytes::Bytes(bytes) => bytes,
            FileBytes::Instead(_) => {
                vm.error(format!(
                    "Error: shape {} could not be read again: the shape it loaded stays.",
                    String::from_utf8_lossy(path)
                ));
                return Ok(None);
            }
        };
        // The old entry leaves the kept set while the new one is made (`keep` never replaces) and comes back unless the new
        // one took its place.
        self.unkeep(key, &old);
        let loaded = self.finish_load(vm, key.to_vec(), bytes);
        if let Ok(Loaded::Shape(new)) = &loaded
            && self.kept_holds(key, new)
        {
            return Ok(Some(Rc::clone(new)));
        }
        self.keep(key.to_vec(), old);
        loaded.map(|_| None)
    }
}

/// The server tick's poll: nothing unless `gate` (or the override, [`Shapes::set_hot_reload`]) is on and `interval_ms` of
/// simulation time passed since the last poll; then the look, the reloads and the marks (see the module docs). A file whose
/// reload the budget cannot pay for is reported on the console and given up until it changes again: the errors returned are
/// the ones nothing else reports (none today).
pub fn poll(vm: &mut Vm, gate: bool, interval_ms: u32) -> Vec<VmError> {
    let Some(shapes) = shapes_of(vm) else {
        return Vec::new();
    };
    let on = shapes.hot_reload_on(gate);
    shapes.watching.set(on);
    if !on {
        return Vec::new();
    }
    let now = vm.sim_time();
    if let Some(last) = shapes.last_poll.get()
        && now.wrapping_sub(last) < interval_ms
    {
        return Vec::new();
    }
    shapes.last_poll.set(Some(now));
    shapes.count(|s| s.polls += 1);
    let changed = shapes.look(vm);
    if changed.is_empty() {
        return Vec::new();
    }
    let mut replaced = Vec::new();
    // Each file under its own budget (a host budget's refusal is sticky until it is closed): one file too large to reload
    // does not stop the others.
    for file in changed.into_iter().take(MAX_RELOADS_PER_POLL) {
        let saved = vm.begin_host_budget(shapes.reload_work.get());
        let outcome = shapes.reload(vm, &file.key, &file.path);
        vm.end_host_budget(saved);
        // Whatever came of it, the file is not tried again until its time moves.
        if let Some(entry) = shapes.watched.borrow_mut().get_mut(&file.key) {
            entry.modified = file.modified;
        }
        match outcome {
            Ok(Some(new)) => {
                shapes.count(|s| s.reloads += 1);
                replaced.push((file.key, new));
            }
            Ok(None) => shapes.count(|s| s.failed += 1),
            Err(_) => {
                shapes.count(|s| s.failed += 1);
                vm.error(format!(
                    "Error: shape {} is too large to reload live: declare its datablock again.",
                    String::from_utf8_lossy(&file.path)
                ));
            }
        }
    }
    if !replaced.is_empty() {
        mark_datablocks(vm, &shapes, &replaced.into_iter().collect());
    }
    Vec::new()
}

/// Every datablock whose shape resource is one of the `replaced` files (by folded path) takes the new shape as the one it
/// loaded and is marked preloaded, so the drain refreshes its users: one walk of DataBlockGroup per poll with changes.
fn mark_datablocks(vm: &mut Vm, shapes: &Shapes, replaced: &HashMap<Vec<u8>, Rc<KeptShape>>) {
    let group = vm.objects().datablock_group();
    let ids: Vec<ObjId> = vm
        .objects()
        .members(group)
        .map(|m| m.as_slice().to_vec())
        .unwrap_or_default();
    let mut marked = 0u64;
    for id in ids {
        let holds = ext::get_mut(vm.objects_mut(), id).is_some_and(|e| {
            let Some(resource) = e.shape_resource.as_mut() else {
                return false;
            };
            let Some(new) = replaced.get(&fold(&resource.path)) else {
                return false;
            };
            resource.loaded = Rc::clone(new);
            e.shape_hold = Some(Rc::clone(new));
            true
        });
        if holds {
            refresh::mark_preloaded(vm, id);
            marked += 1;
        }
    }
    shapes.count(|s| s.marked += marked);
}
