//! A game's server: torque-server's main loop with TGE's game classes ([`torque_server::game`]) and a network host of a net game
//! (`GameServer<G>`; Torque's, [`TorqueNet`], by default and with no [`NetGamePlugin`](crate::net_game::NetGamePlugin)), running
//! the game's `server.cs` and the function that starts its map.
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

use std::any::Any;
use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::rc::Rc;
use web_time::{Duration, Instant};

use torque_engine::command::{CommandSpec, ServerCommand};
use torque_engine::{
    Engine, EngineBuilder, ProtocolKind, ServerStage, ServerTick, TickInfo, TickPhase,
};
use torque_game::sim_classes::SimClasses;
use torque_net::connect::ManifestGate;
use torque_net::engine::bridge::table_digests;
use torque_server::game::GameParts;
use torque_server::net::NetGame;
use torque_server::net::channels;
use torque_server::net::cmdlimit::{CommandTable, register_command_drops};
use torque_server::net::peer::ProtocolMode;
use torque_server::net::rpc::{RpcLanes, ServerRpc, SharedHost, register_server_natives};
use torque_server::net::torque::TorqueNet;
use torque_server::net::transport::{
    LanRules, MemoryEnd, PlainEngineRefusal, SharedTransport, check_plain_engine_bind,
};
use torque_server::net::{HostParts, NetShared};
use torque_server::server::{MissionError, Server, TICK_MS, TickHooks};
use torque_server::state::ServerState;
use torque_ts_vm::host::{FileHost, MemFiles};
use torque_ts_vm::vm::Vm;

use crate::net_game::{AttachedCell, apply_codecs, factory_for};
use crate::plugins::{MinimalPlugins, ServerWiring};
use crate::slots::{Connections, FileSetter, ServerEntry, file_setter};
pub use crate::transports::{BoundTransport, CustomListen};
use crate::transports::{ServerLink, open_listen, server_transport_warnings};

/// The UDP port number games listen on (and the LAN list asks): TGE's.
pub const DEFAULT_PORT: u16 = 28_000;

/// How often a dedicated server's wait services its transport ([`GameServer::run_dedicated`]).
const SERVICE_EVERY: Duration = Duration::from_millis(2);

/// The server's main loop over a network host of net game `G` (Torque's by default).
pub type NetServer<G = TorqueNet> = Server<HostParts<G>>;

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

/// Why a server in engine mode may not listen as `listen` asks: plain UDP on an address outside the private set (the engine's
/// protocol takes plain UDP for LAN and development only). The other transports' links bring their own security.
pub fn plain_engine_refusal(listen: &Listen) -> Option<PlainEngineRefusal> {
    let address = match listen {
        Listen::Udp { address, .. } => *address,
        #[cfg(all(feature = "web", not(target_arch = "wasm32")))]
        Listen::Web { address, web, .. } if web.udp => *address,
        _ => return None,
    };
    check_plain_engine_bind(&LanRules::PLAIN, address.into()).err()
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

/// A game's server, whose network host speaks net game `G` (Torque's by default).
pub struct GameServer<G: NetGame = TorqueNet> {
    pub server: NetServer<G>,
    pub shared: Rc<RefCell<NetShared<G>>>,
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

/// Gives the server's uploads a staging folder of their own (a lock holds it for the run) with a cap, or says why they have none.
fn claim_staging(vm: &mut Vm) {
    let claimed = crate::uploads::data_dir().map(|data| {
        let parent = crate::uploads::staging_parent(&data);
        crate::uploads::remove_dead_runs(&parent);
        crate::uploads::claim_run_folder(&parent, &crate::uploads::run_name())
    });
    let folder = match claimed {
        Some(Ok(folder)) => Some(folder),
        Some(Err(e)) => {
            vm.warn(format!(
                "uploads: no staging folder ({e}): only uploads that fit in memory are taken"
            ));
            None
        }
        None => {
            vm.warn(
                "uploads: no data folder for staging: only uploads that fit in memory are taken"
                    .to_string(),
            );
            None
        }
    };
    let config = crate::uploads::staging_config(folder.as_ref().map(|f| f.path().to_path_buf()));
    vm.set_extension(Rc::new(torque_engine::upload::HostStaging(config)));
    if let Some(folder) = folder {
        vm.set_extension(Rc::new(folder));
    }
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
        Self::start_prepared(&engine, file_setter(files), start, &mut |vm| {
            if let Some(prepare) = prepare.take() {
                prepare(vm);
            }
        })
    }

    /// A server over `files` made by `engine`'s plugins: their server recipes run on the new script engine, stage by stage
    /// ([`ServerStage`]), in the order the engine's plugins were added. The engine can make any number of servers. It needs
    /// the plugins that make the server's parts: [`ServerGamePlugin`](crate::plugins::ServerGamePlugin) and [`NetHostPlugin`](crate::plugins::NetHostPlugin) (with [`MinimalPlugins`], all of
    /// them). What the build warned about ([`Engine::warnings`]) is printed to the server's console first.
    ///
    /// The script engine is the engine's ([`HostSlots::script_engine`](crate::slots::HostSlots::script_engine), else
    /// [`Vm::new`]). Once the stages ran, the server sets `start`'s globals and runs its entry: the engine's
    /// ([`HostSlots::server_entry`](crate::slots::HostSlots::server_entry)), then the `Entry` stage, leaving connections as that
    /// entry left them; else the entry script, the `Entry` stage and `start`'s function, and it takes connections. Refused on an engine
    /// whose net game slot holds another net game ([`GameServer::start_with_net`] of that type, or
    /// [`start_net_game`](crate::net_game::start_net_game), starts it).
    pub fn start_engine(
        engine: &Engine,
        files: MemFiles,
        start: &ServerStart,
    ) -> Result<GameServer, String> {
        Self::start_prepared(engine, file_setter(files), start, &mut |_| {})
    }

    /// [`GameServer::start_engine`] over a file host of the game's own (a folder read in place, archives mounted over it, an
    /// overlay that keeps the writes): the scripts read and write `files` instead of a [`MemFiles`].
    pub fn start_engine_with_files(
        engine: &Engine,
        files: impl FileHost + 'static,
        start: &ServerStart,
    ) -> Result<GameServer, String> {
        Self::start_prepared(engine, file_setter(files), start, &mut |_| {})
    }
}

impl<G: NetGame> GameServer<G> {
    /// A server of net game `G` made by `engine`'s plugins, as [`GameServer::start_engine`] makes one of Torque's. Its network
    /// host is made by the engine's net game slot ([`NetGamePlugin`](crate::net_game::NetGamePlugin)).
    ///
    /// # Errors
    /// The engine's slot holds another net game (or none, and `G` is not [`TorqueNet`]); the message names both. Otherwise,
    /// what the start returned.
    pub fn start_with_net(
        engine: &Engine,
        files: MemFiles,
        start: &ServerStart,
    ) -> Result<GameServer<G>, String> {
        Self::start_prepared(engine, file_setter(files), start, &mut |_| {})
    }

    /// [`GameServer::start_with_net`] with `prepare` run on the script engine after the `Prepare` stage. Generic over the net
    /// game `G` (one copy per net game); only `prepare` is a `dyn` closure, so a game's own closure does not copy this body
    /// into its crate.
    fn start_prepared(
        engine: &Engine,
        files: FileSetter,
        start: &ServerStart,
        prepare: &mut dyn FnMut(&mut Vm),
    ) -> Result<GameServer<G>, String> {
        let factory = factory_for::<G>(engine)?;
        if engine.protocol() == ProtocolKind::Engine
            && let Some(refusal) = plain_engine_refusal(&start.listen)
        {
            return Err(refusal.to_string());
        }
        let ServerLink { transport, end } = open_listen(engine, &start.listen)?;

        let mut vm = crate::slots::make_script_engine(engine);
        for warning in engine.warnings() {
            vm.warn(warning.to_string());
        }
        for warning in server_transport_warnings(engine) {
            vm.warn(warning);
        }
        let state = Rc::new(RefCell::new(ServerState::<HostParts<G>>::default()));
        let attached = AttachedCell::<G>::default();
        let wiring = Rc::new(ServerWiring::new(
            Rc::clone(&state),
            Rc::clone(&transport),
            factory,
            Rc::clone(&attached),
        ));
        vm.set_extension(Rc::clone(&wiring));
        replay(engine, ServerStage::Classes, &mut vm)?;
        files(&mut vm);
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
        server
            .vm
            .set_extension(Rc::new(ProtocolMode(engine.protocol())));
        // Uploads too large for memory are staged in a folder of this run's own, under the user's data folder.
        if !engine
            .list::<torque_engine::upload::UploadEntry>()
            .is_empty()
        {
            claim_staging(&mut server.vm);
        }
        apply_codecs(engine, &mut server.vm);
        replay(engine, ServerStage::Net, &mut server.vm)?;
        let net = attached
            .borrow_mut()
            .take()
            .ok_or("the engine has no network host (NetHostPlugin)")?;
        let shared = net.shared;
        // An engine with networked plugins or an auth provider holds every client to its manifest and tickets, before
        // `onConnectRequest`; one with neither judges no one and speaks Torque's handshake.
        let profile = shared.borrow().handshake_profile();
        let engine_mode = engine.protocol() == ProtocolKind::Engine;
        // In engine mode the manifest is the connect itself: every client is held to it, whatever the plugins say on the wire.
        let gate = engine.connect_gate(&profile).or_else(|| {
            engine_mode.then(|| {
                let gate = ManifestGate::new(engine.net_manifest(&profile));
                match engine.auth() {
                    Some(auth) => gate.with_auth(auth),
                    None => gate,
                }
            })
        });
        if engine_mode {
            let manifest = engine.net_manifest(&profile);
            let mut sh = shared.borrow_mut();
            if let Some(host) = torque_server::net::engine::ensure_host(&server.vm, &mut sh) {
                host.digests = table_digests(Some(&manifest));
                // A server that checks tickets challenges every connect, and checks the ticket against the challenge.
                host.auth = if engine.auth().is_some() {
                    torque_server::net::engine::Auth::Challenge
                } else {
                    torque_server::net::engine::Auth::None
                };
            }
        }
        if let Some(gate) = gate {
            // The mods a server requires are known once its mods are loaded, after the gate is set; the script engine holds the
            // list the gate judges with.
            server.vm.set_extension(Rc::new(gate.required().clone()));
            shared.borrow_mut().set_connect_gate(gate);
            // Every client the gate lets in has the plugins the manifest names, so it is the engine's own: the plugins' ghost
            // classes go to all of them (a game that also takes TGE clients leaves the engine without networked plugins).
            server
                .vm
                .set_global(torque_server::net::torque::ENGINE_CLASSES_GLOBAL, "1");
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
        let hub = channels::install(&shared, channels);
        // Who may do what: the roles and permissions the plugins declared, scripts may add to, and the provider keeps.
        let permissions = torque_engine::permissions::Permissions::new(engine)
            .map_err(|e| format!("the permissions: {e}"))?;
        server.vm.set_extension(Rc::new(permissions));
        // The plugins' RPCs ride the reserved channel (each session's RPC lanes in engine mode); the server's pump runs them once
        // a tick.
        let rpcs = engine.net_rpcs().map_err(|e| e.to_string())?;
        let has_rpcs = !rpcs.is_empty();
        if has_rpcs && let Some(hub) = &hub {
            let host = Rc::new(SharedHost(Rc::downgrade(&shared)));
            let perms = server
                .vm
                .extension::<torque_engine::permissions::Permissions>()
                .ok_or("the server has no permissions")?;
            let rpc = if engine_mode {
                let lanes = Rc::new(RpcLanes::default());
                server.vm.set_extension(Rc::clone(&lanes));
                Rc::new(ServerRpc::over_lanes(rpcs, hub.clone(), host, perms, lanes))
            } else {
                Rc::new(ServerRpc::new(rpcs, hub.clone(), host, perms))
            };
            register_server_natives(&mut server.vm, &rpc);
            // What the engine's uploads need of the connections (a game with no upload kind never asks).
            torque_server::net::uploads::install_server(&mut server.vm, &rpc, hub);
            server.vm.set_extension(rpc);
        }
        if let Some(hub) = hub {
            server.vm.set_extension(Rc::new(hub));
        }
        // The server commands plugins declared are held to their limits, and scripts can read the refusals.
        let mut commands = CommandTable::new(engine.list::<ServerCommand>());
        if engine_mode {
            // No event queue bounds the commands of this protocol, so the ones no plugin declared share default limits.
            commands = commands.with_default_limits(CommandSpec::new());
        }
        if !commands.is_empty() {
            shared.borrow_mut().set_commands(commands);
            register_command_drops(&mut server.vm, &shared);
        }
        // The plugins' tick hooks run around the process list's pass; a server whose engine has none ticks as it always did.
        if has_rpcs || !engine.list::<ServerTick>().is_empty() {
            server.set_tick_hooks(Some(Rc::new(EngineTicks {
                engine: engine.clone(),
                printed: RefCell::default(),
            })));
        }
        // The plugins' hooks and mission loader, made now that every stage but the entry has run.
        if let Some(hooks) = crate::slots::make_server_hooks(engine, &mut server.vm)? {
            server.set_hooks(hooks);
        }
        if let Some(loader) = crate::slots::make_mission_loader(engine, &mut server.vm)? {
            server.set_mission_loader(loader);
        }
        for (name, value) in start.globals {
            server.vm.set_global(name, value);
        }
        match crate::slots::server_entry_fn(engine) {
            Some(run) => {
                // Off until the game's entry turns them on, whatever the network host's attach left.
                state.borrow_mut().allow_connections = false;
                let connections = Connections::over(&state);
                run(&mut ServerEntry::new(&mut server.vm, start, connections))?;
                replay(engine, ServerStage::Entry, &mut server.vm)?;
            }
            None => {
                run_entry_script(engine, &mut server.vm, start)?;
                state.borrow_mut().allow_connections = true;
            }
        }
        Ok(GameServer {
            server,
            shared,
            game,
            transport,
            end,
            status_every: None,
        })
    }

    /// Loads `mission` with the server's mission loader (the engine's [`ServerSlots::mission_loader`](crate::slots::ServerSlots::mission_loader),
    /// else TGE's: the `.mis` is `exec`'d); the answer is the loader's own report.
    pub fn load_mission(&mut self, mission: &str) -> Result<Box<dyn Any>, MissionError> {
        self.server.load_mission(mission.as_bytes())
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

/// A server's plain entry: the entry script, the `Entry` stage, then the start function.
fn run_entry_script(engine: &Engine, vm: &mut Vm, start: &ServerStart) -> Result<(), String> {
    let entry = crate::config::server_entry(engine);
    if !vm
        .exec_file(entry.as_bytes(), false)
        .map_err(|e| format!("{entry}: {e}"))?
    {
        return Err(format!("{entry} is not in the game folder"));
    }
    replay(engine, ServerStage::Entry, vm)?;
    let args: Vec<Vec<u8>> = start.args.iter().map(|a| a.as_bytes().to_vec()).collect();
    vm.invoke(None, start.start.as_bytes(), &args)
        .map_err(|e| format!("{}: {e}", start.start))?;
    Ok(())
}

/// Replays `stage` of `engine`'s server recipes on `vm`.
fn replay(engine: &Engine, stage: ServerStage, vm: &mut Vm) -> Result<(), String> {
    engine.replay(stage, vm).map_err(|e| e.to_string())
}

/// The engine's [`ServerTick`] hooks as the main loop runs them: each phase's hooks in the order they were pushed. A hook
/// that fails prints its error on the server's console (again only when the error changes, not every tick) and the rest run.
struct EngineTicks {
    engine: Engine,
    /// The last error each failing hook printed, by its place in the list.
    printed: RefCell<std::collections::BTreeMap<usize, String>>,
}

impl EngineTicks {
    fn run(&self, phase: TickPhase, vm: &mut Vm, tick: TickInfo) {
        let hooks = self.engine.list::<ServerTick>();
        for (index, hook) in hooks.iter().enumerate() {
            if hook.phase() != phase {
                continue;
            }
            match hook.run(vm, &self.engine, tick) {
                Ok(()) => {
                    self.printed.borrow_mut().remove(&index);
                }
                Err(message) => {
                    let new = self.printed.borrow().get(&index) != Some(&message);
                    if new {
                        vm.warn(format!(
                            "server tick hook {index} failed at tick {}: {message}",
                            tick.number
                        ));
                        self.printed.borrow_mut().insert(index, message);
                    }
                }
            }
        }
    }
}

impl TickHooks for EngineTicks {
    fn before(&self, vm: &mut Vm, tick: TickInfo) {
        if let Some(rpc) = vm.extension::<ServerRpc>() {
            rpc.pump(vm, tick.number);
        }
        self.run(TickPhase::Before, vm, tick);
    }

    fn after(&self, vm: &mut Vm, tick: TickInfo) {
        self.run(TickPhase::After, vm, tick);
        self.run(TickPhase::Resolve, vm, tick);
    }
}
