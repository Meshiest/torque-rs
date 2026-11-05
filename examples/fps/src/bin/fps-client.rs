//! The arena's client.
//!
//! `fps-client` opens the window on the main menu: Host starts a server in this process and joins it (for the local network
//! only, or public: every interface, so players outside the network can join when UDP 28000 is forwarded to this computer),
//! Join lists the games on the local network and takes an address to connect to by hand. `--bind <address>` hosts a LAN game on
//! that address instead of this computer's LAN address, `--name <name>` sets the player's name, `--shots <dir>` draws the review
//! pictures offscreen into `dir` (two clients over the loopback address, no window), and `--motion-shots <dir>` the pictures of
//! the figures' and the pistol's motion (side steps, the walk backward, the look, the falls, a shot, the reload, the pause
//! menu), `--menu-shots <dir>` the menus at 1280x720, 1920x1080 and 2560x1080; `--polish-shots <dir>` the pictures of a bolt's
//! flight frame by frame, the bolts' holes, a hit from both sides and aiming down the sights; `--polish2-shots <dir>` a shot's
//! flares and smoke frame by frame, a bolt's ribbon from the side, bolt holes at a wall's middle and edge and in the floor, and
//! the sights aimed (also at 1920 x 1080); `--hitbox-shots <dir>` the other figure's hitboxes drawn over it in its poses;
//! `--lit-particle-shots <dir>` the muzzle's smoke unlit and lit, in the sun and in shade.

use torque::app::cli::{Args, unknown};
use torque_example_fps::client::{ArenaGame, ClientOptions};
use torque_example_fps::files::GameFiles;
use torque_example_fps::shots;

/// The pictures' size.
const SHOT_SIZE: (u32, u32) = (1280, 720);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut shots_dir = None;
    let mut motion_dir = None;
    let mut menu_dir = None;
    let mut polish_dir = None;
    let mut polish2_dir = None;
    let mut hitbox_dir = None;
    let mut lit_dir = None;
    let mut options = ClientOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => shots_dir = Some(args.dir(&arg)?),
            "--motion-shots" => motion_dir = Some(args.dir(&arg)?),
            "--menu-shots" => menu_dir = Some(args.dir(&arg)?),
            "--polish-shots" => polish_dir = Some(args.dir(&arg)?),
            "--polish2-shots" => polish2_dir = Some(args.dir(&arg)?),
            "--hitbox-shots" => hitbox_dir = Some(args.dir(&arg)?),
            "--lit-particle-shots" => lit_dir = Some(args.dir(&arg)?),
            "--bind" => options.host_address = args.ipv4(&arg)?,
            "--name" => options.name = args.value(&arg, "a name")?,
            _ => return Err(unknown(&arg).into()),
        }
    }
    if let Some(dir) = menu_dir {
        let sizes = [(1280, 720), (1920, 1080), (2560, 1080)];
        for path in shots::write_menus(&dir, &sizes, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = lit_dir {
        for path in shots::write_lit_smoke(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = hitbox_dir {
        for path in shots::write_hitboxes(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
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
        None => torque::app::window::run(ArenaGame::new(GameFiles::bundled()?, options)?)?,
    }
    Ok(())
}
