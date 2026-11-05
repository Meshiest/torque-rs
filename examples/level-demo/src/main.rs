//! The level demo.
//!
//! `example-level-demo` opens the window on the menu of maps; `example-level-demo --shots <dir>` draws pictures of the menu
//! and of every map (an overview, a view from behind the figure and one from its eyes) offscreen into `dir` instead (no
//! window). `example-level-demo --web-pack <file>` writes the files the build for the web carries (`files::web_pack`).
//!
//! Built for the web, it is the page's game (the library's `web` module).

#[cfg(not(target_arch = "wasm32"))]
use torque::app::cli::{Args, unknown};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_level_demo::client::LevelDemo;
#[cfg(not(target_arch = "wasm32"))]
use torque_example_level_demo::files::{self, GameFiles};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_level_demo::shots;

/// The pictures' size.
#[cfg(not(target_arch = "wasm32"))]
const SHOT_SIZE: (u32, u32) = (1280, 720);

#[cfg(target_arch = "wasm32")]
fn main() {
    torque_example_level_demo::web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut shots_dir = None;
    let mut office_dir = None;
    let mut pack_file = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => shots_dir = Some(args.dir(&arg)?),
            "--office-shots" => office_dir = Some(args.dir(&arg)?),
            "--web-pack" => pack_file = Some(args.dir(&arg)?),
            _ => return Err(unknown(&arg).into()),
        }
    }
    let files = GameFiles::bundled()?;
    if let Some(file) = pack_file {
        let pack = files::web_pack(files)?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&file, &pack)?;
        println!("{} ({} bytes)", file.display(), pack.len());
        return Ok(());
    }
    if let Some(dir) = office_dir {
        for path in shots::write_office(files, &dir, SHOT_SIZE)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    match shots_dir {
        Some(dir) => {
            for path in shots::write_all(files, &dir, SHOT_SIZE)? {
                println!("{}", path.display());
            }
        }
        None => torque::app::window::run(LevelDemo::new(files)?)?,
    }
    Ok(())
}
