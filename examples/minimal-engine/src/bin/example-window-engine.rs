//! The example's client in a window: connects to an example server and draws the ghosts it is sent.
//!
//! `cargo run -p torque-example-minimal-engine --features window --bin example-window-engine -- [--server 127.0.0.1:28000 | --local]`

use torque::app::cli::{Args, unknown};
use torque::app::plugins::DefaultPlugins;
use torque::app::run::EngineRun;
use torque::engine::EngineBuilder;
use torque_example_minimal_engine::window::ExampleGame;
use torque_example_minimal_engine::{ExamplePlugin, files};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = Some("127.0.0.1:28000".parse()?);
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--local" => server = None,
            "--server" => server = Some(args.value(&arg, "an address")?.parse()?),
            _ => return Err(unknown(&arg).into()),
        }
    }
    EngineBuilder::new()
        .add_plugins((DefaultPlugins, ExamplePlugin(files()?)))
        .app(move |engine| ExampleGame::new(engine, server))
        .run()?;
    Ok(())
}
