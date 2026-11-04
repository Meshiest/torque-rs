//! The arena's client.
//!
//! `fps-client` opens the window on the main menu: Host starts a server in this process and joins it (a game for one, or for
//! others on the network), Join lists the games on the local network. `--bind <address>` hosts on this computer's LAN address
//! instead of `127.0.0.1` (so others can join), `--name <name>` sets the player's name, `--shots <dir>` draws the review
//! pictures offscreen into `dir` (two clients over the loopback address, no window), and `--motion-shots <dir>` the pictures of
//! the figures' and the pistol's motion (side steps, the walk backward, the look, the falls, a shot, the reload, the pause
//! menu), and `--polish-shots <dir>` the pictures of a bolt's flight frame by frame, the bolts' holes, a hit from both sides
//! and aiming down the sights; `--polish2-shots <dir>` a shot's flares and smoke frame by frame, a bolt's ribbon from the side,
//! bolt holes at a wall's middle and edge and in the floor, and the sights aimed (also at 1920 x 1080).

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
    let mut polish_dir = None;
    let mut polish2_dir = None;
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
            "--polish-shots" => {
                polish_dir = Some(PathBuf::from(
                    args.next().ok_or("--polish-shots needs a folder")?,
                ))
            }
            "--polish2-shots" => {
                polish2_dir = Some(PathBuf::from(
                    args.next().ok_or("--polish2-shots needs a folder")?,
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
    if let Some(dir) = polish2_dir {
        for path in shots::write_polish2(&dir, SHOT_SIZE, GameFiles::bundled)?
            .into_iter()
            .chain(shots::write_sights(&dir, (1920, 1080), GameFiles::bundled)?)
        {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = polish_dir {
        for path in shots::write_polish(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
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
