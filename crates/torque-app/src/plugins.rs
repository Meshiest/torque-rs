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
//! | [`ModsPlugin`] | `torque.mods` | both, server `Prepare` and client `Game` (the files), `Entry` (the scripts) | local mods: folders and zips mounted over the game's files, their scripts run as packages (`torque_mods`) |
//! | [`GameClassesPlugin`] | `torque.game-classes` | both, `Classes` | `torque_game::specs::install` |
//! | [`ServerGamePlugin`] | `torque.server-game` | server, `Game` | `torque_server::game::install` |
//! | [`TerrainPlugin`] | `torque.terrain` | server, `World` | [`terrain::install`](crate::terrain::install) |
//! | [`WaterPlugin`] | `torque.water` | server, `World` | [`water::install`](crate::water::install) |
//! | [`ProtocolPlugins`] | `torque.net.tge`, `torque.net.engine` | both, the engine's protocol slot | [`TgeProtocolPlugin`] sets Torque's own protocol, [`EngineProtocolPlugin`] the engine's; one of them is on ([`protocol_from_env`]) |
//! | [`NetHostPlugin`] | `torque.net-host` | server, `Net` | `attach_host` and the net passes |
//! | [`ServerTransportPlugins`] | `torque.transport-udp-server`, ... | the transports a server listens on, by id (UDP, memory, web, rooms, relay per build) | [`transports`](crate::transports) |
//! | [`ContentFormatPlugins`] | `torque.format-gltf`, `torque.format-dif`, ... | the content formats' asset loaders, a plugin each (OBJ, Blockbench and `.map` added off) | [`torque_content`] |
//! | [`HierarchyPlugin`] | `torque.hierarchy` | server, `Game`, a `Resolve` tick hook; only in engine mode | the scene hierarchy: parent and child transforms (`torque_plugin_hierarchy`) |
//! | [`GuiPlugin`] | `torque.gui` | client, `Gui` | `torque_gui::register`, and the control classes plugins add ([`torque_gui::custom`]) |
//! | [`ActionMapPlugin`] | `torque.action-map` | client, `Input` | the `ActionMap` class and its natives, and the input sources plugins add ([`torque_client::input_source`]) |
//! | [`ScreenPlugin`] | `torque.screen` | client, `Screen` | `torque_client::screen::register` |
//! | [`ConnPlugin`] | `torque.conn` | client, `Conn` | `torque_client::conn::register` |
//! | [`ServerQueryPlugin`] | `torque.server-query` | client, `Query` | `torque_client::query::register` |
//! | [`ClientTransportPlugins`] | `torque.transport-udp-client`, ... | the transports a client connects over, by id (UDP, memory, web, rooms, relay per build) | [`transports`](crate::transports) |
//! | [`AudioPlugin`] | `torque.audio` | client, `Natives` | the `alx` natives, the audio datablocks and the mixer |
//! | [`CameraModesPlugin`] | `torque.camera-modes` | client, `Natives` | the camera modes plugins add, the one in use, and `setCameraMode`, `getCameraMode`, `getCameraModes` ([`camera_modes`](crate::camera_modes)) |
//! | [`ClientHostPlugin`] | `torque.frame-host` | client, `Host` | what [`ClientBuilder::build`](crate::client::ClientBuilder::build) makes the host of |
//! | [`RendererSlotPlugin`] | `torque.renderer-slot` | client, `Natives` | the renderer slot and `setRenderer`/`getRenderer` ([`renderers`](crate::renderers)) |
//! | [`RendererPlugin`] | `torque.renderer` | the engine's renderer on the slot, `default` (`renderer` feature) | the view's renderer |
//! | [`RenderFeaturePlugins`] | `torque.render-shadows`, ... | the renderer's switches, a plugin each (`renderer` feature) | [`RenderFeatures`](crate::renderers::RenderFeatures) |
//! | [`GizmosPlugin`] | `torque.gizmos` | client, `Natives` (not in [`DefaultPlugins`]; `gizmos` feature) | debug lines, boxes, spheres and labels in world space ([`gizmos`](crate::gizmos)) |
//! | [`WindowRunnerPlugin`] | `torque.window-runner` | the engine's runner slot (native builds) | [`window::run`](crate::window::run) |
//! | [`WebPageRunnerPlugin`] | `torque.web-page-runner` | the engine's runner slot, client `Window` (web builds) | the page's canvas and [`page_bridge::install`](crate::page_bridge::install) |
//!
//! A build has one of the two runners in the group (the window natively, the page on the web). The other runners are plugins
//! to add in their place ([`OffscreenRunnerPlugin`], [`DedicatedRunnerPlugin`]; see [`run`](crate::run)):
//! `DefaultPlugins.disable::<WindowRunnerPlugin>()` and the one wanted.
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
use torque_client::input_source::InputSourceEntry;
use torque_client::state::ClientState;
use torque_engine::{
    ClientStage, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder, Requires,
};
use torque_gui::client_script::RealmLayoutEntry;
use torque_gui::custom::ControlEntry;
use torque_gui::{DrawCache, Gui};
use torque_ts_vm::vm::Vm;

pub use torque_app_server::plugins::{
    ContentFormatPlugins, EngineProtocolPlugin, GameClassesPlugin, HierarchyPlugin, MinimalPlugins,
    ModsPlugin, NetHostPlugin, PROTOCOL_ENV, ProtocolPlugins, ServerGamePlugin,
    ServerTransportPlugins, ServerWiring, SimClassesPlugin, TerrainPlugin, TgeProtocolPlugin,
    WaterPlugin, protocol_from_env, protocol_plugin_id, set_protocol,
};

pub use crate::camera_modes::CameraModesPlugin;
use crate::client::ClientNet;
pub use crate::gizmos::GizmosPlugin;
pub use crate::renderers::RendererSlotPlugin;
#[cfg(feature = "renderer")]
pub use crate::renderers::{
    AmbientOcclusionPlugin, AntiAliasingPlugin, BloomPlugin, ClearWaterPlugin, CloudsPlugin,
    GlowLightsPlugin, GradingPlugin, ParticleLightPlugin, ReflectionsPlugin, RenderFeaturePlugins,
    RendererPlugin, ShadowsPlugin, SurfaceDetailPlugin, TransparencyPlugin, VolumetricsPlugin,
};
pub use crate::run::{
    DedicatedRunnerPlugin, OffscreenRunnerPlugin, WebPageRunnerPlugin, WindowRunnerPlugin,
};
pub use crate::transports::ClientTransportPlugins;

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
        let group = PluginGroupBuilder::new()
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
            .add(GuiPlugin::default())
            .add(ActionMapPlugin)
            .add(ScreenPlugin)
            .add(ConnPlugin)
            .add(ServerQueryPlugin)
            .add_group(ClientTransportPlugins)
            .add(AudioPlugin)
            .add(CameraModesPlugin)
            .add(ClientHostPlugin)
            .add(RendererSlotPlugin);
        #[cfg(feature = "renderer")]
        let group = group.add(RendererPlugin).add_group(RenderFeaturePlugins);
        if cfg!(target_arch = "wasm32") {
            group.add(WebPageRunnerPlugin)
        } else {
            group.add(WindowRunnerPlugin)
        }
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
pub(crate) fn client_wiring(vm: &Vm, plugin: &str) -> Result<Rc<ClientWiring>, String> {
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
        engine.client(ClientStage::Gui, move |vm, engine| {
            let wiring = client_wiring(vm, "the GUI")?;
            let gui = torque_gui::register(vm);
            // The control classes the plugins added (`add_gui_control`): their classes come with the script registry's, after.
            gui.add_controls(engine.list::<ControlEntry>());
            // The layout a client-script realm's containers use, when a plugin added one (`add_realm_layout`).
            gui.add_realm_layouts(engine.list::<RealmLayoutEntry>());
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
        engine.client(ClientStage::Input, |vm, engine| {
            let wiring = client_wiring(vm, "the action maps")?;
            vm.register_class("ActionMap", "SimObject", &[]);
            torque_client::actionmap::register(vm, wiring.state());
            // The input sources the plugins added (`add_input_source`): the client polls them for `joystick<N>` events.
            torque_client::input_source::install(
                &mut wiring.state().borrow_mut(),
                engine.list::<InputSourceEntry>(),
            );
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
        engine.client(ClientStage::Conn, |vm, engine| {
            let wiring = client_wiring(vm, "the connection")?;
            if wiring.net() == ClientNet::None {
                return Ok(());
            }
            let gui = client_gui(&wiring, "the connection")?;
            torque_client::conn::register(vm, wiring.state(), &gui);
            // A networked engine sends its manifest (and its auth ticket) with every connect request.
            torque_client::conn::tge::ConnectHello::install(vm, engine);
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
