//! Running an engine: [`Engine::run`] and [`EngineBuilder::run`] hand the built engine to the [`Runner`] a plugin set.

use std::fmt;
use std::process::ExitCode;

use crate::{BuildError, Engine, EngineBuilder};

/// What runs a built engine: opens the game's window, serves a dedicated game, or whatever else the plugin that sets
/// this slot (`set_slot::<dyn Runner>`) stands for. The runner reads the engine's resources and slots for what it needs
/// (the game, the options the host chose).
///
/// ```
/// use std::rc::Rc;
/// use torque_engine::{Engine, EngineBuilder, Plugin, PluginId, Runner};
///
/// #[derive(PluginId)]
/// struct Hello;
/// impl Plugin for Hello {
///     fn build(&self, engine: &mut EngineBuilder) {
///         struct Say;
///         impl Runner for Say {
///             fn run(&self, engine: &Engine) -> Result<(), String> {
///                 println!("{} plugins", engine.plugins().len());
///                 Ok(())
///             }
///         }
///         engine.set_slot::<dyn Runner>(Rc::new(Say));
///     }
/// }
///
/// EngineBuilder::new().add_plugins(Hello).run()?;
/// # Ok::<(), torque_engine::RunError>(())
/// ```
pub trait Runner {
    /// Runs `engine` until the game is over. A message in `Err` is shown to the player as it stands.
    fn run(&self, engine: &Engine) -> Result<(), String>;
}

/// Why an engine did not run to its end.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum RunError {
    /// The engine did not build.
    Build(BuildError),
    /// No plugin set the [`Runner`] slot.
    NoRunner,
    /// The runner stopped with an error: a script engine's recipe failed (the message names the plugin and the stage),
    /// the game's files or scripts were not usable, a window or a socket was not opened.
    Failed(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Build(e) => write!(f, "the engine did not build: {e}"),
            RunError::NoRunner => f.write_str(
                "the engine has no runner: add a plugin that sets one (DefaultPlugins has one)",
            ),
            RunError::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::Build(e) => Some(e),
            _ => None,
        }
    }
}

impl From<BuildError> for RunError {
    fn from(e: BuildError) -> RunError {
        RunError::Build(e)
    }
}

impl Engine {
    /// Runs this engine with the [`Runner`] a plugin set. It returns when the game is over; a browser's page keeps the
    /// game going after this returns.
    pub fn run(&self) -> Result<(), RunError> {
        let runner = self.slot::<dyn Runner>().ok_or(RunError::NoRunner)?;
        runner.run(self).map_err(RunError::Failed)
    }
}

impl EngineBuilder {
    /// Builds the engine and runs it ([`Engine::run`]).
    pub fn run(&mut self) -> Result<(), RunError> {
        self.build()?.run()
    }

    /// [`run`](Self::run) for a program's `main`: the error is printed to stderr and the exit code is a failure.
    ///
    /// ```no_run
    /// use std::process::ExitCode;
    /// use torque_engine::EngineBuilder;
    ///
    /// fn main() -> ExitCode {
    ///     EngineBuilder::new().run_exit()
    /// }
    /// ```
    pub fn run_exit(&mut self) -> ExitCode {
        match self.run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        }
    }
}
