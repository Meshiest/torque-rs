//! The arena's HUD for review: a joined player's view with the corner readout, the pause menu and its Controls panel, at
//! 1280x720 and 800x600, drawn offscreen.
//!
//! `cargo run -p torque-example-fps --release --example hud-shots -- <dir>`

use std::path::PathBuf;

use torque_example_fps::files::GameFiles;
use torque_example_fps::shots::write_hud;

fn main() -> Result<(), String> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or("usage: hud-shots <dir>")?);
    for size in [(1280, 720), (800, 600)] {
        let tag = format!("{}x{}", size.0, size.1);
        for path in write_hud(&dir, size, &tag, GameFiles::bundled)? {
            println!("{}", path.display());
        }
    }
    Ok(())
}
