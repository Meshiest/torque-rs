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

#[cfg(not(target_arch = "wasm32"))]
use std::net::Ipv4Addr;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use torque::app::cli::{Args, unknown};
#[cfg(not(target_arch = "wasm32"))]
use torque::server::net::web::{Access, Origins, Tls, TlsFiles, WebFiles, WebSetup};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::files::GameFiles;
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::server::{
    ArenaServer, DEFAULT_PORT, ServerOptions, Transports, lan_addresses,
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
        let setup = WebSetup {
            files: files_served,
            tls,
            access,
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
