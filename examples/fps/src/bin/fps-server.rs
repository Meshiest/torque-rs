//! The arena's dedicated server: the game's `server.cs` and a map on a UDP socket, answering the LAN list, with the console's
//! lines printed. No window.
//!
//! `fps-server [--bind <address>] [--port <port>] [--name <name>] [--map <path>] [--ticks <n>]`: the address defaults to
//! `127.0.0.1` (this computer only; pass the computer's LAN address so others on the network can join), the port to 28000, the
//! map to the first in `game/missions/`; `--ticks` stops after that many ticks (a short check).

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use torque::server::server::TICK_MS;
use torque_example_fps::files::GameFiles;
use torque_example_fps::server::{ArenaServer, DEFAULT_PORT, ServerOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = GameFiles::bundled()?;
    let mut options = ServerOptions {
        address: Ipv4Addr::LOCALHOST,
        port: DEFAULT_PORT,
        name: "Arena".to_string(),
        mission: String::new(),
    };
    let mut ticks = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--bind" => {
                let text = value()?;
                options.address = text
                    .parse()
                    .map_err(|_| format!("--bind needs an IPv4 address, not {text:?}"))?;
            }
            "--port" => options.port = value()?.parse().map_err(|_| "--port needs a number")?,
            "--name" => options.name = value()?,
            "--map" => options.mission = value()?,
            "--ticks" => {
                ticks = Some(
                    value()?
                        .parse::<u64>()
                        .map_err(|_| "--ticks needs a number")?,
                )
            }
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    if options.mission.is_empty() {
        options.mission = files
            .maps()
            .into_iter()
            .next()
            .ok_or("no map in the game folder's missions")?;
    }
    let mut server = ArenaServer::start_dedicated(&files, &options)?;
    if let Some(addr) = server.address() {
        println!(
            "fps-server: {} on {addr}, map {}",
            options.name, options.mission
        );
    }
    let tick = Duration::from_millis(u64::from(TICK_MS));
    let mut next = Instant::now();
    let mut done = 0u64;
    loop {
        server.tick();
        for line in server.drain_log() {
            println!("{line}");
        }
        done += 1;
        if ticks.is_some_and(|t| done >= t) {
            break;
        }
        next += tick;
        let now = Instant::now();
        match next.checked_duration_since(now) {
            Some(wait) => std::thread::sleep(wait),
            // Behind: catch up from now rather than run a burst of ticks.
            None => next = now,
        }
    }
    Ok(())
}
