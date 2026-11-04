//! The script profile a host turns on to see where a window's frame time goes in the VM ([`Vm::set_profiling`]): every script
//! function and every native called from script is timed (calls, time inside it with what it called, and its own time
//! without the functions it called), and every scheduled event the clock fired is timed under its function's name. Off,
//! the cost is one branch per call. On, two clock reads and a table lookup per call; the tables are capped
//! ([`MAX_FUNCTIONS`], [`MAX_SCHEDULES`]) and what falls past a cap is summed under [`OTHER`].

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::ns::EntryRef;

use super::Vm;

/// The most functions timed apart in one profile (the rest are summed as [`OTHER`]).
pub const MAX_FUNCTIONS: usize = 4096;
/// The most scheduled functions timed apart in one profile.
pub const MAX_SCHEDULES: usize = 1024;
/// The deepest nesting the profile follows (deeper calls are timed by the outer ones).
pub const MAX_DEPTH: usize = 4096;
/// The name the calls past a cap are summed under.
pub const OTHER: &str = "(other)";

/// One function's or schedule's totals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timed {
    /// `Namespace::name` (or `name`), the bytes as the script spelled them.
    pub name: Vec<u8>,
    pub calls: u64,
    /// The time inside it, what it called included (a recursive function counts its inner calls again).
    pub total: Duration,
    /// The time inside it less the script functions and natives it called.
    pub own: Duration,
    /// A native (engine) function.
    pub native: bool,
}

/// What a profile measured since it was last taken.
#[derive(Debug, Clone, Default)]
pub struct ProfileReport {
    /// Every timed function (unsorted).
    pub functions: Vec<Timed>,
    /// Every scheduled function the clock fired (`own` is its `total`).
    pub schedules: Vec<Timed>,
    /// The time in the outermost script calls (what the host's calls into the VM cost, schedules among them).
    pub script: Duration,
    /// Calls the profile timed.
    pub calls: u64,
}

impl ProfileReport {
    /// The `n` functions with the most own time.
    pub fn top_own(&self, n: usize) -> Vec<&Timed> {
        top(&self.functions, n, |t| t.own)
    }

    /// The `n` schedules with the most time.
    pub fn top_schedules(&self, n: usize) -> Vec<&Timed> {
        top(&self.schedules, n, |t| t.total)
    }
}

fn top(list: &[Timed], n: usize, key: impl Fn(&Timed) -> Duration) -> Vec<&Timed> {
    let mut v: Vec<&Timed> = list.iter().collect();
    v.sort_by(|a, b| key(b).cmp(&key(a)).then(b.calls.cmp(&a.calls)));
    v.truncate(n);
    v
}

struct Open {
    key: usize,
    start: Instant,
    /// The time of the calls it made (taken off its own time).
    inner: Duration,
}

struct Slot {
    /// Holds the entry so that its address is not reused for another one while the profile runs.
    _entry: Option<EntryRef>,
    timed: Timed,
}

/// The running profile.
#[derive(Default)]
pub(crate) struct Profiler {
    stack: Vec<Open>,
    slots: HashMap<usize, Slot>,
    schedules: HashMap<Vec<u8>, Timed>,
    script: Duration,
    calls: u64,
}

/// The key the calls past the cap go under (no entry has address 0).
const OTHER_KEY: usize = 0;

impl Profiler {
    fn enter(
        &mut self,
        entry: &EntryRef,
        namespace: impl FnOnce(&[u8]) -> Vec<u8>,
        package: impl FnOnce(crate::ns::PackageId) -> Option<Vec<u8>>,
    ) {
        if self.stack.len() >= MAX_DEPTH {
            return;
        }
        let mut key = Rc::as_ptr(entry) as *const () as usize;
        if !self.slots.contains_key(&key) {
            if self.slots.len() >= MAX_FUNCTIONS {
                key = OTHER_KEY;
                self.slots.entry(OTHER_KEY).or_insert_with(|| Slot {
                    _entry: None,
                    timed: Timed {
                        name: OTHER.as_bytes().to_vec(),
                        ..Timed::default()
                    },
                });
            } else {
                let e = entry.borrow();
                let mut name = if e.namespace.is_empty() {
                    Vec::new()
                } else {
                    namespace(&e.namespace)
                };
                if !name.is_empty() {
                    name.extend_from_slice(b"::");
                }
                name.extend_from_slice(&e.name);
                // A package's layer of the function (a mod's override) is told apart from the base one.
                if let Some(p) = e.package.and_then(package) {
                    name.extend_from_slice(b" [");
                    name.extend_from_slice(&p);
                    name.push(b']');
                }
                let native = matches!(e.callable, crate::ns::Callable::Native { .. });
                drop(e);
                self.slots.insert(
                    key,
                    Slot {
                        _entry: Some(Rc::clone(entry)),
                        timed: Timed {
                            name,
                            native,
                            ..Timed::default()
                        },
                    },
                );
            }
        }
        self.calls += 1;
        self.stack.push(Open {
            key,
            start: Instant::now(),
            inner: Duration::ZERO,
        });
    }

    fn leave(&mut self) {
        let Some(open) = self.stack.pop() else {
            return;
        };
        let spent = open.start.elapsed();
        if let Some(slot) = self.slots.get_mut(&open.key) {
            slot.timed.calls += 1;
            slot.timed.total += spent;
            slot.timed.own += spent.saturating_sub(open.inner);
        }
        match self.stack.last_mut() {
            Some(outer) => outer.inner += spent,
            None => self.script += spent,
        }
    }

    fn schedule(&mut self, name: &[u8], spent: Duration) {
        let full = self.schedules.len() >= MAX_SCHEDULES && !self.schedules.contains_key(name);
        let key = if full { OTHER.as_bytes() } else { name };
        let t = self.schedules.entry(key.to_vec()).or_insert_with(|| Timed {
            name: key.to_vec(),
            ..Timed::default()
        });
        t.calls += 1;
        t.total += spent;
        t.own += spent;
    }

    fn take(&mut self) -> ProfileReport {
        let report = ProfileReport {
            functions: self.slots.values().map(|s| s.timed.clone()).collect(),
            schedules: self.schedules.values().cloned().collect(),
            script: self.script,
            calls: self.calls,
        };
        // The calls still running keep their places (their time goes to the next report); the totals start again.
        self.slots
            .retain(|k, _| self.stack.iter().any(|o| o.key == *k));
        for slot in self.slots.values_mut() {
            slot.timed = Timed {
                name: std::mem::take(&mut slot.timed.name),
                native: slot.timed.native,
                ..Timed::default()
            };
        }
        self.schedules.clear();
        self.script = Duration::ZERO;
        self.calls = 0;
        report
    }
}

impl Vm {
    /// Turns the script profile on (a fresh one) or off ([`profile`](self)).
    pub fn set_profiling(&mut self, on: bool) {
        match (on, self.profile.is_some()) {
            (true, false) => self.profile = Some(Box::default()),
            (false, true) => self.profile = None,
            _ => {}
        }
    }

    /// Whether the script profile is on.
    pub fn profiling(&self) -> bool {
        self.profile.is_some()
    }

    /// What the profile measured since the last take, its totals cleared (None when it is off).
    pub fn take_profile(&mut self) -> Option<ProfileReport> {
        self.profile.as_mut().map(|p| p.take())
    }

    /// The scheduled events waiting (`getNumSchedules`'s count, without dropping the ones of deleted objects).
    pub fn pending_events(&self) -> usize {
        self.events.len()
    }

    /// The waiting events that will still run script: a call or a native event whose object is still registered. Not
    /// the runtime's delete events nor the events of deleted objects, which [`pending_events`](Self::pending_events)
    /// counts until the next advance drops them (a server's end leaves one for every object it deleted).
    pub fn pending_calls(&self) -> usize {
        self.events
            .iter()
            .filter(|e| e.kind.runs_script() && self.objects.contains(e.dest))
            .count()
    }

    #[cold]
    #[inline(never)]
    pub fn profile_enter(&mut self, entry: &EntryRef) {
        let namespaces = &self.namespaces;
        if let Some(p) = self.profile.as_mut() {
            p.enter(
                entry,
                |folded| {
                    namespaces
                        .display_ref(folded)
                        .map_or_else(|| folded.to_vec(), <[u8]>::to_vec)
                },
                |id| namespaces.package_name_of(id).map(<[u8]>::to_vec),
            );
        }
    }

    #[cold]
    #[inline(never)]
    pub fn profile_leave(&mut self) {
        if let Some(p) = self.profile.as_mut() {
            p.leave();
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn profile_schedule(&mut self, name: &[u8], on_object: bool, spent: Duration) {
        if let Some(p) = self.profile.as_mut() {
            if on_object {
                let mut n = b"%obj.".to_vec();
                n.extend_from_slice(name);
                p.schedule(&n, spent);
            } else {
                p.schedule(name, spent);
            }
        }
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
