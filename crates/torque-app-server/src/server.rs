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
use std::fmt;
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
use torque_net::modsync::RequiredMods;
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
pub use torque_server::server::AttachError;
use torque_server::server::{
    GameServer as ServerHost, MissionError, RunEnd, Server, TICK_MS, TickHooks,
};
use torque_server::state::ServerState;
use torque_ts_vm::host::{FileHost, MemFiles};
use torque_ts_vm::vm::Vm;

use crate::net_game::{AttachedCell, NetAttached, apply_codecs, factory_for};
use crate::plugins::{MinimalPlugins, NO_NET_HOST, ServerWiring};
use crate::slots::{Connections, FileSetter, ServerEntry, file_setter};
pub use crate::transports::ServerLink;
pub use crate::transports::{BoundTransport, CustomListen};
use crate::transports::{open_listen, server_transport_warnings};

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
    /// The QUIC link on the same port number beside the web instead of plain UDP (never both on one port): desktop players
    /// over QUIC and, with its WebTransport sessions on, browsers over WebTransport, the page served by the web side
    /// carrying what they need (`webtransport.json`: the QUIC port number, the path and the certificate hashes, kept current as the
    /// certificates rotate).
    #[cfg(feature = "quic")]
    pub quic: Option<Rc<QuicListen>>,
    /// The files served (none: game sockets only, the page comes from another web server), TLS, and the limits.
    pub setup: torque_server::net::web::WebSetup,
}

/// The QUIC side of a server that desktop players join over the QUIC link (`Listen::Quic`), and browsers over WebTransport
/// sessions on the same endpoint when `web_transport` says so.
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
#[derive(Debug, Clone)]
pub struct QuicListen {
    /// The server's identity key.
    pub identity: IdentitySource,
    /// The endpoint's caps and timeouts.
    pub limits: torque_net_quic::QuicLimits,
    /// WebTransport sessions on the endpoint (none: the raw protocol alone): which pages may open one, the path, and the
    /// certificates browsers pin (rotating short ones by default).
    pub web_transport: Option<torque_net_quic::WebTransportSetup>,
}

#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
impl Default for QuicListen {
    fn default() -> QuicListen {
        QuicListen {
            identity: IdentitySource::Default,
            limits: torque_net_quic::QuicLimits::default(),
            web_transport: None,
        }
    }
}

/// Where a QUIC server's identity key comes from.
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
#[derive(Debug, Clone)]
pub enum IdentitySource {
    /// `identity.key` in the server's data folder ([`identity_path`]), made on first start.
    Default,
    /// This file, made on first start.
    File(std::path::PathBuf),
    /// A key already loaded (a test's, or a host that keeps it elsewhere).
    Key(Rc<torque_net_quic::ServerKey>),
}

/// Where the default identity key lives: `identity.key` in the server's folder under the user's data folder (the folder the
/// upload staging is under). None when the system gives no data folder.
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
pub fn identity_path() -> Option<std::path::PathBuf> {
    crate::uploads::data_dir()
        .map(|data| data.join("torque-rs").join(torque_net_quic::IDENTITY_FILE))
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
    /// HTTP and WebSocket on TCP `address`:`port` (0 takes any free one), and UDP on the same port when `web.udp` says so
    /// (or the QUIC link, `web.quic`). Without TLS the address must be a private one; `public` allows `0.0.0.0` as for UDP.
    #[cfg(all(feature = "web", not(target_arch = "wasm32")))]
    Web {
        address: Ipv4Addr,
        port: u16,
        public: bool,
        web: Rc<WebListen>,
    },
    /// The QUIC link on UDP `address`:`port` (0 takes any free one): desktop players, encrypted and address-validated by
    /// QUIC, the server known by the hash of its key (`quic.identity`). `public` allows `0.0.0.0` as for UDP; a public
    /// address is fine here, since the link brings its own security, where plain engine UDP is refused.
    #[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
    Quic {
        address: Ipv4Addr,
        port: u16,
        public: bool,
        quic: Rc<QuicListen>,
    },
    /// A transport the caller made and bound (a player's host found through a room service, torque-server's
    /// `net::web::p2p`, id `rooms`; a browser host's worker link, id `relay`).
    Bound(BoundTransport),
    /// A transport a plugin registered under its own id ([`Listen::custom`]), with the request it reads.
    Custom(CustomListen),
    /// No transport and no network host yet: the server starts without the host's natives, behaviors and passes, and
    /// [`GameServer::attach_net`] gives it both later. Its start must add no datablock (their keys start with the host).
    Late,
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

/// The shared state of a network host of net game `G`.
pub type SharedNet<G> = Rc<RefCell<NetShared<G>>>;

/// A game's server, whose network host speaks net game `G` (Torque's by default).
pub struct GameServer<G: NetGame = TorqueNet> {
    pub server: NetServer<G>,
    pub game: GameParts,
    /// The network host's shared state; none until [`GameServer::attach_net`] on a server started on [`Listen::Late`].
    shared: Option<SharedNet<G>>,
    transport: Option<SharedTransport>,
    end: Option<MemoryEnd>,
    /// What [`GameServer::attach_net`] needs of the start: some on a server started on [`Listen::Late`] until then.
    late: Option<LateHost<G>>,
    /// How often a dedicated server prints its transport's status lines ([`GameServer::set_status_every`]).
    status_every: Option<Duration>,
}

/// What a server started on [`Listen::Late`] keeps for its network host: the engine that made it, its wiring and the cell
/// the host's recipe leaves the host in.
struct LateHost<G: NetGame> {
    engine: Engine,
    wiring: Rc<ServerWiring>,
    attached: AttachedCell<G>,
}

/// Why [`GameServer::attach_net`] gave the server no network host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachNetError {
    /// The server's rule for a host refused it: one is attached already (a server started on any other [`Listen`] has its
    /// own), a datablock exists, or a real socket was given without the system clock.
    Refused(AttachError),
    /// In the engine's protocol, a real plain socket bound outside the private addresses (the protocol takes plain UDP for
    /// LAN and development only).
    PlainEngineBind(PlainEngineRefusal),
    /// The host could not be made: the engine has no `NetHostPlugin`, or the net game's factory or a part of the host failed.
    Failed(String),
}

impl fmt::Display for AttachNetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttachNetError::Refused(e) => write!(f, "the network host was refused: {e}"),
            AttachNetError::PlainEngineBind(e) => write!(f, "the network host was refused: {e}"),
            AttachNetError::Failed(e) => write!(f, "the network host: {e}"),
        }
    }
}

impl std::error::Error for AttachNetError {}

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

    /// [`GameServer::start_with_net`] over a file host of the game's own, as [`GameServer::start_engine_with_files`] is
    /// [`GameServer::start_engine`] over one.
    ///
    /// # Errors
    /// As [`GameServer::start_with_net`].
    pub fn start_with_net_files(
        engine: &Engine,
        files: impl FileHost + 'static,
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
        let link = match &start.listen {
            Listen::Late => None,
            listen => Some(open_listen(engine, listen)?),
        };

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
            link.as_ref().map(|link| Rc::clone(&link.transport)),
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
        let shared = match &link {
            Some(link) => {
                apply_codecs(engine, &mut server.vm);
                replay(engine, ServerStage::Net, &mut server.vm)?;
                let net = attached.borrow_mut().take().ok_or(NO_NET_HOST)?;
                let memory = matches!(start.listen, Listen::Memory(_));
                Some(wire_host(
                    engine,
                    &mut server,
                    net,
                    &link.transport,
                    memory,
                )?)
            }
            // `NetHostPlugin`'s recipe leaves the host waiting for `attach_net`, which sets the codecs then.
            None => {
                replay(engine, ServerStage::Net, &mut server.vm)?;
                None
            }
        };
        let has_rpcs = !engine.net_rpcs().map_err(|e| e.to_string())?.is_empty();
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
        let late = link.is_none().then(|| LateHost {
            engine: engine.clone(),
            wiring,
            attached,
        });
        let (transport, end) = link.map_or((None, None), |link| (Some(link.transport), link.end));
        Ok(GameServer {
            server,
            game,
            shared,
            transport,
            end,
            late,
            status_every: None,
        })
    }

    /// Gives a server started on [`Listen::Late`] its network host over `link` (a [`SharedTransport`], or a [`ServerLink`]
    /// with the other end of an in-memory one, which [`GameServer::memory_end`] then returns): the codecs slot's codecs are
    /// set, the net game's factory runs, and the host's natives, behaviors and parts are attached in the order a server
    /// started on a transport attaches them, its passes last. From then on the server receives, sends and takes connections
    /// as its scripts allow: the attach leaves `allowConnections` as the start left it. The required mods the start loaded
    /// are the connect gate's.
    ///
    /// # Errors
    /// [`AttachNetError::Refused`] when a host is attached already (every server not started on [`Listen::Late`] has one), a
    /// datablock exists (attach before the scripts that add them), or `link` is a real socket and the scripts do not read the
    /// system clock (`server.use_system_clock()`). [`AttachNetError::PlainEngineBind`] in the engine's protocol when `link` is
    /// a real, plain (not validated) socket bound outside the private addresses, as a start on [`Listen::Udp`] refuses it.
    /// [`AttachNetError::Failed`] when the engine has no `NetHostPlugin` or a part of the host fails. On any error the passes
    /// are not attached, so the server reads and sends nothing; the natives and extensions an attach that failed part way
    /// set stay until a later call, which makes the host again from the factory up.
    pub fn attach_net(&mut self, link: impl Into<ServerLink>) -> Result<(), AttachNetError> {
        let ServerLink { transport, end } = link.into();
        let Some(late) = self.late.as_ref() else {
            return Err(AttachNetError::Refused(AttachError::AlreadyAttached));
        };
        let (real, bound, validated) = {
            let t = transport.borrow();
            let bound = t.local_addr();
            let validated = bound.is_some_and(|at| t.link_kind(at).facts.validated);
            (t.is_real(), bound, validated)
        };
        self.server
            .can_attach_net(real)
            .map_err(AttachNetError::Refused)?;
        if late.engine.protocol() == ProtocolKind::Engine
            && real
            && !validated
            && let Some(bound) = bound
        {
            check_plain_engine_bind(&LanRules::PLAIN, bound.ip())
                .map_err(AttachNetError::PlainEngineBind)?;
        }
        let allow = self.server.state.borrow().allow_connections;
        let made = make_late_host(late, &mut self.server, &transport, end.is_some());
        self.server.state.borrow_mut().allow_connections = allow;
        let shared = made?;
        // The start loaded the mods before the gate was made, so the gate takes their list now.
        let vm = &self.server.vm;
        if let Some(required) = vm.extension::<RequiredMods>()
            && let Some(mods) = vm.extension::<torque_mods::Mods>()
        {
            required.set(mods.required());
        }
        self.late = None;
        self.shared = Some(shared);
        self.transport = Some(transport);
        self.end = end;
        Ok(())
    }

    /// Loads `mission` with the server's mission loader (the engine's [`ServerSlots::mission_loader`](crate::slots::ServerSlots::mission_loader),
    /// else TGE's: the `.mis` is `exec`'d); the answer is the loader's own report.
    pub fn load_mission(&mut self, mission: &str) -> Result<Box<dyn Any>, MissionError> {
        self.server.load_mission(mission.as_bytes())
    }

    /// Where the socket is bound (none before [`GameServer::attach_net`] on a server started on [`Listen::Late`]).
    pub fn address(&self) -> Option<SocketAddr> {
        self.transport.as_ref()?.borrow().local_addr()
    }

    /// The network host's shared state: the connections, the handshake, the ghosts. None until [`GameServer::attach_net`]
    /// on a server started on [`Listen::Late`]; every other server has it from the start.
    pub fn shared(&self) -> Option<&SharedNet<G>> {
        self.shared.as_ref()
    }

    /// Tells the server the address clients reach it by (its public address), which its auth provider binds tickets to: a socket
    /// bound to every interface, a NAT or a relay hides it. A server that is never told one refuses every ticket of a provider
    /// that binds to the address. A server with no host yet ([`Listen::Late`]) is told once [`GameServer::attach_net`]
    /// has set the bound address.
    pub fn set_public_address(&self, address: Option<SocketAddr>) {
        if let Some(shared) = &self.shared {
            shared.borrow_mut().set_public_address(address);
        }
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
        if let Some(transport) = &self.transport {
            transport.borrow_mut().close();
        }
    }

    /// Has a dedicated server print its transport's status lines ([`Transport::status`](torque_server::net::transport::Transport::status): a web transport's data channels)
    /// every `every` (never with none, the default).
    pub fn set_status_every(&mut self, every: Option<Duration>) {
        self.status_every = every;
    }

    /// The transport's status lines since the last call ([`Transport::status`](torque_server::net::transport::Transport::status)).
    pub fn transport_status(&mut self) -> Vec<String> {
        self.transport
            .as_ref()
            .map_or_else(Vec::new, |transport| transport.borrow_mut().status())
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
        let Some(transport) = self
            .transport
            .as_ref()
            .filter(|transport| transport.borrow().wants_service())
        else {
            std::thread::sleep(wait);
            return;
        };
        let until = Instant::now() + wait;
        loop {
            transport.borrow_mut().service();
            let now = Instant::now();
            let Some(left) = until.checked_duration_since(now) else {
                break;
            };
            std::thread::sleep(left.min(SERVICE_EVERY));
        }
    }

    /// The transport's work between ticks, done now ([`Transport::service`](torque_server::net::transport::Transport::service); a harness that paces ticks itself calls it).
    pub fn service(&mut self) {
        if let Some(transport) = &self.transport {
            transport.borrow_mut().service();
        }
    }
}

impl<G: NetGame> AsMut<NetServer<G>> for GameServer<G> {
    fn as_mut(&mut self) -> &mut NetServer<G> {
        &mut self.server
    }
}

/// A game's server as the server host of a client that runs over it (`torque_app::client::ClientBuilder::over_server`):
/// the client's work runs before each of its ticks, which are [`GameServer::tick`]'s.
impl<G: NetGame> ServerHost for GameServer<G> {
    fn vm(&self) -> &Vm {
        &self.server.vm
    }

    fn vm_mut(&mut self) -> &mut Vm {
        &mut self.server.vm
    }

    fn run_ticks(&mut self, limit_ms: u32, until: impl FnMut(&mut Self) -> bool) -> RunEnd {
        NetServer::<G>::run_on(self, limit_ms, until)
    }
}

/// The network host of a server started on [`Listen::Late`], made over `transport` as [`GameServer::attach_net`] makes it.
fn make_late_host<G: NetGame>(
    late: &LateHost<G>,
    server: &mut NetServer<G>,
    transport: &SharedTransport,
    memory: bool,
) -> Result<SharedNet<G>, AttachNetError> {
    let failed = AttachNetError::Failed;
    apply_codecs(&late.engine, &mut server.vm);
    late.wiring
        .attach_late(&mut server.vm, Rc::clone(transport))
        .map_err(failed)?;
    let net = late
        .attached
        .borrow_mut()
        .take()
        .ok_or_else(|| failed(NO_NET_HOST.to_string()))?;
    wire_host(&late.engine, server, net, transport, memory).map_err(failed)
}

/// Wires the network host the `Net` stage made (`net`) into `server`, over `transport` (`memory`: the in-memory link, whose
/// bound address is the one its clients dial): the connect gate, the engine protocol's digests, the public address, the bulk
/// channels, the permissions, the RPCs, the server commands and, last, the host's passes, so a step that fails leaves a
/// server that reads and sends nothing. What [`GameServer::start_engine`] does after the `Net` stage and
/// [`GameServer::attach_net`] after its host's attach, in the same order.
fn wire_host<G: NetGame>(
    engine: &Engine,
    server: &mut NetServer<G>,
    net: NetAttached<G>,
    transport: &SharedTransport,
    memory: bool,
) -> Result<SharedNet<G>, String> {
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
        && let Some(warning) = sign_in_address_warning(bound, memory)
    {
        server.vm.warn(warning);
    }
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
    // The passes go on last: until they do, the transport is never read.
    server
        .attach_net(net.passes)
        .map_err(|e| format!("the network host: {e:?}"))?;
    Ok(shared)
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
