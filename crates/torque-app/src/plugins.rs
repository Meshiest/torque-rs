//! [`DefaultPlugins`]: the engine's own wiring of a server's and a client's script engine, as plugins.
//!
//! Each plugin wraps one install that stays where it lives (`torque_game::specs::install`, `torque_server::game::install`,
//! `torque_client::screen::register`, ...) and files it as a recipe at a stage. The host that makes a script engine
//! ([`GameServer::start_engine`](crate::server::GameServer::start_engine),
//! [`ClientBuilder::from_engine`](crate::client::ClientBuilder::from_engine)) replays the stages in order, handing the
//! parts a stage needs to its recipes through the script engine's extension map ([`ServerWiring`], [`ClientWiring`] and
//! [`ClientParts`]). The order of the stages is the order the installs always ran in; within a stage, the order the plugins
//! were added.
//!
//! The client recipes name no conn game: a client is made for one
//! ([`ClientBuilder::from_engine_with_conn`](crate::client::ClientBuilder::from_engine_with_conn); TGE's by default), and
//! [`ClientParts::installs`] holds the engine's installs made for it, which the recipes call.
//!
//! | Plugin | Id | Role and stage | Wraps |
//! |--------|----|----------------|-------|
//! | [`ModsPlugin`] | `torque.mods` | both, server `Prepare` and client `Game` (the files), `Entry` (the scripts) | local mods: folders and zips mounted over the game's files, their scripts run as packages (`torque_mods`) |
//! | [`GameClassesPlugin`] | `torque.game-classes` | both, `Classes` | `torque_game::specs::install` |
//! | [`ServerGamePlugin`] | `torque.server-game` | server, `Game` | `torque_server::game::install` |
//! | [`TerrainPlugin`] | `torque.terrain` | server, `World` | [`terrain::install`](crate::terrain::install) |
//! | [`WaterPlugin`] | `torque.water` | server, `World` | [`water::install`](crate::water::install) |
//! | [`ProtocolPlugins`] | `torque.net.tge`, `torque.net.engine` | both, the engine's protocol slot | [`EngineProtocolPlugin`] sets the engine's own protocol (on by default), [`TgeProtocolPlugin`] Torque's (added off); one of them is on ([`protocol_from_env`]) |
//! | [`NetHostPlugin`] | `torque.net-host` | server, `Net` | `attach_host` and the net passes |
//! | [`ServerTransportPlugins`] | `torque.transport-udp-server`, ... | the transports a server listens on, by id (UDP, memory, web, rooms, relay per build) | [`transports`](crate::transports) |
//! | [`ContentFormatPlugins`] | `torque.format-gltf`, `torque.format-dif`, ... | the content formats' asset loaders, a plugin each (OBJ, Blockbench and `.map` added off) | [`torque_content`] |
//! | [`HierarchyPlugin`] | `torque.hierarchy` | server, `Game`, a `Resolve` tick hook; only in engine mode | the scene hierarchy: parent and child transforms (`torque_plugin_hierarchy`) |
//! | [`GuiPlugin`] | `torque.gui` | client, `Gui` | `torque_gui::register`, the control classes plugins add ([`torque_gui::custom`]) and the GUI extensions ([`torque_gui::fanout`]); a game's own GUI plugin gives the client its GUI with [`ClientParts::set_gui`] instead |
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
use torque_client::conn::ConnGame;
use torque_client::conn::tge::TgeConn;
use torque_client::game::ClientGame;
use torque_client::host::{ClientPass, EnginePass};
use torque_client::input_source::InputSourceEntry;
use torque_client::state::ClientState;
use torque_engine::{ClientStage, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder};
use torque_gui::client_script::RealmLayoutEntry;
use torque_gui::custom::ControlEntry;
use torque_gui::{DrawCache, Gui};
use torque_ts_vm::vm::Vm;
use web_time::Duration;

pub use torque_app_server::net_game::{NetCodecsPlugin, NetGamePlugin};
pub use torque_app_server::plugins::{
    ContentFormatPlugins, EngineProtocolPlugin, GameClassesPlugin, HierarchyPlugin, MinimalPlugins,
    ModsPlugin, NetHostPlugin, PROTOCOL_ENV, ProtocolPlugins, ServerGamePlugin,
    ServerTransportPlugins, ServerWiring, SimClassesPlugin, TerrainPlugin, TgeProtocolPlugin,
    WaterPlugin, protocol_from_env, protocol_plugin_id, set_protocol,
};

pub use crate::camera_modes::CameraModesPlugin;
pub use crate::client::HOSTS_SERVER_GLOBAL;
use crate::client::{ClientError, ClientNet};
pub use crate::gizmos::GizmosPlugin;
use crate::hosting::{Resolver, register_join_natives_with};
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
/// server replays only the server recipes, a client only the client ones. They speak the engine's own protocol
/// ([`ProtocolPlugins`]; natively with the `quic` feature, UDP is the QUIC link on both ends); a game that must speak
/// Torque's turns it on with [`set_protocol`]`(.., ProtocolKind::Tge)` or `[net] protocol = "tge"`.
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
            .add(ClientHostPlugin::default())
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

/// What a client's recipes share while the client is made, typed by its conn game `C`: the natives' state, and the parts
/// every client has ([`ClientParts`]). The host sets it as the script engine's extension before the first stage, so
/// `vm.extension::<ClientWiring>()` finds a client of TGE's conn game and `vm.extension::<ClientWiring<C>>()` one of `C`.
pub struct ClientWiring<C: ConnGame = TgeConn> {
    state: Rc<RefCell<ClientState<C>>>,
    parts: Rc<ClientParts>,
}

impl<C: ConnGame> ClientWiring<C> {
    pub(crate) fn new(
        state: Rc<RefCell<ClientState<C>>>,
        parts: Rc<ClientParts>,
    ) -> ClientWiring<C> {
        ClientWiring { state, parts }
    }

    /// The state the client's natives share.
    pub fn state(&self) -> &Rc<RefCell<ClientState<C>>> {
        &self.state
    }

    /// The kind of socket the client was given.
    pub fn net(&self) -> ClientNet {
        self.parts.net
    }

    /// The GUI ([`ClientParts::gui`]), once its stage ran.
    pub fn gui(&self) -> Option<Gui> {
        self.parts.gui()
    }

    /// The wiring of the client `vm` is being made for, a client of conn game `C`, for the recipe of `plugin`.
    ///
    /// # Errors
    /// No client is being made ([`ClientError::NotAClient`]), or the client is of another conn game
    /// ([`ClientError::WrongConnGame`], naming both).
    pub fn of(vm: &Vm, plugin: &str) -> Result<Rc<ClientWiring<C>>, ClientError> {
        if let Some(wiring) = vm.extension::<ClientWiring<C>>() {
            return Ok(wiring);
        }
        let parts = client_parts(vm, plugin)?;
        Err(ClientError::WrongConnGame {
            plugin: plugin.to_owned(),
            wanted: std::any::type_name::<C>(),
            found: parts.conn,
        })
    }
}

/// What a client's recipes share whatever its conn game: the socket kind, the GUI ([`GuiPlugin`]'s or a game's), and the engine's
/// installs made for the client's conn game. The host sets it as the script engine's extension beside [`ClientWiring`].
pub struct ClientParts {
    /// The type name of the client's conn game.
    conn: &'static str,
    net: ClientNet,
    gui: RefCell<Option<Gui>>,
    installs: Box<dyn ClientInstalls>,
}

impl ClientParts {
    /// The parts of a client of conn game `C` over `state`.
    pub(crate) fn for_conn<C: ConnGame>(
        state: &Rc<RefCell<ClientState<C>>>,
        net: ClientNet,
    ) -> ClientParts {
        ClientParts {
            conn: std::any::type_name::<C>(),
            net,
            gui: RefCell::new(None),
            installs: Box::new(Installs(Rc::clone(state))),
        }
    }

    /// The type name of the client's conn game.
    pub fn conn(&self) -> &'static str {
        self.conn
    }

    /// The kind of socket the client was given.
    pub fn net(&self) -> ClientNet {
        self.net
    }

    /// The GUI ([`GuiPlugin`], or the one a game's GUI plugin set with [`set_gui`](Self::set_gui)), once its stage ran.
    pub fn gui(&self) -> Option<Gui> {
        self.gui.borrow().clone()
    }

    /// Gives the client `gui`, the GUI a game's own plugin made at [`ClientStage::Gui`] in place of [`GuiPlugin`]'s (which
    /// sets its own here): the later stages' recipes and the client find it in [`gui`](Self::gui).
    ///
    /// # Errors
    /// The client has a GUI already: two plugins made one ([`ClientError::TwoGuis`]).
    pub fn set_gui(&self, gui: Gui) -> Result<(), ClientError> {
        let mut slot = self.gui.borrow_mut();
        if slot.is_some() {
            return Err(ClientError::TwoGuis);
        }
        *slot = Some(gui);
        Ok(())
    }

    /// The engine's installs over the client's state, made for its conn game.
    pub fn installs(&self) -> &dyn ClientInstalls {
        &*self.installs
    }
}

/// The engine's client installs over a client's state, made for its conn game ([`ClientParts::installs`]): a recipe calls
/// them without naming the conn game. Each wraps the install in `torque_client` (or [`hosting`](crate::hosting)) it names.
pub trait ClientInstalls {
    /// `ActionMap`'s natives (`torque_client::actionmap::register`), and the input sources `sources` the client polls.
    fn action_maps(&self, vm: &mut Vm, sources: &[InputSourceEntry]);
    /// The screen's natives over `gui` (`torque_client::screen::register`).
    fn screen(&self, vm: &mut Vm, gui: &Gui);
    /// The connection's natives over `gui`: the engine's, then the conn game's own (`torque_client::conn::register`).
    fn conn(&self, vm: &mut Vm, gui: &Gui);
    /// The LAN query's natives (`torque_client::query::register`).
    fn query(&self, vm: &mut Vm);
    /// `setFov` and its kin (`torque_client::zoom::register`).
    fn zoom(&self, vm: &mut Vm);
    /// The join screen's natives, looking names up with `resolver` for at most `timeout`
    /// ([`register_join_natives_with`]).
    fn join_natives(&self, vm: &mut Vm, resolver: Resolver, timeout: Duration);
}

/// [`ClientInstalls`] for conn game `C`.
struct Installs<C: ConnGame>(Rc<RefCell<ClientState<C>>>);

impl<C: ConnGame> ClientInstalls for Installs<C> {
    fn action_maps(&self, vm: &mut Vm, sources: &[InputSourceEntry]) {
        torque_client::actionmap::register(vm, &self.0);
        torque_client::input_source::install(&mut self.0.borrow_mut(), sources);
    }

    fn screen(&self, vm: &mut Vm, gui: &Gui) {
        torque_client::screen::register(vm, gui, &self.0);
    }

    fn conn(&self, vm: &mut Vm, gui: &Gui) {
        torque_client::conn::register(vm, &self.0, gui);
    }

    fn query(&self, vm: &mut Vm) {
        torque_client::query::register(vm, &self.0);
    }

    fn zoom(&self, vm: &mut Vm) {
        torque_client::zoom::register(vm, &self.0);
    }

    fn join_natives(&self, vm: &mut Vm, resolver: Resolver, timeout: Duration) {
        register_join_natives_with(vm, &self.0, resolver, timeout);
    }
}

/// The client parts a recipe is replayed with.
pub(crate) fn client_parts(vm: &Vm, plugin: &str) -> Result<Rc<ClientParts>, ClientError> {
    vm.extension::<ClientParts>()
        .ok_or_else(|| ClientError::NotAClient {
            plugin: plugin.to_owned(),
        })
}

/// The GUI, for a plugin that builds on it.
fn client_gui(parts: &ClientParts, plugin: &str) -> Result<Gui, ClientError> {
    parts.gui().ok_or_else(|| ClientError::GuiNotMade {
        plugin: plugin.to_owned(),
    })
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
        engine.client_script().prefs::<torque_gui::GuiPrefs>();
        engine.client(ClientStage::Gui, move |vm, engine| {
            let parts = client_parts(vm, "the GUI")?;
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
            // The extensions the plugins added (`gui_primary`, `gui_extension`), made for this client's GUI.
            gui.install_extensions(vm, engine);
            Ok(parts.set_gui(gui)?)
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
            let parts = client_parts(vm, "the action maps")?;
            vm.register_class("ActionMap", "SimObject", &[]);
            // The input sources the plugins added (`add_input_source`): the client polls them for `joystick<N>` events.
            parts
                .installs()
                .action_maps(vm, engine.list::<InputSourceEntry>());
            Ok(())
        });
    }
}

/// The screen: resolution, full screen and the canvas's natives, over the client's GUI ([`GuiPlugin`]'s or a game's, made at
/// [`ClientStage::Gui`] before it).
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.screen")]
pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Screen, |vm, _| {
            let parts = client_parts(vm, "the screen")?;
            let gui = client_gui(&parts, "the screen")?;
            parts.installs().screen(vm, &gui);
            Ok(())
        });
    }
}

/// The connection to a server (TGE's connection half and the ghosts' natives), for a client that was given a socket
/// ([`ClientNet`] other than `None`); the socket itself is an argument of the client's start. Like [`ScreenPlugin`], over the
/// client's GUI, [`GuiPlugin`]'s or a game's.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.conn")]
pub struct ConnPlugin;

impl Plugin for ConnPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Conn, |vm, engine| {
            let parts = client_parts(vm, "the connection")?;
            if parts.net() == ClientNet::None {
                return Ok(());
            }
            let gui = client_gui(&parts, "the connection")?;
            parts.installs().conn(vm, &gui);
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
            client_parts(vm, "the server query")?.installs().query(vm);
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
            client_parts(vm, "the zoom")?.installs().zoom(vm);
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

/// What [`ClientBuilder::build`](crate::client::ClientBuilder::build) makes the client host from: the draw cache, the
/// window's mode and the global that tells the scripts whether the client hosts a server. The pass that drives the engine's
/// half of each tick is the engine's slot of `dyn ClientPass<C>` for the client's conn game `C` when a game set one, else
/// [`EnginePass`]. [`ClientHostPlugin`] sets this, and the slot for TGE's.
#[derive(Clone, Copy)]
pub struct ClientHostSetup {
    /// Makes the GUI's draw cache ([`draw_cache`](crate::draw_cache)).
    pub draw_cache: fn() -> DrawCache,
    /// Whether the host is made in a window's mode ([`ClientHost::set_windowed`](torque_client::host::ClientHost::set_windowed)).
    pub windowed: bool,
    /// The global the client sets to `1` while it hosts a server and to `0` otherwise, from the moment it is built
    /// ([`HOSTS_SERVER_GLOBAL`] by default); `None` for a game whose scripts keep no such global, which is then never set.
    pub hosts_server_global: Option<&'static str>,
}

/// The client host that runs the frames: the engine's per-tick pass and draw cache, in a window's mode. The host takes the
/// script engine by value, so [`ClientBuilder::build`](crate::client::ClientBuilder::build) makes it from the slot and
/// resource this plugin sets, once the game's scripts are loaded.
///
/// `hosts_server_global` becomes [`ClientHostSetup::hosts_server_global`]: a game whose scripts have no such global turns it
/// off with `DefaultPlugins.set(ClientHostPlugin { hosts_server_global: None })`.
#[derive(Clone, Copy, Debug, torque_engine::PluginId)]
#[plugin(id = "torque.frame-host")]
pub struct ClientHostPlugin {
    /// The global the client sets to whether it hosts a server ([`HOSTS_SERVER_GLOBAL`] by default); `None` sets none.
    pub hosts_server_global: Option<&'static str>,
}

impl Default for ClientHostPlugin {
    /// The host that sets [`HOSTS_SERVER_GLOBAL`].
    fn default() -> ClientHostPlugin {
        ClientHostPlugin {
            hosts_server_global: Some(HOSTS_SERVER_GLOBAL),
        }
    }
}

impl Plugin for ClientHostPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let pass: Rc<dyn ClientPass<TgeConn>> = Rc::new(EnginePass);
        engine.set_slot::<dyn ClientPass<TgeConn>>(pass);
        engine.insert_resource(ClientHostSetup {
            draw_cache: crate::draw_cache,
            windowed: true,
            hosts_server_global: self.hosts_server_global,
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
