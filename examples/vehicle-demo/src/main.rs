//! The vehicle demo.
//!
//! `vehicle-demo` opens the window on the menu: play alone, host a game (the others on the network join it), or join one from the
//! list of games on the network or at a typed address. `--solo` plays alone at once (no socket for the game, nobody on the network
//! sees it), `--host` hosts at once, `--join <address>` joins at once (one of the three), `--name <name>` sets the player's name,
//! `--bind <address>` hosts on that address instead of this computer's LAN address. `--shots <dir>` draws its pictures offscreen
//! into `dir` (two clients over the loopback address, no window); `--lit-particle-shots <dir>` the tires' dust unlit and lit;
//! `--cloud-shots <dir>` the sky's clouds through the day, with the engine's clouds and with the demo's. `--web-files <dir>`
//! writes the files a build for the web fetches into `dir`: the pack (`files::web_pack`) as `game.pack` and the list naming it
//! (`files.json`).
//!
//! Built for the web, it is the page's game (the library's `web` module): played alone, joined to a server or a player's host,
//! or hosted by the page with its server in the page's worker; the page calls the functions of `page` and `host_page`. The
//! dedicated server is `vehicle-demo-server`.

#[cfg(not(target_arch = "wasm32"))]
use torque::app::cli::{Args, unknown};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_vehicle_demo::client::{ClientOptions, Start, VehicleDemo};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_vehicle_demo::files::{self, GameFiles};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_vehicle_demo::shots;

/// The pictures' size.
#[cfg(not(target_arch = "wasm32"))]
const SHOT_SIZE: (u32, u32) = (1280, 720);

#[cfg(target_arch = "wasm32")]
fn main() {
    torque::app::web_page::init();
}

// What the page calls (`web/index.html` and `web/files.js`): the engine's web entry over the library's `web` module.
#[cfg(target_arch = "wasm32")]
torque::app::web_exports! {
    embedded: torque_example_vehicle_demo::web::EMBEDDED,
    start_embedded: torque_example_vehicle_demo::web::start_embedded,
    start_fetched: torque_example_vehicle_demo::web::start_fetched,
}

// What a browser host's page and worker call (`web/host.js`, `web/host-worker.js`).
#[cfg(target_arch = "wasm32")]
torque::app::web_host_exports! {
    start_host_client: torque_example_vehicle_demo::web::start_host_client,
    host_start: torque_example_vehicle_demo::web::host_start,
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut shots_dir = None;
    let mut lit_dir = None;
    let mut cloud_dir = None;
    let mut web_dir = None;
    let mut options = ClientOptions::default();
    let mut host = false;
    let mut solo = false;
    let mut join = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => shots_dir = Some(args.dir(&arg)?),
            "--lit-particle-shots" => lit_dir = Some(args.dir(&arg)?),
            "--cloud-shots" => cloud_dir = Some(args.dir(&arg)?),
            "--web-files" => web_dir = Some(args.dir(&arg)?),
            "--host" => host = true,
            "--solo" => solo = true,
            "--join" => join = Some(args.value(&arg, "an address")?),
            "--name" => options.name = args.value(&arg, "a name")?,
            "--bind" => options.host_address = args.ipv4(&arg)?,
            _ => return Err(unknown(&arg).into()),
        }
    }
    let start = Start::from_flags(solo, host, join)?;
    if let Some(dir) = web_dir {
        let pack = files::web_pack(&GameFiles::bundled()?);
        let file = torque::app::web_page::write_web_files(&dir, &pack)?;
        println!("{} ({} bytes)", file.display(), pack.len());
        return Ok(());
    }
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
