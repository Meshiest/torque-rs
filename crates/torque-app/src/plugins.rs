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
//! | [`AudioPlugin`] | `torque_audio::AudioPlugin` | client, `Natives` | the `alx` natives, the audio datablocks and the mixer |
//! | [`ClientHostPlugin`] | `torque.frame-host` | client, `Host` | what [`ClientBuilder::build`](crate::client::ClientBuilder::build) makes the host of |
//! | [`PageBridgePlugin`] | `torque.page-bridge` | client, `Window` (web builds) | [`page_bridge::install`](crate::page_bridge::install) |
//! | [`RunnerPlugin`] | `torque.runner` | the engine's runner slot | [`run`](crate::run): the window, the page, a dedicated server, offscreen |
//!
//! [`ZoomPlugin`] (`setFov` and its kin) and [`LivePerfPlugin`] (the frame breakdown's console function) are not in
//! [`DefaultPlugins`]; a game adds them.
//!
//! A game that needs only a part of this takes the group and turns plugins off (`DefaultPlugins.disable::<WaterPlugin>()`),
//! or adds the plugins it wants one by one. The server plugins live in the `torque-app-server` crate (no drawing, GUI, audio
//! or window crates) and are re-exported here with [`MinimalPlugins`], the group a dedicated server needs.

use std::cell::RefCell;
use std::rc::Rc;

pub use torque_audio::plugin::AudioPlugin;
use torque_client::conn::tge::TgeConn;
use torque_client::game::ClientGame;
use torque_client::host::{ClientPass, EnginePass};
use torque_client::state::ClientState;
use torque_engine::{
    ClientStage, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder, Requires, Runner,
};
use torque_gui::{DrawCache, Gui};
use torque_ts_vm::vm::Vm;

pub use torque_app_server::plugins::{
    GameClassesPlugin, MinimalPlugins, NetHostPlugin, ServerGamePlugin, ServerWiring,
    SimClassesPlugin, TerrainPlugin, WaterPlugin,
};

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
            .add(AudioPlugin)
            .add(ClientHostPlugin)
            .add(PageBridgePlugin)
            .add(RunnerPlugin)
    }
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

/// `setFov`, `setDefaultFov`, `setZoomSpeed` and `$cameraFov`, over the zoom the client's state keeps. Not in
/// [`DefaultPlugins`].
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.zoom")]
pub struct ZoomPlugin;

impl Plugin for ZoomPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Natives, |vm, _| {
            let wiring = client_wiring(vm, "the zoom")?;
            torque_client::zoom::register(vm, wiring.state());
            Ok(())
        });
    }
}

/// The console function that sets when the window's frame breakdown prints ([`torque_client::live_perf`]): `name([mode])`,
/// keeping the mode in the pref `pref`. Not in [`DefaultPlugins`]; the game picks the two names.
#[derive(Clone, Copy, Debug, torque_engine::PluginId)]
#[plugin(id = "torque.live-perf")]
pub struct LivePerfPlugin {
    /// The console function's name.
    pub name: &'static str,
    /// The global that keeps the mode (`$pref::...`).
    pub pref: &'static str,
}

impl LivePerfPlugin {
    /// The plugin that registers `name` over the pref `pref`.
    pub fn new(name: &'static str, pref: &'static str) -> LivePerfPlugin {
        LivePerfPlugin { name, pref }
    }
}

impl Plugin for LivePerfPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let (name, pref) = (self.name, self.pref);
        engine.client(ClientStage::Natives, move |vm, _| {
            torque_client::live_perf::register(vm, name, pref);
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

/// What [`Engine::run`](torque_engine::Engine::run) does: sets the engine's runner slot to the runners built in today (the
/// window, the page's canvas, a dedicated server, offscreen), which the [`RunMode`](crate::run::RunMode) the host chose
/// picks between. See [`run`](crate::run).
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.runner")]
pub struct RunnerPlugin;

impl Plugin for RunnerPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let runner: Rc<dyn Runner> = Rc::new(crate::run::AppRunner);
        engine.set_slot::<dyn Runner>(runner);
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
