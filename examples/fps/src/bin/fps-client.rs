//! The arena's client.
//!
//! `fps-client` opens the window on the main menu: Host starts a server in this process and joins it (for the local network
//! only, or public: every interface, so players outside the network can join when UDP 28000 is forwarded to this computer),
//! Join lists the games on the local network and takes an address to connect to by hand. `--bind <address>` hosts a LAN game on
//! that address instead of this computer's LAN address, `--name <name>` sets the player's name, `--shots <dir>` draws the review
//! pictures offscreen into `dir` (two clients over the loopback address, no window), and `--motion-shots <dir>` the pictures of
//! the figures' and the pistol's motion (side steps, the walk backward, the look, the falls, a shot, the reload, the pause
//! menu).

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
    let mut motion_dir = None;
    let mut options = ClientOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => {
                shots_dir = Some(PathBuf::from(args.next().ok_or("--shots needs a folder")?))
            }
            "--motion-shots" => {
                motion_dir = Some(PathBuf::from(
                    args.next().ok_or("--motion-shots needs a folder")?,
                ))
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
    if let Some(dir) = motion_dir {
        for path in shots::write_motion(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
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
