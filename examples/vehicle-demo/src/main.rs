//! The vehicle demo.
//!
//! `vehicle-demo` opens the window on the menu: host a game (the others on the network join it), or join one from the list of
//! games on the network or at a typed address. `--host` hosts at once, `--join <address>` joins at once, `--name <name>` sets the
//! player's name, `--bind <address>` hosts on that address instead of this computer's LAN address. `--shots <dir>` draws its
//! pictures offscreen into `dir` (two clients over the loopback address, no window).

use torque::app::cli::{Args, unknown};
use torque_example_vehicle_demo::client::{ClientOptions, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_example_vehicle_demo::shots;

/// The pictures' size.
const SHOT_SIZE: (u32, u32) = (1280, 720);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut shots_dir = None;
    let mut options = ClientOptions::default();
    let mut host = false;
    let mut join = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => shots_dir = Some(args.dir(&arg)?),
            "--host" => host = true,
            "--join" => join = Some(args.value(&arg, "an address")?),
            "--name" => options.name = args.value(&arg, "a name")?,
            "--bind" => options.host_address = args.ipv4(&arg)?,
            _ => return Err(unknown(&arg).into()),
        }
    }
    if let Some(dir) = shots_dir {
        for path in shots::write_all(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    let mut game = VehicleDemo::new(GameFiles::bundled()?, options)?;
    if host {
        game.host_game()?;
    } else if let Some(address) = join {
        let address = torque::app::hosting::parse_join_address(&address)?;
        game.join(&address.to_string())?;
    }
    torque::app::window::run(game)?;
    Ok(())
}
