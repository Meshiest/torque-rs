//! The example's dedicated server: loads the minimal example's game, binds the loopback address and runs headless.
//!
//! `cargo run -p torque-example-minimal-engine --bin min-engine-server -- [--port 28000] [--ticks N]`
//!
//! With `--ticks N` it stops after N ticks (32 ms each); without, it runs until it is stopped.

use std::net::Ipv4Addr;

use torque::app::cli::{Args, unknown};
use torque::app::plugins::WindowRunnerPlugin;
use torque::app::run::{Dedicated, DedicatedRunnerPlugin, EngineDedicated};
use torque::app::server::Listen;
use torque_example_minimal_engine::{builder, dedicated, files};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut port, mut ticks) = (28000, None);
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => port = args.number(&arg)?,
            "--ticks" => ticks = Some(args.number(&arg)?),
            _ => return Err(unknown(&arg).into()),
        }
    }
    let files = files()?;
    let listen = Listen::Udp {
        address: Ipv4Addr::LOCALHOST,
        port,
        public: false,
    };
    builder(&files)?
        .disable::<WindowRunnerPlugin>()
        .add_plugins(DedicatedRunnerPlugin)
        .dedicated(Dedicated {
            ticks,
            ..dedicated(&files, listen)
        })
        .run()?;
    Ok(())
}
