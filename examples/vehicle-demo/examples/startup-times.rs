//! How long the window's start and end take, piece by piece, drawn offscreen (no window): the files, the client, the device,
//! the view, the menu's first frames, a game played alone until the world is drawn, then each part let go in the order the
//! window lets go of them. `--menu` ends at the menu, the way a window closed there does.
//!
//! A first run after the shaders changed (or of a new executable) shows the driver's cold shader cache: the view and the
//! world's first frames take seconds, and the process takes a second or more to end after `total` while the driver writes
//! its cache.

use std::time::Instant;

use torque::app::App as _;
use torque::app::view::WorldView;
use torque::app::window::offscreen_gpu;
use torque_example_vehicle_demo::client::{ClientOptions, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;

/// The longest the game is run waiting for the world (frames of 16 ms of the clock).
const WORLD_FRAMES: u32 = 2000;

fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    println!(
        "{what:<24} {:>8.1} ms",
        start.elapsed().as_secs_f64() * 1000.0
    );
    out
}

fn main() -> Result<(), String> {
    let menu_only = std::env::args().any(|a| a == "--menu");
    let total = Instant::now();
    let files = timed("files", GameFiles::bundled).map_err(|e| e.to_string())?;
    let mut game = timed("client", || {
        VehicleDemo::new(files, ClientOptions::default())
    })?;
    let gpu = timed("device", offscreen_gpu).map_err(|e| e.to_string())?;
    game.host().surface_size((1280, 720));
    let mut view = timed("view", || game.view(&gpu));
    let target = gpu.offscreen(1280, 720).map_err(|e| e.to_string())?;
    let frame = |game: &mut VehicleDemo, view: &mut WorldView| {
        let (gui, eye) = game.frame(view);
        view.draw(&gpu, &target.color, &gui, eye)
    };
    timed("first frame", || frame(&mut game, &mut view))?;
    timed("second frame", || frame(&mut game, &mut view))?;
    println!(
        "{:<24} {:>8.1} ms",
        "to the menu",
        total.elapsed().as_secs_f64() * 1000.0
    );
    if !menu_only {
        timed("game started", || game.play_alone())?;
        let world = Instant::now();
        let mut frames = 0;
        while !game.in_world() && frames < WORLD_FRAMES {
            game.advance(16);
            frame(&mut game, &mut view)?;
            frames += 1;
        }
        println!(
            "{:<24} {:>8.1} ms ({frames} frames)",
            "to the world",
            world.elapsed().as_secs_f64() * 1000.0
        );
        timed("60 world frames", || -> Result<(), String> {
            for _ in 0..60 {
                game.advance(16);
                frame(&mut game, &mut view)?;
            }
            Ok(())
        })?;
    }
    timed("let go of the view", || drop(view));
    timed("let go of the device", || {
        drop(target);
        drop(gpu);
    });
    timed("let go of the game", || drop(game));
    println!(
        "{:<24} {:>8.1} ms",
        "total",
        total.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}
