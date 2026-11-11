//! The example's server: the engine's game server ([`GameServer`]) over an in-memory transport, running the game's
//! `server.cs` and one map (`startWorld`).
//!
//! **Files.** The server reads its files from memory: the asset kit under `kit/` (the shapes the datablocks load) and the game
//! folder's scripts and maps ([`GameFiles`]).
//!
//! **A tick** ([`GameServer::tick`]) is one 32 ms pass of the main loop: the datagrams the link put in the transport, the
//! scripts' events, the objects' ticks (the client's figure moves by the moves it sent), and a packet to the client.

use std::borrow::{Borrow, BorrowMut};
use std::net::{Ipv4Addr, SocketAddr};
use std::ops::{Deref, DerefMut};

use torque::app::server::{GameServer, Listen, ServerStart};
use torque::engine::Engine;

use crate::files::GameFiles;

/// The server's address on the link.
pub const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

/// The example's server: the engine's [`GameServer`] (which it derefs to) and the figure it gave the player.
pub struct WorldServer(pub GameServer);

impl WorldServer {
    /// A server `engine` makes over `files`, that has run `server.cs` and started the map `mission` (a path under the game
    /// folder, `missions/plaza.mis`).
    pub fn start(engine: &Engine, files: &GameFiles, mission: &str) -> Result<WorldServer, String> {
        WorldServer::start_with(engine, files, &[], mission)
    }

    /// [`WorldServer::start`] with `extra` files (a path and its bytes each) added to the server's alone: what a server has that
    /// its clients do not, such as the mods it requires of them.
    pub fn start_with(
        engine: &Engine,
        files: &GameFiles,
        extra: &[(String, Vec<u8>)],
        mission: &str,
    ) -> Result<WorldServer, String> {
        WorldServer::start_on(
            engine,
            files,
            extra,
            mission,
            Listen::Memory(SERVER_ADDR),
            &[],
        )
    }

    /// [`WorldServer::start_with`] on `listen` (a UDP socket, the web, a room) with `globals` set before `server.cs` runs (the name
    /// the list of matches shows, `$Pref::Server::Name`): a server other players join.
    pub fn start_on(
        engine: &Engine,
        files: &GameFiles,
        extra: &[(String, Vec<u8>)],
        mission: &str,
        listen: Listen,
        globals: &[(&str, &str)],
    ) -> Result<WorldServer, String> {
        let mem = files.mem_files();
        for (path, bytes) in extra {
            mem.insert(path, bytes);
        }
        GameServer::start_engine(
            engine,
            mem,
            &ServerStart {
                listen,
                globals,
                start: "startWorld",
                args: &[mission],
            },
        )
        .map(WorldServer)
    }

    /// The figure the last client was given (`$World::Figure`), if it still exists.
    pub fn figure(&self) -> Option<u32> {
        let id = self.vm().global("$World::Figure")?;
        let id = u32::try_from(torque::script::text::atoi(&id)).ok()?;
        self.vm().objects().contains(id).then_some(id)
    }

    /// Where the figure stands (its feet), if there is one.
    pub fn figure_position(&self) -> Option<[f32; 3]> {
        let id = self.figure()?;
        torque::game::sim::player::position(self.vm(), id)
    }
}

impl Deref for WorldServer {
    type Target = GameServer;

    fn deref(&self) -> &GameServer {
        &self.0
    }
}

impl DerefMut for WorldServer {
    fn deref_mut(&mut self) -> &mut GameServer {
        &mut self.0
    }
}

impl Borrow<GameServer> for WorldServer {
    fn borrow(&self) -> &GameServer {
        &self.0
    }
}

impl BorrowMut<GameServer> for WorldServer {
    fn borrow_mut(&mut self) -> &mut GameServer {
        &mut self.0
    }
}

impl From<WorldServer> for GameServer {
    fn from(server: WorldServer) -> GameServer {
        server.0
    }
}
