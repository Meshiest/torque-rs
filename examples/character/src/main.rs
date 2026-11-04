//! The animated character example.
//!
//! `example-character` opens the window; `example-character --shots <dir>` draws the idle, walk and jump pictures offscreen
//! into `dir` instead (no window).

use std::path::PathBuf;

use torque_example_character::game::CharacterGame;
use torque_example_character::{shots, window};

/// The pictures' size.
const SHOT_SIZE: (u32, u32) = (960, 540);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut shots_dir = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => {
                shots_dir = Some(PathBuf::from(args.next().ok_or("--shots needs a folder")?))
            }
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    match shots_dir {
        Some(dir) => {
            for path in shots::write_states(&dir, SHOT_SIZE)? {
                println!("{}", path.display());
            }
        }
        None => window::run(CharacterGame::new()?)?,
    }
    Ok(())
}
