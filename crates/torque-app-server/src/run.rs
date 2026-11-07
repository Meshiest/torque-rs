//! Running a dedicated server: [`Dedicated`] (the files, the socket and the start function), [`Stop`], and the runner that
//! runs it ([`DedicatedRunnerPlugin`], in [`MinimalPlugins`](crate::plugins::MinimalPlugins)).
//!
//! The runner takes the [`Dedicated`] the engine was given ([`EngineDedicated::dedicated`]) and runs the server
//! ([`GameServer::run_dedicated_until`](crate::server::GameServer::run_dedicated_until)) until it is stopped; there is no window and no app. A native-only runner: in a browser
//! build it returns an error.
//!
//! ```
//! use std::net::{Ipv4Addr, SocketAddr};
//!
//! use torque_app_server::plugins::MinimalPlugins;
//! use torque_app_server::run::{Dedicated, EngineDedicated};
//! use torque_app_server::server::Listen;
//! use torque_engine::EngineBuilder;
//! use torque_ts_vm::host::MemFiles;
//!
//! let files = MemFiles::new();
//! files.insert("server.cs", b"function start() { echo(\"up\"); }");
//! let listen = Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000)));
//! let mut server = Dedicated::new(files, listen, "start");
//! server.ticks = Some(2);
//! server.print = Some(std::rc::Rc::new(|_| {}));
//! EngineBuilder::new()
//!     .add_plugins(MinimalPlugins)
//!     .dedicated(server)
//!     .run()?;
//! # Ok::<(), torque_engine::RunError>(())
//! ```

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use torque_engine::{Engine, EngineBuilder, Plugin, Runner};
use torque_ts_vm::host::MemFiles;

use crate::server::Listen;

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

/// The engine builder's method for a dedicated server.
pub trait EngineDedicated {
    /// Gives the engine the dedicated server its runner ([`DedicatedRunnerPlugin`]) runs.
    fn dedicated(&mut self, server: Dedicated) -> &mut EngineBuilder;
}

impl EngineDedicated for EngineBuilder {
    fn dedicated(&mut self, server: Dedicated) -> &mut EngineBuilder {
        self.insert_resource(server)
    }
}

/// Sets the engine's runner slot to a dedicated server's: [`Engine::run`] runs the [`Dedicated`] the engine was given. The
/// slot is exclusive, so this plugin does not go with a group that has a runner of its own (torque-app's `DefaultPlugins`
/// has a window's or a page's: turn it off to add this one).
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.dedicated-runner")]
pub struct DedicatedRunnerPlugin;

impl Plugin for DedicatedRunnerPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let runner: Rc<dyn Runner> = Rc::new(DedicatedRunner);
        engine.set_slot::<dyn Runner>(runner);
    }
}

struct DedicatedRunner;

impl Runner for DedicatedRunner {
    fn run(&self, engine: &Engine) -> Result<(), String> {
        let server = engine.resource::<Dedicated>().ok_or(
            "the engine has no dedicated server to run: name it with `EngineDedicated::dedicated`",
        )?;
        run_dedicated(engine, server)
    }
}

/// Runs `server` on a game server made from `engine`: until its tick cap or a stop request, with its console lines printed.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_dedicated(engine: &Engine, server: &Dedicated) -> Result<(), String> {
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

/// Runs `server`: not in a browser build.
#[cfg(target_arch = "wasm32")]
pub fn run_dedicated(_: &Engine, _: &Dedicated) -> Result<(), String> {
    Err("a dedicated server does not run in a browser build".to_string())
}
