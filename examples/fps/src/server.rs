//! The example's server: torque-server's main loop with TGE's game classes ([`torque::server::game`]) and a network host of
//! Torque's net game ([`TorqueNet`]) on a UDP socket, running the game's `server.cs` and one map.
//!
//! **The socket.** The server binds one UDP socket on the address it is given: `127.0.0.1` keeps the game on this computer (the
//! tests, a game for one), a private LAN address opens it to the local network. The bind policy refuses a public address. The
//! LAN list's pings and queries are answered on the same socket.
//!
//! **A tick** ([`ArenaServer::tick`]) is one 32 ms pass of the main loop: the datagrams waiting, the scripts' events, the
//! objects' ticks (each client's figure moves by the moves it sent, bolts fly) and a packet to every client.

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque::script::vm::vm::Vm;
use torque::server::game::GameParts;
use torque::server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque::server::net::transport::{
    BindPolicy, LanRules, SharedTransport, Transport, UdpTransport,
};
use torque::server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque::server::server::{Server, TICK_MS};
use torque::server::state::ServerState;

use crate::files::GameFiles;

/// The UDP port number the games listen on (and the LAN list asks).
pub const DEFAULT_PORT: u16 = 28_000;

/// The server's main loop over a network host of Torque's net game.
pub type NetServer = Server<HostParts<TorqueNet>>;

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
}

/// The example's server.
pub struct ArenaServer {
    pub server: NetServer,
    pub shared: Rc<RefCell<NetShared<TorqueNet>>>,
    pub game: GameParts,
    transport: SharedTransport,
}

impl ArenaServer {
    /// A dedicated server (`$Server::Dedicated`: the LAN list shows it as one) bound as `options` say.
    pub fn start_dedicated(
        files: &GameFiles,
        options: &ServerOptions,
    ) -> Result<ArenaServer, String> {
        ArenaServer::start_with(files, options, true)
    }

    /// A server bound as `options` say that has run `server.cs` and started the map.
    pub fn start(files: &GameFiles, options: &ServerOptions) -> Result<ArenaServer, String> {
        ArenaServer::start_with(files, options, false)
    }

    fn start_with(
        files: &GameFiles,
        options: &ServerOptions,
        dedicated: bool,
    ) -> Result<ArenaServer, String> {
        let ip = IpAddr::V4(options.address);
        let policy = BindPolicy::new(ip, false, &LanRules::PLAIN).map_err(|e| e.to_string())?;
        let mut udp = UdpTransport::new(policy);
        udp.bind(ip, options.port)
            .map_err(|e| format!("the server's socket: {e}"))?;
        let transport: SharedTransport = Rc::new(RefCell::new(udp));

        let mut vm = Vm::new();
        torque::game::specs::install(&mut vm);
        vm.set_file_host(files.mem_files());
        let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
        let game = torque::server::game::install(&mut vm, &state);
        let mut server = Server::with_state(vm, Rc::clone(&state));
        let shared = attach_host(
            &mut server.vm,
            Host {
                transport: Rc::clone(&transport),
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
        server.vm.set_global("$Pref::Server::Name", &options.name);
        let dedicated = if dedicated { "1" } else { "0" };
        server.vm.set_global("$Server::Dedicated", dedicated);
        if !server
            .vm
            .exec_file(b"server.cs", false)
            .map_err(|e| format!("server.cs: {e}"))?
        {
            return Err("server.cs is not in the game folder".to_string());
        }
        if files.file(&options.mission).is_none() {
            return Err(format!("no map {} in the game folder", options.mission));
        }
        server
            .vm
            .invoke(None, b"startArena", &[options.mission.as_bytes().to_vec()])
            .map_err(|e| format!("startArena: {e}"))?;
        state.borrow_mut().allow_connections = true;
        Ok(ArenaServer {
            server,
            shared,
            game,
            transport,
        })
    }

    /// Where the socket is bound.
    pub fn address(&self) -> Option<SocketAddr> {
        self.transport.borrow().local_addr()
    }

    /// One tick of the main loop.
    pub fn tick(&mut self) {
        self.server.run(TICK_MS, |_| false);
    }

    pub fn vm(&self) -> &Vm {
        &self.server.vm
    }

    pub fn vm_mut(&mut self) -> &mut Vm {
        &mut self.server.vm
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

    /// Calls the script function `function` with `args` on the server and returns what it returned (for tests and tools).
    pub fn call(&mut self, function: &str, args: &[&str]) -> Result<String, String> {
        let argv: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
        self.server
            .vm
            .invoke(None, function.as_bytes(), &argv)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .map_err(|e| format!("{function}: {e}"))
    }

    /// Closes the socket (the clients see the server go quiet and time out).
    pub fn close(&mut self) {
        self.transport.borrow_mut().close();
    }
}
