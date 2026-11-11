//! A server's net game ([`NetGame`]): what its network host is made of, set once for the engine.
//!
//! | Seam | Setter | Shape |
//! |------|--------|-------|
//! | net game `G` ([`NetGameFactory`]) | [`NetGamePlugin`], [`ServerSlots::net_game`] | exclusive: a second setter fails the build, naming both plugins |
//! | codecs ([`NetCodecs`]) | [`NetCodecsPlugin`], [`ServerSlots::net_codecs`] | exclusive |
//!
//! A net game cannot be a trait object (associated types, consts and generic methods), so the server that speaks it is
//! `GameServer<G>` and is started by the type that knows `G`: [`GameServer::start_with_net`]. The engine keeps the factory
//! erased, with the type's name and a start function made for it, so the engine's own runners start whichever net game the
//! slot holds ([`start_net_game`], a [`RunServer`]). An engine with no net game slot speaks [`TorqueNet`], with the parts
//! [`TorqueNetFactory`] makes; `GameServer::start_engine` is the start of that server and is refused on an engine set for
//! another net game, as `start_with_net` is for any type the slot does not hold.
//!
//! The factory runs once per server, in the `Net` stage, after the server game ([`GameParts`]); the codecs factory runs
//! just before that stage, so every `Net` recipe sees the codecs the host speaks.
//!
//! **The plugins' net classes** reach only the engine's own peers, and only when the net game carries them: its ghost game
//! ([`GhostGame::carries_plugin_classes`]) and its codecs ([`NetCodecs::plugin_classes`]). A server whose engine has plugin
//! classes warns at start, naming them, when either does not.
//!
//! **Whose limits are whose.** The engine keeps its own, whatever the net game: the ghost hub's scope and send budgets
//! (`GhostHub`), the connect, request and reject rates and the per-source caps of the handshake, and the pump's script budgets
//! (a run's steps, the script window, each connection's share). The game sets its own: `G::DEFAULT_MAX_PLAYERS` (the player
//! count a server without the preference takes) and, through its codecs, [`NetCodecs::server_config`] (the max players and the
//! handshake's form).
//!
//! **The payload layer is wrapped, not replaced** ([`WrapPayload`]): the factory's wrapper is handed Torque's event layer and
//! returns the layer the channels wrap. A game that replaces the layer outright does so by ignoring that layer, under the
//! wrapper's contract.

use std::any::{Any, type_name};
use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use torque_engine::{Engine, EngineBuilder, Plugin, PluginId, PluginType};
use torque_server::game::GameParts;
use torque_server::net::codecs::{NetCodecs, plugin_classes, set_net_codecs};
use torque_server::net::datablocks::StaticWriteRule;
use torque_server::net::ghosts::GhostGame;
use torque_server::net::payload::{PayloadLayer, Quiet};
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::SharedTransport;
use torque_server::net::{self, Host, NetClock, NetGame, NetShared, attach_host};
use torque_server::server::NetHost;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use crate::server::{GameServer, ServerStart};
use crate::slots::ServerSlots;

/// Takes the event layer Torque's host made (`inner`) and gives the layer the bulk channels wrap.
///
/// **Contract.** A wrapper forwards every call to `inner`: `on_game_packet`, `on_notify`, `write`, `busy`, `on_accept`,
/// `mark_peer`, `mark_channels`, `take_calls`, `take_work` and `on_drop`. That keeps TGE's event caps and the ghost layer's
/// accounting (its work units, its script calls, the tags a dropped connection releases). A wrapper that ignores `inner`
/// replaces the event layer: it must drive the host's event layer (`NetShared::events`) itself and enforce caps of its own on
/// what a packet may ask for.
pub type WrapPayload = Box<dyn FnOnce(Box<dyn PayloadLayer>) -> Box<dyn PayloadLayer>>;

/// What the network host of one server is made of, for net game `G`.
pub struct NetGameParts<G: NetGame> {
    /// The game's state of the host.
    pub game: G,
    /// The game's ghost sources.
    pub ghosts: G::Ghosts,
    /// The game's rule for static writes to its own datablock classes (none: every write bumps the key, as in TGE).
    pub static_writes: Option<StaticWriteRule>,
    /// The payload layer the channels wrap (none: Torque's event layer as it is).
    pub payload: Option<WrapPayload>,
}

/// Makes the parts of a server's network host, once per server, when its server game has run (`game`: its container and
/// shapes). A closure of the same shape is one.
pub trait NetGameFactory<G: NetGame>: 'static {
    /// The parts of this server's host. An error stops the server's start with its message.
    ///
    /// # Errors
    /// Whatever keeps the game from serving: a missing extension a recipe was to leave, a table that does not load.
    fn make(&self, vm: &mut Vm, game: &GameParts) -> Result<NetGameParts<G>, String>;
}

impl<G: NetGame, F> NetGameFactory<G> for F
where
    F: Fn(&mut Vm, &GameParts) -> Result<NetGameParts<G>, String> + 'static,
{
    fn make(&self, vm: &mut Vm, game: &GameParts) -> Result<NetGameParts<G>, String> {
        self(vm, game)
    }
}

/// Torque's net game as an engine with no net game slot has it: Torque's string table, TGE's objects ghosted, no static-write
/// rule, Torque's event layer.
#[derive(Clone, Copy, Debug, Default)]
pub struct TorqueNetFactory;

impl NetGameFactory<TorqueNet> for TorqueNetFactory {
    fn make(&self, _vm: &mut Vm, _game: &GameParts) -> Result<NetGameParts<TorqueNet>, String> {
        Ok(NetGameParts {
            game: TorqueNet::default(),
            ghosts: TgeGhostGame,
            static_writes: None,
            payload: None,
        })
    }
}

/// Starts a server of the net game a slot holds, as a [`RunServer`].
type StartServer = fn(&Engine, MemFiles, &ServerStart<'_>) -> Result<Box<dyn RunServer>, String>;

/// The engine's net game (the exclusive slot's value): its type's name, its factory (an `Rc<dyn NetGameFactory<G>>`) and the
/// start made for it.
pub(crate) struct NetGameSlot {
    name: &'static str,
    factory: Box<dyn Any>,
    start: StartServer,
}

impl NetGameSlot {
    pub(crate) fn new<G: NetGame>(factory: Rc<dyn NetGameFactory<G>>) -> NetGameSlot {
        NetGameSlot {
            name: type_name::<G>(),
            factory: Box::new(factory),
            start: start_boxed::<G>,
        }
    }
}

fn start_boxed<G: NetGame>(
    engine: &Engine,
    files: MemFiles,
    start: &ServerStart<'_>,
) -> Result<Box<dyn RunServer>, String> {
    GameServer::<G>::start_with_net(engine, files, start).map(|s| Box::new(s) as Box<dyn RunServer>)
}

/// The codecs every server of the engine speaks (the exclusive slot's value).
pub(crate) struct CodecsSlot(pub(crate) Rc<MakeCodecs>);

/// Makes a server's codecs from its script engine as the recipes left it before the `Net` stage.
pub type MakeCodecs = dyn Fn(&Vm) -> Rc<dyn NetCodecs>;

/// The factory of net game `G` on `engine`: the slot's, or [`TorqueNetFactory`] when the engine has none and `G` is
/// [`TorqueNet`].
///
/// # Errors
/// The slot holds another net game, or there is none and `G` is not `TorqueNet`; the message names both types.
pub(crate) fn factory_for<G: NetGame>(
    engine: &Engine,
) -> Result<Rc<dyn NetGameFactory<G>>, String> {
    let wanted = type_name::<G>();
    let Some(slot) = engine.slot::<NetGameSlot>() else {
        let torque: Rc<dyn NetGameFactory<TorqueNet>> = Rc::new(TorqueNetFactory);
        return (&torque as &dyn Any)
            .downcast_ref::<Rc<dyn NetGameFactory<G>>>()
            .cloned()
            .ok_or_else(|| {
                format!(
                    "the engine has no net game slot, so its servers speak `{}`, not `{wanted}` (add NetGamePlugin::<{wanted}>)",
                    type_name::<TorqueNet>()
                )
            });
    };
    slot.factory
        .downcast_ref::<Rc<dyn NetGameFactory<G>>>()
        .cloned()
        .ok_or_else(|| {
            format!(
                "the engine's net game is `{}`, not `{wanted}`: start its servers with GameServer::<{}>::start_with_net or start_net_game",
                slot.name, slot.name
            )
        })
}

/// Sets the codecs the engine's slot makes on `vm`, if a plugin set the slot: called just before the `Net` stage.
pub(crate) fn apply_codecs(engine: &Engine, vm: &mut Vm) {
    if let Some(slot) = engine.slot::<CodecsSlot>() {
        let codecs = (slot.0)(vm);
        set_net_codecs(vm, codecs);
    }
}

/// What the network host's recipe made, for the host to attach to the server's main loop.
pub(crate) struct NetAttached<G: NetGame> {
    pub(crate) shared: Rc<RefCell<NetShared<G>>>,
    pub(crate) passes: Rc<dyn NetHost>,
}

/// Where the network host's recipe leaves what it made for the host that started the server.
pub(crate) type AttachedCell<G> = Rc<RefCell<Option<NetAttached<G>>>>;

/// Attaches the network host of net game `G` to `vm`: the factory's parts, Torque's host over them, the payload layer the
/// factory gives, and the warning when the plugins' net classes do not reach the engine's peers.
pub(crate) fn attach<G: NetGame>(
    vm: &mut Vm,
    state: &net::Engine<G>,
    transport: &SharedTransport,
    factory: &dyn NetGameFactory<G>,
    game: &GameParts,
) -> Result<NetAttached<G>, String> {
    let NetGameParts {
        game: net_game,
        ghosts,
        static_writes,
        payload,
    } = factory
        .make(vm, game)
        .map_err(|e| format!("the net game `{}`: {e}", type_name::<G>()))?;
    let shared = attach_host(
        vm,
        Host {
            transport: Rc::clone(transport),
            game: net_game,
            engine: Rc::clone(state),
            clock: NetClock::Sim,
            ghosts,
            container: Rc::clone(&game.container),
            static_writes,
        },
    );
    if let Some(wrap) = payload {
        let mut sh = shared.borrow_mut();
        let torque = std::mem::replace(&mut sh.payload, Box::new(Quiet));
        sh.payload = wrap(torque);
    }
    if let Some(classes) = plugin_classes(vm) {
        let codecs_carry = shared.borrow().codecs.plugin_classes().is_some();
        let names: Vec<&str> = classes
            .ghosts()
            .iter()
            .chain(classes.datablocks())
            .map(|class| class.name())
            .collect();
        if let Some(warning) = plugin_class_warning(
            type_name::<G>(),
            &names,
            G::Ghosts::carries_plugin_classes(),
            codecs_carry,
        ) {
            vm.warn(warning);
        }
    }
    let passes: Rc<dyn NetHost> = Rc::new(NetPasses(Rc::clone(&shared)));
    Ok(NetAttached { shared, passes })
}

/// The start-up warning of a server whose engine has the plugin net classes `names` when its net game (`net_game`) does not
/// carry them: its ghost game does not ghost them, or its codecs do not answer for them. None when both carry them or there
/// are none.
pub(crate) fn plugin_class_warning(
    net_game: &str,
    names: &[&str],
    ghosts_carry: bool,
    codecs_carry: bool,
) -> Option<String> {
    let missing = match (ghosts_carry, codecs_carry) {
        _ if names.is_empty() => return None,
        (true, true) => return None,
        (false, false) => "its ghost game and its codecs do not",
        (false, true) => "its ghost game does not",
        (true, false) => "its codecs do not",
    };
    Some(format!(
        "the plugins' net classes ({}) reach no client: the net game `{net_game}` is made of parts that do not carry them ({missing})",
        names.join(", ")
    ))
}

/// What the engine's runners drive of a server, whatever its net game: [`GameServer`] of any `G`.
pub trait RunServer {
    /// One 32 ms tick of the main loop ([`GameServer::tick`]).
    fn tick(&mut self);
    /// Ticks on the wall clock until `stop` says so ([`GameServer::run_dedicated_until`]).
    fn run_dedicated_until(
        &mut self,
        stop: &mut dyn FnMut(u64) -> bool,
        print: &mut dyn FnMut(&str),
    );
    fn vm(&self) -> &Vm;
    fn vm_mut(&mut self) -> &mut Vm;
    /// The console's lines since the last call.
    fn drain_log(&mut self) -> Vec<String>;
    /// Where the transport is bound.
    fn address(&self) -> Option<SocketAddr>;
    /// Closes the transport.
    fn close(&mut self);
    /// The type name of the server's net game.
    fn net_game(&self) -> &'static str;
}

impl<G: NetGame> RunServer for GameServer<G> {
    fn tick(&mut self) {
        GameServer::tick(self);
    }

    fn run_dedicated_until(
        &mut self,
        stop: &mut dyn FnMut(u64) -> bool,
        print: &mut dyn FnMut(&str),
    ) {
        GameServer::run_dedicated_until(self, stop, print);
    }

    fn vm(&self) -> &Vm {
        GameServer::vm(self)
    }

    fn vm_mut(&mut self) -> &mut Vm {
        GameServer::vm_mut(self)
    }

    fn drain_log(&mut self) -> Vec<String> {
        GameServer::drain_log(self)
    }

    fn address(&self) -> Option<SocketAddr> {
        GameServer::address(self)
    }

    fn close(&mut self) {
        GameServer::close(self);
    }

    fn net_game(&self) -> &'static str {
        type_name::<G>()
    }
}

/// A server of the net game `engine`'s slot holds ([`TorqueNet`] with none), started as [`GameServer::start_with_net`] starts
/// it: what a runner that does not know the game's types calls.
///
/// # Errors
/// What the server's start returned.
pub fn start_net_game(
    engine: &Engine,
    files: MemFiles,
    start: &ServerStart<'_>,
) -> Result<Box<dyn RunServer>, String> {
    match engine.slot::<NetGameSlot>() {
        Some(slot) => (slot.start)(engine, files, start),
        None => start_boxed::<TorqueNet>(engine, files, start),
    }
}

/// Sets the engine's net game to `G`, made by the factory once per server. Exclusive: a second net game fails the build.
pub struct NetGamePlugin<G: NetGame> {
    factory: Rc<dyn NetGameFactory<G>>,
}

impl<G: NetGame> NetGamePlugin<G> {
    /// The plugin whose servers' hosts `factory` makes.
    pub fn new(factory: impl NetGameFactory<G>) -> NetGamePlugin<G> {
        NetGamePlugin {
            factory: Rc::new(factory),
        }
    }
}

impl<G: NetGame> Clone for NetGamePlugin<G> {
    fn clone(&self) -> NetGamePlugin<G> {
        NetGamePlugin {
            factory: Rc::clone(&self.factory),
        }
    }
}

impl<G: NetGame> Plugin for NetGamePlugin<G> {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net_game(Rc::clone(&self.factory));
    }
}

/// One id for every `G`: an engine has one net game.
impl<G: NetGame> PluginType for NetGamePlugin<G> {
    const ID: PluginId = PluginId::new("torque.net-game");
}

/// Sets the codecs every server's network host speaks, made per server before the `Net` stage. Exclusive.
#[derive(Clone)]
pub struct NetCodecsPlugin {
    make: Rc<MakeCodecs>,
}

impl NetCodecsPlugin {
    /// The plugin whose servers speak the codecs `make` makes.
    pub fn new(make: impl Fn(&Vm) -> Rc<dyn NetCodecs> + 'static) -> NetCodecsPlugin {
        NetCodecsPlugin {
            make: Rc::new(make),
        }
    }
}

impl Plugin for NetCodecsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let make = Rc::clone(&self.make);
        engine.net_codecs(move |vm| make(vm));
    }
}

impl PluginType for NetCodecsPlugin {
    const ID: PluginId = PluginId::new("torque.net-codecs");
}
