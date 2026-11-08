//! A game's server: torque-server's main loop with TGE's game classes ([`torque_server::game`]) and a network host of Torque's
//! net game ([`TorqueNet`]), running the game's `server.cs` and the function that starts its map.
//!
//! **The socket** ([`Listen`]). On UDP the server binds one socket on the address it is given: `127.0.0.1` keeps the game on
//! this computer (tests, a game for one), a private LAN address opens it to the local network, and `0.0.0.0` (every interface,
//! when allowed as public) opens it to players outside the network too (the router must forward its UDP port). The bind policy
//! refuses any other public address. The LAN list's pings and queries are answered on the same socket. In memory, the server's
//! transport is bound at an address and its other end ([`GameServer::memory_end`]) carries the datagrams: a client in the same
//! process joins it with no socket opened (`torque_app::client::ClientNet::Memory`). On the web (the `web` feature,
//! `Listen::Web`) the server takes browsers' game sockets over WebSocket and serves them the game's page on one TCP port,
//! with UDP on the same port number beside it or not at all (torque-server's `net::web`).
//!
//! **The transport** is picked by id: a [`Listen`] names the transport that serves it ([`Listen::transport_id`]: `udp`, `memory`,
//! `web`, or the id of a bound or custom one), and the engine's transport plugins register them ([`crate::transports`]).
//! An id no plugin registered is refused.
//!
//! **A tick** ([`GameServer::tick`]) is one 32 ms pass of the main loop: the datagrams waiting, the scripts' events, the
//! objects' ticks and a packet to every client.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::rc::Rc;
use web_time::{Duration, Instant};

use torque_engine::{Engine, EngineBuilder, ServerStage};
use torque_game::sim_classes::SimClasses;
use torque_server::game::GameParts;
use torque_server::net::channels;
use torque_server::net::torque::TorqueNet;
use torque_server::net::transport::{MemoryEnd, SharedTransport};
use torque_server::net::{HostParts, NetShared};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use crate::plugins::{MinimalPlugins, ServerWiring};
pub use crate::transports::{BoundTransport, CustomListen};
use crate::transports::{ServerLink, open_listen, server_transport_warnings};

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
    /// `net::web::p2p`, id `rooms`; a browser host's worker link, id `relay`).
    Bound(BoundTransport),
    /// A transport a plugin registered under its own id ([`Listen::custom`]), with the request it reads.
    Custom(CustomListen),
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

/// The console warning for a server with an auth provider whose address cannot be what players join it by: a provider that binds
/// tickets to the server's address ([`GameServer::set_public_address`]) refuses every sign-in when the address is unknown (none,
/// every interface, or port 0: a relay transport reports 127.0.0.1:0). `memory` is a server on the in-memory transport, whose
/// bound address is the one its clients dial.
pub(crate) fn sign_in_address_warning(address: Option<SocketAddr>, memory: bool) -> Option<String> {
    let known = address.is_some_and(|a| memory || (!a.ip().is_unspecified() && a.port() != 0));
    if known {
        return None;
    }
    let shown = address.map_or_else(|| "no address".to_string(), |a| a.to_string());
    Some(format!(
        "this server checks sign-ins but does not know the address players join it by ({shown}), so it will refuse every sign-in until GameServer::set_public_address is given that address (behind a NAT, or on a specific private address, it is the public one)"
    ))
}

impl GameServer {
    /// A server over `files`, listening as `start` says, that has run `server.cs` (or the entry script `game.toml` names) and the start function and takes
    /// connections.
    pub fn start(files: MemFiles, start: &ServerStart) -> Result<GameServer, String> {
        GameServer::start_with(files, start, |_| {})
    }

    /// [`GameServer::start`] with `prepare` run on the script engine before the game classes are installed (a game's own
    /// subclasses of the simulated classes: `torque_game::sim_classes::set_sim_classes`). The server is made by
    /// [`MinimalPlugins`].
    pub fn start_with(
        files: MemFiles,
        start: &ServerStart,
        prepare: impl FnOnce(&mut Vm),
    ) -> Result<GameServer, String> {
        let engine = EngineBuilder::new()
            .add_plugins(MinimalPlugins)
            .build()
            .map_err(|e| e.to_string())?;
        let mut prepare = Some(prepare);
        GameServer::start_prepared(&engine, files, start, &mut |vm| {
            if let Some(prepare) = prepare.take() {
                prepare(vm);
            }
        })
    }

    /// A server over `files` made by `engine`'s plugins: their server recipes run on the new script engine, stage by stage
    /// ([`ServerStage`]), in the order the engine's plugins were added. The engine can make any number of servers. It needs
    /// the plugins that make the server's parts: [`ServerGamePlugin`](crate::plugins::ServerGamePlugin) and [`NetHostPlugin`](crate::plugins::NetHostPlugin) (with [`MinimalPlugins`], all of
    /// them). What the build warned about ([`Engine::warnings`]) is printed to the server's console first.
    pub fn start_engine(
        engine: &Engine,
        files: MemFiles,
        start: &ServerStart,
    ) -> Result<GameServer, String> {
        GameServer::start_prepared(engine, files, start, &mut |_| {})
    }

    /// [`GameServer::start_engine`] with `prepare` run on the script engine after the `Prepare` stage (not generic, so a
    /// game's own closure does not copy this body into its crate).
    fn start_prepared(
        engine: &Engine,
        files: MemFiles,
        start: &ServerStart,
        prepare: &mut dyn FnMut(&mut Vm),
    ) -> Result<GameServer, String> {
        let ServerLink { transport, end } = open_listen(engine, &start.listen)?;

        let mut vm = Vm::new();
        for warning in engine.warnings() {
            vm.warn(warning.to_string());
        }
        for warning in server_transport_warnings(engine) {
            vm.warn(warning);
        }
        let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
        let wiring = Rc::new(ServerWiring::new(Rc::clone(&state), Rc::clone(&transport)));
        vm.set_extension(Rc::clone(&wiring));
        replay(engine, ServerStage::Classes, &mut vm)?;
        vm.set_file_host(files);
        replay(engine, ServerStage::Prepare, &mut vm)?;
        if let Some(classes) = engine.slot::<dyn SimClasses>() {
            torque_game::sim_classes::set_sim_classes(&mut vm, classes);
        }
        prepare(&mut vm);
        replay(engine, ServerStage::Game, &mut vm)?;
        let game = wiring
            .game()
            .ok_or("the engine has no server game (ServerGamePlugin)")?;
        replay(engine, ServerStage::World, &mut vm)?;
        let mut server = Server::with_state(vm, Rc::clone(&state));
        replay(engine, ServerStage::Net, &mut server.vm)?;
        let net = wiring
            .take_net()
            .ok_or("the engine has no network host (NetHostPlugin)")?;
        let shared = net.shared;
        // An engine with networked plugins or an auth provider holds every client to its manifest and tickets, before
        // `onConnectRequest`; one with neither judges no one and speaks Torque's handshake.
        let profile = shared.borrow().handshake_profile();
        if let Some(gate) = engine.connect_gate(&profile) {
            shared.borrow_mut().set_connect_gate(gate);
        }
        // The address the socket is bound to is the one clients dial unless the host says otherwise
        // ([`GameServer::set_public_address`]); a socket bound to every interface leaves it unknown.
        let bound = transport.borrow().local_addr();
        shared.borrow_mut().set_public_address(bound);
        if engine.auth().is_some()
            && let Some(warning) =
                sign_in_address_warning(bound, matches!(start.listen, Listen::Memory(_)))
        {
            server.vm.warn(warning);
        }
        server
            .attach_net(net.passes)
            .map_err(|e| format!("the network host: {e:?}"))?;
        // The engine's bulk channels go in each connection's packets; the hub a game sends through is an extension of the
        // server's script engine.
        let channels = engine.net_channels().map_err(|e| e.to_string())?;
        if let Some(hub) = channels::install(&shared, channels) {
            server.vm.set_extension(Rc::new(hub));
        }
        for (name, value) in start.globals {
            server.vm.set_global(name, value);
        }
        let entry = crate::config::server_entry(engine);
        if !server
            .vm
            .exec_file(entry.as_bytes(), false)
            .map_err(|e| format!("{entry}: {e}"))?
        {
            return Err(format!("{entry} is not in the game folder"));
        }
        replay(engine, ServerStage::Entry, &mut server.vm)?;
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

    /// Tells the server the address clients reach it by (its public address), which its auth provider binds tickets to: a socket
    /// bound to every interface, a NAT or a relay hides it. A server that is never told one refuses every ticket of a provider
    /// that binds to the address.
    pub fn set_public_address(&self, address: Option<SocketAddr>) {
        self.shared.borrow_mut().set_public_address(address);
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

    /// Has a dedicated server print its transport's status lines ([`Transport::status`](torque_server::net::transport::Transport::status): a web transport's data channels)
    /// every `every` (never with none, the default).
    pub fn set_status_every(&mut self, every: Option<Duration>) {
        self.status_every = every;
    }

    /// The transport's status lines since the last call ([`Transport::status`](torque_server::net::transport::Transport::status)).
    pub fn transport_status(&mut self) -> Vec<String> {
        self.transport.borrow_mut().status()
    }

    /// Runs as a dedicated server: a tick every 32 ms of the wall clock (catching up from now when behind, rather than running
    /// a burst of ticks), each console line handed to `print`, until `ticks` ticks ran (forever with none).
    pub fn run_dedicated(&mut self, ticks: Option<u64>, print: impl FnMut(&str)) {
        self.run_dedicated_until(|done| ticks.is_some_and(|t| done >= t), print);
    }

    /// [`run_dedicated`](Self::run_dedicated) until `stop`, asked after each tick with the number of ticks run so far, says
    /// so.
    pub fn run_dedicated_until(
        &mut self,
        mut stop: impl FnMut(u64) -> bool,
        mut print: impl FnMut(&str),
    ) {
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
            if stop(done) {
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

    /// Waits `wait` of the wall clock; a transport with work of its own between ticks ([`Transport::service`](torque_server::net::transport::Transport::service): a web
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

    /// The transport's work between ticks, done now ([`Transport::service`](torque_server::net::transport::Transport::service); a harness that paces ticks itself calls it).
    pub fn service(&mut self) {
        self.transport.borrow_mut().service();
    }
}

/// Replays `stage` of `engine`'s server recipes on `vm`.
fn replay(engine: &Engine, stage: ServerStage, vm: &mut Vm) -> Result<(), String> {
    engine.replay(stage, vm).map_err(|e| e.to_string())
}
