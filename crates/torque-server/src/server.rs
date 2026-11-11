//! The server's main loop: a `Vm` and its engine state ([`ServerState`]), its clock, the budgets that bound what scripts may
//! spend, the network host it drives ([`NetHost`]), and its end.
//!
//! **Clock.** `getSimTime()` is 0 until [`Server::run`] runs. Its **first pass** covers the start-up: the main loop caps the
//! time one pass covers at [`MAX_PASS_MS`], and a start-up takes longer than that, so the first pass moves the clock to
//! [`FIRST_PASS_MS`] (1,024) at once and fires every event due by then in due-time order, each reading 1,024 (a `schedule(0)`
//! made inside one is due at 1,024 and fires after the ones already queued); every later tick is [`TICK_MS`] (32 ms), as fast
//! as the host likes (a test runs hours of server time in seconds) or paced by the caller. `quit()` ends the loop before the
//! next tick (an event scheduled after it never fires), so a server whose start-up quit runs no tick and the clock reads 0; a
//! `quit()` inside an event does not stop the other events of its tick, because the tick finishes.
//!
//! **Real time** is the host's: the engine's ticks are exactly 32 ms. A realtime host says how much wall time a pass covers
//! ([`Server::set_pass_elapsed`]) or how far it fell behind ([`Server::set_catch_up`]).
//!
//! **The pass slice** is the host's choice ([`Server::set_pass_slice`]): a tick's events end after the batch in which their
//! steps passed the slice, so a long `schedule(0)` chain spreads over ticks while the network keeps sending; without it the
//! chain runs in one tick. A game's hooks may give a pass a slice of the wall instead ([`ServerHooks::pass_slice`]).
//!
//! **End.** [`Server::finish`] calls `onExit` and then `Vm::shutdown`, as the main loop does on its way out.
//!
//! **Report.** [`Server::report`] says what the run needed that the engine does not have ([`Report`]): the functions scripts
//! called that nothing defines, the stub methods that were called, the events that failed.
//!
//! **Hostile scripts.** Every run has a step limit: the start-up [`STEP_LIMIT`], each event (an outermost run of its own) the
//! same, `onExit` the same; and the work of a whole [`Server::run`] has a total budget ([`RUN_STEP_LIMIT`] steps, the events'
//! together), because one tick may use several limits' worth and a script that schedules itself again at every tick would
//! otherwise keep the host busy for as long as the caller's time limit is long. The budget is per `run` call: after
//! [`RunEnd::StepLimit`] a host should treat the server as finished (a second call would have a whole budget again). The
//! steps of an event are counted to the work it did, including the native it ended with (`Vm::advance_time_counted`). The
//! errors of failing events are kept, up to [`MAX_EVENT_ERRORS`].
//!
//! **What a network host's strangers can and cannot spend.** The scripts the network host starts itself (`onConnectRequest`,
//! `onConnect`, `onDrop`, a client's commands) run under the network's own step limits, which are not this budget: a
//! stranger's datagrams cannot spend it *that way*. What they make a script `schedule()`, though, is a Sim event, and the
//! events of a run draw on this budget: a script that schedules work for every request can end a run with
//! [`RunEnd::StepLimit`]. A server with a socket must therefore not treat `StepLimit` as its end: [`Server::run_restarting`]
//! starts the run again (a budget of its own each time) and tells the caller.
//!
//! **A game's server** holds this one and runs it with itself as the host ([`Server::run_on`]), so that the callbacks of a
//! run see the game's whole server.
//!
//! The `Vm` recurses on the Rust stack: run a `Server` on a thread of at least 1 MiB. `Server` holds `Rc`s, so it is made,
//! run and finished on one thread.
//!
//! Credit: the main loop follows TGE as Torque3D, its MIT-licensed release, keeps it: `app/mainLoop.cpp` and
//! `T3D/gameBase/gameProcess.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use torque_engine::TickInfo;
use torque_game::sim::ProcessHandle;
use torque_game::sim::pass::{self, NoControl, Outcome};
use torque_ts_vm::events::EventLimits;
use torque_ts_vm::host::SystemClock;
use torque_ts_vm::vm::missing_functions;
use torque_ts_vm::vm::{LogLine, Vm, VmError};

use crate::state::{NetParts, NoNet, ServerState};
/// Steps one outermost run may take (`Vm::set_step_limit`): a start-up's whole run, each event, `onExit`. A script that
/// never ends, a hostile script's, say, stops here with `VmError::StepLimit` instead of hanging the host (about 2 to 3
/// seconds of a debug build). A start-up that execs many scripts and a mission stays well below it.
pub const STEP_LIMIT: u64 = 50_000_000;

/// The steps the events of one [`Server::run`] call may use together (see the module documentation; a second
/// call has a budget of its own again, so a host treats [`RunEnd::StepLimit`] as the end of the server). The
/// clock of a quiet server costs none. The steps of an event are what it ran plus the work of the native it
/// ended with, and at least one: a hostile server costs the host about this much work, which depends on the
/// work. Measured in a debug build (a release build is some seven times faster): a self-scheduling storm
/// (`function f() { $n++; schedule(0, 0, f); }`) 0.5 microseconds a step (10 million steps in 5.0 s, 0.66 s in
/// release), a storm of `export` events (300 to 2,000 globals of 1,000 bytes) 0.5 microseconds a step (5 million
/// steps in 2.3 s), a storm of `strreplace` events 0.02 (5.2 million steps in 0.11 s). So 100 million steps are
/// some seconds of a release build and up to a minute of a debug one.
pub const RUN_STEP_LIMIT: u64 = 100_000_000;

/// How far [`Server::run`] moves the clock at a time (TGE's tick, 32 ms).
pub const TICK_MS: u32 = 32;

/// Where the clock stands after the first tick of [`Server::run`], the first main-loop pass: the start-up's wall time, which
/// [`MAX_PASS_MS`] caps (before the pass the time is 0, through the whole start-up).
pub const FIRST_PASS_MS: u32 = 1024;

/// The most wall time one pass of the main loop covers (TGE caps the time event's elapsed milliseconds at 1,024): after a
/// longer stall the server's clock and the Sim events fall behind the wall by the rest ([`Server::set_pass_elapsed`]).
pub const MAX_PASS_MS: u32 = 1024;

/// What a pass's hook sees of the server when it decides the pass's slice ([`ServerHooks::pass_slice`]).
pub struct PassView<'a> {
    pub vm: &'a Vm,
    /// The host's own step slice of a pass ([`Server::set_pass_slice`]); none for a host that runs a tick's events in one go.
    pub host_pass_steps: Option<u64>,
    /// A client of the network is connected.
    pub remote_client: bool,
    /// The in-process local client is connected.
    pub local_client: bool,
}

/// A slice of the wall a pass's Sim events stop after, and the steps they may use whatever the wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassSlice {
    pub wall: std::time::Duration,
    pub steps: u64,
}

/// What a game adds to the server's main loop. Every method has the plain engine's answer (nothing) as its default.
pub trait ServerHooks {
    /// After a pass's tick and send, before its Sim events (`delta_ms` of the clock).
    fn before_events(&self, vm: &mut Vm, delta_ms: u32) {
        let _ = (vm, delta_ms);
    }

    /// The slice of the wall this pass's Sim events take instead of the host's steps; none keeps the host's.
    fn pass_slice(&self, view: &PassView<'_>) -> Option<PassSlice> {
        let _ = view;
        None
    }

    /// Whether this pass's events ran on the hook's slice.
    fn pass_sliced(&self, sliced: bool) {
        let _ = sliced;
    }
}

/// Several hooks as one: `before_events` and `pass_sliced` run in the order the hooks were added, and `pass_slice` is the first
/// answer (the first hook whose answer is not none).
#[derive(Default)]
pub struct HookList(Vec<Rc<dyn ServerHooks>>);

impl HookList {
    #[must_use]
    pub fn new(hooks: Vec<Rc<dyn ServerHooks>>) -> HookList {
        HookList(hooks)
    }
}

impl ServerHooks for HookList {
    fn before_events(&self, vm: &mut Vm, delta_ms: u32) {
        for hooks in &self.0 {
            hooks.before_events(vm, delta_ms);
        }
    }

    fn pass_slice(&self, view: &PassView<'_>) -> Option<PassSlice> {
        self.0.iter().find_map(|hooks| hooks.pass_slice(view))
    }

    fn pass_sliced(&self, sliced: bool) {
        for hooks in &self.0 {
            hooks.pass_sliced(sliced);
        }
    }
}

/// Makes a server's hooks once its stages ran, so it can read what the recipes left on the script engine. An error stops the
/// server's start with its message.
pub type MakeHooks = dyn Fn(&mut Vm) -> Result<Rc<dyn ServerHooks>, String>;

/// Makes a server's mission loader, as [`MakeHooks`] makes its hooks.
pub type MakeMissionLoader = dyn Fn(&mut Vm) -> Result<Rc<dyn MissionLoader>, String>;

/// Work a host runs around the process list's pass of every server tick, apart from [`ServerHooks`] (which a game replaces as a
/// whole): the engine's plugins' `ServerTick` hooks. [`Server::set_tick_hooks`] sets them; a server with none runs its ticks
/// as it always did. A pass that covers several ticks (the first pass, or one after a stall) calls `before` for each of
/// them in tick order, then runs the process list over all of them, then calls `after` for each.
pub trait TickHooks {
    /// Before the process list ticks the objects of `tick`.
    fn before(&self, vm: &mut Vm, tick: TickInfo);

    /// After the process list ticked the objects of `tick`, before the send pass.
    fn after(&self, vm: &mut Vm, tick: TickInfo);
}

/// The plain engine's main loop: no hook.
pub struct NoHooks;

impl ServerHooks for NoHooks {}

/// Why a mission did not load ([`MissionLoader::load`]).
#[derive(Debug)]
pub enum MissionError {
    /// The name is not a mission the server can load, with the reason.
    Refused(String),
    /// A script stopped the run.
    Vm(VmError),
}

impl From<VmError> for MissionError {
    fn from(e: VmError) -> MissionError {
        MissionError::Vm(e)
    }
}

impl std::fmt::Display for MissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MissionError::Refused(why) => f.write_str(why),
            MissionError::Vm(e) => write!(f, "the mission stopped the run: {e:?}"),
        }
    }
}

/// How a server loads a mission a host names ([`Server::load_mission`]). The answer is the loader's own report.
pub trait MissionLoader {
    fn load(&self, vm: &mut Vm, mission: &[u8]) -> Result<Box<dyn std::any::Any>, MissionError>;
}

/// TGE's mission load: the `.mis` is script and is `exec`'d (it builds `MissionGroup`). Its report is `()`.
pub struct ExecMission;

impl MissionLoader for ExecMission {
    fn load(&self, vm: &mut Vm, mission: &[u8]) -> Result<Box<dyn std::any::Any>, MissionError> {
        if vm.exec_file(mission, false)? {
            Ok(Box::new(()))
        } else {
            Err(MissionError::Refused(format!(
                "{}: the mission did not run",
                String::from_utf8_lossy(mission)
            )))
        }
    }
}

/// The most errors of failed events [`Server::event_errors`] keeps; later ones are only counted
/// ([`Server::event_errors_dropped`]).
pub const MAX_EVENT_ERRORS: usize = 1_024;

/// The most rows of each list [`Report`]'s `Display` shows; the heading gives the real total and a line says
/// how many more there were (a hostile script can call a million distinct missing names).
pub const MAX_REPORT_ROWS: usize = 200;

/// The phases of a pass, in the order the loop runs them (a network host may record them: [`NetHost::phase`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The socket is drained.
    Receive,
    /// The server tick (the process list's pass and the connections' moves).
    Tick,
    /// The send pass (only in a pass that ran a server tick).
    Send,
    /// The Sim event queue (`schedule`, deletions).
    SimEvents,
    /// The timeout sweep.
    Timeouts,
}

/// The network host a server's main loop drives ([`Server::attach_net`]): each pass drains the socket, ticks the objects
/// with the connections' moves, sends, and sweeps the timeouts.
pub trait NetHost {
    /// The network's time (ms) for a simulation time of `sim`.
    fn clock_ms(&self, sim: u32) -> u32;
    /// The loop enters `phase`.
    fn phase(&self, phase: Phase) {
        let _ = phase;
    }
    /// The number of server ticks run so far, told before each pass's receive: what the declared server commands' token
    /// buckets count in. A host with no such limits ignores it (the default).
    fn set_tick(&self, tick: u64) {
        let _ = tick;
    }
    /// Drains the socket at `now`; `new_tick`: the pass starts a tick (its network work budget is full again).
    fn receive(&self, vm: &mut Vm, now: u32, new_tick: bool);
    /// The process list's pass over the objects for `steps` ticks within `budget` steps, with the connections' control
    /// objects ticking their moves.
    fn advance_objects(
        &self,
        vm: &mut Vm,
        list: &ProcessHandle,
        steps: u32,
        budget: u64,
    ) -> Outcome;
    /// The send pass at `now`.
    fn send(&self, vm: &mut Vm, now: u32);
    /// The timeout sweep at `now`.
    fn timeouts(&self, vm: &mut Vm, now: u32);
    /// Whether a client of the network is connected, and whether the in-process local client is.
    fn clients(&self) -> (bool, bool);
}

/// Why a network host could not be attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachError {
    AlreadyAttached,
    /// A datablock already exists. The datablocks' `modifiedKey` and the static-write observer exist only from the moment
    /// the network host is attached: a datablock added before it has key 0, so it would never be sent. Attach before the
    /// scripts that add datablocks.
    DatablocksAlreadyExist,
    /// A real socket with the virtual clock.
    RealSocketNeedsSystemClock,
}

impl fmt::Display for AttachError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttachError::AlreadyAttached => f.write_str("a network host is already attached"),
            AttachError::DatablocksAlreadyExist => f.write_str(
                "datablocks already exist: attach the network host before the scripts that add datablocks",
            ),
            AttachError::RealSocketNeedsSystemClock => f.write_str(
                "a server with a socket runs on the system clock (call use_system_clock first)",
            ),
        }
    }
}

impl std::error::Error for AttachError {}

/// How [`Server::run`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// A script called `quit()` (before the call, or during its last tick).
    Quit,
    /// The caller's condition held.
    Until,
    /// The time limit passed.
    TimeLimit,
    /// The events of the run used up the run's step budget ([`RUN_STEP_LIMIT`], or what
    /// [`Server::set_run_step_limit`] gave). The budget is per `run` call: treat this as the end of the
    /// server (a hostile one is still scheduling), finish it, and do not call `run` again expecting a bound.
    StepLimit,
}

/// What a run needed that the engine does not have yet: the functions scripts called that nothing defines (the
/// "Unable to find function" and "Unknown command" warnings, [`missing_functions`]), the class table's stub
/// methods that were called (engine classes whose behaviour is not reproduced: they print nothing and return
/// their kind's neutral value), and how many events failed.
///
/// [`Server::report`] takes one at any moment; [`Finished::report`] is the run's, `onExit` included. `Display`
/// prints the three lists, at most [`MAX_REPORT_ROWS`] rows of each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The missing names and how often each was called, most called first, equal counts by name.
    pub missing: Vec<(String, usize)>,
    /// The called stubs as `Class::method` with their calls, in the same order.
    pub stubs: Vec<(String, u64)>,
    /// The events that failed: the ones [`Server::event_errors`] kept and the ones it only counted.
    pub event_errors: usize,
}

/// One list of a [`Report`]: the heading with the real total, the first [`MAX_REPORT_ROWS`] rows, and what was
/// left out.
fn write_rows<C: fmt::Display, N: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    heading: &str,
    rows: &[(N, C)],
) -> fmt::Result {
    writeln!(f, "{heading}: {}", rows.len())?;
    for (name, count) in rows.iter().take(MAX_REPORT_ROWS) {
        writeln!(f, "  {count:6} {name}")?;
    }
    if let Some(more) = rows.len().checked_sub(MAX_REPORT_ROWS).filter(|m| *m > 0) {
        writeln!(f, "  ... and {more} more")?;
    }
    Ok(())
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_rows(f, "missing functions", &self.missing)?;
        write_rows(f, "stub methods called", &self.stubs)?;
        write!(f, "failed events: {}", self.event_errors)
    }
}

/// What [`Server::finish_report`] returns.
#[derive(Debug)]
pub struct Finished {
    /// The whole log, with whatever `onExit` and the shutdown printed.
    pub log: Vec<LogLine>,
    /// How `onExit` ended: `Ok` also when no script defines it; `Err` when it was stopped (a step limit, the
    /// call depth). The shutdown ran either way.
    pub exit: Result<(), VmError>,
    /// What the whole run needed ([`Server::report`]), taken after `onExit` and before the shutdown (which
    /// runs no script: it only deletes what is left).
    pub report: Report,
}

/// The wall time of the run loop's passes by phase, summed (where a windowed local game's time goes). Reading the clock a few
/// times a pass costs nothing next to a pass.
#[derive(Debug, Clone, Default)]
pub struct PassPerf {
    /// Passes of the loop, the passes that ran the server tick, and the 32 ms steps those ticks covered.
    pub passes: u64,
    pub tick_passes: u64,
    pub ticks: u64,
    /// The host's own work before each pass (`run`'s `until`: a client host's network, prediction and canvas).
    pub until: std::time::Duration,
    /// The socket's drain and the scripts it ran.
    pub receive: std::time::Duration,
    /// The server tick (the objects' pass, the moves).
    pub tick: std::time::Duration,
    /// The send pass (ghosting, scoping, the events and the packets).
    pub send: std::time::Duration,
    /// The Sim events (schedules).
    pub events: std::time::Duration,
    /// The time-outs.
    pub timeouts: std::time::Duration,
    /// The longest single tick-and-send of a pass.
    pub max_tick_send: std::time::Duration,
}

impl PassPerf {
    /// The sums so far, cleared.
    pub fn take(&mut self) -> PassPerf {
        std::mem::take(self)
    }
}

/// The server's main loop and what it holds.
pub struct Server<N: NetParts = NoNet> {
    pub vm: Vm,
    /// What the engine natives record (the connections' settings, the ghost table, the process list, ...) and the game's
    /// state.
    pub state: Rc<RefCell<ServerState<N>>>,
    /// The errors of the events that failed while the server ran (each event is an outermost run of its own;
    /// the others still ran), the first [`MAX_EVENT_ERRORS`] of them.
    pub event_errors: Vec<VmError>,
    /// How many more there were.
    pub event_errors_dropped: usize,
    /// The host's limits on the event queue and on a tick ([`set_event_limits`](Self::set_event_limits)); the
    /// `Vm` has them from the moment they are set.
    event_limits: EventLimits,
    /// Passes whose Sim events ran with the hooks' slice ([`Server::pass_slice`]).
    hook_sliced_passes: u64,
    run_step_limit: u64,
    /// The network host, if one was attached ([`attach_net`](Self::attach_net)).
    net: Option<Rc<dyn NetHost>>,
    /// The scripts read the computer's clock ([`use_system_clock`](Self::use_system_clock)).
    system_clock: Rc<Cell<bool>>,
    /// The steps the events of every [`run`](Self::run) so far used ([`run_steps`](Self::run_steps)).
    run_steps_total: u64,
    /// The last tick's events ended on the time slice ([`last_tick_sliced`](Self::last_tick_sliced)).
    last_tick_sliced: bool,
    /// How many ticks did ([`sliced_ticks`](Self::sliced_ticks)).
    sliced_ticks: u64,
    /// The first main-loop pass has run ([`run`](Self::run)'s first tick, to [`FIRST_PASS_MS`]).
    main_loop_started: bool,
    /// How far a realtime host's clock is behind the wall before the next pass ([`set_catch_up`](Self::set_catch_up));
    /// taken by it.
    catch_up_ms: u32,
    /// The wall time the next pass covers ([`set_pass_elapsed`](Self::set_pass_elapsed)); taken by it.
    pass_ms: Option<u32>,
    /// The wall time the run loop's passes spent per phase; [`PassPerf::take`] reads and clears it.
    pub perf: PassPerf,
    /// What the game adds to the main loop ([`ServerHooks`]).
    hooks: Rc<dyn ServerHooks>,
    /// The plugins' work around each tick ([`TickHooks`]).
    tick_hooks: Option<Rc<dyn TickHooks>>,
    /// The ticks run so far ([`ticks`](Self::ticks)).
    ticks: u64,
    /// How a named mission is loaded ([`MissionLoader`]).
    missions: Rc<dyn MissionLoader>,
}

impl<N: NetParts> AsMut<Server<N>> for Server<N> {
    fn as_mut(&mut self) -> &mut Server<N> {
        self
    }
}

impl Server {
    /// A plain Torque server over `vm` (its files, classes and natives are the caller's) with a fresh engine state, no hooks
    /// and TGE's mission load.
    pub fn new(vm: Vm) -> Server {
        Server::with_state(vm, Rc::new(RefCell::new(ServerState::default())))
    }
}

impl<N: NetParts> Server<N> {
    /// A server over `vm` and the engine state its natives share (`state`), with no hooks and TGE's mission load
    /// ([`ExecMission`]); the `Vm` gets the [`STEP_LIMIT`].
    pub fn with_state(mut vm: Vm, state: Rc<RefCell<ServerState<N>>>) -> Server<N> {
        vm.set_step_limit(Some(STEP_LIMIT));
        Server {
            vm,
            state,
            event_errors: Vec::new(),
            event_errors_dropped: 0,
            event_limits: EventLimits::default(),
            hook_sliced_passes: 0,
            run_step_limit: RUN_STEP_LIMIT,
            net: None,
            system_clock: Rc::new(Cell::new(false)),
            run_steps_total: 0,
            last_tick_sliced: false,
            sliced_ticks: 0,
            main_loop_started: false,
            catch_up_ms: 0,
            pass_ms: None,
            perf: PassPerf::default(),
            hooks: Rc::new(NoHooks),
            tick_hooks: None,
            ticks: 0,
            missions: Rc::new(ExecMission),
        }
    }

    /// Sets the work run around each tick's process list pass ([`TickHooks`]), or clears it. Apart from [`set_hooks`](Self::set_hooks):
    /// a game that replaces its hooks keeps these.
    pub fn set_tick_hooks(&mut self, hooks: Option<Rc<dyn TickHooks>>) {
        self.tick_hooks = hooks;
    }

    /// The server ticks run so far (the first pass runs 32 of them at once).
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Replaces the main loop's hooks.
    pub fn set_hooks(&mut self, hooks: Rc<dyn ServerHooks>) {
        self.hooks = hooks;
    }

    /// Replaces the mission loader.
    pub fn set_mission_loader(&mut self, loader: Rc<dyn MissionLoader>) {
        self.missions = loader;
    }

    /// Loads `mission` with the server's [`MissionLoader`]; the answer is the loader's report.
    pub fn load_mission(&mut self, mission: &[u8]) -> Result<Box<dyn std::any::Any>, MissionError> {
        let loader = Rc::clone(&self.missions);
        loader.load(&mut self.vm, mission)
    }

    /// Gives the scripts the computer's clocks ([`SystemClock`]) instead of the `Vm`'s own: for a server that runs in real
    /// time (any server with a socket: a client's clock is the wall's).
    pub fn use_system_clock(&mut self) {
        self.vm.set_clock(SystemClock::new());
        self.system_clock.set(true);
    }

    /// Shares `flag` as the server's "the scripts read the system clock" flag (a network that refuses to open a real socket
    /// until [`use_system_clock`](Self::use_system_clock) was called reads the same flag).
    pub fn share_clock_flag(&mut self, flag: Rc<Cell<bool>>) {
        self.system_clock = flag;
    }

    /// Whether the scripts read the system clock.
    pub fn uses_system_clock(&self) -> bool {
        self.system_clock.get()
    }

    /// Whether a network host may be attached now: none is yet, no datablock exists (their keys start with the host), and a
    /// host over a `real` socket needs the system clock.
    pub fn can_attach_net(&self, real: bool) -> Result<(), AttachError> {
        if self.net.is_some() {
            return Err(AttachError::AlreadyAttached);
        }
        let objects = self.vm.objects();
        if objects
            .members(objects.datablock_group())
            .is_some_and(|m| !m.is_empty())
        {
            return Err(AttachError::DatablocksAlreadyExist);
        }
        if real && !self.system_clock.get() {
            return Err(AttachError::RealSocketNeedsSystemClock);
        }
        Ok(())
    }

    /// Gives the server its network host, which every pass of [`run`](Self::run) drives from now on
    /// ([`can_attach_net`](Self::can_attach_net) says whether one may be attached).
    pub fn attach_net(&mut self, net: Rc<dyn NetHost>) -> Result<(), AttachError> {
        if self.net.is_some() {
            return Err(AttachError::AlreadyAttached);
        }
        self.net = Some(net);
        Ok(())
    }

    /// The network host, if one is attached.
    pub fn net_host(&self) -> Option<&Rc<dyn NetHost>> {
        self.net.as_ref()
    }

    /// Whether no event is pending: the server has nothing more to do on its own (a start-up's delete events count until
    /// they fire).
    pub fn is_idle(&mut self) -> bool {
        self.vm.events().is_empty()
    }

    /// Sets the steps the events of one [`run`](Self::run) may use together (default [`RUN_STEP_LIMIT`]).
    pub fn set_run_step_limit(&mut self, steps: u64) {
        self.run_step_limit = steps;
    }

    /// The steps the events of one [`run`](Self::run) may use together.
    pub fn run_step_limit(&self) -> u64 {
        self.run_step_limit
    }

    /// The steps the events of all the [`run`](Self::run) calls so far used (counted as the budget is: what an
    /// event ran plus the work of the native it ended with, at least one for each). A host sizing
    /// [`RUN_STEP_LIMIT`] reads the difference over the stretch it measures.
    pub fn run_steps(&self) -> u64 {
        self.run_steps_total
    }

    /// Sets the limits on the event queue (how many events and bytes of arguments can be pending: `schedule`
    /// returns 0 with a warning past them) and on a tick (how many events, and how many steps all of them
    /// together, one `advance_time` may fire; [`run`](Self::run) also bounds a tick's steps by what is left of
    /// the run's budget). The `Vm` has them at once, so they hold for the start-up and for every `schedule`
    /// before the first tick; set them through here and not through `server.vm`, which [`run`](Self::run)
    /// would override. The default is `EventLimits::default()`, which is what the `Vm` starts with.
    pub fn set_event_limits(&mut self, limits: EventLimits) {
        self.event_limits = limits;
        self.vm.set_event_limits(limits);
    }

    /// The limits [`set_event_limits`](Self::set_event_limits) set.
    pub fn event_limits(&self) -> EventLimits {
        self.event_limits
    }

    /// A failed bind does not run the bind-failure callback the host's scripts define (which may move the server to another
    /// port), so the server keeps the socket its start-up bound. A host that serves clients over a real socket sets it; off by
    /// default.
    pub fn keep_first_socket(&mut self, keep: bool) {
        self.state.borrow_mut().keep_first_socket = keep;
    }

    /// The pass slice of a server with a socket: a tick's Sim events end after the batch in which their steps passed `slice`
    /// (`EventLimits::per_pass_steps`), the rest due first in the next tick, so a long `schedule(0)` chain spreads over ticks
    /// while the network keeps sending. None (the default) runs a tick's whole chain in the tick.
    pub fn set_pass_slice(&mut self, slice: Option<u64>) {
        self.set_event_limits(EventLimits {
            per_pass_steps: slice,
            ..self.event_limits
        });
    }

    /// The slice of the wall this pass's Sim events take, as the hooks decide it ([`ServerHooks::pass_slice`]).
    pub fn pass_slice(&self) -> Option<PassSlice> {
        let (remote_client, local_client) = self
            .net
            .as_ref()
            .map_or((false, false), |net| net.clients());
        self.hooks.pass_slice(&PassView {
            vm: &self.vm,
            host_pass_steps: self.event_limits.per_pass_steps,
            remote_client,
            local_client,
        })
    }

    /// How many passes so far ran their Sim events with the hooks' slice ([`ServerHooks::pass_slice`]).
    pub fn hook_sliced_passes(&self) -> u64 {
        self.hook_sliced_passes
    }

    /// Whether the last tick's events ended on the time slice ([`set_pass_slice`](Self::set_pass_slice)) with events still due.
    pub fn last_tick_sliced(&self) -> bool {
        self.last_tick_sliced
    }

    /// How many ticks so far ended on the time slice.
    pub fn sliced_ticks(&self) -> u64 {
        self.sliced_ticks
    }

    /// Advances the clock until a script calls `quit()`, `until` holds or the limit or the run's step budget
    /// is spent: the very first tick of a server is the first main-loop pass, from the clock's 0 to
    /// [`FIRST_PASS_MS`] at once (it counts against `limit_ms`, and is made whatever the limit, however small:
    /// a `limit_ms` of 1 ends with the clock at 1,024; only a limit of 0 runs no tick); every other tick is
    /// [`TICK_MS`] (the last one shorter, so the clock never passes `limit_ms` from where it was, the first
    /// pass apart). Before every tick, in this order: `quit()` ends the run ([`RunEnd::Quit`]: a start-up that
    /// quit runs no tick), then `until` is asked (it sees the server and may read anything; [`RunEnd::Until`]; a
    /// condition that already holds runs no tick), then the limits. The tick that calls `quit()` finishes.
    ///
    /// Each tick fires the events due by its end, each an outermost run of its own whose error goes to
    /// [`event_errors`](Self::event_errors) while the others still run. Every fired call is billed at least
    /// one step, and the work of the native it ended with (`export`, `strreplace`: nothing runs after them to
    /// charge it); so a run costs at most the budget plus the steps of the one event that crossed it, which is
    /// at most [`STEP_LIMIT`]; the loop itself is bounded by `limit_ms` / 32 ticks.
    ///
    /// **The budget is per call.** A second `run` after [`RunEnd::StepLimit`] has a whole budget again, so a
    /// host that wants a bound on all the work a server may do treats `StepLimit` as the end of the server
    /// (and finishes it), or keeps its own total. While it runs, the `Vm`'s own `per_advance_steps` is the
    /// tick's total (the smaller of the host's and what is left of the budget); afterwards it is the host's
    /// again ([`event_limits`](Self::event_limits)), so a direct `vm.advance_time` is not capped by it.
    ///
    /// A realtime host that fell behind the wall says so before a tick ([`set_catch_up`](Self::set_catch_up)): that pass is
    /// then the whole backlog at once.
    pub fn run(&mut self, limit_ms: u32, until: impl FnMut(&mut Self) -> bool) -> RunEnd {
        Self::run_on(self, limit_ms, until)
    }

    /// [`run`](Self::run) of the server `host` holds, whose `until` sees `host` (a game's server that holds this one).
    pub fn run_on<H: AsMut<Self>>(
        host: &mut H,
        limit_ms: u32,
        until: impl FnMut(&mut H) -> bool,
    ) -> RunEnd {
        let end = Self::run_ticks_on(host, limit_ms, until);
        let this = host.as_mut();
        this.vm.set_event_limits(this.event_limits);
        end
    }

    /// For a realtime host (one whose clock follows the wall), from its `until` hook: the server's clock is `behind_ms` behind
    /// the wall (a stall: a long script, a large load at once). The next pass then runs every whole [`TICK_MS`] of it as ONE
    /// batch, as the main loop does with the time a pass took (`processServerTick` once per 32 ms step, then the move credit of
    /// every connection raised by the step count, capped at 30), and the receive, send and timeout passes run once around it:
    /// one packet per connection, whatever the backlog. Stepping 32 ms a pass instead would send one packet per missed tick,
    /// all in the same millisecond, because the send gate's credit (up to 1,000 ms) lets each through. Less than one tick (and
    /// the default, 0) is one tick; the first pass is not changed; the value is taken by the next pass.
    pub fn set_catch_up(&mut self, behind_ms: u32) {
        self.catch_up_ms = behind_ms;
    }

    /// For a realtime host that runs the main loop's passes ([`crate::pacing`]), from its `until` hook: the next pass covers
    /// `elapsed_ms` of wall time (the loop's time event). That pass then does what `processTimeEvent` does with it: the elapsed
    /// time capped at [`MAX_PASS_MS`] (the rest is lost), the server tick once for every 32 ms boundary the server's time
    /// crosses (`processServerTick` per step, the move credit raised by the step count; none, and so no send pass, when it
    /// crosses none), then the Sim events advanced by the whole elapsed time (`Sim::advanceToTime(now + elapsed)`, whose events
    /// see the pass's target as the current time). The first pass is not changed (it jumps to [`FIRST_PASS_MS`]); the value is
    /// taken by the next pass and wins over [`set_catch_up`](Self::set_catch_up). Without it every pass is one 32 ms tick (the
    /// in-memory and virtual-clock hosts).
    pub fn set_pass_elapsed(&mut self, elapsed_ms: u32) {
        self.pass_ms = Some(elapsed_ms);
    }

    /// [`run`](Self::run) for a server with a socket, which does not end on [`RunEnd::StepLimit`]: the events of a run draw on
    /// its step budget, and the Sim events a stranger's datagrams cause (what `onConnectRequest` or a command `schedule()`s)
    /// are among them, so a budget that ends the run would let a stranger end the server. On `StepLimit` the run starts again
    /// with a budget of its own (the clock goes on, `limit_ms` counted from the first call), after `on_restart(server, restarts
    /// so far)` (the caller says so on its console). Every other end is returned. A server without a socket should use `run`
    /// and finish on `StepLimit`.
    pub fn run_restarting(
        &mut self,
        limit_ms: u32,
        until: impl FnMut(&mut Self) -> bool,
        on_restart: impl FnMut(&mut Self, u64),
    ) -> RunEnd {
        Self::run_restarting_on(self, limit_ms, until, on_restart)
    }

    /// [`run_restarting`](Self::run_restarting) of the server `host` holds ([`run_on`](Self::run_on)).
    pub fn run_restarting_on<H: AsMut<Self>>(
        host: &mut H,
        limit_ms: u32,
        mut until: impl FnMut(&mut H) -> bool,
        mut on_restart: impl FnMut(&mut H, u64),
    ) -> RunEnd {
        let mut restarts = 0u64;
        let mut limit = limit_ms;
        loop {
            let end = Self::run_on(host, limit, &mut until);
            if end != RunEnd::StepLimit || host.as_mut().run_step_limit == 0 {
                return end;
            }
            restarts = restarts.saturating_add(1);
            on_restart(host, restarts);
            // The clock started at 0 with the first call, and the first pass counted against the limit like every tick.
            limit = limit_ms.saturating_sub(host.as_mut().vm.sim_time());
        }
    }

    /// The loop of [`run_on`](Self::run_on).
    fn run_ticks_on<H: AsMut<Self>>(
        host: &mut H,
        limit_ms: u32,
        mut until: impl FnMut(&mut H) -> bool,
    ) -> RunEnd {
        let mut elapsed = 0u32;
        let mut steps = 0u64;
        loop {
            if host.as_mut().vm.quit_requested() {
                return RunEnd::Quit;
            }
            let t_until = web_time::Instant::now();
            let stop = until(host);
            let this = host.as_mut();
            this.perf.until += t_until.elapsed();
            if stop {
                return RunEnd::Until;
            }
            let Some(left) = limit_ms.checked_sub(elapsed).filter(|left| *left > 0) else {
                return RunEnd::TimeLimit;
            };
            if this.run_step_limit.saturating_sub(steps) == 0 {
                return RunEnd::StepLimit;
            }
            // The first pass moves the clock to 1,024 whatever is left of the limit (it counts against it). A pass after a
            // realtime host's stall is the whole backlog (`set_catch_up`), one tick otherwise.
            let backlog = std::mem::take(&mut this.catch_up_ms);
            let pass = this.pass_ms.take();
            let sim = this.vm.sim_time();
            let (delta, steps_in_batch, tick_ran) = match pass {
                // A pass of the main loop: the server ticks once for every 32 ms boundary its time crosses (the server time and
                // the last tick's are kept apart, the tick's at a multiple of 32).
                Some(ms) if this.main_loop_started => {
                    let delta = left.min(ms.min(MAX_PASS_MS));
                    let ticks = (u64::from(sim) + u64::from(delta)) / u64::from(TICK_MS)
                        - u64::from(sim) / u64::from(TICK_MS);
                    let ticks = ticks as u32;
                    (delta, ticks, ticks > 0)
                }
                _ => {
                    let delta = if this.main_loop_started {
                        left.min((backlog / TICK_MS).max(1).saturating_mul(TICK_MS))
                    } else {
                        FIRST_PASS_MS.saturating_sub(sim)
                    };
                    // The tick the loop is about to run: the first pass and every full tick run the server tick (and so the
                    // send pass); a last tick cut short by the limit does not (the server ticks once 32 ms have accumulated).
                    (
                        delta,
                        delta / TICK_MS,
                        !this.main_loop_started || delta >= TICK_MS,
                    )
                }
            };
            let net = this.net.clone();
            let mut now = 0;
            this.perf.passes += 1;
            let t_receive = web_time::Instant::now();
            if let Some(net) = &net {
                // 1. The socket is drained first, with the scripts it runs inside the drain. The network's scripts run under
                // the network's own limits, not this run's budget (what a script schedules for later is a Sim event and does
                // draw on it, step 4).
                now = net.clock_ms(this.vm.sim_time());
                net.set_tick(this.ticks);
                net.phase(Phase::Receive);
                net.receive(&mut this.vm, now, true);
                net.phase(Phase::Tick);
            }
            // 2. The server tick: the process list's pass over the objects (`advanceObjects`, once per 32 ms step; with a
            // network host the connections' control objects tick their moves in it) and the move credit, and 3. the send pass,
            // only if a tick ran. The batch is the 32 ms steps the clock moves (the first pass: 32). A server with no network
            // host ticks every object too.
            this.perf.receive += t_receive.elapsed();
            let t_tick = web_time::Instant::now();
            if tick_ran {
                this.perf.tick_passes += 1;
                this.perf.ticks += u64::from(steps_in_batch);
                let process = this.state.borrow().process.clone();
                let first_tick = this.ticks;
                this.ticks = this.ticks.saturating_add(u64::from(steps_in_batch));
                // A development server (the asset watcher's gate) looks at its shape files once per interval: a changed one is
                // read again and its datablocks marked for the drain (`torque_game::shapes::reload`).
                let dev = torque_engine::script::dev_allowed(&this.vm);
                let reload_errors = torque_game::shapes::reload::poll(
                    &mut this.vm,
                    dev,
                    torque_engine::assets::HOT_RELOAD_INTERVAL_MS,
                );
                this.keep_errors(reload_errors);
                // In development, a script file that declared datablocks and changed on disk is run again (its own drain
                // and round inside), then the datablocks the scripts changed since the last tick refresh their users, once
                // for the batch, before any object steps (`torque_game::scene::refresh`).
                let reload_errors = crate::reload::poll(&mut this.vm);
                this.keep_errors(reload_errors);
                let refresh_errors = torque_game::scene::refresh::drain(&mut this.vm);
                this.keep_errors(refresh_errors);
                let hooks = this.tick_hooks.clone();
                if let Some(hooks) = &hooks {
                    for n in 1..=u64::from(steps_in_batch) {
                        hooks.before(&mut this.vm, TickInfo::new(first_tick.saturating_add(n)));
                    }
                }
                // The pass pays from what is left of the run's budget, a step per visit before the object's turn and its tick's
                // work (`sim::pass`); it stops before the next object once that is spent.
                let left = this.run_step_limit.saturating_sub(steps);
                let outcome = match &net {
                    Some(net) => net.advance_objects(&mut this.vm, &process, steps_in_batch, left),
                    None => pass::advance_objects_within(
                        &mut this.vm,
                        &process,
                        &mut NoControl,
                        steps_in_batch,
                        left,
                    ),
                };
                // Not part of `run_steps` (that is what the events used).
                steps = steps.saturating_add(outcome.steps);
                this.keep_errors(outcome.errors);
                if let Some(hooks) = &hooks {
                    for n in 1..=u64::from(steps_in_batch) {
                        hooks.after(&mut this.vm, TickInfo::new(first_tick.saturating_add(n)));
                    }
                }
                this.perf.tick += t_tick.elapsed();
                let t_send = web_time::Instant::now();
                if let Some(net) = &net {
                    net.phase(Phase::Send);
                    net.send(&mut this.vm, now);
                }
                this.perf.send += t_send.elapsed();
                this.perf.max_tick_send = this.perf.max_tick_send.max(t_tick.elapsed());
            }
            let t_events = web_time::Instant::now();
            // The game's work of the pass between the tick and the events.
            let hooks = Rc::clone(&this.hooks);
            hooks.before_events(&mut this.vm, delta);
            if let Some(net) = &net {
                net.phase(Phase::SimEvents);
            }
            // 4. The Sim event queue, with what is left of the run's budget.
            let steps_left = this.run_step_limit.saturating_sub(steps);
            let per_tick = this
                .event_limits
                .per_advance_steps
                .map_or(steps_left, |own| own.min(steps_left));
            // The game may give the events a slice of the wall instead of the host's steps.
            let slice = this.pass_slice();
            let wall = slice.map(|s| s.wall);
            let per_pass_steps = match slice {
                Some(s) => Some(s.steps),
                None => this.event_limits.per_pass_steps,
            };
            this.hook_sliced_passes = this
                .hook_sliced_passes
                .saturating_add(u64::from(wall.is_some()));
            hooks.pass_sliced(wall.is_some());
            this.vm.set_event_limits(EventLimits {
                per_advance_steps: Some(per_tick),
                per_pass_steps,
                per_pass_wall: wall,
                ..this.event_limits
            });
            let tick = this.vm.advance_time_counted(delta);
            this.perf.events += t_events.elapsed();
            this.last_tick_sliced = tick.sliced;
            this.sliced_ticks = this.sliced_ticks.saturating_add(u64::from(tick.sliced));
            this.main_loop_started = true;
            elapsed = elapsed.saturating_add(delta);
            steps = steps.saturating_add(tick.steps);
            this.run_steps_total = this.run_steps_total.saturating_add(tick.steps);
            this.keep_errors(tick.errors);
            // 5. The timeouts.
            let t_timeouts = web_time::Instant::now();
            if let Some(net) = &net {
                let now = net.clock_ms(this.vm.sim_time());
                net.phase(Phase::Timeouts);
                net.timeouts(&mut this.vm, now);
            }
            this.perf.timeouts += t_timeouts.elapsed();
        }
    }

    /// Keeps the errors of a tick's events up to [`MAX_EVENT_ERRORS`] and counts the rest.
    fn keep_errors(&mut self, errors: Vec<VmError>) {
        for error in errors {
            if self.event_errors.len() < MAX_EVENT_ERRORS {
                self.event_errors.push(error);
            } else {
                self.event_errors_dropped = self.event_errors_dropped.saturating_add(1);
            }
        }
    }

    /// What this run needed that the engine does not have yet (see [`Report`]): the missing functions the log's
    /// warnings name, the stub methods called so far, and the failed events (kept and dropped).
    pub fn report(&self) -> Report {
        let mut stubs: Vec<(String, u64)> = self
            .vm
            .stub_calls()
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect();
        stubs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Report {
            missing: missing_functions(&self.vm),
            stubs,
            event_errors: self
                .event_errors
                .len()
                .saturating_add(self.event_errors_dropped),
        }
    }

    /// The end of the server: `onExit` (what the host loop calls after its last tick; silent when no script
    /// defines it, and an error in it does not stop the shutdown), then `Vm::shutdown`. Returns the whole
    /// log, with whatever `onExit` and the shutdown printed. [`finish_report`](Self::finish_report) also says
    /// how `onExit` ended, and what the run needed.
    pub fn finish(self) -> Vec<LogLine> {
        self.finish_report().log
    }

    /// [`finish`](Self::finish) with the result of `onExit`: a script that loops or recurses in it ends with
    /// its error ([`Finished::exit`]) and the shutdown still happens. `onExit` is a run of its own under the
    /// `Vm`'s step limit; what it schedules never fires. [`Finished::report`] is taken once `onExit` has run.
    pub fn finish_report(mut self) -> Finished {
        let exit = self.vm.invoke(None, b"onExit", &[]).map(|_| ());
        let report = self.report();
        Finished {
            log: self.vm.shutdown(),
            exit,
            report,
        }
    }
}

/// What a host built over a server needs of it (a client host over a game's server, which holds this one): the script engine
/// and the main loop.
pub trait GameServer {
    fn vm(&self) -> &Vm;
    fn vm_mut(&mut self) -> &mut Vm;
    /// Runs the clock for at most `limit_ms` of simulation time, `until` asked before every tick (true ends the run), as
    /// [`Server::run`] does.
    fn run_ticks(&mut self, limit_ms: u32, until: impl FnMut(&mut Self) -> bool) -> RunEnd;
}

impl<N: NetParts> GameServer for Server<N> {
    fn vm(&self) -> &Vm {
        &self.vm
    }

    fn vm_mut(&mut self) -> &mut Vm {
        &mut self.vm
    }

    fn run_ticks(&mut self, limit_ms: u32, until: impl FnMut(&mut Self) -> bool) -> RunEnd {
        self.run(limit_ms, until)
    }
}
