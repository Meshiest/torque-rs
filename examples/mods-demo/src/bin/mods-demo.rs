//! Plays the demo headless: a server whose game mode is the `orbs` mod (Lua), and two clients that download the mod's content
//! when they join. Prints what each does, then a few rounds of the score. `--shots <dir>` draws pictures of the first round from
//! the first client instead of playing on (needs a GPU, offscreen: no window opens).
//!
//! Needs the Lua extension: build it with `just lua` (the library lands in `<build folder>/release`), or name it with `--lua
//! <library>` or `TORQUE_LUA_LIBRARY`. A packaged game (`just bundle examples/mods-demo`) loads it from its `plugins/` folder.

use std::path::PathBuf;

use torque_example_mods_demo::viewer::write_shots;
use torque_example_mods_demo::{Lua, Session, lua_library};

struct Options {
    lua: Option<PathBuf>,
    lua_given: bool,
    work: PathBuf,
    shots: Option<PathBuf>,
    rounds: u32,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        lua: lua_library(),
        lua_given: false,
        work: PathBuf::from("target/mods-demo"),
        shots: None,
        rounds: 2,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--lua" => {
                options.lua = Some(PathBuf::from(value("--lua")?));
                options.lua_given = true;
            }
            "--work" => options.work = PathBuf::from(value("--work")?),
            "--shots" => options.shots = Some(PathBuf::from(value("--shots")?)),
            "--rounds" => {
                options.rounds = value("--rounds")?
                    .parse()
                    .map_err(|_| "--rounds takes a number".to_string())?;
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

fn main() -> Result<(), String> {
    let options = options()?;
    let library = options
        .lua
        .ok_or("no Lua extension: build it with `just lua`, or pass --lua <library>")?;
    // A packaged game loads the extension from its own `plugins/` folder; a build copies the one it was given into one.
    let lua = if cfg!(feature = "bundled") && !options.lua_given {
        let plugins = library.parent().ok_or("the library has no folder")?;
        Lua::bundled(plugins.parent().ok_or("the plugins folder has no folder")?)?
    } else {
        Lua::install(&library, &options.work.join("lua")).map_err(|e| e.to_string())?
    };
    let downloads = options.work.join("downloads");
    let _ = std::fs::remove_dir_all(&downloads);
    let mut session = Session::new(Some(lua), false, downloads)?;
    for client in 0..2 {
        session.add_client()?;
        session.connect(client)?;
    }
    if !session.run_until(4_000, |s| s.clients.iter().all(|c| c.ghost_count() >= 8)) {
        return Err("the clients did not join".into());
    }
    for (n, client) in session.clients.iter_mut().enumerate() {
        let progress = client.call("getModDownloadProgress", &[])?;
        let mods = client.call("getModList", &[])?;
        println!(
            "client {n}: downloaded (bytes, of, mods mounted, fetched) {progress}; mods {mods:?}"
        );
    }
    let list = session.server.call("getModList", &[])?;
    for id in list.split_whitespace() {
        let status = session.server.call("getModField", &[id, "status"])?;
        println!("server mod {id}: {status}");
    }
    // Pictures are of the first round, from the start: a session that has played on is not the one to draw.
    if let Some(dir) = options.shots {
        for path in write_shots(session, &dir)? {
            println!("picture: {}", path.display());
        }
        return Ok(());
    }
    let mut round = 0;
    let mut ticks = 0;
    while round <= options.rounds && ticks < 20_000 {
        session.step();
        ticks += 1;
        let now = session.server_global("$Orbs::Round").parse().unwrap_or(0);
        if now != round {
            round = now;
            println!("round {round}");
        }
        if ticks % 160 == 0 {
            println!(
                "  {}  {}  scores {}  orbs taken {}",
                session.server_global("$Orbs::Round"),
                session.server_global("$Orbs::Phase"),
                session.server_global("$Orbs::Scores"),
                session.server_global("$Orbs::Collected"),
            );
        }
    }
    for line in session.server_console() {
        if line.starts_with("orbs:") || line.starts_with("golden-orbs:") {
            println!("server console: {line}");
        }
    }
    Ok(())
}
