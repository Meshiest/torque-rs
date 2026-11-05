//! The example's server: the engine's game server ([`GameServer`]) on a UDP socket, running the game's `server.cs` and the
//! map (`startWorld`), its Players the demo's ([`crate::seats`]).
//!
//! **The socket.** The server binds one UDP socket on the address it is given: `127.0.0.1` keeps the game on this computer (the
//! tests, a game for one), a private LAN address opens it to the local network. The LAN list's pings and queries are answered
//! on the same socket.
//!
//! **Files.** The server reads its files from memory: the asset kit under `kit/` (the shapes the datablocks load, the terrain
//! the `TerrainBlock` reads) and the game folder's scripts and map ([`GameFiles`]).
//!
//! **A tick** ([`GameServer::tick`]) is one 32 ms pass of the main loop: the datagrams waiting, the scripts' events, the
//! objects' ticks (each figure, or the buggy its driver drives, moves by the moves its client sent), and a packet to every
//! client.

use std::borrow::{Borrow, BorrowMut};
use std::net::Ipv4Addr;
use std::ops::{Deref, DerefMut};

use torque::app::server::{GameServer, Listen, ServerStart};
use torque::collide::math::F3;
use torque::vehicle::VehicleState;

use crate::files::{GameFiles, MISSION};

pub use torque::app::server::DEFAULT_PORT;

/// How a server is started.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// The address the socket binds (`127.0.0.1`, or this computer's LAN address).
    pub address: Ipv4Addr,
    /// The UDP port number; 0 takes any free one.
    pub port: u16,
    /// The name the LAN list shows.
    pub name: String,
}

/// The example's server: the engine's [`GameServer`] (which it derefs to), its figures and the buggy.
pub struct WorldServer(pub GameServer);

impl WorldServer {
    /// A server bound as `options` say that has run `server.cs` and started the map.
    pub fn start(files: &GameFiles, options: &ServerOptions) -> Result<WorldServer, String> {
        GameServer::start_with(
            files.mem_files(),
            &ServerStart {
                listen: Listen::Udp {
                    address: options.address,
                    port: options.port,
                    public: false,
                },
                globals: &[("$Pref::Server::Name", &options.name)],
                start: "startWorld",
                args: &[MISSION],
            },
            crate::seats::install,
        )
        .map(WorldServer)
    }

    fn object(&self, global: &str) -> Option<u32> {
        let id = self.vm().global(global)?;
        let id = u32::try_from(torque::script::text::atoi(&id)).ok()?;
        self.vm().objects().contains(id).then_some(id)
    }

    /// Each client's figure, in the order the clients joined.
    pub fn figures(&self) -> Vec<u32> {
        let objects = self.vm().objects();
        let Some(members) = objects
            .find(b"ClientGroup")
            .and_then(|g| objects.get(g))
            .and_then(|g| g.members())
        else {
            return Vec::new();
        };
        members
            .as_slice()
            .iter()
            .filter_map(|&c| objects.find(&objects.field(c, b"player", b"")?))
            .collect()
    }

    /// The first client's figure.
    pub fn figure(&self) -> Option<u32> {
        self.figures().first().copied()
    }

    /// Where figure `id` stands (its feet).
    pub fn figure_position_of(&self, id: u32) -> Option<[f32; 3]> {
        torque::game::sim::player::position(self.vm(), id)
    }

    /// Where the first figure stands.
    pub fn figure_position(&self) -> Option<[f32; 3]> {
        self.figure_position_of(self.figure()?)
    }

    /// What figure `id` is mounted on and its seat (the mount node).
    pub fn seat_of(&self, id: u32) -> Option<(u32, u32)> {
        let (on, node) = torque::game::sim::mount::link(self.vm(), id);
        on.map(|on| (on, node))
    }

    /// What the first figure is mounted on.
    pub fn figure_mount(&self) -> Option<u32> {
        self.seat_of(self.figure()?).map(|(on, _)| on)
    }

    /// The buggy (`$World::Buggy`).
    pub fn buggy(&self) -> Option<u32> {
        self.object("$World::Buggy")
    }

    /// The buggy's state as the server's step holds it.
    pub fn buggy_state(&self) -> Option<&VehicleState> {
        torque::game::sim::vehicle::state(self.vm(), self.buggy()?)
    }

    /// Where the buggy is.
    pub fn buggy_position(&self) -> Option<F3> {
        self.buggy_state().map(VehicleState::position)
    }

    /// The buggy's velocity.
    pub fn buggy_velocity(&self) -> Option<F3> {
        self.buggy_state().map(|s| s.rigid.lin_vel)
    }

    /// The buggy's wheels' spring extensions (0 pressed up to 1 hanging).
    pub fn buggy_extensions(&self) -> Vec<f32> {
        use torque::game::sim::vehicle::{Body, ext_of};
        match self
            .buggy()
            .and_then(|j| ext_of(self.vm(), j)?.body.as_ref())
        {
            Some(Body::Wheeled(v)) => v.k.wheels[..4].iter().map(|w| w.extension).collect(),
            _ => Vec::new(),
        }
    }

    /// The buggy's axis `k` in the world (0 right, 1 forward, 2 up).
    pub fn buggy_axis(&self, k: usize) -> Option<F3> {
        let m = self.buggy_state()?.obj_to_world;
        Some([m[k], m[4 + k], m[8 + k]])
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
