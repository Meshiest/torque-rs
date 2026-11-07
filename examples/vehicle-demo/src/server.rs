//! The example's server: the engine's game server ([`GameServer`]) on a UDP socket, running the game's `server.cs` and the
//! map (`startWorld`), its Players the demo's ([`crate::seats`]).
//!
//! **The socket.** The server binds one UDP socket on the address it is given: `127.0.0.1` keeps the game on this computer (the
//! tests), a private LAN address opens it to the local network. The LAN list's pings and queries are answered on the same
//! socket. A game played alone ([`WorldServer::start_alone`]) binds none: the server listens on an in-memory transport at
//! [`ALONE_ADDR`] that only the client in this process reaches, so nobody on the network sees or joins it.
//!
//! **Browsers.** A dedicated server (`vehicle-demo-server`) may also listen on the web, as its command line says
//! (`torque::app::serve`): HTTP and WebSocket on TCP at the same address and port, serving the browser build's page and
//! taking browsers' game sockets (with WebRTC data channels beside them), with the UDP socket beside it or not; or it is
//! hosted through a room service, players joining by the room's code ([`WorldServer::start_dedicated_on`]). A browser that
//! hosts runs the server in its page's worker on a relay transport ([`WorldServer::start_bound`]).
//!
//! **Files.** The server reads its files from memory: the asset kit under `kit/` (the shapes the datablocks load, the terrain
//! the `TerrainBlock` reads) and the game folder's scripts and map ([`GameFiles`]).
//!
//! **A tick** ([`GameServer::tick`]) is one 32 ms pass of the main loop: the datagrams waiting, the scripts' events, the
//! objects' ticks (each figure, or the buggy its driver drives, moves by the moves its client sent), and a packet to every
//! client.

use std::borrow::{Borrow, BorrowMut};
use std::net::{Ipv4Addr, SocketAddr};
use std::ops::{Deref, DerefMut};

use torque::app::server::{GameServer, Listen, ServerStart};
use torque::collide::math::F3;
use torque::engine::Engine;
use torque::vehicle::VehicleState;

use crate::files::{GameFiles, MISSION};

pub use torque::app::server::DEFAULT_PORT;

/// The server's address on the in-memory link of a game played alone.
pub const ALONE_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

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
    pub fn start(
        engine: &Engine,
        files: &GameFiles,
        options: &ServerOptions,
    ) -> Result<WorldServer, String> {
        let listen = Listen::Udp {
            address: options.address,
            port: options.port,
            public: false,
        };
        WorldServer::start_on(engine, files, listen, &options.name, false)
    }

    /// A server for a game played alone, on the in-memory transport at [`ALONE_ADDR`] (module docs), that has run `server.cs`
    /// and started the map.
    pub fn start_alone(
        engine: &Engine,
        files: &GameFiles,
        name: &str,
    ) -> Result<WorldServer, String> {
        WorldServer::start_on(engine, files, Listen::Memory(ALONE_ADDR), name, false)
    }

    /// A dedicated server (`$Server::Dedicated`: the LAN list shows it as one) named `name` on `listen` (UDP, the web
    /// beside it or instead, or a room's host transport: module docs).
    pub fn start_dedicated_on(
        engine: &Engine,
        files: &GameFiles,
        listen: Listen,
        name: &str,
    ) -> Result<WorldServer, String> {
        WorldServer::start_on(engine, files, listen, name, true)
    }

    /// A server named `name` on a transport the caller made and bound (a browser host's worker relays its links).
    pub fn start_bound(
        engine: &Engine,
        files: &GameFiles,
        transport: torque::server::net::transport::SharedTransport,
        name: &str,
    ) -> Result<WorldServer, String> {
        use torque::app::server::BoundTransport;
        let listen = Listen::Bound(BoundTransport(transport));
        WorldServer::start_on(engine, files, listen, name, true)
    }

    fn start_on(
        engine: &Engine,
        files: &GameFiles,
        listen: Listen,
        name: &str,
        dedicated: bool,
    ) -> Result<WorldServer, String> {
        let dedicated = if dedicated { "1" } else { "0" };
        GameServer::start_engine(
            engine,
            files.mem_files(),
            &ServerStart {
                listen,
                globals: &[
                    ("$Pref::Server::Name", name),
                    ("$Server::Dedicated", dedicated),
                ],
                start: "startWorld",
                args: &[MISSION],
            },
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

    /// Sets the map's day (`WorldDay`) to `time` (0 sunrise, 0.25 noon, 0.5 sunset), running on or held there.
    pub fn set_day(&mut self, time: f32, play: bool) -> Result<(), String> {
        let day = self
            .vm()
            .objects()
            .find(b"WorldDay")
            .ok_or("the map has no WorldDay")?;
        let vm = self.0.vm_mut();
        vm.call_method(day, b"setTimeOfDay", &[time.to_string().into_bytes()])
            .map_err(|e| e.to_string())?;
        vm.call_method(
            day,
            b"setPlay",
            &[if play { b"1".to_vec() } else { b"0".to_vec() }],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The action figure `id` plays (its action thread's).
    pub fn figure_action_of(&self, id: u32) -> Option<String> {
        let st = torque::game::scene::ext::get(self.vm().objects(), id)?
            .shape
            .as_ref()?;
        let p = st.player.as_ref()?;
        let a = p.actions.get(usize::try_from(p.action).ok()?)?;
        Some(String::from_utf8_lossy(&a.name).into_owned())
    }

    /// The arm action figure `id` plays (its arm thread's: `look` with empty hands, `point`).
    pub fn figure_arm_of(&self, id: u32) -> Option<String> {
        let st = torque::game::scene::ext::get(self.vm().objects(), id)?
            .shape
            .as_ref()?;
        let p = st.player.as_ref()?;
        let a = p.actions.get(usize::try_from(p.arm_action).ok()?)?;
        Some(String::from_utf8_lossy(&a.name).into_owned())
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

    /// Each client's own buggy (`%client.buggy`), in the order the clients joined (none: it was given none).
    pub fn client_buggies(&self) -> Vec<Option<u32>> {
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
            .map(|&c| objects.find(&objects.field(c, b"buggy", b"")?))
            .collect()
    }

    /// How many buggies the world has (`$World::Buggies`).
    pub fn buggy_count(&self) -> i64 {
        self.vm()
            .global("$World::Buggies")
            .map_or(0, |v| i64::from(torque::script::text::atoi(&v)))
    }

    /// Where buggy `id` is, as the server's step holds it.
    pub fn buggy_position_of(&self, id: u32) -> Option<F3> {
        torque::game::sim::vehicle::state(self.vm(), id).map(VehicleState::position)
    }

    /// Buggy `id`'s velocity, as the server's step holds it.
    pub fn buggy_velocity_of(&self, id: u32) -> Option<F3> {
        torque::game::sim::vehicle::state(self.vm(), id).map(|s| s.rigid.lin_vel)
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
