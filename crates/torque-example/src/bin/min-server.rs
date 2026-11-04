//! The example's dedicated server: loads the bundled game, binds the loopback address and runs headless.
//!
//! `cargo run -p torque-example --bin min-server -- [--port 28000] [--ticks N] [--root <game folder>]`
//!
//! With `--ticks N` it stops after N ticks (64 a second); without, it runs until it is stopped.

use std::path::PathBuf;

use torque_example::server::{DedicatedServer, ServerNote, ServerOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = ServerOptions::loopback(28000);
    let mut ticks = u64::MAX;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or(format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--port" => options.port = value.parse()?,
            "--ticks" => ticks = value.parse()?,
            "--root" => options.root = PathBuf::from(value),
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    let mut server = DedicatedServer::start(&options)?;
    println!(
        "Serving {} ghosted objects on {}",
        server.ghosted_count(),
        server.local_addr().ok_or("no socket")?
    );
    server.run(ticks, |server| {
        for note in server.take_notes() {
            match note {
                ServerNote::Console(line) => println!("console: {line}"),
                ServerNote::Connected { conn, from } => {
                    println!("client {conn} connected from {from}")
                }
                ServerNote::Ghosting { conn } => println!("client {conn} is ghosting"),
                ServerNote::Left { conn, reason } => println!("client {conn} left: {reason}"),
            }
        }
    });
    server.shutdown();
    println!("Stopped after {} ticks", server.ticks());
    Ok(())
}
