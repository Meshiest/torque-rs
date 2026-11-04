//! The level demo.
//!
//! `example-level-demo` opens the window on the menu of maps; `example-level-demo --shots <dir>` draws pictures of the menu
//! and of every map (an overview, a view from behind the figure and one from its eyes) offscreen into `dir` instead (no
//! window).

use std::path::PathBuf;

use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;
use torque_example_level_demo::{shots, window};

/// The pictures' size.
const SHOT_SIZE: (u32, u32) = (1280, 720);

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
    let files = GameFiles::bundled()?;
    match shots_dir {
        Some(dir) => {
            for path in shots::write_all(files, &dir, SHOT_SIZE)? {
                println!("{}", path.display());
            }
        }
        None => window::run(LevelDemo::new(files)?)?,
    }
    Ok(())
}
