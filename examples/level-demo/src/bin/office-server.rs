//! The office server people join, with no window: the level demo's office with its props, which requires the catalog mod of
//! its players (and sends it to a player who lacks it), on a UDP socket that also answers the network's list of matches. The
//! console's lines are printed.
//!
//! `office-server [--bind <address>] [--port <port>] [--name <name>] [--ticks <n>]`: the address defaults to `127.0.0.1`
//! (this computer only; a LAN address lets others on the network join, `0.0.0.0` every interface, for players outside it
//! too), the port to 28000; `--ticks` stops after that many ticks.
//!
//! Browsers, as the arena's server (`fps-server`): `--transport udp|web|both` adds HTTP and WebSocket on the same address and
//! port, serving the level demo's browser build (`node web/build.mjs level-demo --client-scripts` writes
//! `<target>/web/level-demo`, or `--web-root <dir>`), whose page joins this server at `/?join`; `--webrtc` gives browsers a data
//! channel beside their game socket; `--room <ws://address:port>` hosts through a room service (`torque-rooms`) and prints the
//! room's code. The rest of the web's switches are `torque::app::serve`'s.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::net::Ipv4Addr;

    use torque::app::cli::{Args, unknown};
    use torque::app::serve::{Bind, WebFlags};
    use torque::mods::CacheChoice;
    use torque_example_level_demo::catalog::mod_files;
    use torque_example_level_demo::files::GameFiles;
    use torque_example_level_demo::office::{
        DEFAULT_PORT, OfficeStart, office_engine, start_office,
    };

    let mut address = Ipv4Addr::LOCALHOST;
    let mut port = DEFAULT_PORT;
    let mut name = "The office".to_string();
    let mut ticks = None;
    let mut web = WebFlags::default();
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => address = args.ipv4(&arg)?,
            "--port" => port = args.number(&arg)?,
            "--name" => name = args.value(&arg, "a name")?,
            "--ticks" => ticks = Some(args.number(&arg)?),
            _ if web.take(&arg, &mut args)? => {}
            _ => return Err(unknown(&arg).into()),
        }
    }
    let public = address.is_unspecified();
    web.check()?;
    for warning in web.warnings() {
        eprintln!("office-server: warning: {warning}");
    }
    let served = web.listen(
        Bind {
            address,
            port,
            public,
        },
        "level-demo",
    )?;
    let files = GameFiles::bundled()?;
    let engine = office_engine(files, mod_files()?, CacheChoice::Memory)?.build()?;
    let start = OfficeStart {
        name: &name,
        dedicated: true,
    };
    let mut server = start_office(&engine, served.listen.clone(), &start)?;
    for line in served.start_lines(&mut server, &name, "the office", "/?join") {
        println!("office-server: {line}");
    }
    println!("office-server: players get the prop catalog (Q in the office) when they join");
    server.run_dedicated(ticks, |line| println!("{line}"));
    Ok(())
}

/// No dedicated server runs in a browser.
#[cfg(target_arch = "wasm32")]
fn main() {}
