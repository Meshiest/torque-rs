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
use torque_engine::{EngineBuilder, Plugins};
use torque_ts_vm::host::{FileHost, FileRead, MemFiles};
use torque_ts_vm::vm::Vm;

/// One tick, in milliseconds.
pub const TICK_MS: u32 = 32;

const SERVER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50_000);

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
fn files() -> MemFiles {
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

/// The client's game: Torque's answers, and the server's files for the shapes the client reads.
#[derive(Debug)]
struct TraceClient(MemFiles);

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
    pub server: GameServer,
    pub client_vm: Vm,
    pub client: Rc<RefCell<ClientState<TgeConn>>>,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    /// Ticks stepped so far.
    pub ticks: u32,
}

impl Rig {
    /// Starts the server over `MinimalPlugins` and `plugins` with the client controlling `control`, and has the client connect.
    /// The first steps run the handshake and the datablocks: call [`Rig::settle`] before a scripted run.
    pub fn new<M>(plugins: impl Plugins<M>, control: Control) -> Result<Rig, String> {
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
        vm.set_step_limit(Some(50_000_000));
        vm.advance_time(1_000);
        vm.eval(
            format!("new GameConnection(TraceConn); TraceConn.connect(\"{SERVER}\");").as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        Ok(Rig {
            server,
            client_vm: vm,
            client,
            sent,
            incoming,
            ticks: 0,
        })
    }

    /// Carries what each side sent to the other.
    fn carry(&self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SocketAddr::V4(SERVER) {
                end.inject(SocketAddr::V4(CLIENT), &data);
            }
        }
        let mut incoming = self.incoming.borrow_mut();
        for (to, data) in end.take_sent() {
            if to == SocketAddr::V4(CLIENT) {
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
