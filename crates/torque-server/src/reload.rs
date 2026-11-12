//! Reloading a script file's datablocks in a running server: `reloadDataBlocks(%file [, %removeMissing])` and the
//! development watcher that runs it for a file that changed on disk.
//!
//! **The path** ([`reload`]): the file is run again with [`Vm::exec_file_checked`] (its source, compiled once; a syntax
//! error runs nothing, so every datablock stays as it was), which declares its datablocks again (each marked for the
//! server's refresh, [`torque_game::scene::refresh`]); the marks are drained at once, the hooks a plugin registered run
//! ([`before_round`]), and one live round sends what changed to the connected clients (`updateDataBlocks()`, through the
//! [`LiveRound`] the network host gave [`install`]). The datablocks the file declared last time and not this time are
//! reported (`no longer declared by <file>: <name>`) and removed only with `%removeMissing` or [`REMOVE_MISSING_PREF`],
//! each only while no object uses it ([`refresh::users`]): a datablock with users is refused and reported. A file that
//! registers objects besides its datablocks runs them again at every reload; its first reload warns once, naming them.
//!
//! **The watcher** ([`poll`], run by the server's tick): only in development ([`DevGate`]: a debug build or `$Pref::Dev`,
//! the gate of the assets' hot reload), once per [`HOT_RELOAD_INTERVAL_MS`] of server time, it reads the change time of
//! every script file that declared a datablock ([`Vm::script_files_declaring_datablocks`]) and reloads the ones whose time
//! differs from the one `exec` saw. The table takes the new time when the file is run again, syntax error or not, so a
//! file that does not compile is tried again only when it changes again. Outside development the watcher reads nothing.
//!
//! **Bounds.** The file table and the declarations are the script engine's, capped there
//! ([`torque_ts_vm::vm::MAX_TRACKED_SCRIPT_FILES`], one entry per datablock id); the hooks are capped at
//! [`MAX_BEFORE_ROUND_HOOKS`]; the warned set holds at most as many files as the table, and a file past that cap is not
//! warned about. A reload holds its state from its start to its round, so a reload asked for inside one (by the file's
//! code, a hook or an `onReloaded`) is refused and printed. A reload runs a file through
//! `exec`'s own expansion, checks and policy, so it cannot reach a file `exec` could not. Nothing here is reachable from
//! a client: the native is a server script's, development only, and the watcher is the host's.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

use torque_engine::assets::HOT_RELOAD_INTERVAL_MS;
use torque_engine::script::{dev_allowed_in, dev_refusal};
use torque_game::scene::refresh;
use torque_ts_vm::effects::Effects;
use torque_ts_vm::fields::atob;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::text::{fold, until_nul};
use torque_ts_vm::vm::{
    ExecCheck, MAX_TRACKED_SCRIPT_FILES, Native, NativeCall, Vm, VmError, lossy,
};

/// The hooks [`before_round`] keeps at most.
pub const MAX_BEFORE_ROUND_HOOKS: usize = 64;
/// The pref that makes every reload remove the datablocks a file no longer declares, as `%removeMissing` does.
pub const REMOVE_MISSING_PREF: &str = "$Pref::Dev::RemoveMissing";
/// The objects a reload's warning names at most; the rest are counted.
const NAMED_OBJECTS: usize = 8;
/// The native's name.
const NATIVE: &str = "reloadDataBlocks";

/// Starts a live round on every connection past its mission chain and returns how many it reached: what
/// `updateDataBlocks()` does, given to [`install`] by the network host.
pub type LiveRound = Rc<dyn Fn(&mut Vm) -> Result<usize, VmError>>;

/// A hook run after a reload's refresh and before its round ([`before_round`]): a plugin reads its content again here. An
/// `Err` is printed as a console error and the round still runs.
pub type BeforeRound = Rc<dyn Fn(&mut Vm, &Reloaded) -> Result<(), String>>;

/// What a reload did ([`reload`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reloaded {
    /// The file, as `exec` spells it.
    pub file: Vec<u8>,
    /// Datablocks the file declared before and declared again.
    pub changed: usize,
    /// Datablocks the file declares that it did not declare before.
    pub added: usize,
    /// Datablocks the file no longer declares that were removed.
    pub removed: usize,
    /// Datablocks the file no longer declares, reported; the removed ones among them.
    pub missing: Vec<ObjId>,
    /// The missing datablocks removal was asked for and refused, each with its user count.
    pub refused: Vec<(ObjId, u32)>,
    /// Objects besides datablocks the file registered this time.
    pub others: usize,
}

impl Reloaded {
    /// The native's result: `changed:N added:N removed:N`.
    pub fn line(&self) -> String {
        format!(
            "changed:{} added:{} removed:{}",
            self.changed, self.added, self.removed
        )
    }
}

/// When the watcher runs and the native may be called.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DevGate {
    /// A debug build, or `$Pref::Dev` true: the assets' hot reload gate.
    #[default]
    Build,
    /// `$Pref::Dev` true only, as a release build reads it (the tests hold a debug build to it).
    PrefOnly,
    /// Always.
    On,
    /// Never.
    Off,
}

/// What the watcher and the reloads did so far (the tests read it).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Polls that looked at the files (one per interval while the gate is open).
    pub polls: u64,
    /// Change times read.
    pub checks: u64,
    /// Files run again to the end.
    pub reloads: u64,
    /// Reloads that ran nothing: a syntax error, a missing or refused file.
    pub failed: u64,
    /// Live rounds started.
    pub rounds: u64,
}

/// The reload state, an extension of the server's script engine ([`install`]).
pub struct ScriptWatch {
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    round: Option<LiveRound>,
    hooks: Vec<BeforeRound>,
    gate: DevGate,
    /// The server time of the last poll.
    last_poll: Option<u32>,
    /// The folded paths of the files warned about for the objects they register.
    warned: HashSet<Vec<u8>>,
    /// A reload is running: one a reloaded file asks for is refused.
    reloading: bool,
    stats: Stats,
}

impl ScriptWatch {
    fn dev_allowed(&self, vm: &Vm) -> bool {
        match self.state.borrow().gate {
            DevGate::Build => dev_allowed_in(vm, cfg!(debug_assertions)),
            DevGate::PrefOnly => dev_allowed_in(vm, false),
            DevGate::On => true,
            DevGate::Off => false,
        }
    }
}

fn watch(vm: &Vm) -> Option<Rc<ScriptWatch>> {
    vm.extension::<ScriptWatch>()
}

/// Attaches the reload state to `vm` with the network host's `round` and registers `reloadDataBlocks`. Run once, by the
/// network host.
pub fn install(vm: &mut Vm, round: LiveRound) {
    vm.set_extension(Rc::new(ScriptWatch {
        state: RefCell::new(State {
            round: Some(round),
            ..State::default()
        }),
    }));
    vm.register(
        NATIVE,
        2,
        3,
        "reloadDataBlocks(file [, removeMissing]) - development only: runs the script file again, refreshes the objects built from the datablocks it declares and sends the changed datablocks to the connected clients; reports the datablocks the file no longer declares, and removes the ones no object uses when removeMissing is true; returns changed:N added:N removed:N",
        reload_native,
    );
    vm.add_native_effects("", NATIVE, Effects::CODE | Effects::FILES);
}

/// Registers a hook that runs on every reload after the refresh and before the round, so a plugin can read its content
/// again before the clients are told; false when there is no reload state or [`MAX_BEFORE_ROUND_HOOKS`] are registered.
pub fn before_round(vm: &Vm, hook: BeforeRound) -> bool {
    let Some(watch) = watch(vm) else {
        return false;
    };
    let mut state = watch.state.borrow_mut();
    if state.hooks.len() >= MAX_BEFORE_ROUND_HOOKS {
        return false;
    }
    state.hooks.push(hook);
    true
}

/// Sets when the watcher runs ([`DevGate::Build`] by default).
pub fn set_gate(vm: &Vm, gate: DevGate) {
    if let Some(watch) = watch(vm) {
        watch.state.borrow_mut().gate = gate;
    }
}

/// Whether the watcher runs and the native may be called now.
pub fn dev_allowed(vm: &Vm) -> bool {
    watch(vm).is_some_and(|w| w.dev_allowed(vm))
}

/// What the watcher and the reloads did so far; zero without the reload state.
pub fn stats(vm: &Vm) -> Stats {
    watch(vm).map_or_else(Stats::default, |w| w.state.borrow().stats)
}

fn reload_native(vm: &mut Vm, call: &NativeCall) -> Native {
    if !dev_allowed(vm) {
        vm.error(dev_refusal(NATIVE));
        return Native::Str(Vec::new());
    }
    let file = call.argv.get(1).map(|a| until_nul(a)).unwrap_or_default();
    let remove_missing = call.boolean(2);
    match reload(vm, file, remove_missing) {
        Ok(Some(done)) => Native::Str(done.line().into_bytes()),
        Ok(None) => Native::Str(Vec::new()),
        Err(error) => Native::Fail(error),
    }
}

/// Runs `name` (a script file name as `exec` takes it) again and brings the server and its clients up to date (see the
/// module docs). None when nothing ran (the reason was printed: a syntax error, a missing or refused file, a reload
/// inside a reload) or there is no reload state. `remove_missing` removes the datablocks the file no longer declares, as
/// [`REMOVE_MISSING_PREF`] does for every reload.
///
/// # Errors
/// The file's code stopped the run, or the run's budget was spent.
pub fn reload(vm: &mut Vm, name: &[u8], remove_missing: bool) -> Result<Option<Reloaded>, VmError> {
    let Some(watch) = watch(vm) else {
        return Ok(None);
    };
    let path = match vm.expand_script_filename(name) {
        Ok(path) => path,
        Err(message) => {
            vm.error(message);
            return Ok(None);
        }
    };
    // Held until the round is started: the file's code, a hook and an `onReloaded` all run inside.
    if std::mem::replace(&mut watch.state.borrow_mut().reloading, true) {
        vm.error(format!(
            "{NATIVE}: {} asked for while a reload runs: not reloaded",
            lossy(&path)
        ));
        return Ok(None);
    }
    let result = reload_held(vm, &watch, &path, remove_missing);
    watch.state.borrow_mut().reloading = false;
    result
}

/// [`reload`] with the state held: the file run, what it declared compared with what it declared before, the missing
/// ones, then the refresh, the hooks and the round.
fn reload_held(
    vm: &mut Vm,
    watch: &ScriptWatch,
    path: &[u8],
    remove_missing: bool,
) -> Result<Option<Reloaded>, VmError> {
    let before: BTreeSet<ObjId> = vm.datablocks_declared_by(path).into_iter().collect();
    vm.trace_new_objects(true);
    let outcome = vm.exec_file_checked(path);
    let (traced, over) = vm.trace_new_objects(false);
    if outcome? != ExecCheck::Ran {
        watch.state.borrow_mut().stats.failed += 1;
        return Ok(None);
    }
    let file = vm
        .script_file(path)
        .map_or_else(|| path.to_vec(), |f| f.path.to_vec());
    let (declared, others) = split_registered(vm, &traced);
    let mut done = Reloaded {
        file,
        changed: before.intersection(&declared).count(),
        added: declared.difference(&before).count(),
        removed: 0,
        // A datablock the file's own code deleted this time is gone, not missing.
        missing: before
            .difference(&declared)
            .copied()
            .filter(|id| vm.objects().contains(*id))
            .collect(),
        refused: Vec::new(),
        others: others.len().saturating_add(over),
    };
    warn_about_others(vm, watch, path, &others, over);
    let remove = remove_missing
        || vm
            .global(REMOVE_MISSING_PREF)
            .is_some_and(|value| atob(&value));
    report_missing(vm, &mut done, remove)?;
    for error in refresh::drain(vm) {
        vm.error(format!("{NATIVE}: refresh: {error}"));
    }
    let hooks = watch.state.borrow().hooks.clone();
    for hook in hooks {
        if let Err(message) = hook(vm, &done) {
            vm.error(format!("{NATIVE}: {message}"));
        }
    }
    let round = watch.state.borrow().round.clone();
    if let Some(round) = round {
        round(vm)?;
        watch.state.borrow_mut().stats.rounds += 1;
    }
    watch.state.borrow_mut().stats.reloads += 1;
    Ok(Some(done))
}

/// Prints each missing datablock of `done` and, with `remove`, deletes the ones without users (the refused ones are
/// listed with their counts).
fn report_missing(vm: &mut Vm, done: &mut Reloaded, remove: bool) -> Result<(), VmError> {
    for id in done.missing.clone() {
        let name = object_name(vm, id);
        vm.print(format!("no longer declared by {}: {name}", lossy(&done.file)).as_bytes());
        if !remove {
            continue;
        }
        let users = refresh::users(vm, id);
        if users > 0 {
            vm.error(format!(
                "{NATIVE}: {name} is used by {users} object(s): not removed"
            ));
            done.refused.push((id, users));
            continue;
        }
        vm.delete_object(id)?;
        done.removed += 1;
    }
    Ok(())
}

/// The traced objects that are live datablocks and the other live ones, in trace order.
fn split_registered(vm: &Vm, traced: &[ObjId]) -> (BTreeSet<ObjId>, Vec<ObjId>) {
    let objects = vm.objects();
    let group = objects.datablock_group();
    let mut declared = BTreeSet::new();
    let mut others = Vec::new();
    for &id in traced {
        let Some(object) = objects.get(id) else {
            continue;
        };
        if object.group() == Some(group) {
            declared.insert(id);
        } else {
            others.push(id);
        }
    }
    (declared, others)
}

/// The one warning for a file that registers objects besides its datablocks; a file past the warned set's cap gets none.
fn warn_about_others(vm: &mut Vm, watch: &ScriptWatch, path: &[u8], others: &[ObjId], over: usize) {
    if others.is_empty() && over == 0 {
        return;
    }
    let key = fold(path);
    {
        let mut state = watch.state.borrow_mut();
        if state.warned.contains(&key) || state.warned.len() >= MAX_TRACKED_SCRIPT_FILES {
            return;
        }
        state.warned.insert(key);
    }
    let count = others.len().saturating_add(over);
    let mut names: Vec<String> = others
        .iter()
        .take(NAMED_OBJECTS)
        .map(|&id| object_name(vm, id))
        .collect();
    if count > names.len() {
        names.push(format!("and {} more", count - names.len()));
    }
    vm.warn(format!(
        "{} creates {count} object(s) besides its datablocks, made again at every reload: {}",
        lossy(path),
        names.join(", ")
    ));
}

/// `Name (Class)`, or `<id> (Class)` for an unnamed object.
fn object_name(vm: &Vm, id: ObjId) -> String {
    let objects = vm.objects();
    let Some(object) = objects.get(id) else {
        return id.to_string();
    };
    let class = lossy(objects.classes.name(object.class()));
    match object.name().filter(|n| !n.is_empty()) {
        Some(name) => format!("{} ({class})", lossy(name)),
        None => format!("{id} ({class})"),
    }
}

/// The watcher's look at the files, run once per server tick: nothing outside development, nothing within
/// [`HOT_RELOAD_INTERVAL_MS`] of the last look, else every file that declared a datablock and changed since `exec` ran
/// it is reloaded ([`reload`]). The errors are the reloads' that stopped a run.
pub fn poll(vm: &mut Vm) -> Vec<VmError> {
    let Some(watch) = watch(vm) else {
        return Vec::new();
    };
    if !watch.dev_allowed(vm) {
        return Vec::new();
    }
    let now = vm.sim_time();
    {
        let mut state = watch.state.borrow_mut();
        if let Some(last) = state.last_poll
            && now.wrapping_sub(last) < HOT_RELOAD_INTERVAL_MS
        {
            return Vec::new();
        }
        state.last_poll = Some(now);
        state.stats.polls += 1;
    }
    let files = vm.script_files_declaring_datablocks();
    watch.state.borrow_mut().stats.checks += files.len() as u64;
    let stale: Vec<Rc<[u8]>> = files
        .into_iter()
        .filter(|f| vm.file_host().modified(&f.path) != f.modified)
        .map(|f| f.path)
        .collect();
    let mut errors = Vec::new();
    for path in stale {
        if let Err(error) = reload(vm, &path, false) {
            errors.push(error);
        }
    }
    errors
}
