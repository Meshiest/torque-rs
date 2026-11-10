//! The soccer server people join: the game's server engine (it requires the soccer mod and runs its Lua) on a real socket,
//! as the arena's server is started (`examples/fps`'s `server`).
//!
//! **Where it listens** ([`Listen`]): a UDP socket on the address it is given (`127.0.0.1` keeps the game on this computer, a
//! LAN address opens it to the network, `0.0.0.0` with [`ServerOptions::public`] to players outside it too), and with the
//! web's switches ([`torque::app::serve::WebFlags`]) HTTP and WebSocket beside it, a browser's data channel, or a room of a
//! room service. The network's list of games is answered on the UDP socket.
//!
//! **Bots** play in the places nobody has taken, at most [`ServerOptions::bots`] of them (the game mode's own, in Lua), and
//! leave as people come in.

use std::net::Ipv4Addr;

use torque::app::files::GameFiles;
use torque::app::server::{GameServer, Listen, ServerStart};
use torque::engine::Engine;

use crate::{BOTS_PREF, Lua, Role, START, engine, server_files};

pub use torque::app::server::DEFAULT_PORT;

/// How a server is started.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// The address the UDP socket binds.
    pub address: Ipv4Addr,
    /// The UDP port number; 0 takes any free one.
    pub port: u16,
    /// The name the network's list of games shows.
    pub name: String,
    /// Whether `0.0.0.0` (every interface) is allowed as the address.
    pub public: bool,
    /// The most bots that play in the places nobody has taken.
    pub bots: u32,
    /// Whether it is a dedicated server (the list of games says so).
    pub dedicated: bool,
}

impl Default for ServerOptions {
    fn default() -> ServerOptions {
        ServerOptions {
            address: Ipv4Addr::LOCALHOST,
            port: DEFAULT_PORT,
            name: "Soccer".to_string(),
            public: false,
            bots: 2,
            dedicated: true,
        }
    }
}

impl ServerOptions {
    /// The UDP socket these options bind.
    #[must_use]
    pub fn udp(&self) -> Listen {
        Listen::Udp {
            address: self.address,
            port: self.port,
            public: self.public,
        }
    }
}

/// The server's engine: the game's files with every mod, the soccer mod required, the Lua extension loaded.
pub fn server_engine(lua: &Lua) -> Result<Engine, String> {
    let files = server_files().map_err(|e| e.to_string())?;
    engine(&files, Role::Server, Some(lua))
}

/// A server made by `engine` ([`server_engine`]) on `listen`, named and with bots as `options` say, for a host that ticks it
/// (or runs it with [`GameServer::run_dedicated`]).
pub fn start(
    engine: &Engine,
    options: &ServerOptions,
    listen: Listen,
) -> Result<GameServer, String> {
    let files = engine
        .resource::<GameFiles>()
        .ok_or("no ExamplePlugin")?
        .mem_files();
    let bots = options.bots.to_string();
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen,
            globals: &[
                (BOTS_PREF, &bots),
                ("$Pref::Server::Name", &options.name),
                (
                    "$Server::Dedicated",
                    if options.dedicated { "1" } else { "0" },
                ),
            ],
            start: START,
            args: &[],
        },
    )
}
