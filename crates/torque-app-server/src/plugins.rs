//! The plugins that wire a game server's script engine, and [`MinimalPlugins`], the group a dedicated server needs.
//!
//! Each plugin wraps one install that stays where it lives (`torque_game::specs::install`, `torque_server::game::install`, ...)
//! and files it as a recipe at a stage. The host that makes a script engine
//! ([`GameServer::start_engine`](crate::server::GameServer::start_engine)) replays the stages in order, handing the parts a
//! stage needs to its recipes through the script engine's extension map ([`ServerWiring`]). The order of the stages is the
//! order the installs always ran in; within a stage, the order the plugins were added.
//!
//! | Plugin | Id | Role and stage | Wraps |
//! |--------|----|----------------|-------|
//! | [`ModsPlugin`] | `torque.mods` | both, server `Prepare` and client `Game` (the files), `Entry` (the scripts) | local mods: folders and zips mounted over the game's files, their scripts run as packages (`torque_mods`) |
//! | [`GameClassesPlugin`] | `torque.game-classes` | both, `Classes` | `torque_game::specs::install` |
//! | [`ServerGamePlugin`] | `torque.server-game` | server, `Game` | `torque_server::game::install` |
//! | [`TerrainPlugin`] | `torque.terrain` | server, `World` | [`terrain::install`](crate::terrain::install) |
//! | [`WaterPlugin`] | `torque.water` | server, `World` | [`water::install`](crate::water::install) |
//! | [`ProtocolPlugins`] | `torque.net.tge`, `torque.net.engine` | both, the engine's protocol slot | [`TgeProtocolPlugin`] sets Torque's own protocol, [`EngineProtocolPlugin`] the engine's; one of them is on ([`protocol_from_env`]) |
//! | [`NetHostPlugin`] | `torque.net-host` | server, `Net` | `attach_host` and the net passes |
//! | [`ServerTransportPlugins`] | `torque.transport-udp-server`, ... | the transports a server listens on, a plugin each, by id ([`transports`](crate::transports)) | `UdpTransport`, `MemoryTransport`, the web and p2p ones |
//! | [`ContentFormatPlugins`] | `torque.format-gltf`, `torque.format-dif`, ... | the content formats' asset loaders, a plugin each (OBJ, Blockbench and `.map` added off) | [`torque_content`] |
//! | [`HierarchyPlugin`] | `torque.hierarchy` | server, `Game`, a `Resolve` tick hook; only in engine mode | the scene hierarchy: parent and child transforms ([`torque_plugin_hierarchy`]) |
//! | [`DedicatedRunnerPlugin`] | `torque.dedicated-runner` | the engine's runner slot | [`run`](crate::run): a dedicated server |
//!
//! `SimClassesPlugin` sets the slot of the game's own simulated classes; it is in no group.

use std::cell::RefCell;
use std::rc::Rc;

use torque_engine::{
    ClientStage, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder, PluginType, ProtocolKind,
    Requires, ServerStage,
};
use torque_game::sim_classes::SimClasses;
use torque_server::game::GameParts;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::SharedTransport;
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::NetHost;
use torque_server::state::ServerState;
use torque_ts_vm::vm::Vm;

use crate::run::DedicatedRunnerPlugin;
pub use crate::transports::ServerTransportPlugins;
pub use torque_content::ContentFormatPlugins;
pub use torque_mods::ModsPlugin;
pub use torque_plugin_hierarchy::HierarchyPlugin;

/// What a dedicated server needs and nothing else: TGE's game classes, the server game, the terrain and water blocks, the
/// network host and the content formats' asset loaders ([`ContentFormatPlugins`]), with the runner that runs a
/// [`Dedicated`](crate::run::Dedicated) server. No client, GUI, drawing, audio or window crates are involved. A server made
/// on `MinimalPlugins` runs the same scripts as one made on `DefaultPlugins`.
/// It carries its own dedicated runner, so it does not go with a window or page runner (the runner slot is exclusive):
/// torque-app's `DefaultPlugins` swaps its own for this one with `DefaultPlugins.disable::<WindowRunnerPlugin>()` and
/// `DedicatedRunnerPlugin`.
///
/// ```
/// use torque_app_server::plugins::{MinimalPlugins, WaterPlugin};
/// use torque_engine::{EngineBuilder, PluginGroup};
///
/// let engine = EngineBuilder::new()
///     .add_plugins(MinimalPlugins.disable::<WaterPlugin>())
///     .build()?;
/// assert!(!engine.is_plugin_added::<WaterPlugin>());
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct MinimalPlugins;

impl PluginGroup for MinimalPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::new()
            .add(ModsPlugin::default())
            .add(GameClassesPlugin)
            .add(ServerGamePlugin)
            .add(TerrainPlugin)
            .add(WaterPlugin)
            .add_group(ProtocolPlugins)
            .add(NetHostPlugin)
            .add_group(ServerTransportPlugins)
            .add_group(ContentFormatPlugins)
            .add(HierarchyPlugin::default())
            .add(DedicatedRunnerPlugin)
    }
}

/// What a server's recipes share while the server is made: the engine state and the transport it was given, and the parts
/// the server game and the network host hand on. The host sets it as the script engine's extension before the first stage.
pub struct ServerWiring {
    state: Rc<RefCell<ServerState<HostParts<TorqueNet>>>>,
    transport: SharedTransport,
    game: RefCell<Option<GameParts>>,
    net: RefCell<Option<NetAttached>>,
}

/// What the network host's recipe made, for the host to attach to the server's main loop.
pub(crate) struct NetAttached {
    pub(crate) shared: Rc<RefCell<NetShared<TorqueNet>>>,
    pub(crate) passes: Rc<dyn NetHost>,
}

impl ServerWiring {
    pub(crate) fn new(
        state: Rc<RefCell<ServerState<HostParts<TorqueNet>>>>,
        transport: SharedTransport,
    ) -> ServerWiring {
        ServerWiring {
            state,
            transport,
            game: RefCell::new(None),
            net: RefCell::new(None),
        }
    }

    /// The engine state the server's natives share.
    pub fn state(&self) -> &Rc<RefCell<ServerState<HostParts<TorqueNet>>>> {
        &self.state
    }

    /// The transport the server listens on.
    pub fn transport(&self) -> &SharedTransport {
        &self.transport
    }

    /// The parts the server game made ([`ServerGamePlugin`]), once its stage ran.
    pub fn game(&self) -> Option<GameParts> {
        self.game.borrow().clone()
    }

    pub(crate) fn take_net(&self) -> Option<NetAttached> {
        self.net.borrow_mut().take()
    }
}

/// The server wiring a recipe is replayed with.
fn server_wiring(vm: &Vm, plugin: &str) -> Result<Rc<ServerWiring>, String> {
    vm.extension::<ServerWiring>()
        .ok_or_else(|| format!("{plugin} runs only when a game server is made"))
}

/// The server game's parts, for a plugin that builds on them.
fn server_game(wiring: &ServerWiring, plugin: &str) -> Result<GameParts, String> {
    wiring
        .game()
        .ok_or_else(|| format!("{plugin} needs the server game, which has not run"))
}
/// TGE's game classes as console classes (the datablocks a connection receives, the simulated objects): on a server first
/// of all, on a client when the engine has this plugin.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.game-classes")]
pub struct GameClassesPlugin;

impl Plugin for GameClassesPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.server(ServerStage::Classes, |vm, _| {
            torque_game::specs::install(vm);
            Ok(())
        });
        engine.client(ClientStage::Classes, |vm, _| {
            torque_game::specs::install(vm);
            Ok(())
        });
    }
}

/// The server game: the simulation's classes and natives over the scene container the world's objects share.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.server-game")]
pub struct ServerGamePlugin;

impl Plugin for ServerGamePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.server(ServerStage::Game, |vm, engine| {
            let wiring = server_wiring(vm, "the server game")?;
            let game = torque_server::game::install(vm, wiring.state());
            *wiring.game.borrow_mut() = Some(game);
            // The plugins' collision the container's queries include.
            let entries = engine.list::<torque_collide::source::PluginCollision>();
            for warning in torque_game::plugin_collision::install(
                vm,
                entries,
                torque_collide::source::Side::Server,
            ) {
                vm.warn(warning);
            }
            Ok(())
        });
    }
}

/// `TerrainBlock`: its collision on the server.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.terrain")]
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<ServerGamePlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.server(ServerStage::World, |vm, _| {
            let wiring = server_wiring(vm, "the terrain")?;
            let game = server_game(&wiring, "the terrain")?;
            crate::terrain::install(vm, &game.container);
            Ok(())
        });
    }
}

/// `WaterBlock`: water the players swim in, on the server.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.water")]
pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<ServerGamePlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.server(ServerStage::World, |vm, _| {
            let wiring = server_wiring(vm, "the water")?;
            let game = server_game(&wiring, "the water")?;
            crate::water::install(vm, &game.container);
            Ok(())
        });
    }
}

/// The network protocol slot, set to Torque's own protocol. A game speaks one protocol, so this is the only plugin of the groups that
/// sets the slot: a game that speaks another turns this one off and adds the plugin of that protocol, or names the protocol in
/// `game.toml` (`[net] protocol`). A second plugin that sets the slot fails the build, naming both.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.net.tge")]
pub struct TgeProtocolPlugin;

impl Plugin for TgeProtocolPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().protocol(ProtocolKind::Tge);
    }
}

/// The network protocol slot, set to the engine's own protocol: its own framing over a link contract (QUIC and the web's links
/// later, plain UDP on private addresses, WebRTC, WebSockets, the memory link), every peer the engine's own, the manifest
/// always sent, RPCs in their own lanes ([`torque_server::net::engine`]). A game turns [`TgeProtocolPlugin`] off and this on
/// to speak it, or names it in `game.toml` (`[net] protocol = "engine"`). It also registers the two RPCs the script commands
/// (`commandToServer`, `commandToClient`) are sent as ([`torque_server::net::command_rpc`]).
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.net.engine")]
pub struct EngineProtocolPlugin;

impl Plugin for EngineProtocolPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().protocol(ProtocolKind::Engine);
        // Script commands ride two RPCs of their own in this protocol (Torque's puts them on its event queue).
        torque_server::net::command_rpc::register(engine);
    }
}

/// The variable that picks the protocol of [`ProtocolPlugins`] for a run (`tge` or `engine`): what the `net-matrix` recipe sets to
/// run the same tests in both modes. A game's `game.toml` (`[net] protocol`) still has the last word.
pub const PROTOCOL_ENV: &str = "TORQUE_NET_PROTOCOL";

/// The protocol [`ProtocolPlugins`] turns on: the one [`PROTOCOL_ENV`] names when it names one, else Torque's. In a browser,
/// which has no environment, the build's (`TORQUE_NET_PROTOCOL` when it was compiled).
pub fn protocol_from_env() -> ProtocolKind {
    named_protocol()
        .as_deref()
        .and_then(ProtocolKind::from_label)
        .unwrap_or(ProtocolKind::Tge)
}

/// The build's choice: read at compile time, so only the browser build records the variable as an input of the compile.
#[cfg(target_arch = "wasm32")]
fn named_protocol() -> Option<String> {
    option_env!("TORQUE_NET_PROTOCOL").map(str::to_owned)
}

#[cfg(not(target_arch = "wasm32"))]
fn named_protocol() -> Option<String> {
    std::env::var(PROTOCOL_ENV).ok()
}

/// Both protocol plugins, the one [`protocol_from_env`] picks on and the other added off, so `game.toml` or a game's own
/// `enable`/`disable` can switch them.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProtocolPlugins;

impl PluginGroup for ProtocolPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::new()
            .add(TgeProtocolPlugin)
            .add(EngineProtocolPlugin);
        match protocol_from_env() {
            ProtocolKind::Engine => group.disable::<TgeProtocolPlugin>(),
            _ => group.disable::<EngineProtocolPlugin>(),
        }
    }
}

/// The plugin that sets the protocol `kind`: [`TgeProtocolPlugin`] or [`EngineProtocolPlugin`]. The one place that names which is
/// which, for a test that must say what a group turns on.
///
/// # Panics
///
/// On a protocol this build of the engine has no plugin for (the enum is `non_exhaustive`).
pub fn protocol_plugin_id(kind: ProtocolKind) -> torque_engine::PluginId {
    match kind {
        ProtocolKind::Tge => TgeProtocolPlugin::ID,
        ProtocolKind::Engine => EngineProtocolPlugin::ID,
        other => panic!("no protocol plugin for {}", other.label()),
    }
}

/// Makes the engine `builder` speaks `kind`, whichever protocol plugin it had on: the other one is turned off. For a host or a
/// test that must speak one protocol whatever [`PROTOCOL_ENV`] says.
pub fn set_protocol(builder: &mut EngineBuilder, kind: ProtocolKind) -> &mut EngineBuilder {
    let (on, off) = match kind {
        ProtocolKind::Engine => (EngineProtocolPlugin::ID, TgeProtocolPlugin::ID),
        _ => (TgeProtocolPlugin::ID, EngineProtocolPlugin::ID),
    };
    if !builder.contains(on.as_str()) {
        match kind {
            ProtocolKind::Engine => builder.add_plugins(EngineProtocolPlugin),
            _ => builder.add_plugins(TgeProtocolPlugin),
        };
    }
    builder.disable_id(off.as_str()).enable_id(on.as_str())
}

/// The server's network host of Torque's net game and the passes that drive it from the main loop.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.net-host")]
pub struct NetHostPlugin;

impl Plugin for NetHostPlugin {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<ServerGamePlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.server(ServerStage::Net, |vm, _| {
            let wiring = server_wiring(vm, "the network host")?;
            let game = server_game(&wiring, "the network host")?;
            let shared = attach_host(
                vm,
                Host {
                    transport: Rc::clone(wiring.transport()),
                    game: TorqueNet::default(),
                    engine: Rc::clone(wiring.state()),
                    clock: NetClock::Sim,
                    ghosts: TgeGhostGame,
                    container: Rc::clone(&game.container),
                    static_writes: None,
                },
            );
            let passes: Rc<dyn NetHost> = Rc::new(NetPasses(Rc::clone(&shared)));
            *wiring.net.borrow_mut() = Some(NetAttached { shared, passes });
            Ok(())
        });
    }
}

/// The game's own subclasses of the simulated classes: sets the engine's slot of `dyn SimClasses`, which
/// [`GameServer::start_engine`](crate::server::GameServer::start_engine) installs at the `Prepare` stage, before the server
/// game reads it. Not in a group: a game picks its own.
#[derive(Clone, torque_engine::PluginId)]
#[plugin(id = "torque.sim-classes")]
pub struct SimClassesPlugin {
    classes: Rc<dyn SimClasses>,
}

impl SimClassesPlugin {
    /// The plugin that gives the server `classes`.
    pub fn new(classes: Rc<dyn SimClasses>) -> SimClassesPlugin {
        SimClassesPlugin { classes }
    }
}

impl Plugin for SimClassesPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.set_slot::<dyn SimClasses>(Rc::clone(&self.classes));
    }
}
