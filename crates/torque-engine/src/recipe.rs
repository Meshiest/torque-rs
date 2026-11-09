//! Recipes: what plugins record to run on every new script engine, filed by [`Stage`].

use std::fmt;
use std::rc::Rc;

use torque_ts_vm::vm::Vm;

use crate::param::IntoServerTick;
use crate::{Engine, PluginId};

/// A point in making a server's script engine at which recipes run. The host making the engine replays each stage in
/// this order, at the moment the stage's parts exist.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum ServerStage {
    /// The game classes' console classes, first of all.
    Classes,
    /// The game's own preparations that the server game reads when it is installed (its simulation classes).
    Prepare,
    /// The server game: its simulation and natives.
    Game,
    /// The world's objects: terrain, water.
    World,
    /// The network host and its passes.
    Net,
    /// Once the server's entry script has run, before its start function: where mods' scripts run (the engine's
    /// own hosts replay it; a host that runs no entry script replays it when it would have).
    Entry,
}

impl ServerStage {
    /// Every server stage, in replay order.
    pub const ALL: &'static [ServerStage] = &[
        ServerStage::Classes,
        ServerStage::Prepare,
        ServerStage::Game,
        ServerStage::World,
        ServerStage::Net,
        ServerStage::Entry,
    ];
}

/// A point in making a client's script engine at which recipes run. The host making the engine replays each stage in
/// this order, at the moment the stage's parts exist.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum ClientStage {
    /// The game classes' console classes, when the client has them.
    Classes,
    /// The client game: what draws and predicts the game's objects.
    Game,
    /// The GUI's classes and natives.
    Gui,
    /// Input: action maps and their natives.
    Input,
    /// The screen: resolution, full screen and the canvas's natives.
    Screen,
    /// The connection to a server, when the client has one.
    Conn,
    /// The server query (the server browser's natives).
    Query,
    /// The game's own natives and globals, just before its client script runs.
    Natives,
    /// Once the client's entry script has run: where mods' scripts run (`ClientBuilder::exec_entry` replays it).
    Entry,
    /// The client host that runs the frames.
    Host,
    /// Once the window exists.
    Window,
}

impl ClientStage {
    /// Every client stage, in replay order.
    pub const ALL: &'static [ClientStage] = &[
        ClientStage::Classes,
        ClientStage::Game,
        ClientStage::Gui,
        ClientStage::Input,
        ClientStage::Screen,
        ClientStage::Conn,
        ClientStage::Query,
        ClientStage::Natives,
        ClientStage::Entry,
        ClientStage::Host,
        ClientStage::Window,
    ];
}

/// A server or a client stage: what [`Engine::replay`] takes (both stage types convert into it).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Stage {
    /// A stage of a server's script engine.
    Server(ServerStage),
    /// A stage of a client's script engine.
    Client(ClientStage),
}

impl From<ServerStage> for Stage {
    fn from(stage: ServerStage) -> Stage {
        Stage::Server(stage)
    }
}

impl From<ClientStage> for Stage {
    fn from(stage: ClientStage) -> Stage {
        Stage::Client(stage)
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stage::Server(stage) => write!(f, "server stage {stage:?}"),
            Stage::Client(stage) => write!(f, "client stage {stage:?}"),
        }
    }
}

/// The function a recipe runs: it gets the new script engine and the engine it was built from (for its resources and
/// slots; clone the [`Engine`] to keep it in a native). An `Err` stops the replay.
pub(crate) type RecipeFn = Box<dyn Fn(&mut Vm, &Engine) -> Result<(), String>>;

pub(crate) struct Recipe {
    pub(crate) stage: Stage,
    pub(crate) plugin: PluginId,
    pub(crate) run: RecipeFn,
}

/// A recipe that failed while [`Engine::replay`] ran it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecipeError {
    /// The plugin that recorded the recipe.
    pub plugin: PluginId,
    /// The stage being replayed.
    pub stage: Stage,
    /// What the recipe returned.
    pub message: String,
}

impl fmt::Display for RecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "plugin `{}` failed at the {}: {}",
            self.plugin, self.stage, self.message
        )
    }
}

impl std::error::Error for RecipeError {}

/// Work a plugin has every client do once per tick, beside its recipes: pushed onto the engine's list of `ClientTick`s
/// (`engine.push(ClientTick::new(..))`) and run by the host after the tick's other client work, in the order pushed. It
/// gets the client's script engine, where a plugin finds its state again (a [`set_extension`](Vm::set_extension) its recipe
/// made).
#[derive(Clone)]
pub struct ClientTick(Rc<dyn Fn(&mut Vm)>);

impl ClientTick {
    /// A hook that runs `work`.
    pub fn new(work: impl Fn(&mut Vm) + 'static) -> ClientTick {
        ClientTick(Rc::new(work))
    }

    /// Runs the hook on `vm`.
    pub fn run(&self, vm: &mut Vm) {
        (self.0)(vm);
    }
}

/// One tick of a server: what a [`ServerTick`] hook is told.
///
/// Tick `number` is the count of server ticks started, from 1, and happens at `number * 32` ms of server time
/// ([`time_ms`](Self::time_ms)), whatever the host's pacing (the first pass of a server runs 32 ticks at once, the way the
/// main loop's first pass covers its start-up). It is a [`Param`](crate::Param): a hook that names `TickInfo` as a parameter
/// is given it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TickInfo {
    /// The tick's number, 1 for the first.
    pub number: u64,
    /// The server time of the tick in milliseconds: `number * 32`.
    pub time_ms: u64,
}

impl TickInfo {
    /// The length of a server tick in milliseconds.
    pub const MS: u64 = 32;

    /// Tick number `number` (the first is 1).
    pub const fn new(number: u64) -> TickInfo {
        TickInfo {
            number,
            time_ms: number.saturating_mul(TickInfo::MS),
        }
    }
}

/// Where in a server tick a [`ServerTick`] hook runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum TickPhase {
    /// Before the process list ticks the objects, after the tick's network receive.
    Before,
    /// After the process list ticked the objects, before the send pass and the Sim events.
    After,
    /// After every `After` hook of the tick, whatever plugin pushed it: where work that settles what the others moved
    /// goes (the scene hierarchy writes the children of the objects an `After` hook moved, a physics step's props, say).
    Resolve,
}

/// The function a tick hook runs.
pub(crate) type TickFn = Rc<dyn Fn(&mut Vm, &Engine, TickInfo) -> Result<(), String>>;

/// Work a plugin has every server do once per tick (32 ms), pushed onto the engine's list of `ServerTick`s
/// (`engine.push(ServerTick::after(..))`). The host runs the hooks around the process list's pass over the objects:
/// [`before`](Self::before) hooks first, [`after`](Self::after) hooks once the objects have ticked, then
/// [`resolve`](Self::resolve) hooks, each group in the order the hooks were pushed (so in the order plugins were built).
/// Within a tick all the `before` hooks run, then the process list, then all the `after` hooks, then all the `resolve`
/// hooks.
///
/// A hook is a function of [injected parameters](crate::param): the server's script engine (`VmMut`), resources, slots and
/// the tick (`TickInfo`), so a plugin ticks without the game calling a script function. A hook that returns an error
/// prints it on the server's console and the server goes on, with the other hooks.
///
/// A pass that covers several ticks at once (the first pass of a server, or one after a stall) runs the hooks for each of
/// them, in tick order: the `before` hooks of every tick, then the process list's pass over all of them, then the `after`
/// and `resolve` hooks of each tick in turn. A server with no hook pushed runs its ticks exactly as without this type.
///
/// ```
/// use torque_engine::{EngineBuilder, ServerTick, TickInfo, VmMut};
///
/// fn clock(mut vm: VmMut, tick: TickInfo) {
///     vm.set_global("$Server::Ticks", &tick.number.to_string());
/// }
///
/// let mut builder = EngineBuilder::new();
/// builder.push(ServerTick::after(clock));
/// let engine = builder.build()?;
/// let hooks = engine.list::<ServerTick>();
/// let mut vm = torque_ts_vm::vm::Vm::new();
/// hooks[0].run(&mut vm, &engine, TickInfo::new(7))?;
/// assert_eq!(vm.global("$Server::Ticks").as_deref(), Some(&b"7"[..]));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct ServerTick {
    phase: TickPhase,
    run: TickFn,
}

impl ServerTick {
    /// A hook that runs `work` before the process list's pass of each tick.
    pub fn before<M>(work: impl IntoServerTick<M>) -> ServerTick {
        ServerTick {
            phase: TickPhase::Before,
            run: work.into_tick(),
        }
    }

    /// A hook that runs `work` after the process list's pass of each tick.
    pub fn after<M>(work: impl IntoServerTick<M>) -> ServerTick {
        ServerTick {
            phase: TickPhase::After,
            run: work.into_tick(),
        }
    }

    /// A hook that runs `work` after every [`after`](Self::after) hook of each tick.
    pub fn resolve<M>(work: impl IntoServerTick<M>) -> ServerTick {
        ServerTick {
            phase: TickPhase::Resolve,
            run: work.into_tick(),
        }
    }

    /// Where in the tick the hook runs.
    pub fn phase(&self) -> TickPhase {
        self.phase
    }

    /// Runs the hook on `vm` for `tick` (a host's call; an `Err` is for it to print).
    pub fn run(&self, vm: &mut Vm, engine: &Engine, tick: TickInfo) -> Result<(), String> {
        (self.run)(vm, engine, tick)
    }
}
