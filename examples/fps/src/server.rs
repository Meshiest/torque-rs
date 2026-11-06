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

#[cfg(not(target_arch = "wasm32"))]
pub use torque::app::serve::Transports;
pub use torque::app::server::{DEFAULT_PORT, lan_addresses};

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
        let bind = torque::app::serve::Bind {
            address: options.address,
            port: options.port,
            public: options.public,
        };
        let listen = torque::app::serve::web_listen(transports, bind, setup);
        ArenaServer::start_dedicated_listen(files, options, listen)
    }

    /// A dedicated server on `listen` (a dedicated server's command line says where: `torque::app::serve`).
    pub fn start_dedicated_listen(
        files: &GameFiles,
        options: &ServerOptions,
        listen: Listen,
    ) -> Result<GameServer, String> {
        ArenaServer::start_on(files, options, listen, true)
    }

    /// A dedicated server on a transport the caller made and bound (a browser host's worker relays its links).
    pub fn start_bound(
        files: &GameFiles,
        options: &ServerOptions,
        transport: torque::server::net::transport::SharedTransport,
    ) -> Result<GameServer, String> {
        use torque::app::server::BoundTransport;
        ArenaServer::start_on(
            files,
            options,
            Listen::Bound(BoundTransport(transport)),
            true,
        )
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
