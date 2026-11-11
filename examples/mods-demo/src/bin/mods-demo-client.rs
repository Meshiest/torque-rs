//! The soccer game in a window: the main menu (host a match here, join one from the network's list of games or by its
//! address), the screen that shows the server's mods downloading, then the match (W A S D run, Esc the menu).
//!
//! `mods-demo-client [<address>] [--host] [--name <name>] [--port <port>] [--bind <address>] [--cache <dir>] [--lua <library>]
//! [--work <dir>]`: an address (`a.b.c.d[:port]`) joins that server at once; `--host` starts a match here at once (LAN only)
//! and joins it. Hosting needs the Lua extension (`just lua`, or `--lua`, `TORQUE_LUA_LIBRARY`), copied into
//! `<work>/lua/plugins`. A server's mods are kept in the per-user cache, or in `--cache <dir>`. `--port` is the UDP port number the list
//! of games asks on and a hosted match binds (28000), `--bind` the address a hosted match binds (this computer's LAN address).
//!
//! `--shots <dir>` draws the review pictures offscreen into `dir` (no window): the menu, the list of games, the download, a
//! join with the mod already downloaded, and the match. `--web-files <dir>` writes the files a build for the web fetches
//! (`game.pack` and its list, `files.json`).
//!
//! Built for the web, it is the page's client (the library's `web` module): the page calls the functions of `page`.

#[cfg(target_arch = "wasm32")]
fn main() {
    torque::app::web_page::init();
}

// What the page calls (`web/index.html` and `web/files.js`): the engine's web entry over the library's `web` module.
#[cfg(target_arch = "wasm32")]
torque::app::web_exports! {
    embedded: false,
    start_embedded: torque_example_mods_demo::web::start_embedded,
    start_fetched: torque_example_mods_demo::web::start_fetched,
    prepare: torque_example_mods_demo::web::prepare,
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;

    use torque::app::cli::{Args, unknown};
    use torque::app::hosting::parse_join_address;
    use torque::app::run::EngineRun;
    use torque::mods::CacheChoice;
    use torque_example_mods_demo::game::{ClientOptions, SoccerGame};
    use torque_example_mods_demo::{Lua, client_builder, client_files, lua_library};

    let mut options = ClientOptions::default();
    let mut join = None;
    let mut host = false;
    let mut cache = CacheChoice::PerUser;
    let mut library = lua_library();
    let mut work = PathBuf::from("target/mods-demo-client");
    let mut shots = None;
    let mut web_files = None;
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => host = true,
            "--name" => options.name = args.value(&arg, "a name")?,
            "--port" => options.port = args.number(&arg)?,
            "--bind" => options.host_address = args.ipv4(&arg)?,
            "--cache" => cache = CacheChoice::Folder(args.dir(&arg)?),
            "--lua" => library = Some(args.dir(&arg)?),
            "--work" => work = args.dir(&arg)?,
            "--shots" => shots = Some(args.dir(&arg)?),
            "--web-files" => web_files = Some(args.dir(&arg)?),
            _ if !arg.starts_with("--") && join.is_none() => {
                join = Some(parse_join_address(&arg)?.to_string());
            }
            _ => return Err(unknown(&arg).into()),
        }
    }
    let files = client_files()?;
    if let Some(dir) = web_files {
        let list = torque::app::web_page::write_web_files(&dir, &files.pack())?;
        println!("wrote {}", list.display());
        return Ok(());
    }
    options.lua = match library {
        Some(library) => Some(Lua::install(&library, &work.join("lua"))?),
        None => None,
    };
    if let Some(dir) = shots {
        let options = ClientOptions {
            net: torque_example_mods_demo::game::NetMode::Loopback,
            ..options
        };
        for path in torque_example_mods_demo::shots::write_shots(&dir, options, &work)? {
            println!("picture: {}", path.display());
        }
        return Ok(());
    }
    client_builder(&files, cache)?
        .app(move |engine| {
            let mut game = SoccerGame::from_engine(engine, &files, options.clone(), None)?;
            if host {
                game.client.set_global("$Game::HostScope", "lan");
                game.start_match()?;
            } else if let Some(address) = &join {
                game.join(address)?;
            }
            Ok(game)
        })
        .run()?;
    Ok(())
}
