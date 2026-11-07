//! A game's server: torque-server's main loop with TGE's game classes ([`torque_server::game`]) and a network host of Torque's
//! net game ([`TorqueNet`]), running the game's `server.cs` and the function that starts its map.
//!
//! **The socket** ([`Listen`]). On UDP the server binds one socket on the address it is given: `127.0.0.1` keeps the game on
//! this computer (tests, a game for one), a private LAN address opens it to the local network, and `0.0.0.0` (every interface,
//! when allowed as public) opens it to players outside the network too (the router must forward its UDP port). The bind policy
//! refuses any other public address. The LAN list's pings and queries are answered on the same socket. In memory, the server's
//! transport is bound at an address and its other end ([`GameServer::memory_end`]) carries the datagrams: a client in the same
//! process joins it with no socket opened ([`crate::client::ClientNet::Memory`]). On the web (the `web` feature,
//! `Listen::Web`) the server takes browsers' game sockets over WebSocket and serves them the game's page on one TCP port,
//! with UDP on the same port number beside it or not at all (torque-server's `net::web`).
//!
//! **A tick** ([`GameServer::tick`]) is one 32 ms pass of the main loop: the datagrams waiting, the scripts' events, the
//! objects' ticks and a packet to every client.

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::rc::Rc;
use web_time::{Duration, Instant};

use torque_server::game::GameParts;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{
    BindPolicy, LanRules, MemoryEnd, MemoryTransport, SharedTransport, Transport, UdpTransport,
};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

/// The UDP port number games listen on (and the LAN list asks): TGE's.
pub const DEFAULT_PORT: u16 = 28_000;

/// How often a dedicated server's wait services its transport ([`GameServer::run_dedicated`]).
const SERVICE_EVERY: Duration = Duration::from_millis(2);

/// The server's main loop over a network host of Torque's net game.
pub type NetServer = Server<HostParts<TorqueNet>>;

/// The web side of a server that browsers join (`Listen::Web`).
#[cfg(all(feature = "web", not(target_arch = "wasm32")))]
#[derive(Debug, Clone)]
pub struct WebListen {
    /// UDP on the same port number beside the web (desktop and browser players in one game); else no UDP socket is opened.
    pub udp: bool,
    /// The files served (none: game sockets only, the page comes from another web server), TLS, and the limits.
    pub setup: torque_server::net::web::WebSetup,
}

/// Where a server listens.
#[derive(Debug, Clone)]
pub enum Listen {
    /// A UDP socket on `address` (`127.0.0.1`, or this computer's LAN address) and `port` (0 takes any free one). `public`
    /// allows `0.0.0.0` (every interface, so players outside the network can join when their traffic reaches this computer)
    /// as the address; no other public address is.
    Udp {
        address: Ipv4Addr,
        port: u16,
        public: bool,
    },
    /// An in-memory transport bound at this address: a client in the same process reaches it through
    /// [`GameServer::memory_end`].
    Memory(SocketAddr),
    /// HTTP and WebSocket on TCP `address`:`port` (0 takes any free one), and UDP on the same port when `web.udp` says so.
    /// Without TLS the address must be a private one; `public` allows `0.0.0.0` as for UDP.
    #[cfg(all(feature = "web", not(target_arch = "wasm32")))]
    Web {
        address: Ipv4Addr,
        port: u16,
        public: bool,
        web: Rc<WebListen>,
    },
    /// A transport the caller made and bound (a player's host found through a room service, torque-server's
    /// `net::web::p2p`; a browser host's worker link).
    Bound(BoundTransport),
}

/// A transport made and bound before the server starts ([`Listen::Bound`]).
#[derive(Clone)]
pub struct BoundTransport(pub SharedTransport);

impl std::fmt::Debug for BoundTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let at = self.0.borrow().local_addr();
        f.debug_tuple("BoundTransport").field(&at).finish()
    }
}

/// How a server starts: where it listens, the globals set before `server.cs` runs, and the script function that starts the
/// map, with its arguments.
#[derive(Debug, Clone)]
pub struct ServerStart<'a> {
    pub listen: Listen,
    pub globals: &'a [(&'a str, &'a str)],
    pub start: &'a str,
    pub args: &'a [&'a str],
}

/// This computer's private IPv4 addresses (the ones other players on its network join by), the one the routing table prefers
/// for each private range first. Nothing is sent: each target only asks the system which local address a datagram would leave
/// from.
pub fn lan_addresses() -> Vec<Ipv4Addr> {
    let mut found = Vec::new();
    for target in [
        Ipv4Addr::new(192, 168, 255, 254),
        Ipv4Addr::new(10, 255, 255, 254),
        Ipv4Addr::new(172, 31, 255, 254),
    ] {
        let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
            continue;
        };
        if socket.connect((target, 9)).is_err() {
            continue;
        }
        if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
            let ip = *local.ip();
            if (ip.is_private() || ip.is_link_local()) && !found.contains(&ip) {
                found.push(ip);
            }
        }
    }
    found
}

/// A game's server.
pub struct GameServer {
    pub server: NetServer,
    pub shared: Rc<RefCell<NetShared<TorqueNet>>>,
    pub game: GameParts,
    transport: SharedTransport,
    end: Option<MemoryEnd>,
    /// How often a dedicated server prints its transport's status lines ([`GameServer::set_status_every`]).
    status_every: Option<Duration>,
}

impl GameServer {
    /// A server over `files`, listening as `start` says, that has run `server.cs` and the start function and takes
    /// connections.
    pub fn start(files: MemFiles, start: &ServerStart) -> Result<GameServer, String> {
        GameServer::start_with(files, start, |_| {})
    }

    /// [`GameServer::start`] with `prepare` run on the script engine before the game classes are installed (a game's own
    /// subclasses of the simulated classes: `torque_game::sim_classes::set_sim_classes`).
    pub fn start_with(
        files: MemFiles,
        start: &ServerStart,
        prepare: impl FnOnce(&mut Vm),
    ) -> Result<GameServer, String> {
        let (transport, end) = match &start.listen {
            &Listen::Udp {
                address,
                port,
                public,
            } => (udp_transport(address, port, public)?, None),
            #[cfg(all(feature = "web", not(target_arch = "wasm32")))]
            Listen::Web {
                address,
                port,
                public,
                web,
            } => (web_transport(*address, *port, *public, web)?, None),
            Listen::Bound(bound) => (Rc::clone(&bound.0), None),
            &Listen::Memory(at) => {
                let (transport, end) = MemoryTransport::new();
                let transport: SharedTransport = Rc::new(RefCell::new(transport));
                transport
                    .borrow_mut()
                    .bind(at.ip(), at.port())
                    .map_err(|e| format!("the link: {e:?}"))?;
                (transport, Some(end))
            }
        };

        let mut vm = Vm::new();
        torque_game::specs::install(&mut vm);
        vm.set_file_host(files);
        prepare(&mut vm);
        let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
        let game = torque_server::game::install(&mut vm, &state);
        super::terrain::install(&mut vm, &game.container);
        super::water::install(&mut vm, &game.container);
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
        for (name, value) in start.globals {
            server.vm.set_global(name, value);
        }
        if !server
            .vm
            .exec_file(b"server.cs", false)
            .map_err(|e| format!("server.cs: {e}"))?
        {
            return Err("server.cs is not in the game folder".to_string());
        }
        let args: Vec<Vec<u8>> = start.args.iter().map(|a| a.as_bytes().to_vec()).collect();
        server
            .vm
            .invoke(None, start.start.as_bytes(), &args)
            .map_err(|e| format!("{}: {e}", start.start))?;
        state.borrow_mut().allow_connections = true;
        Ok(GameServer {
            server,
            shared,
            game,
            transport,
            end,
            status_every: None,
        })
    }

    /// Where the socket is bound.
    pub fn address(&self) -> Option<SocketAddr> {
        self.transport.borrow().local_addr()
    }

    /// The in-memory transport's other end (a server started on [`Listen::Memory`]): what a link carries to and from clients.
    pub fn memory_end(&self) -> Option<&MemoryEnd> {
        self.end.as_ref()
    }

    /// One tick of the main loop (the first covers the start-up: the clock goes to 1,024 ms at once).
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

    /// Has a dedicated server print its transport's status lines ([`Transport::status`]: a web transport's data channels)
    /// every `every` (never with none, the default).
    pub fn set_status_every(&mut self, every: Option<Duration>) {
        self.status_every = every;
    }

    /// The transport's status lines since the last call ([`Transport::status`]).
    pub fn transport_status(&mut self) -> Vec<String> {
        self.transport.borrow_mut().status()
    }

    /// Runs as a dedicated server: a tick every 32 ms of the wall clock (catching up from now when behind, rather than running
    /// a burst of ticks), each console line handed to `print`, until `ticks` ticks ran (forever with none).
    pub fn run_dedicated(&mut self, ticks: Option<u64>, mut print: impl FnMut(&str)) {
        let tick = Duration::from_millis(u64::from(TICK_MS));
        let mut next = Instant::now();
        let mut done = 0u64;
        let mut last_status = Instant::now();
        loop {
            self.tick();
            for line in self.drain_log() {
                print(&line);
            }
            if let Some(every) = self.status_every
                && last_status.elapsed() >= every
            {
                last_status = Instant::now();
                for line in self.transport_status() {
                    print(&line);
                }
            }
            done += 1;
            if ticks.is_some_and(|t| done >= t) {
                break;
            }
            next += tick;
            let now = Instant::now();
            match next.checked_duration_since(now) {
                Some(wait) => self.wait(wait),
                None => next = now,
            }
        }
    }

    /// Waits `wait` of the wall clock; a transport with work of its own between ticks ([`Transport::service`]: a web
    /// transport's files and sockets) is serviced every few milliseconds meanwhile.
    fn wait(&mut self, wait: Duration) {
        if !self.transport.borrow().wants_service() {
            std::thread::sleep(wait);
            return;
        }
        let until = Instant::now() + wait;
        loop {
            self.transport.borrow_mut().service();
            let now = Instant::now();
            let Some(left) = until.checked_duration_since(now) else {
                break;
            };
            std::thread::sleep(left.min(SERVICE_EVERY));
        }
    }

    /// The transport's work between ticks, done now ([`Transport::service`]; a harness that paces ticks itself calls it).
    pub fn service(&mut self) {
        self.transport.borrow_mut().service();
    }
}

/// The bind policy for `address` (`public` allows `0.0.0.0` alone).
fn bind_policy(address: Ipv4Addr, public: bool) -> Result<BindPolicy, String> {
    BindPolicy::new(
        IpAddr::V4(address),
        public && address.is_unspecified(),
        &LanRules::PLAIN,
    )
    .map_err(|e| {
        format!(
            "the address {address} is not a private one; only 0.0.0.0 (every interface) may be public: {e}"
        )
    })
}

/// The web transport (and UDP beside it when `web.udp` says so) bound on `address` and `port` under the bind policy.
#[cfg(all(feature = "web", not(target_arch = "wasm32")))]
fn web_transport(
    address: Ipv4Addr,
    port: u16,
    public: bool,
    web: &WebListen,
) -> Result<SharedTransport, String> {
    use torque_server::net::web::{MultiTransport, WebTransport};
    let policy = bind_policy(address, public)?;
    let ip = IpAddr::V4(address);
    let site = WebTransport::new(policy, web.setup.clone());
    let transport: SharedTransport = if web.udp {
        Rc::new(RefCell::new(MultiTransport::new(
            UdpTransport::new(policy),
            site,
        )))
    } else {
        Rc::new(RefCell::new(site))
    };
    transport
        .borrow_mut()
        .bind(ip, port)
        .map_err(|e| format!("the server's socket: {e}"))?;
    Ok(transport)
}

/// A UDP transport bound on `address` and `port` under the bind policy (`public` allows `0.0.0.0` alone).
fn udp_transport(address: Ipv4Addr, port: u16, public: bool) -> Result<SharedTransport, String> {
    let ip = IpAddr::V4(address);
    let policy = bind_policy(address, public)?;
    let mut udp = UdpTransport::new(policy);
    udp.bind(ip, port)
        .map_err(|e| format!("the server's socket: {e}"))?;
    Ok(Rc::new(RefCell::new(udp)))
}
