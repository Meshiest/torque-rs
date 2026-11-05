//! The level demo.
//!
//! `example-level-demo` opens the window on the menu of maps; `example-level-demo --shots <dir>` draws pictures of the menu
//! and of every map (an overview, a view from behind the figure and one from its eyes) offscreen into `dir` instead (no
//! window). `example-level-demo --web-files <dir>` writes the files a build for the web fetches into `dir`: the pack
//! (`files::web_pack`) as `game.pack`, and the list of files beside the page (`files.json`, `torque::app::manifest`) naming it.
//! `example-level-demo --web-pack <file>` writes the pack alone, for a build that carries it (the `embed-pack` feature).
//!
//! Built for the web, it is the page's game (the library's `web` module): the page calls the functions of `page`.

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
    torque::app::web_page::init();
}

// What the page calls (`web/index.html` and `web/files.js`): the engine's web entry over the library's `web` module.
#[cfg(target_arch = "wasm32")]
torque::app::web_exports! {
    embedded: torque_example_level_demo::web::EMBEDDED,
    start_embedded: torque_example_level_demo::web::start_embedded,
    start_fetched: torque_example_level_demo::web::start_fetched,
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut shots_dir = None;
    let mut office_dir = None;
    let mut pack_file = None;
    let mut web_dir = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => shots_dir = Some(args.dir(&arg)?),
            "--office-shots" => office_dir = Some(args.dir(&arg)?),
            "--web-pack" => pack_file = Some(args.dir(&arg)?),
            "--web-files" => web_dir = Some(args.dir(&arg)?),
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
    if let Some(dir) = web_dir {
        let pack = files::web_pack(files)?;
        let file = torque::app::web_page::write_web_files(&dir, &pack)?;
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
