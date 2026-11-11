//! Live datablocks on a server: the objects built from a datablock follow its edits.
//!
//! **The model.** A datablock that scripts change after objects were built from it is *dirty*, with a kind: [`Written`]
//! (a static field write, from the static-write observer: [`written`]) or [`Preloaded`] (it was declared again and its
//! `preload` ran: the shape and the other preload products may be new). The marks are coalesced in one set and drained
//! once per server tick ([`drain`]), after the scripts of the tick's packets and before the process list's pass, so a
//! script that writes a field every tick costs one light refresh per user per tick and a thousand redeclares in one tick
//! cost one heavy refresh per user.
//!
//! **Users.** Every datablock counts the objects whose `dataBlock` it is ([`ObjectExt::db_users`]: `GameBase::onAdd`,
//! `setDataBlock` and `onRemove` move the count, [`user_added`], [`user_removed`]; an object remembers the datablock it is
//! counted on, [`ObjectExt::counted_db`], so a `dataBlock` field written directly never drifts a count). A datablock without users takes no mark
//! (an object made after the change is built from the changed datablock, and a first declaration never has one), and the
//! drain reads the counts again and scans the process list only when a dirty datablock still has a user: a mission load,
//! which declares thousands of datablocks and the objects on them before the first tick, costs a count read per write and
//! preload, no mark, no scan and no rebuild.
//!
//! **Refresh.** The class chain of the datablock refreshes its users through [`EngineClass::datablock_changed`]
//! (root first, as `preload` runs): the engine's own classes answer from the behavior [`install`] chains onto `GameBaseData`
//! ([`EngineRefresh`]: a [`Written`] mark runs the light refresh, the fields an object copies from its datablock; a
//! [`Preloaded`] mark runs the class's whole `onNewDataBlock` work again, [`super::object::new_datablock`], which keeps the
//! script threads by sequence name and position; it takes the refresh of CameraData and the ShapeBaseData classes but the
//! vehicles', whose words it does not refresh yet). A game's own datablock class registers its refresh on its behavior (a
//! plugin class has a `refresh` hook beside `preload`); a class with users and no refresh is warned about once. The users a
//! behavior is given are live objects (an earlier datablock's `onReloaded` in the same drain may have deleted some: they are
//! filtered first). After the refresh of a datablock that had users its script `onReloaded()` runs.
//!
//! **Limits.** A drain refreshes the users of dirty datablocks in id order until it has refreshed [`MAX_REFRESHES_PER_DRAIN`]
//! objects (the first datablock's users are always taken, however many) and runs under a host work budget ([`DRAIN_WORK`]),
//! in chunks of [`REFRESH_CHUNK`] users: once the budget refuses, the datablock keeps a cursor at its last finished chunk and
//! every datablock not reached stays dirty, so a datablock whose users outgrow one tick's budget finishes over several ticks
//! (a new change to it starts it over). The set holds at most one mark per live datablock (a mark for an id that is not a
//! datablock is dropped), so it is bounded by the datablock id range.
//!
//! [`Written`]: DatablockChange::Written
//! [`Preloaded`]: DatablockChange::Preloaded
//! [`ObjectExt::db_users`]: super::ext::ObjectExt::db_users
//! [`ObjectExt::counted_db`]: super::ext::ObjectExt::counted_db

use std::cell::RefCell;
use std::rc::Rc;

use collide::hash::{IdMap, IdSet};
use ts_vm::classes::{DatablockChange, EngineClass, PreloadError};
use ts_vm::sim::{ClassId, ObjId, SIM_DATA_BLOCK};
use ts_vm::vm::{Vm, VmError};

use super::access::object_field;
use super::ext;
use crate::sim::ProcessHandle;

/// The (datablock, user) refreshes a drain stops taking more datablocks at; the rest wait for the next tick.
pub const MAX_REFRESHES_PER_DRAIN: usize = 4096;

/// The host work budget of one drain (shape instances, boxes, bins), in the units of `Vm::charge_work`: the work of one
/// tick's pass ([`crate::sim::pass::TICK_PASS_WORK`]). [`set_drain_work`] changes it.
pub const DRAIN_WORK: u64 = crate::sim::pass::TICK_PASS_WORK;

/// The users a drain refreshes between two checks of its budget: a datablock put off by the budget resumes after its last
/// finished chunk.
pub const REFRESH_CHUNK: usize = 32;

/// What the drains did so far (the tests read it).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Marks taken, of datablocks with users (a second mark of the same datablock in one tick is not counted).
    pub marks: u64,
    /// Drains that found a mark.
    pub drains: u64,
    /// Drains that scanned the process list (a dirty datablock had a user).
    pub scans: u64,
    /// Objects refreshed after a field write.
    pub light: u64,
    /// Objects refreshed after a redeclare.
    pub heavy: u64,
    /// Datablocks whose refresh was put off to the next tick by the drain's limits (a datablock resumed mid-way counts each
    /// time).
    pub deferred: u64,
    /// The work the drains charged, in the units of `Vm::charge_work`.
    pub work: u64,
}

/// A dirty datablock's mark: the change and, after a drain the budget stopped, the last user it finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    change: DatablockChange,
    resume_after: Option<ObjId>,
}

#[derive(Default)]
struct State {
    marks: IdMap<ObjId, Mark>,
    /// The classes warned about for having users and no refresh.
    warned: IdSet<ClassId>,
    stats: Stats,
    /// The drain's work budget ([`DRAIN_WORK`] unless set).
    work: Option<u64>,
}

/// The dirty set and the process list it scans: the `Vm` extension [`install`] attaches.
pub struct DirtyDatablocks {
    state: RefCell<State>,
    process: ProcessHandle,
}

fn dirty(vm: &Vm) -> Option<Rc<DirtyDatablocks>> {
    vm.extension::<DirtyDatablocks>()
}

/// Marks `db` dirty with `change`; a redeclare's mark outranks a write's. Nothing without the extension (a client, a
/// server without the game classes), for a datablock no object uses, or for an object that is no datablock.
fn mark(vm: &Vm, db: ObjId, change: DatablockChange) {
    let Some(set) = dirty(vm) else {
        return;
    };
    if users(vm, db) == 0 {
        return;
    }
    let is_datablock = vm
        .objects()
        .get(db)
        .is_some_and(|o| vm.objects().classes.is_a(o.class(), SIM_DATA_BLOCK));
    if !is_datablock {
        return;
    }
    let mut state = set.state.borrow_mut();
    let State { marks, stats, .. } = &mut *state;
    let mark = marks.entry(db).or_insert_with(|| {
        stats.marks += 1;
        Mark {
            change,
            resume_after: None,
        }
    });
    // A new change reaches the users already refreshed too: the datablock starts over.
    mark.change = mark.change.max(change);
    mark.resume_after = None;
}

/// The observer's part for a datablock ([`super::object::static_written`]): a write to a registered datablock of any class
/// bumps its [`db_generation`](super::ext::ObjectExt::db_generation) (the products built from it are rebuilt at their next
/// use) and, when objects use it, marks it [`DatablockChange::Written`]. A write to one still being declared for the first
/// time (`id` none) has no users and marks nothing.
pub fn written(vm: &mut Vm, id: Option<ObjId>, class: ClassId) {
    let Some(id) = id else {
        return;
    };
    if !vm.objects().classes.is_a(class, SIM_DATA_BLOCK) {
        return;
    }
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.db_generation = e.db_generation.wrapping_add(1);
    }
    mark(vm, id, DatablockChange::Written);
}

/// `SimDataBlock`'s preload, chained by [`install`]: a datablock declared again over users is marked
/// [`DatablockChange::Preloaded`] (a first declaration has no users and marks nothing).
struct MarkPreloaded;

impl EngineClass for MarkPreloaded {
    fn preload(&self, vm: &mut Vm, id: ObjId) -> Result<(), PreloadError> {
        mark(vm, id, DatablockChange::Preloaded);
        Ok(())
    }
}

/// The object `user` took `db` as its datablock (`GameBase::onAdd`, `setDataBlock`): `db` counts it, and it remembers which
/// datablock counts it ([`user_removed`] takes it off that one, whatever its `dataBlock` field says then). A datablock it was
/// counted on before is let go of first.
pub fn user_added(vm: &mut Vm, user: ObjId, db: ObjId) {
    user_removed(vm, user);
    if let Some(e) = ext::get_mut(vm.objects_mut(), db) {
        e.db_users = e.db_users.saturating_add(1);
    }
    if let Some(e) = ext::get_mut(vm.objects_mut(), user) {
        e.counted_db = Some(db);
    }
}

/// The object `user` let go of the datablock it is counted on (`GameBase::onRemove`, `setDataBlock`'s old datablock).
pub fn user_removed(vm: &mut Vm, user: ObjId) {
    let Some(db) = ext::get_mut(vm.objects_mut(), user).and_then(|e| e.counted_db.take()) else {
        return;
    };
    if let Some(e) = ext::get_mut(vm.objects_mut(), db) {
        e.db_users = e.db_users.saturating_sub(1);
    }
}

/// How many objects have `db` as their datablock.
pub fn users(vm: &Vm, db: ObjId) -> u32 {
    ext::get(vm.objects(), db).map_or(0, |e| e.db_users)
}

/// What the drains did so far; zero without the extension.
pub fn stats(vm: &Vm) -> Stats {
    dirty(vm).map_or_else(Stats::default, |set| set.state.borrow().stats)
}

/// Sets the work budget of a drain ([`DRAIN_WORK`] by default): for a host that tunes it and the tests of the budget.
pub fn set_drain_work(vm: &Vm, units: u64) {
    if let Some(set) = dirty(vm) {
        set.state.borrow_mut().work = Some(units);
    }
}

/// Attaches the dirty set over the server's process list, gives `SimDataBlock` the mark of a redeclare and `GameBaseData` the
/// engine classes' refresh (both chained after what the classes have). Run once, with the game classes.
pub fn install(vm: &mut Vm, process: &ProcessHandle) {
    vm.set_extension(Rc::new(DirtyDatablocks {
        state: RefCell::new(State::default()),
        process: process.clone(),
    }));
    crate::behaviour::chain(vm, "SimDataBlock", Rc::new(MarkPreloaded));
    crate::behaviour::chain(vm, "GameBaseData", Rc::new(EngineRefresh));
}

/// The tick's drain: every dirty datablock with users refreshes them (see the module docs). The errors are the refreshes' and
/// the `onReloaded` callbacks' that ran past the budget; a datablock whose refresh did not run, or did not finish, stays dirty.
pub fn drain(vm: &mut Vm) -> Vec<VmError> {
    let Some(set) = dirty(vm) else {
        return Vec::new();
    };
    let (marks, work) = {
        let mut state = set.state.borrow_mut();
        (
            std::mem::take(&mut state.marks),
            state.work.unwrap_or(DRAIN_WORK),
        )
    };
    if marks.is_empty() {
        return Vec::new();
    }
    set.state.borrow_mut().stats.drains += 1;
    // The live datablocks with a user, by one count each (a count needs the object): a mark of one that is gone or has no
    // user is dropped.
    let mut dirty_users: IdMap<ObjId, (Mark, Vec<ObjId>)> = marks
        .into_iter()
        .filter(|(db, _)| users(vm, *db) > 0)
        .map(|(db, mark)| (db, (mark, Vec::new())))
        .collect();
    if dirty_users.is_empty() {
        return Vec::new();
    }
    set.state.borrow_mut().stats.scans += 1;
    // One scan of the process list (every object with a datablock is in it): each object joins its datablock's users.
    for id in set.process.borrow().keys() {
        if let Some(db) = object_field(vm, id, b"dataBlock")
            && let Some((_, users)) = dirty_users.get_mut(&db)
        {
            users.push(id);
        }
    }
    let mut errors = Vec::new();
    let mut refreshed = 0usize;
    let saved = vm.begin_host_budget(work);
    // Datablock order: the ids, so a tick's work is the same whatever the map's order.
    let mut order: Vec<ObjId> = dirty_users.keys().copied().collect();
    order.sort_unstable();
    let mut stopped = false;
    for db in order {
        let Some((mark, mut users)) = dirty_users.remove(&db) else {
            continue;
        };
        // A datablock resumed after a stopped drain goes on from its cursor; the users deleted since the scan (an earlier
        // datablock's `onReloaded`) are left out, so a behavior only ever sees live ones.
        users.sort_unstable();
        users.retain(|&u| {
            mark.resume_after.is_none_or(|after| u > after) && vm.objects().contains(u)
        });
        if users.is_empty() {
            continue;
        }
        // Once the budget refused (a refusal holds until the budget closes), or the cap is reached, the rest waits for the
        // next tick; a drain refreshes at least one datablock's chunk, so every datablock makes progress.
        if stopped
            || (refreshed > 0 && refreshed.saturating_add(users.len()) > MAX_REFRESHES_PER_DRAIN)
        {
            defer(&set, db, mark);
            continue;
        }
        match refresh_one(vm, &set, db, &users, mark.change) {
            Ok(()) => refreshed += users.len(),
            Err(Stopped {
                error,
                resume_after,
            }) => {
                defer(
                    &set,
                    db,
                    Mark {
                        change: mark.change,
                        resume_after: resume_after.or(mark.resume_after),
                    },
                );
                errors.push(error);
                stopped = true;
            }
        }
    }
    let used = vm.end_host_budget(saved);
    set.state.borrow_mut().stats.work += used;
    errors
}

/// Keeps `db` dirty for the next tick with `mark` (a new mark of the tick outranks it, a cursor is kept).
fn defer(set: &DirtyDatablocks, db: ObjId, mark: Mark) {
    let mut state = set.state.borrow_mut();
    let kept = state.marks.entry(db).or_insert(mark);
    kept.change = kept.change.max(mark.change);
    kept.resume_after = kept.resume_after.or(mark.resume_after);
    state.stats.deferred += 1;
}

/// A refresh the budget stopped: the error and the last user whose chunk finished (none when the first chunk did not).
struct Stopped {
    error: VmError,
    resume_after: Option<ObjId>,
}

/// The class chain's refresh of `db`'s users, [`REFRESH_CHUNK`] at a time, the warning for a class without one, then
/// `%db.onReloaded()`.
fn refresh_one(
    vm: &mut Vm,
    set: &DirtyDatablocks,
    db: ObjId,
    users: &[ObjId],
    change: DatablockChange,
) -> Result<(), Stopped> {
    let Some(class) = vm.objects().get(db).map(ts_vm::sim::SimObject::class) else {
        return Ok(());
    };
    let behaviors = vm.objects().classes.behaviours(class);
    let mut taken = false;
    let mut resume_after = None;
    for chunk in users.chunks(REFRESH_CHUNK) {
        for behavior in &behaviors {
            taken |= behavior
                .datablock_changed(vm, db, chunk, change)
                .map_err(|error| Stopped {
                    error,
                    resume_after,
                })?;
            if !vm.objects().contains(db) {
                return Ok(());
            }
        }
        resume_after = chunk.last().copied();
        // Counted per finished chunk: a datablock resumed over several ticks counts each user once (a chunk the budget
        // stopped mid-way is run again whole, and those users are refreshed again, to the same state).
        let mut state = set.state.borrow_mut();
        let count = chunk.len() as u64;
        match change {
            DatablockChange::Written => state.stats.light += count,
            DatablockChange::Preloaded => state.stats.heavy += count,
        }
    }
    let warn = !taken && set.state.borrow_mut().warned.insert(class);
    if warn {
        let name = String::from_utf8_lossy(vm.objects().classes.name(class)).into_owned();
        vm.warn(format!(
            "datablock class {name} has no refresh: its {} object(s) keep what they were built with",
            users.len()
        ));
    }
    vm.call_method(db, b"onReloaded", &[])
        .map_err(|error| Stopped {
            error,
            resume_after,
        })?;
    Ok(())
}

/// The engine's own classes' refresh, chained onto `GameBaseData`: each user of the datablock takes the light or the heavy
/// refresh ([`light`], [`super::object::new_datablock`]) with the type mask its constructor made, then its `dynamicType` bits
/// again, in the order its add ran them. Taken for `CameraData` and the `ShapeBaseData` classes other than the vehicles'
/// (whose rigid body and wheels the light refresh does not reach yet): any other `GameBaseData` class with users is warned
/// about, though the work above still runs for it.
struct EngineRefresh;

impl EngineClass for EngineRefresh {
    fn datablock_changed(
        &self,
        vm: &mut Vm,
        db: ObjId,
        users: &[ObjId],
        change: DatablockChange,
    ) -> Result<bool, VmError> {
        let is = |class| super::camera::is_class(vm, db, class);
        let taken = is("CameraData") || (is("ShapeBaseData") && !is("VehicleData"));
        for &user in users {
            if !vm.objects().contains(user) {
                continue;
            }
            super::shapebase::clear_dynamic_type(vm, user);
            let refreshed = match change {
                DatablockChange::Written => light(vm, user, db),
                DatablockChange::Preloaded => super::object::new_datablock(vm, user, db),
            };
            // The bits go back whatever the work did: a refresh the budget stopped runs again next tick.
            super::shapebase::refresh_dynamic_type(vm, user, db);
            refreshed?;
        }
        Ok(taken)
    }
}

/// The light refresh of one object after a field write: the fields it copied from its datablock (a Player's mass, drag,
/// speeds, recharge rate, box and vitals scaling; another ShapeBase's vitals words), without the shape part.
fn light(vm: &mut Vm, id: ObjId, db: ObjId) -> Result<(), VmError> {
    super::shapebase::refresh_box(vm, id, db);
    crate::sim::player::new_datablock(vm, id, db)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Vm` with the game classes, the dirty set over `process` and the observer, and a StaticShape `S` on datablock `D`
    /// counted as its user (what a server's `GameBase::onAdd` does). The declaration itself marks nothing (`D` had no user).
    fn scene(process: &ProcessHandle) -> (Vm, ObjId, ObjId) {
        let mut vm = Vm::new();
        crate::specs::install(&mut vm);
        install(&mut vm, process);
        vm.set_static_write_observer(Some(Rc::new(super::super::object::static_written)));
        vm.eval(b"datablock StaticShapeData(D) {}; new StaticShape(S) { dataBlock = D; };")
            .unwrap();
        let (d, s) = (
            vm.objects().find(b"D").unwrap(),
            vm.objects().find(b"S").unwrap(),
        );
        process.register(&vm, s);
        assert!(drain(&mut vm).is_empty());
        assert_eq!(stats(&vm), Stats::default());
        user_added(&mut vm, s, d);
        (vm, d, s)
    }

    fn warnings(vm: &Vm) -> usize {
        vm.log_texts()
            .iter()
            .filter(|l| l.contains("has no refresh"))
            .count()
    }

    #[test]
    fn a_redeclare_outranks_a_write_and_a_mark_needs_the_extension() {
        let process = ProcessHandle::new();
        let (vm, d, _) = scene(&process);
        mark(&vm, d, DatablockChange::Preloaded);
        mark(&vm, d, DatablockChange::Written);
        let set = dirty(&vm).unwrap();
        assert_eq!(
            set.state.borrow().marks.get(&d).map(|m| m.change),
            Some(DatablockChange::Preloaded)
        );
        assert_eq!(set.state.borrow().stats.marks, 1, "one mark per datablock");
        let mut bare = Vm::new();
        crate::specs::install(&mut bare);
        bare.eval(b"datablock StaticShapeData(D) {};").unwrap();
        let bare_d = bare.objects().find(b"D").unwrap();
        mark(&bare, bare_d, DatablockChange::Written);
        assert!(
            drain(&mut bare).is_empty(),
            "nothing to drain without the extension"
        );
    }

    #[test]
    fn a_class_with_users_and_no_refresh_is_warned_about_once() {
        let process = ProcessHandle::new();
        let (mut vm, d, _) = scene(&process);
        // A game whose GameBaseData behavior is its own, without the engine classes' refresh.
        vm.set_class_behaviour("GameBaseData", Rc::new(ts_vm::classes::GameBaseData));
        for value in 0..3 {
            vm.eval(format!("D.dynamicType = {value};").as_bytes())
                .unwrap();
            assert!(drain(&mut vm).is_empty());
        }
        assert_eq!(warnings(&vm), 1);
        let stats = stats(&vm);
        assert_eq!((stats.drains, stats.scans, stats.light), (3, 3, 3));
        assert_eq!(users(&vm, d), 1);
    }

    #[test]
    fn a_datablock_class_the_engine_does_not_refresh_is_warned_about_with_the_engines_refresh_in_place()
     {
        let process = ProcessHandle::new();
        let (mut vm, _, _) = scene(&process);
        vm.eval(b"datablock ProjectileData(Shot) {}; new Projectile(P) { dataBlock = Shot; };")
            .unwrap();
        let (shot, p) = (
            vm.objects().find(b"Shot").unwrap(),
            vm.objects().find(b"P").unwrap(),
        );
        process.register(&vm, p);
        user_added(&mut vm, p, shot);
        for _ in 0..2 {
            vm.eval(b"Shot.lifetime = 100; D.dynamicType = 3;").unwrap();
            assert!(drain(&mut vm).is_empty());
        }
        assert_eq!(
            warnings(&vm),
            1,
            "the ProjectileData once, the StaticShapeData never"
        );
        assert!(
            vm.log_texts()
                .iter()
                .any(|l| l.contains("class ProjectileData has no refresh"))
        );
    }

    #[test]
    fn a_count_follows_the_datablock_an_object_was_counted_on() {
        let process = ProcessHandle::new();
        let (mut vm, d, s) = scene(&process);
        vm.eval(b"datablock StaticShapeData(E) {};").unwrap();
        let e = vm.objects().find(b"E").unwrap();
        // A `dataBlock` written as a field moves no count; the removal takes the object off the datablock that counted it.
        vm.eval(b"S.dataBlock = E;").unwrap();
        assert_eq!((users(&vm, d), users(&vm, e)), (1, 0));
        user_removed(&mut vm, s);
        assert_eq!((users(&vm, d), users(&vm, e)), (0, 0));
        user_removed(&mut vm, s);
        assert_eq!(users(&vm, d), 0, "a second removal takes nothing");
        user_added(&mut vm, s, e);
        user_added(&mut vm, s, d);
        assert_eq!(
            (users(&vm, d), users(&vm, e)),
            (1, 0),
            "a new count lets go of the old"
        );
    }

    #[test]
    fn a_datablock_without_users_takes_no_mark() {
        let process = ProcessHandle::new();
        let (mut vm, d, s) = scene(&process);
        user_removed(&mut vm, s);
        vm.eval(b"D.dynamicType = 1; datablock StaticShapeData(D) {};")
            .unwrap();
        assert!(drain(&mut vm).is_empty());
        assert_eq!(stats(&vm), Stats::default());
        let generation = ext::get(vm.objects(), d).map_or(0, |e| e.db_generation);
        assert_eq!(generation, 1, "the write still moved the generation");
    }
}
