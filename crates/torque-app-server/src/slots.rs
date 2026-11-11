//! The engine builder's seams for what a host is made of: the hooks every server's main loop runs and how its missions load,
//! and, for servers and clients alike, the script engine they run on and what starts them.
//!
//! | Seam | Setter | Shape |
//! |------|--------|-------|
//! | hooks ([`ServerHooks`]) | [`ServerSlots::server_hooks`] | a list: every plugin may add, in add order |
//! | mission loader ([`MissionLoader`]) | [`ServerSlots::mission_loader`] | exclusive: a second setter fails the build, naming both plugins |
//! | server game ([`GameParts`]) | [`ServerSlots::server_game`] | exclusive: the game's own container and shapes in place of TGE's |
//! | net game ([`NetGameFactory`]) | [`ServerSlots::net_game`] | exclusive ([`net_game`](crate::net_game)) |
//! | codecs ([`NetCodecs`]) | [`ServerSlots::net_codecs`] | exclusive ([`net_game`](crate::net_game)) |
//! | script engine ([`Vm`]) | [`HostSlots::script_engine`] | exclusive |
//! | server entry ([`ServerEntry`]) | [`HostSlots::server_entry`] | exclusive |
//! | client entry | [`HostSlots::client_entry`] | exclusive |
//!
//! A plugin gives a factory, not an instance: [`GameServer::start_engine`](crate::server::GameServer::start_engine) calls each
//! one once per server, after the `Net` stage and before the entry script, so what it makes can read what the server's recipes
//! left on the script engine (its extensions, its state). An error from a factory stops that server's start with the error's
//! message. A server whose engine has no entry keeps the plain engine's hooks and loader.
//!
//! The server game's factory runs in place of TGE's install in the `Game` stage (`ServerGamePlugin`'s recipe, which still adds
//! the plugins' collision after it); the terrain, the water and the network host then build on the parts it returned.
//!
//! The hooks of one server are run together: `before_events` and `pass_sliced` in add order, `pass_slice` answered by the first
//! hook that gives a slice ([`HookList`]). The `ServerTick` list is separate and unchanged (work per 32 ms tick); hooks are
//! for the once-a-pass work and the pass's slice of the wall, which a tick cannot express.
//!
//! **The script engine** of every server and client the engine makes comes from [`make_script_engine`], before any recipe
//! runs. Its executor and object tables are fixed when it is made, so a game that needs a script engine other than TGE's gives
//! its own factory. With none it is [`Vm::new`].
//!
//! **The entry** is what a host does once every stage but `Entry` has run. A server's plain entry runs the entry script
//! (`server.cs`, or the one `game.toml` names), the `Entry` stage, the start function, and turns connections on. A client's
//! (`ClientBuilder::exec_entry` in torque-app) runs its entry script, then the `Entry` stage. A game's entry replaces the
//! script and, on a server, the start function and the connections. The `Entry` stage still replays after it, and a server's
//! connections stay off until the game turns them on ([`Connections`]).

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use torque_engine::{Engine, EngineBuilder, PluginId};
use torque_server::game::GameParts;
use torque_server::net::NetGame;
use torque_server::net::codecs::NetCodecs;
use torque_server::server::{HookList, MakeHooks, MakeMissionLoader, MissionLoader, ServerHooks};
use torque_server::state::{NetParts, ServerState};
use torque_ts_vm::host::FileHost;
use torque_ts_vm::vm::Vm;

use crate::net_game::{CodecsSlot, NetGameFactory, NetGameSlot};
use crate::plugins::ServerWiring;
use crate::server::ServerStart;

/// A plugin's hooks factory, as the engine's list holds it, with the plugin that registered it.
struct HooksEntry {
    plugin: PluginId,
    make: Rc<MakeHooks>,
}

/// The engine's mission loader factory (the exclusive slot's value).
struct LoaderSlot(Rc<MakeMissionLoader>);

/// Makes a server's game parts over the server's wiring (its engine state: [`ServerWiring::state`]).
pub type MakeServerGame = dyn Fn(&mut Vm, &ServerWiring) -> Result<GameParts, String>;

/// The engine's server game factory (the exclusive slot's value).
pub(crate) struct ServerGameSlot(pub(crate) Rc<MakeServerGame>);

/// The engine builder's calls that set a server's main-loop seams.
pub trait ServerSlots {
    /// Adds the hooks `make` makes to every server's main loop. A list: each plugin's hooks run in add order.
    fn server_hooks(
        &mut self,
        make: impl Fn(&mut Vm) -> Result<Rc<dyn ServerHooks>, String> + 'static,
    ) -> &mut EngineBuilder;

    /// Sets how every server loads a mission: the loader `make` makes replaces TGE's (which `exec`s the `.mis`). Exclusive: a
    /// second setter fails the build, naming both plugins.
    fn mission_loader(
        &mut self,
        make: impl Fn(&mut Vm) -> Result<Rc<dyn MissionLoader>, String> + 'static,
    ) -> &mut EngineBuilder;

    /// Sets every server's game parts: `make` runs in the `Game` stage in place of TGE's install
    /// ([`torque_server::game::install`]), over the server's state, and its container and shapes are what the terrain, the
    /// water and the network host build on. Exclusive.
    fn server_game(
        &mut self,
        make: impl Fn(&mut Vm, &ServerWiring) -> Result<GameParts, String> + 'static,
    ) -> &mut EngineBuilder;

    /// Sets the engine's net game: `factory` makes each server's host parts ([`crate::net_game`]). Exclusive.
    fn net_game<G: NetGame>(&mut self, factory: Rc<dyn NetGameFactory<G>>) -> &mut EngineBuilder;

    /// Sets the codecs every server's network host speaks: `make` runs per server just before the `Net` stage. Exclusive.
    fn net_codecs(
        &mut self,
        make: impl Fn(&Vm) -> Rc<dyn NetCodecs> + 'static,
    ) -> &mut EngineBuilder;
}

impl ServerSlots for EngineBuilder {
    fn server_hooks(
        &mut self,
        make: impl Fn(&mut Vm) -> Result<Rc<dyn ServerHooks>, String> + 'static,
    ) -> &mut EngineBuilder {
        let plugin = self.current_plugin();
        self.push(HooksEntry {
            plugin,
            make: Rc::new(make),
        })
    }

    fn mission_loader(
        &mut self,
        make: impl Fn(&mut Vm) -> Result<Rc<dyn MissionLoader>, String> + 'static,
    ) -> &mut EngineBuilder {
        self.set_slot(Rc::new(LoaderSlot(Rc::new(make))))
    }

    fn server_game(
        &mut self,
        make: impl Fn(&mut Vm, &ServerWiring) -> Result<GameParts, String> + 'static,
    ) -> &mut EngineBuilder {
        self.set_slot(Rc::new(ServerGameSlot(Rc::new(make))))
    }

    fn net_game<G: NetGame>(&mut self, factory: Rc<dyn NetGameFactory<G>>) -> &mut EngineBuilder {
        self.set_slot(Rc::new(NetGameSlot::new(factory)))
    }

    fn net_codecs(
        &mut self,
        make: impl Fn(&Vm) -> Rc<dyn NetCodecs> + 'static,
    ) -> &mut EngineBuilder {
        self.set_slot(Rc::new(CodecsSlot(Rc::new(make))))
    }
}

/// Makes the server hooks `engine`'s plugins registered ([`ServerSlots::server_hooks`]) over `vm`, each factory once, in add
/// order. None when no plugin registered any (the server keeps its plain hooks), the hooks themselves when there is one, a
/// [`HookList`] of them when there are several. A factory's error is returned with its message.
///
/// For a host that does not go through `GameServer::start*` (those already apply the slots, after the `Net` stage and before the
/// entry script): call this at the same point, then `Server::set_hooks`. Calling it for a server `GameServer` made runs the
/// factories a second time.
pub fn make_server_hooks(
    engine: &Engine,
    vm: &mut Vm,
) -> Result<Option<Rc<dyn ServerHooks>>, String> {
    let mut made = Vec::new();
    for entry in engine.list::<HooksEntry>() {
        let hooks = (entry.make)(vm)
            .map_err(|e| format!("the server hooks of plugin `{}`: {e}", entry.plugin))?;
        made.push(hooks);
    }
    Ok(match made.len() {
        0 => None,
        1 => made.pop(),
        _ => Some(Rc::new(HookList::new(made))),
    })
}

/// Makes the mission loader `engine`'s plugins set ([`ServerSlots::mission_loader`]) over `vm`. None when no plugin set one (the
/// server keeps TGE's, which `exec`s the `.mis`); a factory's error is returned with its message. Only for a host that does not
/// go through `GameServer::start*` (which already applies it): call it at the same point as
/// [`make_server_hooks`], then `Server::set_mission_loader`.
pub fn make_mission_loader(
    engine: &Engine,
    vm: &mut Vm,
) -> Result<Option<Rc<dyn MissionLoader>>, String> {
    let Some(slot) = engine.slot::<LoaderSlot>() else {
        return Ok(None);
    };
    (slot.0)(vm)
        .map(Some)
        .map_err(|e| format!("the mission loader: {e}"))
}

/// Hands a script engine its files: a closure over the file host, so a host's constructor over any [`FileHost`] does not copy
/// its body for each file host (`GameServer::start_prepared`, torque-app's `ClientBuilder::from_engine_files`).
pub type FileSetter = Box<dyn FnOnce(&mut Vm)>;

/// The [`FileSetter`] that gives a script engine `files`.
pub fn file_setter(files: impl FileHost + 'static) -> FileSetter {
    Box::new(move |vm| vm.set_file_host(files))
}

/// Makes the script engine of a server or a client.
pub type MakeScriptEngine = dyn Fn() -> Vm;

/// A game's server entry: what a server runs in place of its entry script, its start function and turning connections on.
pub type RunServerEntry = dyn Fn(&mut ServerEntry<'_>) -> Result<(), String>;

/// A game's client entry: what a client runs in place of its entry script.
pub type RunClientEntry = dyn Fn(&mut Vm) -> Result<(), String>;

/// The engine's script engine factory (the exclusive slot's value).
struct ScriptEngineSlot(Rc<MakeScriptEngine>);

/// The engine's server entry (the exclusive slot's value).
struct ServerEntrySlot(Rc<RunServerEntry>);

/// The engine's client entry (the exclusive slot's value).
struct ClientEntrySlot(Rc<RunClientEntry>);

/// Turns a server's connections on or off (`allowConnections`): while off, every handshake datagram is dropped and the LAN
/// list's queries go unanswered. Cheap to clone: a game keeps one for the script function that opens its server. It does not
/// keep the server alive.
#[derive(Clone)]
pub struct Connections(Rc<dyn Fn(bool)>);

impl Connections {
    /// The switch of the server whose engine state is `state` (held weakly).
    pub(crate) fn over<N: NetParts>(state: &Rc<RefCell<ServerState<N>>>) -> Connections {
        let state = Rc::downgrade(state);
        Connections(Rc::new(move |allow| {
            if let Some(state) = state.upgrade()
                && let Ok(mut state) = state.try_borrow_mut()
            {
                state.allow_connections = allow;
            }
        }))
    }

    /// Takes connections (`true`) or refuses them (`false`). Does nothing once the server is gone, or while its state is
    /// borrowed (a call from inside the server's own pass that holds it).
    pub fn allow(&self, allow: bool) {
        (self.0)(allow);
    }
}

impl fmt::Debug for Connections {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Connections")
    }
}

/// What a server's entry is handed: the script engine, with every stage but `Entry` run and the start's globals set; how the
/// server was asked to start; and its connections, which are off.
#[non_exhaustive]
pub struct ServerEntry<'a> {
    pub vm: &'a mut Vm,
    pub start: &'a ServerStart<'a>,
    pub connections: Connections,
}

impl<'a> ServerEntry<'a> {
    pub(crate) fn new(
        vm: &'a mut Vm,
        start: &'a ServerStart<'a>,
        connections: Connections,
    ) -> ServerEntry<'a> {
        ServerEntry {
            vm,
            start,
            connections,
        }
    }
}

/// The engine builder's calls that set what every server and client is made of and how it starts.
pub trait HostSlots {
    /// Sets the script engine every server and client runs on: `make` makes a new one for each, before any recipe runs.
    /// Exclusive: a second setter fails the build, naming both plugins.
    fn script_engine(&mut self, make: impl Fn() -> Vm + 'static) -> &mut EngineBuilder;

    /// Sets what every server runs once its stages ran, in place of the entry script, the start function and turning
    /// connections on ([`ServerEntry`]). The `Entry` stage replays after it; an error stops the server's start with its
    /// message. Exclusive: a second setter fails the build, naming both plugins.
    fn server_entry(
        &mut self,
        run: impl Fn(&mut ServerEntry<'_>) -> Result<(), String> + 'static,
    ) -> &mut EngineBuilder;

    /// Sets what every client's `exec_entry` runs in place of its entry script, on its script engine. The `Entry` stage
    /// replays after it; an error is `exec_entry`'s. Exclusive: a second setter fails the build, naming both plugins.
    fn client_entry(
        &mut self,
        run: impl Fn(&mut Vm) -> Result<(), String> + 'static,
    ) -> &mut EngineBuilder;
}

impl HostSlots for EngineBuilder {
    fn script_engine(&mut self, make: impl Fn() -> Vm + 'static) -> &mut EngineBuilder {
        self.set_slot(Rc::new(ScriptEngineSlot(Rc::new(make))))
    }

    fn server_entry(
        &mut self,
        run: impl Fn(&mut ServerEntry<'_>) -> Result<(), String> + 'static,
    ) -> &mut EngineBuilder {
        self.set_slot(Rc::new(ServerEntrySlot(Rc::new(run))))
    }

    fn client_entry(
        &mut self,
        run: impl Fn(&mut Vm) -> Result<(), String> + 'static,
    ) -> &mut EngineBuilder {
        self.set_slot(Rc::new(ClientEntrySlot(Rc::new(run))))
    }
}

/// A new script engine for a server or a client of `engine`: the factory a plugin set ([`HostSlots::script_engine`]), else
/// [`Vm::new`]. `GameServer::start*` and `ClientBuilder::from_engine*` make theirs with it; a host of its own calls it too.
#[must_use]
pub fn make_script_engine(engine: &Engine) -> Vm {
    engine
        .slot::<ScriptEngineSlot>()
        .map_or_else(Vm::new, |slot| (slot.0)())
}

/// The server entry a plugin set on `engine` ([`HostSlots::server_entry`]); none: the server runs its entry script.
#[must_use]
pub fn server_entry_fn(engine: &Engine) -> Option<Rc<RunServerEntry>> {
    engine
        .slot::<ServerEntrySlot>()
        .map(|slot| Rc::clone(&slot.0))
}

/// The client entry a plugin set on `engine` ([`HostSlots::client_entry`]); none: the client runs its entry script.
#[must_use]
pub fn client_entry_fn(engine: &Engine) -> Option<Rc<RunClientEntry>> {
    engine
        .slot::<ClientEntrySlot>()
        .map(|slot| Rc::clone(&slot.0))
}
