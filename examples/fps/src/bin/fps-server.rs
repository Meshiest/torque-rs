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
//! public address). With `--bind 0.0.0.0` (a public server) at least one `--rtc-public-ip` is required, and the computer's
//! private LAN addresses are left out of the candidates (every browser would be told them) unless `--rtc-lan` adds them.
//! Browsers whose channel does not open stay on their game socket. For checks only, each printing a warning at startup:
//! `--rtc-loss <percent>` drops that share of the channels' datagrams both ways, and `--rtc-stats` prints the channels'
//! traffic every 2 s (each peer's longest gap between game messages).

#[cfg(not(target_arch = "wasm32"))]
use std::net::Ipv4Addr;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use torque::app::cli::{Args, unknown};
#[cfg(not(target_arch = "wasm32"))]
use torque::server::net::web::rtc::RtcSetup;
#[cfg(not(target_arch = "wasm32"))]
use torque::server::net::web::{Access, Origins, Tls, TlsFiles, WebFiles, WebSetup};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::files::GameFiles;
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::server::{
    ArenaServer, DEFAULT_PORT, ServerOptions, Transports, lan_addresses, rtc_candidates,
};

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
    let mut transports = Transports::Udp;
    let mut web_root = None;
    let mut no_files = false;
    let mut cert = None;
    let mut key = None;
    let mut access = Access::default();
    let mut origins = Vec::new();
    let mut webrtc = false;
    let mut rtc_port = None;
    let mut rtc_public = Vec::new();
    let mut rtc_lan = false;
    let mut rtc_loss = 0u8;
    let mut rtc_stats = false;
    let mut args = Args::from_env();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => options.address = args.ipv4(&arg)?,
            "--port" => options.port = args.number(&arg)?,
            "--name" => options.name = args.value(&arg, "a name")?,
            "--map" => options.mission = args.value(&arg, "a map")?,
            "--ticks" => ticks = Some(args.number(&arg)?),
            "--transport" => {
                transports = Transports::parse(&args.value(&arg, "udp, web or both")?)?
            }
            "--web-root" => web_root = Some(args.dir(&arg)?),
            "--no-files" => no_files = true,
            "--tls-cert" => cert = Some(args.dir(&arg)?),
            "--tls-key" => key = Some(args.dir(&arg)?),
            "--web-host" => access.hosts.push(args.value(&arg, "a host name")?),
            "--allow-origin" => origins.push(args.value(&arg, "an origin")?),
            "--webrtc" => webrtc = true,
            "--rtc-port" => rtc_port = Some(args.number(&arg)?),
            "--rtc-public-ip" => rtc_public.push(args.ipv4(&arg)?),
            "--rtc-lan" => rtc_lan = true,
            "--rtc-loss" => rtc_loss = args.number::<u8>(&arg)?.min(100),
            "--rtc-stats" => rtc_stats = true,
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
    let mut web_scheme = "http";
    let web_flags = web_root.is_some()
        || no_files
        || cert.is_some()
        || key.is_some()
        || !access.hosts.is_empty()
        || !origins.is_empty();
    if transports == Transports::Udp && (web_flags || webrtc) {
        return Err("--web-root, --no-files, --tls-cert, --tls-key, --web-host, --allow-origin and --webrtc are for the web: add --transport web or --transport both"
            .into());
    }
    if !webrtc
        && (rtc_port.is_some() || !rtc_public.is_empty() || rtc_lan || rtc_loss > 0 || rtc_stats)
    {
        return Err("--rtc-port, --rtc-public-ip, --rtc-lan, --rtc-loss and --rtc-stats are for the data channels: add --webrtc".into());
    }
    if rtc_loss > 0 {
        eprintln!(
            "fps-server: warning: --rtc-loss {rtc_loss} drops {rtc_loss}% of the data channels' datagrams both ways; it is for checks, not for a server players use"
        );
    }
    if rtc_stats {
        eprintln!(
            "fps-server: warning: --rtc-stats prints each data channel peer's traffic every 2 s; it is for checks, not for a server players use"
        );
    }
    let mut server = if transports == Transports::Udp {
        ArenaServer::start_dedicated(&files, &options)?
    } else {
        let tls = match (cert, key) {
            (Some(cert), Some(key)) => {
                web_scheme = "https";
                Some(Tls::load(&TlsFiles { cert, key })?)
            }
            (None, None) => None,
            _ => return Err("--tls-cert and --tls-key go together".into()),
        };
        let files_served = if no_files {
            None
        } else {
            let root = match web_root {
                Some(dir) => dir,
                None => default_web_root()
                    .ok_or("no folder for the page: pass --web-root <dir> or --no-files")?,
            };
            Some(WebFiles::from_dir(&root).map_err(|e| {
                format!(
                    "the page's files in {}: {e} (build them with `node web/build.mjs fps`, or pass --web-root or --no-files)",
                    root.display()
                )
            })?)
        };
        access.origins = if origins.iter().any(|o| o == "*") {
            Origins::Any
        } else if origins.is_empty() {
            Origins::SameHost
        } else {
            Origins::List(origins)
        };
        let rtc = if webrtc {
            let port = match rtc_port {
                Some(port) => port,
                None if options.port == 0 => 0,
                None => options
                    .port
                    .checked_add(1)
                    .ok_or("--rtc-port is needed when --port is 65535")?,
            };
            rtc_public = rtc_candidates(options.public, &rtc_public, rtc_lan, &lan_addresses())?;
            Some(RtcSetup {
                port,
                public: rtc_public,
                loss_percent: rtc_loss,
                ..RtcSetup::default()
            })
        } else {
            None
        };
        let setup = WebSetup {
            files: files_served,
            tls,
            access,
            rtc,
            ..WebSetup::default()
        };
        ArenaServer::start_dedicated_on(&files, &options, transports, setup)?
    };
    if let Some(addr) = server.address() {
        println!(
            "fps-server: {} on {addr}, map {}",
            options.name, options.mission
        );
        if transports != Transports::Udp {
            println!("fps-server: browsers join {web_scheme}://{addr}/");
        }
        if webrtc {
            for line in server.transport_status() {
                println!("fps-server: {line}");
            }
        }
        if rtc_stats {
            server.set_status_every(Some(std::time::Duration::from_secs(2)));
        }
        if options.public {
            let port = addr.port();
            let lan = lan_addresses();
            if lan.is_empty() {
                println!("fps-server: no LAN address found on this computer");
            }
            for ip in lan {
                println!("fps-server: players on this network join {ip}:{port}");
            }
            println!(
                "fps-server: players outside the network need UDP {port} forwarded to this computer, and the firewall to allow it"
            );
        }
    }
    server.run_dedicated(ticks, |line| println!("{line}"));
    Ok(())
}

/// No dedicated server runs in a browser.
#[cfg(target_arch = "wasm32")]
fn main() {}

/// The browser build's folder next to this program's: `<target>/web/fps` for `<target>/release/fps-server`.
#[cfg(not(target_arch = "wasm32"))]
fn default_web_root() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?;
    let dir = program.parent()?.parent()?.join("web").join("fps");
    dir.is_dir().then_some(dir)
}
