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

use crate::files::GameFiles;

/// The server's address on the link.
pub const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

/// The example's server: the engine's [`GameServer`] (which it derefs to) and the figure it gave the player.
pub struct WorldServer(pub GameServer);

impl WorldServer {
    /// A server that has run `server.cs` and started the map `mission` (a path under the game folder, `missions/plaza.mis`).
    pub fn start(files: &GameFiles, mission: &str) -> Result<WorldServer, String> {
        GameServer::start(
            files.mem_files(),
            &ServerStart {
                listen: Listen::Memory(SERVER_ADDR),
                globals: &[],
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
