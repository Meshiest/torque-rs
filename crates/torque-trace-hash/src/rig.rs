//! A server and a client in one process, joined over the engine's in-memory datagram link and stepped by hand.
//!
//! No clock is read: every step is one 32 ms tick of the server, then one of the client, with the datagrams each sent carried
//! across between them, so a run is a function of the engine's plugins and the inputs a scenario sets.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque_app_server::plugins::MinimalPlugins;
use torque_app_server::server::{GameServer, Listen, ServerStart};
use torque_client::conn::tge::TgeConn;
use torque_client::game::ClientGame;
use torque_client::host::{ClientPass, EnginePass};
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_client::predict::FileReader;
use torque_client::state::ClientState;
use torque_engine::{Engine, EngineBuilder, Plugins};
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
}

impl Mode {
    /// Every mode, in the order the lines print.
    pub const ALL: [Mode; 1] = [Mode::Tge];

    /// The word that labels the mode's lines.
    pub const fn label(self) -> &'static str {
        match self {
            Mode::Tge => "tge",
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

/// A server of `MinimalPlugins` and the plugin under test, a client connected to it, and the link between them.
pub struct Rig {
    /// The protocol the server and the client speak.
    pub mode: Mode,
    /// The server's engine (a plugin's server resources are its).
    pub engine: Engine,
    pub server: GameServer,
    pub client_vm: Vm,
    pub client: Rc<RefCell<ClientState<TgeConn>>>,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    /// Ticks stepped so far.
    pub ticks: u32,
    /// Every datagram carried while [`Rig::record_wire`] was on, in the order carried: true when it went to the server.
    pub wire: Vec<(bool, Vec<u8>)>,
    recording: bool,
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
        let engine = EngineBuilder::new()
            .add_plugins(MinimalPlugins)
            .add_plugins(plugins)
            .build()
            .map_err(|e| e.to_string())?;
        let files = files();
        let server = GameServer::start_engine(
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

        let mut vm = Vm::new();
        torque_game::specs::install(&mut vm);
        let gui = torque_gui::register(&mut vm);
        let mem = MemoryDatagrams {
            local_port: CLIENT.port(),
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
        torque_client::conn::tge::ConnectHello::install(&mut vm, &engine);
        vm.set_step_limit(Some(50_000_000));
        setup(&mut vm)?;
        vm.advance_time(1_000);
        vm.eval(
            format!("new GameConnection(TraceConn); TraceConn.connect(\"{SERVER}\");").as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        Ok(Rig {
            mode,
            engine,
            server,
            client_vm: vm,
            client,
            sent,
            incoming,
            ticks: 0,
            wire: Vec::new(),
            recording: false,
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
        let mut incoming = self.incoming.borrow_mut();
        for (to, data) in end.take_sent() {
            if to == SocketAddr::V4(CLIENT) {
                if self.recording {
                    self.wire.push((false, data.clone()));
                }
                incoming.push_back((SERVER, data));
            }
        }
    }

    /// One tick of the server and one of the client, the link carried around each.
    pub fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        self.client_vm.advance_time(TICK_MS);
        EnginePass.net(&mut self.client_vm, &self.client);
        self.carry();
        self.ticks += 1;
    }

    /// One tick of the server, then the client's clock moved on a tick in `frames` equal steps with the client's network pass
    /// after each (it reads what arrived at every step and ticks when a tick is due, as a window's frames do), `frame` called
    /// after each step with where the frame falls between the client's last two ticks (`back`, as a window sets it: 1 right
    /// after a tick, nearer 0 as the next one comes).
    pub fn step_frames(&mut self, frames: u32, mut frame: impl FnMut(&Rig, f32)) {
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
            self.carry();
            if self.client_ticks() != before {
                since = 0;
            }
            frame(self, 1.0 - since as f32 / TICK_MS as f32);
        }
        self.ticks += 1;
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
