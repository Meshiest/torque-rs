//! The vehicle demo.
//!
//! `vehicle-demo` opens the window on the menu: play alone, host a game (the others on the network join it), or join one from the
//! list of games on the network or at a typed address. `--solo` plays alone at once (no socket for the game, nobody on the network
//! sees it), `--host` hosts at once, `--join <address>` joins at once (one of the three), `--name <name>` sets the player's name,
//! `--bind <address>` hosts on that address instead of this computer's LAN address. `--shots <dir>` draws its pictures offscreen
//! into `dir` (two clients over the loopback address, no window); `--lit-particle-shots <dir>` the tires' dust unlit and lit;
//! `--cloud-shots <dir>` the sky's clouds through the day, with the engine's clouds and with the demo's.

use torque::app::cli::{Args, unknown};
use torque_example_vehicle_demo::client::{ClientOptions, Start, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_example_vehicle_demo::shots;

/// The pictures' size.
const SHOT_SIZE: (u32, u32) = (1280, 720);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut shots_dir = None;
    let mut lit_dir = None;
    let mut cloud_dir = None;
    let mut options = ClientOptions::default();
    let mut host = false;
    let mut solo = false;
    let mut join = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => shots_dir = Some(args.dir(&arg)?),
            "--lit-particle-shots" => lit_dir = Some(args.dir(&arg)?),
            "--cloud-shots" => cloud_dir = Some(args.dir(&arg)?),
            "--host" => host = true,
            "--solo" => solo = true,
            "--join" => join = Some(args.value(&arg, "an address")?),
            "--name" => options.name = args.value(&arg, "a name")?,
            "--bind" => options.host_address = args.ipv4(&arg)?,
            _ => return Err(unknown(&arg).into()),
        }
    }
    let start = Start::from_flags(solo, host, join)?;
    if let Some(dir) = cloud_dir {
        let shots = torque_example_vehicle_demo::cloud_shots::write_cloud_shots;
        for path in shots(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = lit_dir {
        for path in shots::write_lit_particles(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = shots_dir {
        for path in shots::write_all(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    let mut game = VehicleDemo::new(GameFiles::bundled()?, options)?;
    start.apply(&mut game)?;
    torque::app::window::run(game)?;
    Ok(())
}
