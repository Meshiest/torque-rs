//! The arena's dedicated server: the game's `server.cs` and a map on a UDP socket, answering the LAN list, with the console's
//! lines printed. No window.
//!
//! `fps-server [--bind <address>] [--port <port>] [--name <name>] [--map <path>] [--ticks <n>]`: the address defaults to
//! `127.0.0.1` (this computer only; pass the computer's LAN address so others on the network can join, or `0.0.0.0` to listen
//! on every interface so players outside the network can join too), the port to 28000, the map to the first in
//! `game/missions/`; `--ticks` stops after that many ticks (a short check).
//!
//! Browsers: `--transport udp|web|both` (default `udp`) adds HTTP and WebSocket on TCP at the same address and port (`both`:
//! UDP beside it, desktop and browser players in one game; `web`: no UDP socket). The page served is the browser build's
//! folder (`node web/build.mjs fps` writes `<target>/web/fps`, found next to this program's folder), or `--web-root <dir>`;
//! `--no-files` serves none (the page comes from another web server). `--tls-cert <pem> --tls-key <pem>` serves `https` and
//! `wss`; without them the web listens on a private address only. A request must name an IP address, `localhost` or a
//! `--web-host <name>` (repeatable) as its host; a browser's game socket must come from the page this server serves, or
//! from an `--allow-origin <origin>` (repeatable, `https://games.example`; `*` takes any page) when the page is elsewhere.
//! These switches need `--transport web` or `both`. TLS handshakes run on the server's thread: an ECDSA (P-256) key makes each
//! far cheaper than an RSA one. The address rule looks at the bind address only: a private address that a router maps 1:1
//! to a public one serves plain `http` and `ws` to the internet, so use TLS there.
//!
//! Data channels: `--webrtc` (with `--transport web` or `both`) gives browsers a WebRTC data channel beside their game
//! socket, on a UDP port of its own: `--rtc-port <port>` (default the game's port number plus one; 0 any free one). Its
//! candidates are the bind address and each `--rtc-public-ip <address>` (repeatable: a host behind a 1:1 NAT names its
//! public address). With `--bind 0.0.0.0` (a public server) `--rtc-public-ip` or `--rtc-lan` is required, and the computer's
//! private LAN addresses are left out of the candidates (every browser would be told them) unless `--rtc-lan` adds them
//! (with no public address named, only players on the network get a data channel; the rest stay on their game socket).
//! Browsers whose channel does not open stay on their game socket. For checks only, each printing a warning at startup:
//! `--rtc-loss <percent>` drops that share of the channels' datagrams both ways, and `--rtc-stats` prints the channels'
//! traffic every 2 s (each peer's longest gap between game messages).
//!
//! Hosted through a room service: `--room <ws://address:port>` (a `torque-rooms` service) opens a room and prints its code;
//! players join with the code over WebRTC data channels straight to this computer, with no port forwarded and no UDP game
//! socket. The channels' socket binds `--bind` and `--port`: the default `127.0.0.1` reaches only this computer, so pass the
//! computer's LAN address for players elsewhere (the room's STUN server, if it gives one, finds the address the internet
//! sees). Not with `--transport web` or `both`. `--rtc-lan` takes joiners' private (LAN) addresses too, for players on
//! this computer's network (off by default: a joiner could aim the host's checks at machines on the network).

#[cfg(not(target_arch = "wasm32"))]
use std::net::Ipv4Addr;

#[cfg(not(target_arch = "wasm32"))]
use torque::app::cli::{Args, unknown};
#[cfg(not(target_arch = "wasm32"))]
use torque::app::serve::{Bind, WebFlags};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::files::GameFiles;
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::server::{ArenaServer, DEFAULT_PORT, ServerOptions};

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = GameFiles::bundled()?;
    let mut options = ServerOptions {
        address: Ipv4Addr::LOCALHOST,
        port: DEFAULT_PORT,
        name: "Arena".to_string(),
        mission: String::new(),
        public: false,
    };
    let mut ticks = None;
    let mut web = WebFlags::default();
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => options.address = args.ipv4(&arg)?,
            "--port" => options.port = args.number(&arg)?,
            "--name" => options.name = args.value(&arg, "a name")?,
            "--map" => options.mission = args.value(&arg, "a map")?,
            "--ticks" => ticks = Some(args.number(&arg)?),
            _ if web.take(&arg, &mut args)? => {}
            _ => return Err(unknown(&arg).into()),
        }
    }
    options.public = options.address.is_unspecified();
    if options.mission.is_empty() {
        options.mission = files
            .maps()
            .into_iter()
            .next()
            .ok_or("no map in the game folder's missions")?;
    }
    web.check()?;
    for warning in web.warnings() {
        eprintln!("fps-server: warning: {warning}");
    }
    let bind = Bind {
        address: options.address,
        port: options.port,
        public: options.public,
    };
    let served = web.listen(bind, "fps")?;
    let mut server = ArenaServer::start_dedicated_listen(&files, &options, served.listen.clone())?;
    for line in served.start_lines(&mut server, &options.name, &options.mission, "/") {
        println!("fps-server: {line}");
    }
    server.run_dedicated(ticks, |line| println!("{line}"));
    Ok(())
}

/// No dedicated server runs in a browser.
#[cfg(target_arch = "wasm32")]
fn main() {}
