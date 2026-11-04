//! The arena's dedicated server: the game's `server.cs` and a map on a UDP socket, answering the LAN list, with the console's
//! lines printed. No window.
//!
//! `fps-server [--bind <address>] [--port <port>] [--name <name>] [--map <path>] [--ticks <n>]`: the address defaults to
//! `127.0.0.1` (this computer only; pass the computer's LAN address so others on the network can join, or `0.0.0.0` to listen
//! on every interface so players outside the network can join too), the port to 28000, the map to the first in
//! `game/missions/`; `--ticks` stops after that many ticks (a short check).

use std::net::Ipv4Addr;

use torque::app::cli::{Args, unknown};
use torque_example_fps::files::GameFiles;
use torque_example_fps::server::{ArenaServer, DEFAULT_PORT, ServerOptions, lan_addresses};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = GameFiles::bundled()?;
    let mut options = ServerOptions {
        address: Ipv4Addr::LOCALHOST,
        port: DEFAULT_PORT,
        name: "Arena".to_string(),
        mission: String::new(),
        public: false,
    };
    let mut ticks = None;
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => options.address = args.ipv4(&arg)?,
            "--port" => options.port = args.number(&arg)?,
            "--name" => options.name = args.value(&arg, "a name")?,
            "--map" => options.mission = args.value(&arg, "a map")?,
            "--ticks" => ticks = Some(args.number(&arg)?),
            _ => return Err(unknown(&arg).into()),
        }
    }
    options.public = options.address.is_unspecified();
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
        if options.public {
            let port = addr.port();
            let lan = lan_addresses();
            if lan.is_empty() {
                println!("fps-server: no LAN address found on this computer");
            }
            for ip in lan {
                println!("fps-server: players on this network join {ip}:{port}");
            }
            println!(
                "fps-server: players outside the network need UDP {port} forwarded to this computer, and the firewall to allow it"
            );
        }
    }
    server.run_dedicated(ticks, |line| println!("{line}"));
    Ok(())
}
