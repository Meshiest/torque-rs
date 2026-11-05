//! The example's server: the engine's game server ([`GameServer`]) on a UDP socket, running the game's `server.cs` and one
//! map (`startArena`).
//!
//! **The socket.** The server binds one UDP socket on the address it is given: `127.0.0.1` keeps the game on this computer (the
//! tests, a game for one), a private LAN address opens it to the local network, and `0.0.0.0` (every interface, with
//! [`ServerOptions::public`]) opens it to players outside the network too (the router must forward its UDP port). The bind policy
//! refuses any other public address. The LAN list's pings and queries are answered on the same socket.
//!
//! **Browsers** ([`Transports`], [`ArenaServer::start_dedicated_on`]). A dedicated server may also listen on the web: HTTP and
//! WebSocket on TCP at the same address and port, serving the browser build's page (or not: the page then comes from another
//! web server) and taking browsers' game sockets, with the UDP socket beside it (`both`: desktop and browser players in one
//! game) or without it (`web`).
//!
//! **A tick** ([`GameServer::tick`]) is one 32 ms pass of the main loop: the datagrams waiting, the scripts' events, the
//! objects' ticks (each client's figure moves by the moves it sent, bolts fly) and a packet to every client.

use std::net::Ipv4Addr;

use torque::app::server::{GameServer, Listen, ServerStart};

use crate::files::GameFiles;

pub use torque::app::server::{DEFAULT_PORT, lan_addresses};

/// The extra addresses a dedicated server's data channels advertise to browsers beside the bind address: `named` (each
/// `--rtc-public-ip`), and on `0.0.0.0` the computer's `lan` addresses, which are private. A public server (`0.0.0.0`) is
/// seen by the internet, so it leaves the private ones out unless `with_lan` asks for them (a LAN address told to every
/// browser discloses the network behind the server), and refuses to run data channels with no public address named.
pub fn rtc_candidates(
    public: bool,
    named: &[Ipv4Addr],
    with_lan: bool,
    lan: &[Ipv4Addr],
) -> Result<Vec<Ipv4Addr>, String> {
    if !public {
        return Ok(named.to_vec());
    }
    if named.is_empty() {
        return Err("--webrtc on a public server (--bind 0.0.0.0) needs --rtc-public-ip <address>, the address browsers reach this computer by: without one the data channels have nothing to advertise but private addresses".to_string());
    }
    let mut found = named.to_vec();
    if with_lan {
        for &ip in lan {
            if !found.contains(&ip) {
                found.push(ip);
            }
        }
    }
    Ok(found)
}

/// How a server is started.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// The address the socket binds (`127.0.0.1`, or this computer's LAN address).
    pub address: Ipv4Addr,
    /// The UDP port number; 0 takes any free one.
    pub port: u16,
    /// The name the LAN list shows.
    pub name: String,
    /// The map: a path under the game folder (`missions/arena.mis`).
    pub mission: String,
    /// Whether `0.0.0.0` (every interface, so players outside the network can join when their traffic reaches this computer) is
    /// allowed as the address; no other public address is.
    pub public: bool,
}

/// Which transports a dedicated server listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transports {
    /// UDP only.
    #[default]
    Udp,
    /// HTTP and WebSocket only: no UDP socket is opened.
    Web,
    /// UDP, and HTTP and WebSocket on the same port number.
    Both,
}

impl Transports {
    /// `udp`, `web` or `both`.
    pub fn parse(text: &str) -> Result<Transports, String> {
        match text {
            "udp" => Ok(Transports::Udp),
            "web" => Ok(Transports::Web),
            "both" => Ok(Transports::Both),
            _ => Err(format!("--transport takes udp, web or both, not {text:?}")),
        }
    }
}

/// The kit's files the figure, the pistol and the cube are loaded from: `glb` when the environment variable `ARENA_SHAPES` is
/// `glb` (the kit's `.glb` exports, which load as the same shapes), else `dts`.
pub fn shape_files() -> &'static str {
    match std::env::var("ARENA_SHAPES").as_deref() {
        Ok("glb") => "glb",
        _ => "dts",
    }
}

/// The arena's server: a [`GameServer`] that has run `server.cs` and started the map.
pub struct ArenaServer;

impl ArenaServer {
    /// A dedicated server (`$Server::Dedicated`: the LAN list shows it as one) bound as `options` say.
    pub fn start_dedicated(
        files: &GameFiles,
        options: &ServerOptions,
    ) -> Result<GameServer, String> {
        ArenaServer::start_with(files, options, true)
    }

    /// A server bound as `options` say that has run `server.cs` and started the map.
    pub fn start(files: &GameFiles, options: &ServerOptions) -> Result<GameServer, String> {
        ArenaServer::start_with(files, options, false)
    }

    /// A dedicated server on `transports`: UDP as [`ArenaServer::start_dedicated`] binds it, and the web as `setup` says (its
    /// files, its TLS, its limits).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_dedicated_on(
        files: &GameFiles,
        options: &ServerOptions,
        transports: Transports,
        setup: torque::server::net::web::WebSetup,
    ) -> Result<GameServer, String> {
        use std::rc::Rc;
        use torque::app::server::WebListen;
        let listen = match transports {
            Transports::Udp => udp_listen(options),
            Transports::Web | Transports::Both => Listen::Web {
                address: options.address,
                port: options.port,
                public: options.public,
                web: Rc::new(WebListen {
                    udp: transports == Transports::Both,
                    setup,
                }),
            },
        };
        ArenaServer::start_on(files, options, listen, true)
    }

    fn start_with(
        files: &GameFiles,
        options: &ServerOptions,
        dedicated: bool,
    ) -> Result<GameServer, String> {
        ArenaServer::start_on(files, options, udp_listen(options), dedicated)
    }

    fn start_on(
        files: &GameFiles,
        options: &ServerOptions,
        listen: Listen,
        dedicated: bool,
    ) -> Result<GameServer, String> {
        if files.file(&options.mission).is_none() {
            return Err(format!("no map {} in the game folder", options.mission));
        }
        let dedicated = if dedicated { "1" } else { "0" };
        GameServer::start(
            files.mem_files(),
            &ServerStart {
                listen,
                globals: &[
                    ("$Pref::Server::Name", &options.name),
                    ("$Server::Dedicated", dedicated),
                    ("$Arena::Shapes", shape_files()),
                ],
                start: "startArena",
                args: &[&options.mission],
            },
        )
    }
}

fn udp_listen(options: &ServerOptions) -> Listen {
    Listen::Udp {
        address: options.address,
        port: options.port,
        public: options.public,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);
    const LAN: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 20);

    #[test]
    fn a_public_server_advertises_no_private_address_unless_asked() {
        // A private bind keeps what it is given (the library adds the bind address).
        assert_eq!(rtc_candidates(false, &[], false, &[LAN]), Ok(vec![]));
        assert_eq!(
            rtc_candidates(false, &[PUBLIC], false, &[LAN]),
            Ok(vec![PUBLIC])
        );
        // A public bind needs a public address, and does not add the LAN's on its own.
        assert!(rtc_candidates(true, &[], false, &[LAN]).is_err());
        assert!(rtc_candidates(true, &[], true, &[LAN]).is_err());
        assert_eq!(
            rtc_candidates(true, &[PUBLIC], false, &[LAN]),
            Ok(vec![PUBLIC])
        );
        // The flag adds them, once each.
        assert_eq!(
            rtc_candidates(true, &[PUBLIC], true, &[LAN, LAN, PUBLIC]),
            Ok(vec![PUBLIC, LAN])
        );
    }
}
