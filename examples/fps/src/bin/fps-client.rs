//! The arena's client.
//!
//! `fps-client` opens the window on the main menu: Host starts a server in this process and joins it (a game for one, or for
//! others on the network), Join lists the games on the local network. `--bind <address>` hosts on this computer's LAN address
//! instead of `127.0.0.1` (so others can join), `--name <name>` sets the player's name, and `--shots <dir>` draws the review
//! pictures offscreen into `dir` (two clients over the loopback address, no window).

use std::net::Ipv4Addr;
use std::path::PathBuf;

use torque_example_fps::client::{ArenaGame, ClientOptions};
use torque_example_fps::files::GameFiles;
use torque_example_fps::{shots, window};

/// The pictures' size.
const SHOT_SIZE: (u32, u32) = (1280, 720);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut shots_dir = None;
    let mut options = ClientOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => {
                shots_dir = Some(PathBuf::from(args.next().ok_or("--shots needs a folder")?))
            }
            "--bind" => {
                let text = args.next().ok_or("--bind needs an address")?;
                options.host_address = text
                    .parse::<Ipv4Addr>()
                    .map_err(|_| format!("--bind needs an IPv4 address, not {text:?}"))?;
            }
            "--name" => options.name = args.next().ok_or("--name needs a name")?,
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    match shots_dir {
        Some(dir) => {
            for path in shots::write_all(&dir, SHOT_SIZE, GameFiles::bundled)? {
                println!("{}", path.display());
            }
        }
        None => window::run(ArenaGame::new(GameFiles::bundled()?, options)?)?,
    }
    Ok(())
}
