//! The soccer server people join, with no window: the game's server, which requires the soccer mod (and sends it to a player
//! who lacks it) and runs its Lua game mode, on a UDP socket that also answers the network's list of games. Bots play in the
//! places nobody has taken and leave as people join. The console's lines are printed.
//!
//! `mods-demo-server [--bind <address>] [--port <port>] [--name <name>] [--bots <n>] [--lua <library>] [--work <dir>]
//! [--ticks <n>]`: the address defaults to `127.0.0.1` (this computer only; a LAN address lets others on the network join,
//! `0.0.0.0` every interface, for players outside it too), the port to 28000, the bots to 2; `--ticks` stops after that many
//! ticks. The Lua extension is the one `just lua` builds (or `--lua`, `TORQUE_LUA_LIBRARY`), copied into `<work>/lua/plugins`.
//!
//! Browsers, as the arena's server (`fps-server`): `--transport udp|web|both` adds HTTP and WebSocket on the same address and
//! port, serving the browser build (`node web/build.mjs mods-demo` writes `<target>/web/mods-demo`, or `--web-root <dir>`);
//! `--webrtc` gives browsers a data channel beside their game socket; `--room <ws://address:port>` hosts through a room
//! service (`torque-rooms`) and prints the room's code. The rest of the web's switches are `torque::app::serve`'s.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;

    use torque::app::cli::{Args, unknown};
    use torque::app::serve::{Bind, WebFlags};
    use torque_example_mods_demo::server::{ServerOptions, server_engine, start};
    use torque_example_mods_demo::{Lua, lua_library};

    let mut options = ServerOptions::default();
    let mut ticks = None;
    let mut library = lua_library();
    let mut work = PathBuf::from("target/mods-demo-server");
    let mut web = WebFlags::default();
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => options.address = args.ipv4(&arg)?,
            "--port" => options.port = args.number(&arg)?,
            "--name" => options.name = args.value(&arg, "a name")?,
            "--bots" => options.bots = args.number(&arg)?,
            "--lua" => library = Some(args.dir(&arg)?),
            "--work" => work = args.dir(&arg)?,
            "--ticks" => ticks = Some(args.number(&arg)?),
            _ if web.take(&arg, &mut args)? => {}
            _ => return Err(unknown(&arg).into()),
        }
    }
    options.public = options.address.is_unspecified();
    let library =
        library.ok_or("no Lua extension: build it with `just lua`, or pass --lua <library>")?;
    let lua = Lua::install(&library, &work.join("lua"))?;
    web.check()?;
    for warning in web.warnings() {
        eprintln!("mods-demo-server: warning: {warning}");
    }
    let bind = Bind {
        address: options.address,
        port: options.port,
        public: options.public,
    };
    let served = web.listen(bind, "mods-demo")?;
    let engine = server_engine(&lua)?;
    let mut server = start(&engine, &options, served.listen.clone())?;
    for line in served.start_lines(&mut server, &options.name, "the pitch", "/") {
        println!("mods-demo-server: {line}");
    }
    println!(
        "mods-demo-server: at most {} bots play while places are free",
        options.bots
    );
    server.run_dedicated(ticks, |line| println!("{line}"));
    Ok(())
}

/// No dedicated server runs in a browser.
#[cfg(target_arch = "wasm32")]
fn main() {}
