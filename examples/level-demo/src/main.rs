//! The level demo.
//!
//! `example-level-demo` opens the window on the menu of maps; `example-level-demo --shots <dir>` draws pictures of the menu
//! and of every map (an overview, a view from behind the figure and one from its eyes) offscreen into `dir` instead (no
//! window); `--valley-shots <dir>` the valley's views of the world alone (and their GPU frame times); `--valley-clip <dir>` eight
//! seconds of the valley's sky moving, a picture every other frame; `--swim-shots <dir>` the figure swimming in the valley's
//! lake; `--swim-eye-shots <dir>` the same from its eyes (its arms, a dive and the waterline); `--switch-shots <dir>` the
//! figure in the valley from behind and from its eyes, looking down, level and up; `--rope-shots <dir>` (a build with the
//! `rope` feature) the rope the plaza hangs between its front pillars, still and thrown up and swinging;
//! `--rope-style-shots <dir>` the same build's ribbon cable and laser line beside it, with the figure crossing them;
//! `--correction-shots <dir>` (a build with the `smooth-prediction` feature) a correction the server forces on the figure,
//! frame by frame, with TGE's predictor and with the smoothed one.
//! `example-level-demo --web-files <dir>` writes the files a
//! build for the web fetches into `dir`: the pack (`files::web_pack`) as `game.pack`, and the list of files beside the page
//! (`files.json`, `torque::app::manifest`) naming it.
//! `example-level-demo --web-pack <file>` writes the pack alone, for a build that carries it (the `embed-pack` feature).
//!
//! The window's menu also hosts the office for others and joins theirs (Host a match, Join a match): `--name <name>` is the
//! player's name, `--port <port>` the UDP port number the list of matches asks on and a hosted match binds (28000), `--bind
//! <address>` the address a hosted match binds (this computer's LAN address), and `--cache <dir>` where the servers' mods are
//! kept (the per-user cache). The catalog's menu needs the realm guest (`just lua-client-web`, or `TORQUE_CLIENT_GUEST`); without
//! it the office is played without the catalog. `--match-shots <dir>` draws that path offscreen into `dir` instead: the menu,
//! the list of matches, the download, the consent prompt and the catalog with another player.
//!
//! Built for the web, it is the page's game (the library's `web` module): the page calls the functions of `page`.

#[cfg(not(target_arch = "wasm32"))]
use torque::app::cli::{Args, unknown};
#[cfg(not(target_arch = "wasm32"))]
use torque::app::run::EngineRun as _;
#[cfg(not(target_arch = "wasm32"))]
use torque_example_level_demo::client::LevelDemo;
#[cfg(not(target_arch = "wasm32"))]
use torque_example_level_demo::files::{self, GameFiles};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_level_demo::shots;

/// The pictures' size, and the valley clip's frames'.
#[cfg(not(target_arch = "wasm32"))]
const SHOT_SIZE: (u32, u32) = (1280, 720);
#[cfg(not(target_arch = "wasm32"))]
const CLIP_SIZE: (u32, u32) = (960, 540);

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
    client_guests: torque_client_script_host::accepted_hex,
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut shots_dir = None;
    let mut office_dir = None;
    let mut valley_dir = None;
    let mut clip_dir = None;
    let mut swim_dir = None;
    let mut swim_eye_dir = None;
    let mut switch_dir = None;
    let mut rope_dir = None;
    let mut rope_style_dir = None;
    let mut correction_dir = None;
    let mut pack_file = None;
    let mut web_dir = None;
    let mut match_dir = None;
    let mut options = torque_example_level_demo::client::MatchOptions::default();
    let mut cache = torque::mods::CacheChoice::PerUser;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => shots_dir = Some(args.dir(&arg)?),
            "--office-shots" => office_dir = Some(args.dir(&arg)?),
            "--valley-shots" => valley_dir = Some(args.dir(&arg)?),
            "--valley-clip" => clip_dir = Some(args.dir(&arg)?),
            "--swim-shots" => swim_dir = Some(args.dir(&arg)?),
            "--swim-eye-shots" => swim_eye_dir = Some(args.dir(&arg)?),
            "--switch-shots" => switch_dir = Some(args.dir(&arg)?),
            "--rope-shots" if cfg!(feature = "rope") => rope_dir = Some(args.dir(&arg)?),
            "--rope-shots" => return Err("--rope-shots needs a build with the rope feature".into()),
            "--rope-style-shots" if cfg!(feature = "rope") => {
                rope_style_dir = Some(args.dir(&arg)?);
            }
            "--rope-style-shots" => {
                return Err("--rope-style-shots needs a build with the rope feature".into());
            }
            "--correction-shots" if cfg!(feature = "smooth-prediction") => {
                correction_dir = Some(args.dir(&arg)?);
            }
            "--correction-shots" => {
                return Err(
                    "--correction-shots needs a build with the smooth-prediction feature".into(),
                );
            }
            "--web-pack" => pack_file = Some(args.dir(&arg)?),
            "--web-files" => web_dir = Some(args.dir(&arg)?),
            "--match-shots" => match_dir = Some(args.dir(&arg)?),
            "--name" => options.name = args.value(&arg, "a name")?,
            "--port" => options.port = args.number(&arg)?,
            "--bind" => options.host_address = args.ipv4(&arg)?,
            "--cache" => cache = torque::mods::CacheChoice::Folder(args.dir(&arg)?),
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
    if let Some(dir) = clip_dir {
        let gpu = torque::app::window::offscreen_gpu()?;
        let paths = torque_example_level_demo::showcase::write_valley_clip(
            &gpu, files, &dir, CLIP_SIZE, 8.0, 2,
        )?;
        println!("{} frames in {}", paths.len(), dir.display());
        return Ok(());
    }
    if let Some(dir) = valley_dir {
        let gpu = torque::app::window::offscreen_gpu()?;
        for path in torque_example_level_demo::showcase::write_valley(&gpu, files, &dir, SHOT_SIZE)?
        {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = swim_dir {
        for path in shots::write_swim(files, &dir, SHOT_SIZE)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = swim_eye_dir {
        for path in shots::write_swim_eye(files, &dir, SHOT_SIZE)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = switch_dir {
        for path in shots::write_switch(files, &dir, SHOT_SIZE)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    #[cfg(feature = "rope")]
    if let Some(dir) = rope_dir {
        for path in shots::write_rope(files, &dir, SHOT_SIZE)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    #[cfg(feature = "rope")]
    if let Some(dir) = rope_style_dir {
        for path in shots::write_rope_styles(files, &dir, SHOT_SIZE)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    #[cfg(not(feature = "rope"))]
    let _: (Option<std::path::PathBuf>, Option<std::path::PathBuf>) = (rope_dir, rope_style_dir);
    #[cfg(feature = "smooth-prediction")]
    if let Some(dir) = correction_dir {
        for path in shots::write_correction(files, &dir, SHOT_SIZE)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    #[cfg(not(feature = "smooth-prediction"))]
    let _: Option<std::path::PathBuf> = correction_dir;
    if let Some(dir) = match_dir {
        use torque_example_level_demo::catalog::{guest_path, load_guest};
        let guest = load_guest(&guest_path())?;
        let work = std::path::PathBuf::from("target/level-demo-match-shots");
        for path in torque_example_level_demo::session::write_shots(&guest, &work, &dir)? {
            println!("{}", path.display());
        }
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
        None => {
            use torque_example_level_demo::catalog::{mod_files, player_scripts};
            use torque_example_level_demo::office::office_engine;
            office_engine(files, mod_files()?, cache)?
                .app(move |engine| {
                    LevelDemo::for_matches(engine, options.clone(), None, player_scripts())
                })
                .run()?;
        }
    }

    Ok(())
}
