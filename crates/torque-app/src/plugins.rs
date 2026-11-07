//! [`DefaultPlugins`]: the engine's own wiring of a server's and a client's script engine, as plugins.
//!
//! Each plugin wraps one install that stays where it lives (`torque_game::specs::install`, `torque_server::game::install`,
//! `torque_client::screen::register`, ...) and files it as a recipe at a stage. The host that makes a script engine
//! ([`GameServer::start_engine`](crate::server::GameServer::start_engine),
//! [`ClientBuilder::from_engine`](crate::client::ClientBuilder::from_engine)) replays the stages in order, handing the
//! parts a stage needs to its recipes through the script engine's extension map ([`ServerWiring`], [`ClientWiring`]).
//! The order of the stages is the order the installs always ran in; within a stage, the order the plugins were added.
//!
//! | Plugin | Id | Role and stage | Wraps |
//! |--------|----|----------------|-------|
//! | [`GameClassesPlugin`] | `torque.game-classes` | both, `Classes` | `torque_game::specs::install` |
//! | [`ServerGamePlugin`] | `torque.server-game` | server, `Game` | `torque_server::game::install` |
//! | [`TerrainPlugin`] | `torque.terrain` | server, `World` | [`terrain::install`](crate::terrain::install) |
//! | [`WaterPlugin`] | `torque.water` | server, `World` | [`water::install`](crate::water::install) |
//! | [`NetHostPlugin`] | `torque.net-host` | server, `Net` | `attach_host` and the net passes |
//! | [`GuiPlugin`] | `torque.gui` | client, `Gui` | `torque_gui::register` |
//! | [`ActionMapPlugin`] | `torque.action-map` | client, `Input` | the `ActionMap` class and its natives |
//! | [`ScreenPlugin`] | `torque.screen` | client, `Screen` | `torque_client::screen::register` |
//! | [`ConnPlugin`] | `torque.conn` | client, `Conn` | `torque_client::conn::register` |
//! | [`ServerQueryPlugin`] | `torque.server-query` | client, `Query` | `torque_client::query::register` |
//! | [`ClientHostPlugin`] | `torque.frame-host` | client, `Host` | what [`ClientBuilder::build`](crate::client::ClientBuilder::build) makes the host of |
//! | [`PageBridgePlugin`] | `torque.page-bridge` | client, `Window` (web builds) | [`page_bridge::install`](crate::page_bridge::install) |
//!
//! A game that needs only a part of this takes the group and turns plugins off (`DefaultPlugins.disable::<WaterPlugin>()`),
//! or adds the plugins it wants one by one.

use std::cell::RefCell;
use std::rc::Rc;

use torque_client::conn::tge::TgeConn;
use torque_client::game::ClientGame;
use torque_client::host::{ClientPass, EnginePass};
use torque_client::state::ClientState;
use torque_engine::{
    ClientStage, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder, Requires, ServerStage,
};
use torque_game::sim_classes::SimClasses;
use torque_gui::{DrawCache, Gui};
use torque_server::game::GameParts;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::SharedTransport;
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::NetHost;
use torque_server::state::ServerState;
use torque_ts_vm::vm::Vm;

use crate::client::ClientNet;

/// The plugins that wire a server's and a client's script engine, in the order their installs always ran in. A dedicated
/// server replays only the server recipes, a client only the client ones.
///
/// ```
/// use torque_app::plugins::{DefaultPlugins, WaterPlugin};
/// use torque_engine::{EngineBuilder, PluginGroup};
///
/// let engine = EngineBuilder::new()
///     .add_plugins(DefaultPlugins.disable::<WaterPlugin>())
///     .build()?;
/// assert!(!engine.is_plugin_added::<WaterPlugin>());
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultPlugins;

impl PluginGroup for DefaultPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::new()
            .add(GameClassesPlugin)
            .add(ServerGamePlugin)
            .add(TerrainPlugin)
            .add(WaterPlugin)
            .add(NetHostPlugin)
            .add(GuiPlugin::default())
            .add(ActionMapPlugin)
            .add(ScreenPlugin)
            .add(ConnPlugin)
            .add(ServerQueryPlugin)
            .add(ClientHostPlugin)
            .add(PageBridgePlugin)
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

/// What a client's recipes share while the client is made: the natives' state, the socket kind, and the GUI the GUI plugin
/// makes. The host sets it as the script engine's extension before the first stage.
pub struct ClientWiring {
    state: Rc<RefCell<ClientState<TgeConn>>>,
    net: ClientNet,
    gui: RefCell<Option<Gui>>,
}

impl ClientWiring {
    pub(crate) fn new(state: Rc<RefCell<ClientState<TgeConn>>>, net: ClientNet) -> ClientWiring {
        ClientWiring {
            state,
            net,
            gui: RefCell::new(None),
        }
    }

    /// The state the client's natives share.
    pub fn state(&self) -> &Rc<RefCell<ClientState<TgeConn>>> {
        &self.state
    }

    /// The kind of socket the client was given.
    pub fn net(&self) -> ClientNet {
        self.net
    }

    /// The GUI ([`GuiPlugin`]), once its stage ran.
    pub fn gui(&self) -> Option<Gui> {
        self.gui.borrow().clone()
    }
}

/// The client wiring a recipe is replayed with.
fn client_wiring(vm: &Vm, plugin: &str) -> Result<Rc<ClientWiring>, String> {
    vm.extension::<ClientWiring>()
        .ok_or_else(|| format!("{plugin} runs only when a game client is made"))
}

/// The GUI, for a plugin that builds on it.
fn client_gui(wiring: &ClientWiring, plugin: &str) -> Result<Gui, String> {
    wiring
        .gui()
        .ok_or_else(|| format!("{plugin} needs the GUI, which has not run"))
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
        engine.server(ServerStage::Game, |vm, _| {
            let wiring = server_wiring(vm, "the server game")?;
            let game = torque_server::game::install(vm, wiring.state());
            *wiring.game.borrow_mut() = Some(game);
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
                    step_limit: None,
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

/// The GUI's classes and natives. `layout_fonts`: the GUI's layout measures text with the fonts it is drawn with
/// ([`layout_fonts`](crate::layout_fonts)); else with its default.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.gui")]
pub struct GuiPlugin {
    pub layout_fonts: bool,
}

impl Plugin for GuiPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let layout_fonts = self.layout_fonts;
        engine.client(ClientStage::Gui, move |vm, _| {
            let wiring = client_wiring(vm, "the GUI")?;
            let gui = torque_gui::register(vm);
            if layout_fonts {
                // The controls that measure text as they lay out (a list of games) find their fonts where the drawing does.
                gui.0.borrow_mut().fonts =
                    torque_gui::SharedFonts::new(Box::new(crate::layout_fonts()));
            }
            *wiring.gui.borrow_mut() = Some(gui);
            Ok(())
        });
    }
}

/// `ActionMap` (a plain `SimObject` class, as TGE's) and its natives.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.action-map")]
pub struct ActionMapPlugin;

impl Plugin for ActionMapPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Input, |vm, _| {
            let wiring = client_wiring(vm, "the action maps")?;
            vm.register_class("ActionMap", "SimObject", &[]);
            torque_client::actionmap::register(vm, wiring.state());
            Ok(())
        });
    }
}

/// The screen: resolution, full screen and the canvas's natives.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.screen")]
pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<GuiPlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Screen, |vm, _| {
            let wiring = client_wiring(vm, "the screen")?;
            let gui = client_gui(&wiring, "the screen")?;
            torque_client::screen::register(vm, &gui, wiring.state());
            Ok(())
        });
    }
}

/// The connection to a server (TGE's connection half and the ghosts' natives), for a client that was given a socket
/// ([`ClientNet`] other than `None`); the socket itself is an argument of the client's start.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.conn")]
pub struct ConnPlugin;

impl Plugin for ConnPlugin {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<GuiPlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Conn, |vm, _| {
            let wiring = client_wiring(vm, "the connection")?;
            if wiring.net() == ClientNet::None {
                return Ok(());
            }
            let gui = client_gui(&wiring, "the connection")?;
            torque_client::conn::register(vm, wiring.state(), &gui);
            Ok(())
        });
    }
}

/// The LAN query's natives (the server browser's list of games).
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.server-query")]
pub struct ServerQueryPlugin;

impl Plugin for ServerQueryPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Query, |vm, _| {
            let wiring = client_wiring(vm, "the server query")?;
            torque_client::query::register(vm, wiring.state());
            Ok(())
        });
    }
}

/// What [`ClientBuilder::build`](crate::client::ClientBuilder::build) makes the client host from: the draw cache and the
/// window's mode. The pass that drives the engine's half of each tick is the engine's slot of `dyn ClientPass<TgeConn>`.
/// [`ClientHostPlugin`] sets both.
#[derive(Clone, Copy)]
pub struct ClientHostSetup {
    /// Makes the GUI's draw cache ([`draw_cache`](crate::draw_cache)).
    pub draw_cache: fn() -> DrawCache,
    /// Whether the host is made in a window's mode ([`ClientHost::set_windowed`](torque_client::host::ClientHost::set_windowed)).
    pub windowed: bool,
}

/// The client host that runs the frames: the engine's per-tick pass and draw cache, in a window's mode. The host takes the
/// script engine by value, so [`ClientBuilder::build`](crate::client::ClientBuilder::build) makes it from the slot and
/// resource this plugin sets, once the game's scripts are loaded.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.frame-host")]
pub struct ClientHostPlugin;

impl Plugin for ClientHostPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let pass: Rc<dyn ClientPass<TgeConn>> = Rc::new(EnginePass);
        engine.set_slot::<dyn ClientPass<TgeConn>>(pass);
        engine.insert_resource(ClientHostSetup {
            draw_cache: crate::draw_cache,
            windowed: true,
        });
    }
}

/// A web build's page and the game's scripts calling each other (`callPage`, `setRenderQuality`). Its recipe is filed at the
/// `Window` stage of web builds only, which the window runs once its surface exists.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.page-bridge")]
pub struct PageBridgePlugin;

impl Plugin for PageBridgePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        if cfg!(target_arch = "wasm32") {
            engine.client(ClientStage::Window, |vm, _| {
                crate::page_bridge::install(vm);
                Ok(())
            });
        }
    }
}

/// The client game (what draws and predicts the game's objects): sets the engine's slot of `dyn ClientGame`, which
/// [`ClientBuilder::from_engine`](crate::client::ClientBuilder::from_engine) installs at the `Game` stage. Not in
/// [`DefaultPlugins`]: a game picks its own.
#[derive(Clone, torque_engine::PluginId)]
#[plugin(id = "torque.client-game")]
pub struct ClientGamePlugin {
    game: Rc<dyn ClientGame>,
}

impl ClientGamePlugin {
    /// The plugin that gives the client `game`.
    pub fn new(game: Rc<dyn ClientGame>) -> ClientGamePlugin {
        ClientGamePlugin { game }
    }
}

impl Plugin for ClientGamePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.set_slot::<dyn ClientGame>(Rc::clone(&self.game));
    }
}

/// The game's own subclasses of the simulated classes: sets the engine's slot of `dyn SimClasses`, which
/// [`GameServer::start_engine`](crate::server::GameServer::start_engine) installs at the `Prepare` stage, before the server
/// game reads it. Not in [`DefaultPlugins`]: a game picks its own.
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
