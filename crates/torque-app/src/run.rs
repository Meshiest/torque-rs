//! Running an engine: what [`Engine::run`](torque_engine::Engine::run) does with the runners compiled in today.
//!
//! The engine's [`RunnerPlugin`](crate::plugins::RunnerPlugin) (in [`DefaultPlugins`](crate::plugins::DefaultPlugins)) sets
//! the engine's runner, which picks one of these by the [`RunMode`] the host chose:
//!
//! | Mode | Runs | Built for |
//! |------|------|-----------|
//! | [`RunMode::Auto`] (the default) | the window natively, the page's canvas in a browser | any |
//! | [`RunMode::Window`] | the game in a native window ([`window::run`](crate::window::run)) | native |
//! | [`RunMode::Web`] | the game on the page's canvas ([`window::run`](crate::window::run) in a browser) | the web |
//! | [`RunMode::Dedicated`] | a dedicated server ([`GameServer::run_dedicated_until`](crate::server::GameServer::run_dedicated_until)) with no window | native |
//! | [`RunMode::Offscreen`] | the game drawn without a window ([`shots`](crate::shots)), with a picture at the end if asked | native |
//!
//! The window, web and offscreen modes need the game: the app the plugins named with [`EngineRun::app`], made from the
//! engine when the mode starts. A dedicated server needs files and a start function ([`Dedicated`]) and no app.
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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use torque_engine::{Engine, EngineBuilder, Runner};
use torque_gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;
use torque_ts_vm::host::MemFiles;

use crate::server::Listen;
use crate::view::WorldView;
use crate::{App, Host};

/// How the engine's runner starts the game. Set with [`EngineRun::run_mode`].
#[derive(Clone, Default)]
#[non_exhaustive]
pub enum RunMode {
    /// The window on a computer, the page's canvas in a browser: what the build is for.
    #[default]
    Auto,
    /// The game in a native window. An error in a browser build.
    Window,
    /// The game on the page's canvas, with the page's start-up (a panic shown in its console). An error natively.
    Web,
    /// A dedicated server with no window and no app. An error in a browser build.
    Dedicated(Dedicated),
    /// The game drawn offscreen with no window. An error in a browser build.
    Offscreen(Offscreen),
}

/// A request to stop a [`Dedicated`] server, from anywhere: a plugin's native, a signal handler, another thread. Clones
/// share one request.
#[derive(Clone, Debug, Default)]
pub struct Stop(Arc<AtomicBool>);

impl Stop {
    /// A request not made yet.
    pub fn new() -> Stop {
        Stop::default()
    }

    /// Asks the server to stop after the tick it is on.
    pub fn request(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether a stop was requested.
    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Takes a console line of a dedicated server.
pub type PrintLine = Rc<dyn Fn(&str)>;

/// A dedicated server: [`GameServer::start_engine`](crate::server::GameServer::start_engine)'s arguments, and when to stop.
#[derive(Clone)]
pub struct Dedicated {
    /// The game's files, as the server's scripts see them.
    pub files: MemFiles,
    /// Where the server listens.
    pub listen: Listen,
    /// The globals set before `server.cs` runs.
    pub globals: Vec<(String, String)>,
    /// The script function that starts the map.
    pub start: String,
    /// The start function's arguments.
    pub args: Vec<String>,
    /// Stops after this many ticks (never with none).
    pub ticks: Option<u64>,
    /// Stops when this is requested.
    pub stop: Stop,
    /// Takes each console line (none: printed to standard output).
    pub print: Option<PrintLine>,
}

impl Dedicated {
    /// A server over `files` on `listen` that calls the script function `start` and runs until it is stopped.
    pub fn new(files: MemFiles, listen: Listen, start: impl Into<String>) -> Dedicated {
        Dedicated {
            files,
            listen,
            globals: Vec::new(),
            start: start.into(),
            args: Vec::new(),
            ticks: None,
            stop: Stop::new(),
            print: None,
        }
    }
}

/// The game drawn without a window.
#[derive(Clone, Debug)]
pub struct Offscreen {
    /// The size of the frames.
    pub size: (u32, u32),
    /// How much of the game's clock to run (ms), a frame every [`shots::FRAME_MS`](crate::shots::FRAME_MS).
    pub ms: u32,
    /// A PNG of the last frame, written here when the clock has run (its folder is made).
    pub picture: Option<PathBuf>,
}

/// How the app is made from the engine.
pub(crate) type MakeApp = dyn Fn(&Engine) -> Result<Box<dyn App>, String>;

/// The engine builder's methods for running: the game, and how to run it.
pub trait EngineRun {
    /// Names the game: `make` makes the [`App`] from the built engine when a window or an offscreen mode starts. An
    /// exclusive slot, like the runner's: naming a second game fails the build.
    fn app<A: App + 'static>(
        &mut self,
        make: impl Fn(&Engine) -> Result<A, String> + 'static,
    ) -> &mut EngineBuilder;

    /// Chooses how the runner starts the game (the default is [`RunMode::Auto`]).
    fn run_mode(&mut self, mode: RunMode) -> &mut EngineBuilder;

    /// [`run_mode`](Self::run_mode) with a dedicated server.
    fn dedicated(&mut self, server: Dedicated) -> &mut EngineBuilder {
        self.run_mode(RunMode::Dedicated(server))
    }
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

    fn run_mode(&mut self, mode: RunMode) -> &mut EngineBuilder {
        self.insert_resource(mode)
    }
}

/// The engine's runner ([`RunnerPlugin`](crate::plugins::RunnerPlugin)).
pub(crate) struct AppRunner;

impl Runner for AppRunner {
    fn run(&self, engine: &Engine) -> Result<(), String> {
        match engine.resource::<RunMode>().unwrap_or(&RunMode::Auto) {
            RunMode::Auto if cfg!(target_arch = "wasm32") => web(engine),
            RunMode::Auto | RunMode::Window => window(engine),
            RunMode::Web => web(engine),
            RunMode::Dedicated(server) => dedicated(engine, server),
            RunMode::Offscreen(offscreen) => offscreen_run(engine, offscreen),
        }
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
    Err("a browser build runs on the page's canvas: use RunMode::Web".to_string())
}

#[cfg(target_arch = "wasm32")]
fn web(engine: &Engine) -> Result<(), String> {
    crate::web_page::init();
    crate::window::run(make_app(engine)?)
}

#[cfg(not(target_arch = "wasm32"))]
fn web(_: &Engine) -> Result<(), String> {
    Err("RunMode::Web runs only in a browser build: use RunMode::Window".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn dedicated(engine: &Engine, server: &Dedicated) -> Result<(), String> {
    use crate::server::{GameServer, ServerStart};

    let globals: Vec<(&str, &str)> = server
        .globals
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let args: Vec<&str> = server.args.iter().map(String::as_str).collect();
    let mut game = GameServer::start_engine(
        engine,
        server.files.clone(),
        &ServerStart {
            listen: server.listen.clone(),
            globals: &globals,
            start: &server.start,
            args: &args,
        },
    )?;
    game.run_dedicated_until(
        |ticks| server.ticks.is_some_and(|t| ticks >= t) || server.stop.is_requested(),
        |line| match &server.print {
            Some(print) => print(line),
            None => println!("{line}"),
        },
    );
    game.close();
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn dedicated(_: &Engine, _: &Dedicated) -> Result<(), String> {
    Err("a dedicated server does not run in a browser build".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn offscreen_run(engine: &Engine, offscreen: &Offscreen) -> Result<(), String> {
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
fn offscreen_run(_: &Engine, _: &Offscreen) -> Result<(), String> {
    Err("an offscreen run does not run in a browser build".to_string())
}
