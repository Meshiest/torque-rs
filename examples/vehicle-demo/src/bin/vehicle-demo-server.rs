//! The vehicle demo's dedicated server: the game's `server.cs` and the map on a UDP socket, answering the LAN list, with the
//! console's lines printed. No window, no player of its own: desktop players join it from the menu's list or by address.
//!
//! `vehicle-demo-server [--bind <address>] [--port <port>] [--name <name>] [--ticks <n>]`: the address defaults to
//! `127.0.0.1` (this computer only; pass the computer's LAN address so others on the network can join, or `0.0.0.0` to
//! listen on every interface), the port to 28000; `--ticks` stops after that many ticks (a short check).
//!
//! Browsers, as the arena's server takes them (`torque::app::serve`): `--transport both` adds HTTP and WebSocket on TCP at
//! the same address and port, serving the browser build (`node web/build.mjs vehicle-demo` writes `<target>/web/vehicle-demo`,
//! or `--web-root <dir>`), and `--webrtc` gives browsers a WebRTC data channel beside their game socket. The page joins
//! this server at `http://<address>:<port>/?join` (the page alone plays alone). `--transport web` opens no UDP socket;
//! `--tls-cert`/`--tls-key`, `--web-host`, `--allow-origin`, `--no-files` and the `--rtc-*` switches are the arena's.
//! `--room <ws://address:port>` hosts through a room service (`torque-rooms`) instead: it prints the room's code, and
//! browsers join with `?room=<code>` over data channels straight to this computer.

#[cfg(not(target_arch = "wasm32"))]
use std::net::Ipv4Addr;

#[cfg(not(target_arch = "wasm32"))]
use torque::app::cli::{Args, unknown};
#[cfg(not(target_arch = "wasm32"))]
use torque::app::serve::{Bind, WebFlags};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_vehicle_demo::files::{GameFiles, MISSION};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_vehicle_demo::server::{DEFAULT_PORT, WorldServer};

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = GameFiles::bundled()?;
    let mut bind = Bind {
        address: Ipv4Addr::LOCALHOST,
        port: DEFAULT_PORT,
        public: false,
    };
    let mut name = "Hills".to_string();
    let mut ticks = None;
    let mut web = WebFlags::default();
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => bind.address = args.ipv4(&arg)?,
            "--port" => bind.port = args.number(&arg)?,
            "--name" => name = args.value(&arg, "a name")?,
            "--ticks" => ticks = Some(args.number(&arg)?),
            _ if web.take(&arg, &mut args)? => {}
            _ => return Err(unknown(&arg).into()),
        }
    }
    bind.public = bind.address.is_unspecified();
    web.check()?;
    for warning in web.warnings() {
        eprintln!("vehicle-demo-server: warning: {warning}");
    }
    let served = web.listen(bind, "vehicle-demo")?;
    let mut server = WorldServer::start_dedicated_on(&files, served.listen.clone(), &name)?;
    for line in served.start_lines(&mut server, &name, MISSION, "/?join") {
        println!("vehicle-demo-server: {line}");
    }
    server.run_dedicated(ticks, |line| println!("{line}"));
    Ok(())
}

/// No dedicated server runs in a browser.
#[cfg(target_arch = "wasm32")]
fn main() {}
