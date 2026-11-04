//! The example's server: torque-server's main loop with TGE's game classes ([`torque::server::game`]) and a network host of
//! Torque's net game ([`TorqueNet`]) over an in-memory transport, running the game's `server.cs` and one map.
//!
//! **Files.** The server reads its files from memory: the asset kit under `kit/` (the shapes the datablocks load) and the game
//! folder's scripts and maps ([`GameFiles`]).
//!
//! **A tick** ([`WorldServer::tick`]) is one 32 ms pass of the main loop: the datagrams the link put in the transport, the
//! scripts' events, the objects' ticks (the client's figure moves by the moves it sent), and a packet to the client.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque::script::vm::host::MemFiles;
use torque::script::vm::vm::Vm;
use torque::server::game::GameParts;
use torque::server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque::server::net::transport::{MemoryEnd, MemoryTransport, SharedTransport};
use torque::server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque::server::server::{Server, TICK_MS};
use torque::server::state::ServerState;

use crate::files::GameFiles;

/// The server's address on the link.
pub const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

/// The server's main loop over a network host of Torque's net game.
pub type NetServer = Server<HostParts<TorqueNet>>;

/// The example's server.
pub struct WorldServer {
    pub server: NetServer,
    pub shared: Rc<RefCell<NetShared<TorqueNet>>>,
    pub game: GameParts,
    /// The transport's other end: what the link carries to and from the client.
    pub end: MemoryEnd,
}

impl WorldServer {
    /// A server that has run `server.cs` and started the map `mission` (a path under the game folder, `missions/plaza.mis`).
    pub fn start(files: &GameFiles, mission: &str) -> Result<WorldServer, String> {
        let mem = MemFiles::new();
        files.fill(&mem);
        let mut vm = Vm::new();
        torque::game::specs::install(&mut vm);
        vm.set_file_host(mem);
        let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
        let game = torque::server::game::install(&mut vm, &state);
        let mut server = Server::with_state(vm, Rc::clone(&state));
        let (transport, end) = MemoryTransport::new();
        let transport: SharedTransport = Rc::new(RefCell::new(transport));
        transport
            .borrow_mut()
            .bind(SERVER_ADDR.ip(), SERVER_ADDR.port())
            .map_err(|e| format!("the link: {e:?}"))?;
        let shared = attach_host(
            &mut server.vm,
            Host {
                transport,
                game: TorqueNet::default(),
                engine: Rc::clone(&state),
                step_limit: None,
                clock: NetClock::Sim,
                ghosts: TgeGhostGame,
                container: Rc::clone(&game.container),
                static_writes: None,
            },
        );
        server
            .attach_net(Rc::new(NetPasses(Rc::clone(&shared))))
            .map_err(|e| format!("the network host: {e:?}"))?;
        if !server
            .vm
            .exec_file(b"server.cs", false)
            .map_err(|e| format!("server.cs: {e}"))?
        {
            return Err("server.cs is not in the game folder".to_string());
        }
        server
            .vm
            .invoke(None, b"startWorld", &[mission.as_bytes().to_vec()])
            .map_err(|e| format!("startWorld: {e}"))?;
        state.borrow_mut().allow_connections = true;
        Ok(WorldServer {
            server,
            shared,
            game,
            end,
        })
    }

    /// One tick of the main loop (the first covers the start-up: the clock goes to 1,024 ms at once).
    pub fn tick(&mut self) {
        self.server.run(TICK_MS, |_| false);
    }

    pub fn vm(&self) -> &Vm {
        &self.server.vm
    }

    /// The console's lines since the last call.
    pub fn drain_log(&mut self) -> Vec<String> {
        self.server
            .vm
            .drain_log()
            .into_iter()
            .map(|l| String::from_utf8_lossy(&l.text).into_owned())
            .collect()
    }

    /// The figure the last client was given (`$World::Figure`), if it still exists.
    pub fn figure(&self) -> Option<u32> {
        let id = self.server.vm.global("$World::Figure")?;
        let id = u32::try_from(torque::script::text::atoi(&id)).ok()?;
        self.server.vm.objects().contains(id).then_some(id)
    }

    /// Where the figure stands (its feet), if there is one.
    pub fn figure_position(&self) -> Option<[f32; 3]> {
        let id = self.figure()?;
        torque::game::sim::player::position(&self.server.vm, id)
    }
}
