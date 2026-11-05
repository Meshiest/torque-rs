//! The room service's command line.
//!
//! `torque-rooms [--bind <address>] [--port <port>] [--tls-cert <pem> --tls-key <pem>] [--web-host <name>]...
//! [--allow-origin <origin>]... [--web-root <dir>] [--stun <url>]... [--turn <url>]... [--turn-secret-file <path>]
//! [--turn-ttl <seconds>] [--room-hours <hours>]`
//!
//! The address defaults to `127.0.0.1` and the port to 28100 (0 takes any free one). Without TLS the service listens on a
//! private address only; `0.0.0.0` (every interface) needs `--tls-cert` and `--tls-key` (an ECDSA P-256 key makes each
//! handshake far cheaper than an RSA one). Rooms are at `ws(s)://<address>/ws`. A request must name an IP address,
//! `localhost` or a `--web-host` as its host, and a socket opened by a page must come from a page this service serves or
//! from an `--allow-origin` (`*` takes any). `--web-root` serves a folder (the game's web build) on the same port.
//!
//! ICE: each `--stun` URL (`stun:host:port`) is given to every room's players, and each `--turn` URL (`turn:` or `turns:`)
//! with a credential made from the secret in `--turn-secret-file` (shared with the TURN server: coturn's
//! `use-auth-secret` and `static-auth-secret`) lasting `--turn-ttl` seconds (default 4 hours). Rooms last
//! `--room-hours` (default 6).

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::time::Duration;

use torque_rooms::ice::{DEFAULT_TTL, read_secret};
use torque_rooms::{Ice, RoomLimits, Rooms, Service, service_limits};
use torque_server::net::transport::{BindPolicy, LanRules};
use torque_server::net::web::{Access, Origins, Tls, TlsFiles, WebFiles, WebSetup};

/// The TCP port number the service listens on unless told.
const DEFAULT_PORT: u16 = 28_100;

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn number<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<T, String> {
    let text = value(args, flag)?;
    text.parse()
        .map_err(|_| format!("{flag} takes a number, not {text:?}"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bind = Ipv4Addr::LOCALHOST;
    let mut port = DEFAULT_PORT;
    let (mut cert, mut key) = (None::<PathBuf>, None::<PathBuf>);
    let mut access = Access::default();
    let mut origins = Vec::new();
    let mut web_root = None::<PathBuf>;
    let mut ice = Ice {
        ttl: DEFAULT_TTL,
        ..Ice::default()
    };
    let mut secret_file = None::<PathBuf>;
    let mut limits = RoomLimits::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                let text = value(&mut args, &arg)?;
                bind = text
                    .parse()
                    .map_err(|_| format!("--bind takes an IPv4 address, not {text:?}"))?;
            }
            "--port" => port = number(&mut args, &arg)?,
            "--tls-cert" => cert = Some(value(&mut args, &arg)?.into()),
            "--tls-key" => key = Some(value(&mut args, &arg)?.into()),
            "--web-host" => access.hosts.push(value(&mut args, &arg)?),
            "--allow-origin" => origins.push(value(&mut args, &arg)?),
            "--web-root" => web_root = Some(value(&mut args, &arg)?.into()),
            "--stun" => ice.stun.push(value(&mut args, &arg)?),
            "--turn" => ice.turn.push(value(&mut args, &arg)?),
            "--turn-secret-file" => secret_file = Some(value(&mut args, &arg)?.into()),
            "--turn-ttl" => ice.ttl = Duration::from_secs(number(&mut args, &arg)?),
            "--room-hours" => {
                let hours: u64 = number(&mut args, &arg)?;
                if !(1..=48).contains(&hours) {
                    return Err("--room-hours takes 1 to 48".into());
                }
                limits.room_lifetime = Duration::from_secs(hours * 3600);
            }
            "--help" | "-h" => {
                println!(
                    "usage: torque-rooms [--bind <address>] [--port <port>] [--tls-cert <pem> --tls-key <pem>] [--web-host <name>]... [--allow-origin <origin>]... [--web-root <dir>] [--stun <url>]... [--turn <url>]... [--turn-secret-file <path>] [--turn-ttl <seconds>] [--room-hours <hours>]"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown switch {arg:?} (--help lists them)").into()),
        }
    }
    if let Some(path) = &secret_file {
        let text = std::fs::read(path)
            .map_err(|e| format!("the TURN secret file {}: {e}", path.display()))?;
        ice.secret = Some(read_secret(&text)?);
    }
    if secret_file.is_some() && ice.turn.is_empty() {
        return Err("--turn-secret-file is for --turn URLs: add one".into());
    }
    ice.check()?;
    let tls = match (cert, key) {
        (Some(cert), Some(key)) => Some(Tls::load(&TlsFiles { cert, key })?),
        (None, None) => None,
        _ => return Err("--tls-cert and --tls-key go together".into()),
    };
    let scheme = if tls.is_some() { "wss" } else { "ws" };
    access.origins = if origins.iter().any(|o| o == "*") {
        Origins::Any
    } else if origins.is_empty() {
        Origins::SameHost
    } else {
        Origins::List(origins)
    };
    let files = match &web_root {
        Some(dir) => Some(
            WebFiles::from_dir(dir).map_err(|e| format!("the files in {}: {e}", dir.display()))?,
        ),
        None => None,
    };
    let ip = IpAddr::V4(bind);
    let policy = BindPolicy::new(ip, bind.is_unspecified(), &LanRules::PLAIN)
        .map_err(|e| format!("--bind {bind}: only a private address or 0.0.0.0: {e}"))?;
    let setup = WebSetup {
        files,
        tls,
        access,
        limits: service_limits(),
        ..WebSetup::default()
    };
    let mut service = Service::new(policy, setup, Rooms::new(limits, ice.clone()));
    service
        .bind(ip, port)
        .map_err(|e| format!("the service's socket: {e}"))?;
    let at = service.local_addr().ok_or("the service has no address")?;
    println!("torque-rooms: rooms at {scheme}://{at}/ws");
    if web_root.is_some() {
        let page = if scheme == "wss" { "https" } else { "http" };
        println!("torque-rooms: the page at {page}://{at}/");
    }
    println!(
        "torque-rooms: {} STUN and {} TURN URLs{}",
        ice.stun.len(),
        ice.turn.len(),
        if ice.turn.is_empty() {
            String::new()
        } else {
            format!(", credentials for {} s", ice.ttl.as_secs())
        }
    );
    service.run(Duration::from_millis(2), |_| false);
    Ok(())
}
