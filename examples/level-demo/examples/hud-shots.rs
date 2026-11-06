//! The level demo's HUD for review: the view in play with the corner readout, the pause menu and its Controls panel, at
//! 1280x720 and 800x600, drawn offscreen.
//!
//! `cargo run -p torque-example-level-demo --release --example hud-shots -- <dir>`

use std::path::PathBuf;

use torque::app::shots::{Seat, Shots};
use torque::app::window::offscreen_gpu;
use torque::client::input::Key;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;

/// The longest the map may take to start (ms of the clock).
const START_LIMIT_MS: u32 = 60_000;

fn main() -> Result<(), String> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or("usage: hud-shots <dir>")?);
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut shots = Shots::new(&dir)?;
    for size in [(1280, 720), (800, 600)] {
        let tag = format!("{}x{}", size.0, size.1);
        let files = GameFiles::bundled().map_err(|e| e.to_string())?;
        let mut seat = Seat::new(LevelDemo::new(files)?, &gpu, size)?;
        seat.run(&gpu, 200)?;
        seat.game.key(Key::Digit1, true);
        seat.game.key(Key::Digit1, false);
        let mut waited = 0;
        while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
            seat.run(&gpu, 100)?;
            waited += 100;
        }
        if !seat.game.in_world() {
            return Err("the first map did not start".into());
        }
        // The map's name gone from the middle of the view.
        seat.run(&gpu, 3000)?;
        shots.save(&seat.picture(&gpu)?, &format!("level-play-{tag}"))?;
        seat.game.key(Key::Escape, true);
        seat.game.key(Key::Escape, false);
        seat.run(&gpu, 200)?;
        shots.save(&seat.picture(&gpu)?, &format!("level-pause-{tag}"))?;
        seat.game.call("showControls", &[])?;
        seat.run(&gpu, 100)?;
        shots.save(&seat.picture(&gpu)?, &format!("level-controls-{tag}"))?;
    }
    for path in &shots.paths {
        println!("{}", path.display());
    }
    Ok(())
}
