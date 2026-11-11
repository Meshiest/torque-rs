//! A server and a client in one process, joined over the engine's in-memory datagram link and stepped by hand.
//!
//! No clock is read: every step is one 32 ms tick of the server, then one of the client, with the datagrams each sent carried
//! across between them, so a run is a function of the engine's plugins and the inputs a scenario sets.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque_app_server::plugins::{MinimalPlugins, set_protocol};
use torque_app_server::server::{GameServer, Listen, ServerStart};
use torque_client::conn::engine_wire::EngineNonceKey;
use torque_client::conn::tge::TgeConn;
use torque_client::game::ClientGame;
use torque_client::host::{ClientPass, EnginePass};
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_client::predict::FileReader;
use torque_client::state::ClientState;
use torque_engine::{Engine, EngineBuilder, Plugins, ProtocolKind};
use torque_server::net::NetGame;
use torque_server::net::channels::ClientChannels;
use torque_server::net::engine::ensure_host;
use torque_server::net::rpc::{ClientRpc, RpcLanes, register_client_natives};
use torque_server::net::torque::TorqueNet;
use torque_ts_vm::host::{FileHost, FileRead, MemFiles};
use torque_ts_vm::vm::Vm;

/// One tick, in milliseconds.
pub const TICK_MS: u32 = 32;

/// The key of the server's challenge digests, which the rig sets so a run's datagrams repeat.
pub const CHALLENGE_KEY: [u8; 16] = [7; 16];

const SERVER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50_000);

/// The network protocol a run speaks. Each line `trace-hash` prints names it, so a protocol's hashes are pinned apart from the
/// others'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mode {
    /// Torque's own protocol.
    Tge,
    /// The engine's own protocol, over the memory link.
    Engine,
}

impl Mode {
    /// Every mode, in the order the lines print.
    pub const ALL: [Mode; 2] = [Mode::Tge, Mode::Engine];

    /// The word that labels the mode's lines.
    pub const fn label(self) -> &'static str {
        match self {
            Mode::Tge => "tge",
            Mode::Engine => "engine",
        }
    }

    /// The protocol the engines of a run in this mode speak.
    pub const fn protocol(self) -> ProtocolKind {
        match self {
            Mode::Tge => ProtocolKind::Tge,
            Mode::Engine => ProtocolKind::Engine,
        }
    }
}

/// What the connection's client controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// The figure on foot.
    Figure,
    /// The buggy.
    Buggy,
}

impl Control {
    fn script_name(self) -> &'static str {
        match self {
            Control::Figure => "figure",
            Control::Buggy => "buggy",
        }
    }
}

/// The scripts and shapes of the trace world, as the server's files.
pub fn files() -> MemFiles {
    let files = MemFiles::new();
    files.insert("server.cs", include_bytes!("game/server.cs"));
    let shape = |path: &str, model: &torque_shapes::Model| {
        files.insert(path, &torque_shapes::write::write_dts(model));
    };
    shape("kit/cube.dts", &torque_example_assets::shapes::cube());
    shape("kit/figure.dts", &torque_example_assets::shapes::figure());
    shape("kit/buggy.dts", &torque_example_assets::buggy::buggy());
    shape(
        "kit/buggy_wheel.dts",
        &torque_example_assets::buggy::wheel(),
    );
    files
}

/// The client's game: Torque's answers (no static index of its own: the client's container holds the floor, a StaticShape, as
/// TGE's does), and the server's files for the shapes the client reads.
#[derive(Debug)]
pub struct TraceClient(pub MemFiles);

impl ClientGame for TraceClient {
    fn file_reader(&self) -> Option<FileReader> {
        let files = self.0.clone();
        Some(Rc::new(move |path: &[u8]| match files.read(path) {
            FileRead::Found(bytes) => Some(bytes),
            _ => None,
        }))
    }
}

/// A client of the rig's server: its script engine, its state and its end of the memory link.
pub struct Peer {
    pub vm: Vm,
    pub client: Rc<RefCell<ClientState<TgeConn>>>,
    addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    /// The datagrams from the server waiting out the rig's latency, with the step each is due at.
    held: std::collections::VecDeque<(u32, Vec<u8>)>,
}

impl Peer {
    /// A client of `engine`'s plugins at `addr`, reading the shapes from `files`, with `setup` run on its script engine before
    /// it connects to the rig's server.
    fn connect(
        engine: &Engine,
        files: MemFiles,
        addr: SocketAddrV4,
        setup: impl FnOnce(&mut Vm) -> Result<(), String>,
    ) -> Result<Peer, String> {
        let mut vm = Vm::new();
        torque_game::specs::install(&mut vm);
        let gui = torque_gui::register(&mut vm);
        let mem = MemoryDatagrams {
            local_port: addr.port(),
            ..MemoryDatagrams::default()
        };
        let (sent, incoming) = (Rc::clone(&mem.sent), Rc::clone(&mem.incoming));
        let net = Rc::new(RefCell::new(torque_client::net::ClientNet::default()));
        {
            let mut n = net.borrow_mut();
            n.policy = NetPolicy::Loopback;
            n.socket = Some(Box::new(mem));
        }
        let client = Rc::new(RefCell::new(ClientState::<TgeConn> {
            net,
            root: ".".into(),
            ..ClientState::default()
        }));
        torque_client::conn::register(&mut vm, &client, &gui);
        // The client reads the shapes of the datablocks it is sent (a vehicle's wheel count, the floor's hulls) from the same files.
        torque_client::game::set_client_game(&mut vm, Rc::new(TraceClient(files)));
        // What the engine's plugins say in a connect request (their manifest), as a client of the same engine says it: nothing
        // for an engine that says nothing on the wire.
        torque_client::conn::tge::ConnectHello::install(&mut vm, engine);
        install_rpcs(&mut vm, engine)?;
        // The client's nonces, as the server's keys, are fixed so that a run's datagrams repeat.
        vm.set_extension(Rc::new(EngineNonceKey(CHALLENGE_KEY)));
        vm.set_step_limit(Some(50_000_000));
        setup(&mut vm)?;
        vm.advance_time(1_000);
        vm.eval(
            format!("new GameConnection(TraceConn); TraceConn.connect(\"{SERVER}\");").as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        Ok(Peer {
            vm,
            client,
            addr,
            sent,
            incoming,
            held: std::collections::VecDeque::new(),
        })
    }

    /// The client's half of a step: its clock moved on a tick and its network pass.
    fn step(&mut self) {
        self.vm.advance_time(TICK_MS);
        EnginePass.net(&mut self.vm, &self.client);
        pump_rpcs(&mut self.vm);
    }

    /// The client's server heads that corrected its prediction.
    pub fn corrections(&self) -> u64 {
        self.client
            .borrow()
            .conn
            .conn
            .as_ref()
            .map_or(0, |c| c.predict.stats.corrections)
    }

    /// Whether the client has a connection with a control object it predicts.
    pub fn predicting(&self) -> bool {
        self.client
            .borrow()
            .conn
            .conn
            .as_ref()
            .is_some_and(|c| c.predict.packet_data().is_some())
    }

    /// Sets a client move global for the next tick.
    pub fn set_global(&mut self, name: &str, value: f32) {
        self.vm.set_global_float(name.as_bytes(), f64::from(value));
    }
}

/// A server of `MinimalPlugins` and the plugin under test, a client connected to it, and the link between them. The server's
/// network host speaks net game `G` (Torque's by default).
pub struct Rig<G: NetGame = TorqueNet> {
    /// The protocol the server and the client speak.
    pub mode: Mode,
    /// The server's engine (a plugin's server resources are its).
    pub engine: Engine,
    pub server: GameServer<G>,
    pub client_vm: Vm,
    pub client: Rc<RefCell<ClientState<TgeConn>>>,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    /// The clients joined after the first ([`Rig::add_client`]), each on a port of its own, stepped after the first.
    pub peers: Vec<Peer>,
    /// Ticks stepped so far.
    pub ticks: u32,
    /// Every datagram carried while [`Rig::record_wire`] was on, in the order carried: true when it went to the server.
    pub wire: Vec<(bool, Vec<u8>)>,
    recording: bool,
    /// Steps a datagram from the server waits before the client gets it ([`Rig::set_latency`]), and those waiting, with the
    /// step each is due at.
    latency: u32,
    held: std::collections::VecDeque<(u32, Vec<u8>)>,
}

impl Rig {
    /// Starts the server over `MinimalPlugins` and `plugins` with the client controlling `control`, and has the client connect.
    /// The first steps run the handshake and the datablocks: call [`Rig::settle`] before a scripted run.
    pub fn new<M>(plugins: impl Plugins<M>, control: Control) -> Result<Rig, String> {
        Rig::in_mode(Mode::Tge, plugins, control)
    }

    /// [`Rig::new`] over the protocol `mode`.
    pub fn in_mode<M>(
        mode: Mode,
        plugins: impl Plugins<M>,
        control: Control,
    ) -> Result<Rig, String> {
        Rig::with_client(mode, plugins, control, |_| Ok(()))
    }

    /// [`Rig::in_mode`], with `setup` run on the client's script engine before it connects (a plugin's client half: its
    /// classes, which the connection reads when it starts).
    pub fn with_client<M>(
        mode: Mode,
        plugins: impl Plugins<M>,
        control: Control,
        setup: impl FnOnce(&mut Vm) -> Result<(), String>,
    ) -> Result<Rig, String> {
        Rig::with_net_game(mode, plugins, control, setup)
    }
}

impl<G: NetGame> Rig<G> {
    /// [`Rig::with_client`] with a server of net game `G`, which `plugins` set ([`NetGamePlugin`](torque_app_server::net_game::NetGamePlugin)).
    pub fn with_net_game<M>(
        mode: Mode,
        plugins: impl Plugins<M>,
        control: Control,
        setup: impl FnOnce(&mut Vm) -> Result<(), String>,
    ) -> Result<Rig<G>, String> {
        // The run's protocol whatever `TORQUE_NET_PROTOCOL` says: each line is pinned for its own mode.
        let mut builder = EngineBuilder::new();
        builder.add_plugins(MinimalPlugins).add_plugins(plugins);
        set_protocol(&mut builder, mode.protocol());
        let engine = builder.build().map_err(|e| e.to_string())?;
        let files = files();
        let server = GameServer::<G>::start_with_net(
            &engine,
            files.clone(),
            &ServerStart {
                listen: Listen::Memory(SocketAddr::V4(SERVER)),
                globals: &[("$Trace::Control", control.script_name())],
                start: "startTrace",
                args: &[],
            },
        )?;
        // The server draws the key of its challenges at random; fixed, the datagrams are a function of the inputs too.
        server.shared.borrow_mut().hs.set_key(CHALLENGE_KEY);
        if let Some(host) = ensure_host(server.vm(), &mut server.shared.borrow_mut()) {
            host.table.rekey(CHALLENGE_KEY);
        }

        let peer = Peer::connect(&engine, files, CLIENT, setup)?;
        Ok(Rig {
            mode,
            engine,
            server,
            client_vm: peer.vm,
            client: peer.client,
            sent: peer.sent,
            incoming: peer.incoming,
            peers: Vec::new(),
            ticks: 0,
            wire: Vec::new(),
            recording: false,
            latency: 0,
            held: std::collections::VecDeque::new(),
        })
    }

    /// Keeps a copy of every datagram carried from now on in [`Rig::wire`].
    pub fn record_wire(&mut self) {
        self.recording = true;
    }

    /// Carries what each side sent to the other.
    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SocketAddr::V4(SERVER) {
                if self.recording {
                    self.wire.push((true, data.clone()));
                }
                end.inject(SocketAddr::V4(CLIENT), &data);
            }
        }
        for peer in &self.peers {
            for (to, data) in peer.sent.borrow_mut().drain(..) {
                if to == SERVER {
                    end.inject(SocketAddr::V4(peer.addr), &data);
                }
            }
        }
        let mut incoming = self.incoming.borrow_mut();
        let (ticks, latency) = (self.ticks, self.latency);
        for (to, data) in end.take_sent() {
            if let Some(peer) = self.peers.iter_mut().find(|p| SocketAddr::V4(p.addr) == to) {
                peer.held.push_back((ticks + latency, data));
                continue;
            }
            if to == SocketAddr::V4(CLIENT) {
                if self.recording {
                    self.wire.push((false, data.clone()));
                }
                if self.latency == 0 {
                    incoming.push_back((SERVER, data));
                } else {
                    self.held.push_back((self.ticks + self.latency, data));
                }
            }
        }
        while let Some((due, _)) = self.held.front()
            && *due <= self.ticks
        {
            let (_, data) = self.held.pop_front().expect("the front");
            incoming.push_back((SERVER, data));
        }
        for peer in &mut self.peers {
            while let Some((due, _)) = peer.held.front()
                && *due <= ticks
            {
                let (_, data) = peer.held.pop_front().expect("the front");
                peer.incoming.borrow_mut().push_back((SERVER, data));
            }
        }
    }

    /// Holds each datagram from the server for `steps` steps before a client gets it (a link with latency: the client's moves
    /// pile up unacknowledged, and a correction replays them), for every client. 0, the default, carries every datagram at
    /// once.
    pub fn set_latency(&mut self, steps: u32) {
        self.latency = steps;
    }

    /// One tick of the server and one of the client, the link carried around each.
    pub fn step(&mut self) {
        self.step_server();
        self.step_client();
    }

    /// The server's half of [`Rig::step`]: its tick, the link carried around it.
    pub fn step_server(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
    }

    /// The client's half of [`Rig::step`]: its clock moved on a tick and its network pass (reading what arrived, its tick, its
    /// prediction), the link carried after it.
    pub fn step_client(&mut self) {
        self.client_vm.advance_time(TICK_MS);
        EnginePass.net(&mut self.client_vm, &self.client);
        self.pump_rpcs();
        for peer in &mut self.peers {
            peer.step();
        }
        self.carry();
        self.ticks += 1;
    }

    /// Connects another client (the next port up) and steps until it predicts a control object, at most `limit` ticks. The
    /// server's `onDataBlocksDone` gives it its control object (`$Trace::Next`, when set).
    ///
    /// # Errors
    /// The client could not start, or never predicted in `limit` ticks.
    pub fn add_client(&mut self, limit: u32) -> Result<usize, String> {
        let port = CLIENT.port() + 1 + self.peers.len() as u16;
        let addr = SocketAddrV4::new(*CLIENT.ip(), port);
        let peer = Peer::connect(&self.engine, files(), addr, |_| Ok(()))?;
        self.peers.push(peer);
        let index = self.peers.len() - 1;
        for _ in 0..limit {
            self.step();
            if self.peers[index].predicting() {
                return Ok(index);
            }
        }
        Err(format!(
            "client {} never predicted its control object in {limit} ticks",
            index + 1
        ))
    }

    /// One tick of the server, then the client's clock moved on a tick in `frames` equal steps with the client's network pass
    /// after each (it reads what arrived at every step and ticks when a tick is due, as a window's frames do), `frame` called
    /// after each step with where the frame falls between the client's last two ticks (`back`, as a window sets it: 1 right
    /// after a tick, nearer 0 as the next one comes).
    pub fn step_frames(&mut self, frames: u32, mut frame: impl FnMut(&Self, f32)) {
        let frames = frames.clamp(1, TICK_MS);
        self.carry();
        self.server.tick();
        self.carry();
        let mut since = 0;
        for k in 1..=frames {
            let to = TICK_MS * k / frames;
            self.client_vm.advance_time(to - since);
            since = to;
            let before = self.client_ticks();
            EnginePass.net(&mut self.client_vm, &self.client);
            self.pump_rpcs();
            self.carry();
            if self.client_ticks() != before {
                since = 0;
            }
            frame(self, 1.0 - since as f32 / TICK_MS as f32);
        }
        self.ticks += 1;
    }

    /// The client's RPCs, as a window client's tick pumps them after its network pass.
    fn pump_rpcs(&mut self) {
        pump_rpcs(&mut self.client_vm);
    }

    /// The client's ticks so far (its last tick's time).
    fn client_ticks(&self) -> Option<u32> {
        self.client
            .borrow()
            .conn
            .conn
            .as_ref()
            .and_then(|c| c.last_tick)
    }

    /// Steps until the client is connected and has a predicted control object, at most `limit` ticks; false when it never is.
    pub fn settle(&mut self, limit: u32) -> bool {
        for _ in 0..limit {
            self.step();
            if self.predicting() {
                return true;
            }
        }
        false
    }

    /// Whether the client has a connection with a control object it predicts.
    pub fn predicting(&self) -> bool {
        self.client
            .borrow()
            .conn
            .conn
            .as_ref()
            .is_some_and(|c| c.predict.packet_data().is_some())
    }

    /// Sets a client move global (`$mvForwardAction`, `$mvYaw`, ...) for the next tick.
    pub fn set_global(&mut self, name: &str, value: f32) {
        self.client_vm
            .set_global_float(name.as_bytes(), f64::from(value));
    }
}

/// Runs the client RPCs of `vm` (a client's) due at its tick.
fn pump_rpcs(vm: &mut Vm) {
    if let Some(rpc) = vm.extension::<ClientRpc>() {
        let tick = u64::from(vm.sim_time()) / u64::from(TICK_MS);
        rpc.pump(vm, tick);
    }
}

/// What a window client installs for the engine's channels (in either protocol) and, in the engine's protocol, its RPCs over
/// the session's lanes (the script commands ride two of them).
fn install_rpcs(vm: &mut Vm, engine: &Engine) -> Result<(), String> {
    // The channels the engine registers are read from every data packet the server's gate turned them on for, in either
    // protocol.
    let channels = engine.net_channels().map_err(|e| e.to_string())?;
    if channels.is_empty() {
        return Ok(());
    }
    let channels = Rc::new(ClientChannels::new(&channels));
    vm.set_extension(Rc::clone(&channels));
    let rpcs = engine.net_rpcs().map_err(|e| e.to_string())?;
    if engine.protocol() != ProtocolKind::Engine || rpcs.is_empty() {
        return Ok(());
    }
    let lanes = Rc::new(RpcLanes::default());
    vm.set_extension(Rc::clone(&lanes));
    let rpc = Rc::new(ClientRpc::over_lanes(rpcs, channels, lanes));
    register_client_natives(vm, &rpc);
    vm.set_extension(rpc);
    Ok(())
}
