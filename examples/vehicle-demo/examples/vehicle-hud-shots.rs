//! The vehicle demo's HUD for review: a joined player's view on foot with the corner readout, the pause menu and its Controls
//! panel, then the view at the wheel, at 1280x720, 800x600 and 640x480, drawn offscreen over the loopback address.
//!
//! `cargo run -p torque-example-vehicle-demo --release --example vehicle-hud-shots -- <dir>`

use std::path::PathBuf;

use torque::app::shots::{Seat, Shots, run};
use torque::app::window::offscreen_gpu;
use torque::client::input::MouseButton;
use torque_example_vehicle_demo::client::{ClientOptions, NetMode, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_render_core::Gpu;

const WAIT_LIMIT_MS: u32 = 20_000;

fn client(gpu: &Gpu, name: &str, size: (u32, u32)) -> Result<Seat<VehicleDemo>, String> {
    let files = GameFiles::bundled().map_err(|e| e.to_string())?;
    let game = VehicleDemo::new(
        files,
        ClientOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: 0,
            ..ClientOptions::default()
        },
    )?;
    Seat::new(game, gpu, size)
}

/// Runs both clocks until `done` holds.
fn until(
    gpu: &Gpu,
    a: &mut Seat<VehicleDemo>,
    b: &mut Seat<VehicleDemo>,
    what: &str,
    done: impl Fn(&VehicleDemo, &VehicleDemo) -> bool,
) -> Result<(), String> {
    let mut waited = 0;
    while !done(&a.game, &b.game) {
        if waited > WAIT_LIMIT_MS {
            return Err(format!("timed out waiting for {what}"));
        }
        run(gpu, &mut [&mut *a, &mut *b], 100)?;
        waited += 100;
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: vehicle-hud-shots <dir>")?,
    );
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut shots = Shots::new(&dir)?;
    for size in [(1280, 720), (800, 600), (640, 480)] {
        let tag = format!("{}x{}", size.0, size.1);
        let (mut a, mut b) = (client(&gpu, "Ann", size)?, client(&gpu, "Bob", size)?);
        a.game.host_game()?;
        until(&gpu, &mut a, &mut b, "the hosted game", |a, _| {
            a.in_world() && a.buggy_ghost().is_some()
        })?;
        let address = a.game.hosted_address().ok_or("no hosted address")?;
        b.game.join(&address.to_string())?;
        until(&gpu, &mut a, &mut b, "the second player", |_, b| {
            b.in_world() && b.buggy_ghost().is_some()
        })?;
        // The map's name gone from the middle of the view.
        run(&gpu, &mut [&mut a, &mut b], 3000)?;
        shots.save(&b.picture(&gpu)?, &format!("vehicle-play-{tag}"))?;
        b.game.call("pauseKey", &["1"])?;
        run(&gpu, &mut [&mut a, &mut b], 200)?;
        shots.save(&b.picture(&gpu)?, &format!("vehicle-pause-{tag}"))?;
        b.game.call("showControls", &[])?;
        run(&gpu, &mut [&mut a, &mut b], 100)?;
        shots.save(&b.picture(&gpu)?, &format!("vehicle-controls-{tag}"))?;
        b.game.call("resumeKey", &["1"])?;
        // Bob looks down at the buggy and clicks: at the wheel.
        b.game.host.mouse_motion(0.0, 110.0);
        run(&gpu, &mut [&mut a, &mut b], 300)?;
        b.game
            .host
            .mouse_button(MouseButton::Left, true, (400, 300));
        b.game
            .host
            .mouse_button(MouseButton::Left, false, (400, 300));
        run(&gpu, &mut [&mut a, &mut b], 1500)?;
        shots.save(&b.picture(&gpu)?, &format!("vehicle-wheel-{tag}"))?;
    }
    for path in &shots.paths {
        println!("{}", path.display());
    }
    Ok(())
}
