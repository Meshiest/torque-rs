//! Running an engine: the runners compiled in, each a plugin on the engine's exclusive runner slot
//! ([`Engine::run`](torque_engine::Engine::run) hands the engine to whichever one the plugins set).
//!
//! | Plugin | Id | Runs | Built for |
//! |--------|----|------|-----------|
//! | [`WindowRunnerPlugin`] | `torque.window-runner` | the game in a native window ([`window::run`](crate::window::run)) | native (in [`DefaultPlugins`](crate::plugins::DefaultPlugins)) |
//! | [`WebPageRunnerPlugin`] | `torque.web-page-runner` | the game on the page's canvas, with the page's start-up and its script bridge | the web (in [`DefaultPlugins`](crate::plugins::DefaultPlugins)) |
//! | [`OffscreenRunnerPlugin`] | `torque.offscreen-runner` | the game drawn without a window ([`shots`](crate::shots)), with a picture at the end if asked | native |
//! | [`DedicatedRunnerPlugin`] | `torque.dedicated-runner` | a dedicated server ([`GameServer::run_dedicated_until`](crate::server::GameServer::run_dedicated_until)) with no window | native |
//!
//! The slot holds one runner. [`DefaultPlugins`](crate::plugins::DefaultPlugins) has the platform's: a game that runs another
//! way turns that one off and adds the one it wants.
//!
//! ```
//! use torque_app::plugins::{DefaultPlugins, WindowRunnerPlugin};
//! use torque_app::run::OffscreenRunnerPlugin;
//! use torque_engine::{EngineBuilder, PluginGroup};
//!
//! let engine = EngineBuilder::new()
//!     .add_plugins((
//!         DefaultPlugins.disable::<WindowRunnerPlugin>(),
//!         OffscreenRunnerPlugin::new((320, 180), 500),
//!     ))
//!     .build()?;
//! assert!(engine.is_plugin_added::<OffscreenRunnerPlugin>());
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! Two runners in one engine fail the build, naming both plugins.
//!
//! The window, web page and offscreen runners need the game: the app the plugins named with [`EngineRun::app`], made from the
//! engine when the runner starts. A dedicated server needs files and a start function ([`Dedicated`]) and no app.
//!
//! ```no_run
//! use torque_app::App;
//! use torque_app::plugins::DefaultPlugins;
//! use torque_app::run::EngineRun;
//! use torque_engine::{Engine, EngineBuilder};
//!
//! fn run_game<G: App + 'static>(make: fn(&Engine) -> Result<G, String>) -> Result<(), torque_engine::RunError> {
//!     EngineBuilder::new()
//!         .add_plugins(DefaultPlugins)
//!         .app(make)
//!         .run()
//! }
//! ```

use std::path::PathBuf;
use std::rc::Rc;

use torque_engine::{ClientStage, Engine, EngineBuilder, Plugin, Runner};
use torque_gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::view::WorldView;
use crate::{App, Host};
pub use torque_app_server::run::{
    Dedicated, DedicatedRunnerPlugin, EngineDedicated, PrintLine, Stop, run_dedicated,
};

/// How the app is made from the engine.
pub(crate) type MakeApp = dyn Fn(&Engine) -> Result<Box<dyn App>, String>;

/// The engine builder's method for naming the game.
pub trait EngineRun {
    /// Names the game: `make` makes the [`App`] from the built engine when a window, a page or an offscreen runner starts.
    /// An exclusive slot, like the runner's: naming a second game fails the build.
    fn app<A: App + 'static>(
        &mut self,
        make: impl Fn(&Engine) -> Result<A, String> + 'static,
    ) -> &mut EngineBuilder;
}

impl EngineRun for EngineBuilder {
    fn app<A: App + 'static>(
        &mut self,
        make: impl Fn(&Engine) -> Result<A, String> + 'static,
    ) -> &mut EngineBuilder {
        let make: Rc<MakeApp> = Rc::new(move |engine| {
            let app: Box<dyn App> = Box::new(make(engine)?);
            Ok(app)
        });
        self.set_slot::<MakeApp>(make)
    }
}

/// Sets the engine's runner slot to a native window's: [`Engine::run`](torque_engine::Engine::run) opens the window and
/// plays the game [`EngineRun::app`] named in it. In [`DefaultPlugins`](crate::plugins::DefaultPlugins) on native builds. In a
/// browser build it only fails, saying to use [`WebPageRunnerPlugin`].
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.window-runner")]
pub struct WindowRunnerPlugin;

impl Plugin for WindowRunnerPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let runner: Rc<dyn Runner> = Rc::new(WindowRunner);
        engine.set_slot::<dyn Runner>(runner);
    }
}

struct WindowRunner;

impl Runner for WindowRunner {
    fn run(&self, engine: &Engine) -> Result<(), String> {
        window(engine)
    }
}

/// Sets the engine's runner slot to the web page's: the game plays on the page's canvas, with the page's start-up (a panic
/// shown in its console). It also files the page bridge ([`page_bridge::install`](crate::page_bridge::install): the page and
/// the game's scripts calling each other, `callPage` and `setRenderQuality`) at the client's `Window` stage, which the window
/// runs once its surface exists. In [`DefaultPlugins`](crate::plugins::DefaultPlugins) on web builds. Natively it only fails,
/// saying to use [`WindowRunnerPlugin`].
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.web-page-runner")]
pub struct WebPageRunnerPlugin;

impl Plugin for WebPageRunnerPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let runner: Rc<dyn Runner> = Rc::new(WebPageRunner);
        engine.set_slot::<dyn Runner>(runner);
        if cfg!(target_arch = "wasm32") {
            engine.client(ClientStage::Window, |vm, _| {
                crate::page_bridge::install(vm);
                Ok(())
            });
        }
    }
}

struct WebPageRunner;

impl Runner for WebPageRunner {
    fn run(&self, engine: &Engine) -> Result<(), String> {
        web(engine)
    }
}

/// Sets the engine's runner slot to an offscreen one: the game is drawn without a window for [`ms`](Self::ms) of its clock (a
/// frame every [`FRAME_MS`](crate::shots::FRAME_MS)), then a PNG of the last frame is written to [`picture`](Self::picture) if
/// one is set. Not in [`DefaultPlugins`](crate::plugins::DefaultPlugins). An error in a browser build.
#[derive(Clone, Debug, torque_engine::PluginId)]
#[plugin(id = "torque.offscreen-runner")]
pub struct OffscreenRunnerPlugin {
    /// The size of the frames.
    pub size: (u32, u32),
    /// How much of the game's clock to run (ms).
    pub ms: u32,
    /// A PNG of the last frame, written here when the clock has run (its folder is made).
    pub picture: Option<PathBuf>,
}

impl OffscreenRunnerPlugin {
    /// Frames of `size` for `ms` of the game's clock, with no picture written.
    pub fn new(size: (u32, u32), ms: u32) -> OffscreenRunnerPlugin {
        OffscreenRunnerPlugin {
            size,
            ms,
            picture: None,
        }
    }

    /// Writes a PNG of the last frame to `path`.
    #[must_use]
    pub fn picture(mut self, path: impl Into<PathBuf>) -> OffscreenRunnerPlugin {
        self.picture = Some(path.into());
        self
    }
}

impl Plugin for OffscreenRunnerPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let runner: Rc<dyn Runner> = Rc::new(OffscreenRunner(self.clone()));
        engine.set_slot::<dyn Runner>(runner);
    }
}

struct OffscreenRunner(OffscreenRunnerPlugin);

impl Runner for OffscreenRunner {
    fn run(&self, engine: &Engine) -> Result<(), String> {
        offscreen_run(engine, &self.0)
    }
}

/// The app the engine's game makes.
fn make_app(engine: &Engine) -> Result<Boxed, String> {
    let make = engine
        .slot::<MakeApp>()
        .ok_or("the engine has no game to show: name it with `EngineRun::app`")?;
    make(engine).map(Boxed)
}

/// The app the engine made, as an [`App`] of its own.
struct Boxed(Box<dyn App>);

impl App for Boxed {
    fn host(&mut self) -> &mut Host {
        self.0.host()
    }

    fn host_ref(&self) -> &Host {
        self.0.host_ref()
    }

    fn title(&self) -> String {
        self.0.title()
    }

    fn log_name(&self) -> &str {
        self.0.log_name()
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        self.0.view(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.0.advance(ms);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        self.0.frame(view)
    }

    fn loading(&self) -> bool {
        self.0.loading()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn window(engine: &Engine) -> Result<(), String> {
    crate::window::run(make_app(engine)?)
}

#[cfg(target_arch = "wasm32")]
fn window(_: &Engine) -> Result<(), String> {
    Err("a browser build runs on the page's canvas: use WebPageRunnerPlugin".to_string())
}

#[cfg(target_arch = "wasm32")]
fn web(engine: &Engine) -> Result<(), String> {
    crate::web_page::init();
    crate::window::run(make_app(engine)?)
}

#[cfg(not(target_arch = "wasm32"))]
fn web(_: &Engine) -> Result<(), String> {
    Err("WebPageRunnerPlugin runs only in a browser build: use WindowRunnerPlugin".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn offscreen_run(engine: &Engine, offscreen: &OffscreenRunnerPlugin) -> Result<(), String> {
    use crate::shots::{Seat, write_png};

    let app = make_app(engine)?;
    let gpu = crate::window::offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = Seat::new(app, &gpu, offscreen.size)?;
    seat.run(&gpu, offscreen.ms)?;
    if let Some(path) = &offscreen.picture {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        }
        let frame = seat.picture(&gpu)?;
        write_png(&frame, path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn offscreen_run(_: &Engine, _: &OffscreenRunnerPlugin) -> Result<(), String> {
    Err("an offscreen run does not run in a browser build".to_string())
}
