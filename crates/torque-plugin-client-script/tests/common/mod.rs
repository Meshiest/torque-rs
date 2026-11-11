//! The tests' client and server, joined by the RPC lanes (no network): a client script engine with a GUI, a `ClientRpc` and a
//! `Realms`, a server script engine, and one server-side `RpcLink` standing for the client's connection.
//! tests/app (`torque-test-app`) also compiles this file through `#[path]`: a new dev-dependency here must be added there too.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use torque_client_script_format::{Grants, Tier, Value, Writer, abi, op};
use torque_client_script_host::{Entry, GrantSet, InstanceLimits, Outcome, Reply, Runner, Trap};
use torque_engine::net::ConnectTarget;
use torque_engine::permissions::NoPermissions;
use torque_engine::rpc::{
    CarrierFull, Delivery, NoGhosts, NoHooks, RpcCarrier, RpcEnv, RpcLink, Side,
};
use torque_engine::{Engine, EngineBuilder, Plugin, PluginId, ServerStage};
use torque_ghosts::codec::NetWidths;
use torque_gui::Gui;
use torque_gui::canvas::create_canvas;
use torque_mods::CacheOwner;
use torque_net::handshake::ConnId;
use torque_plugin_client_script::consent::{ConsentAnswer, ConsentPrompt, ConsentRequest};
use torque_plugin_client_script::messages::OfferedMod;
use torque_plugin_client_script::{
    Choices, ClientScriptsPlugin, OfferSource, Realms, RunnerFactory, Setting,
};
use torque_server::net::channels::ClientChannels;
use torque_server::net::rpc::{ClientRpc, RpcLanes};
use torque_ts_vm::vm::Vm;

/// The number of cases a fuzz test runs: `full` when `TORQUE_CLIENT_SCRIPT_FUZZ` is set (the justfile's
/// `client-script-check` sets it), else `quick`.
pub fn fuzz_cases(quick: u32, full: u32) -> u32 {
    if std::env::var_os("TORQUE_CLIENT_SCRIPT_FUZZ").is_some() {
        full
    } else {
        quick
    }
}

/// A small xorshift generator, so a fuzz run is a function of its seed.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// One of `n` choices.
    pub fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 11) % n.max(1)
    }
}

pub const CLIENT: ConnId = ConnId(1);
pub const FRAME: Duration = Duration::from_micros(16_667);
pub const ALL: [Tier; 9] = [
    Tier::Ui,
    Tier::Net,
    Tier::State,
    Tier::Draw,
    Tier::Effects,
    Tier::Input,
    Tier::Camera,
    Tier::Local,
    Tier::Predict,
];

/// A folder under the build's own temporary folder for a test's files, empty.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a test folder");
    dir
}

pub fn address() -> SocketAddr {
    SocketAddr::from(([203, 0, 113, 9], 28000))
}

pub fn server_key() -> CacheOwner {
    CacheOwner::of_server(address())
}

/// RPCs the server marks for the realm, beside ones it does not.
#[derive(torque_engine::NetMessage, Debug, Clone, PartialEq, Default)]
pub struct Spawn {
    #[net(max_len = 32)]
    pub data: String,
    #[net(finite)]
    pub at: [f32; 3],
}

#[derive(torque_engine::NetMessage, Debug, Clone, PartialEq, Default)]
pub struct Spawned {
    #[net(max_len = 40)]
    pub text: String,
}

#[derive(torque_engine::NetMessage, Debug, Clone, PartialEq, Default)]
pub struct Note {
    #[net(max_len = 32)]
    pub text: String,
}

#[derive(PluginId)]
#[plugin(id = "test.game")]
pub struct Game {
    pub spawns: Rc<RefCell<Vec<Spawn>>>,
}

impl Plugin for Game {
    fn build(&self, engine: &mut EngineBuilder) {
        let spawns = Rc::clone(&self.spawns);
        engine
            .net()
            .rpc::<Spawn, Spawned>("props.spawn")
            .to_server()
            .realm()
            .rate(4, 8)
            .server(move |_cx, req: Spawn| {
                spawns.borrow_mut().push(req.clone());
                Ok(Spawned { text: req.data })
            });
        engine
            .net()
            .rpc::<Spawn, Spawned>("props.delete")
            .to_server()
            .server(|_cx, req: Spawn| Ok(Spawned { text: req.data }));
        engine
            .net()
            .rpc::<Note, ()>("hud.note")
            .to_client()
            .realm()
            .rate(64, 128)
            .client(|_cx, _req: Note| Ok(()));
        engine
            .net()
            .rpc::<Note, ()>("hud.secret")
            .to_client()
            .rate(64, 128)
            .client(|_cx, _req: Note| Ok(()));
        torque_gui::client_script::add_controls(engine);
    }
}

/// A server-side carrier that keeps what the link hands it.
#[derive(Default)]
pub struct Tap(pub RefCell<Vec<Vec<u8>>>);

impl RpcCarrier for Tap {
    fn unacked(&self) -> usize {
        0
    }

    fn send(&self, batch: Vec<u8>, _delivery: Delivery) -> Result<(), CarrierFull> {
        self.0.borrow_mut().push(batch);
        Ok(())
    }
}

/// A prompt a test answers itself.
#[derive(Default)]
pub struct Asked {
    pub requests: RefCell<Vec<ConsentRequest>>,
    pub answer: RefCell<Option<ConsentAnswer>>,
    pub cancelled: Cell<u32>,
}

pub struct TestPrompt(pub Rc<Asked>);

impl ConsentPrompt for TestPrompt {
    fn ask(&mut self, _vm: &mut Vm, _gui: &Gui, request: &ConsentRequest) {
        self.0.requests.borrow_mut().push(request.clone());
    }

    fn poll(&mut self, _vm: &mut Vm, _gui: &Gui) -> Option<ConsentAnswer> {
        self.0.answer.borrow_mut().take()
    }

    fn cancel(&mut self, _vm: &mut Vm, _gui: &Gui) {
        self.0.cancelled.set(self.0.cancelled.get() + 1);
    }
}

/// Where a server's files are, and how many times they were read.
#[derive(Default)]
pub struct Files {
    pub here: Cell<bool>,
    pub reads: Cell<u32>,
}

pub struct TestSource(pub Rc<Files>);

impl OfferSource for TestSource {
    fn ready(&self, _vm: &Vm, _server: CacheOwner, _offered: &OfferedMod) -> bool {
        self.0.here.get()
    }

    fn content(
        &self,
        _vm: &Vm,
        _server: CacheOwner,
        offered: &OfferedMod,
    ) -> Option<Vec<(String, Arc<[u8]>)>> {
        self.0.reads.set(self.0.reads.get() + 1);
        let entry: Arc<[u8]> = Arc::from(&b"-- entry"[..]);
        Some(vec![(
            format!("mods/{}/{}", offered.id, offered.entry),
            entry,
        )])
    }
}

/// The flushes a scripted guest hands back, in order, one for each `Run` and then each `Frame`.
#[derive(Default)]
pub struct Script {
    pub flushes: RefCell<VecDeque<Vec<u8>>>,
    pub instances: Cell<u32>,
    pub events: RefCell<Vec<Vec<u8>>>,
}

pub struct Guest {
    script: Rc<Script>,
    out: Option<Outcome>,
}

impl Runner for Guest {
    fn start(&mut self, entry: Entry) {
        let ops = match &entry {
            Entry::Run { .. } | Entry::Frame { .. } => {
                if let Entry::Frame { events, .. } = &entry {
                    self.script.events.borrow_mut().push(events.clone());
                }
                self.script
                    .flushes
                    .borrow_mut()
                    .pop_front()
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        };
        self.out = Some(Ok(Reply {
            status: abi::OK,
            ops,
            pending: 0,
            elapsed: Duration::ZERO,
            long: false,
        }));
    }

    fn poll(&mut self) -> Option<Outcome> {
        self.out.take()
    }
}

pub struct GuestFactory(pub Rc<Script>);

impl RunnerFactory for GuestFactory {
    fn open(&self, _limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap> {
        self.0.instances.set(self.0.instances.get() + 1);
        Ok(Box::new(Guest {
            script: Rc::clone(&self.0),
            out: None,
        }))
    }
}

/// A flush of one `net.send` of `value`.
pub fn send_flush(value: &Value) -> Vec<u8> {
    let mut out = Writer::new();
    let mark = out.begin(Tier::Net.id(), op::net::SEND);
    value.encode(&mut out).expect("a value");
    out.end(mark).expect("fits");
    out.into_bytes()
}

/// A flush of one `net.call`.
pub fn call_flush(id: u32, name: &str, args: &Value) -> Vec<u8> {
    let mut out = Writer::new();
    let mark = out.begin(Tier::Net.id(), op::net::CALL);
    out.u32(id);
    out.text(name);
    args.encode(&mut out).expect("a value");
    out.end(mark).expect("fits");
    out.into_bytes()
}

pub fn table(pairs: &[(&str, Value)]) -> Value {
    Value::Table(
        pairs
            .iter()
            .map(|(k, v)| (Value::String(k.as_bytes().to_vec()), v.clone()))
            .collect(),
    )
}

pub fn text(s: &str) -> Value {
    Value::String(s.as_bytes().to_vec())
}

pub struct World {
    pub engine: Engine,
    pub vm: Vm,
    pub gui: Gui,
    pub rpc: Rc<ClientRpc>,
    pub channels: Rc<ClientChannels>,
    pub lanes: Rc<RpcLanes>,
    pub realms: Realms,
    pub server: RpcLink,
    pub server_tap: Rc<Tap>,
    pub server_vm: Vm,
    pub asked: Rc<Asked>,
    pub files: Rc<Files>,
    pub script: Rc<Script>,
    pub spawns: Rc<RefCell<Vec<Spawn>>>,
    pub got: Rc<RefCell<Vec<(ConnId, Value)>>>,
    pub tick: u64,
    pub now_us: u64,
}

impl World {
    /// A client over the lanes with `Realms` in the given setting, offering all of `offerable`, and the grants unreviewed so
    /// that `ui` and `net` run.
    pub fn new(setting: Setting, offerable: Grants, choices: Choices) -> World {
        World::with_layout(setting, offerable, choices, false)
    }

    /// [`World::new`], with the layout plugin that lays a realm's containers out when `layout`.
    pub fn with_layout(
        setting: Setting,
        offerable: Grants,
        choices: Choices,
        layout: bool,
    ) -> World {
        let spawns = Rc::new(RefCell::new(Vec::new()));
        let got = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&got);
        let mut builder = EngineBuilder::new();
        builder.add_plugins((
            ClientScriptsPlugin::new().on_message(move |_vm, conn, value| {
                sink.borrow_mut().push((conn, value.clone()));
            }),
            Game {
                spawns: Rc::clone(&spawns),
            },
        ));
        if layout {
            builder.add_plugins(torque_plugin_gui_layout::GuiLayoutPlugin);
        }
        let engine = builder.build().expect("the engine builds");
        let table = engine.net_rpcs().expect("the table");
        let registry = engine.net_channels().expect("the channels");
        let channels = Rc::new(ClientChannels::new(&registry));
        channels.set_ghost_bits(8);
        channels.set_active(true);
        let lanes = Rc::new(RpcLanes::default());
        let rpc = Rc::new(ClientRpc::over_lanes(
            table.clone(),
            Rc::clone(&channels),
            Rc::clone(&lanes),
        ));
        let mut vm = Vm::new();
        let gui = torque_gui::register(&mut vm);
        gui.add_controls(engine.list::<torque_gui::custom::ControlEntry>());
        gui.add_realm_layouts(engine.list::<torque_gui::client_script::RealmLayoutEntry>());
        engine
            .replay(torque_engine::script::CLIENT_SCRIPT_STAGE, &mut vm)
            .expect("the classes register");
        for stage in torque_engine::ClientStage::ALL {
            let _ = engine.replay(*stage, &mut vm);
        }
        gui.force_canvas_size(Some((640, 480)));
        assert!(create_canvas(&mut vm, &gui).expect("the canvas"));
        vm.eval(
            b"new GuiControl(PlayGui) { position = \"0 0\"; extent = \"640 480\"; };\
              Canvas.setContent(PlayGui);",
        )
        .expect("the game's screen");
        gui.set_client_script_screens(&["PlayGui"]);
        vm.set_extension(Rc::new(ConnectTarget(address())));
        vm.set_extension(Rc::clone(&rpc));
        let asked = Rc::new(Asked::default());
        let files = Rc::new(Files::default());
        let script = Rc::new(Script::default());
        let mut realms = Realms::new(choices, Some(Box::new(GuestFactory(Rc::clone(&script)))));
        realms.setting = setting;
        realms.offerable = offerable;
        realms.set_prompt(Box::new(TestPrompt(Rc::clone(&asked))));
        realms.set_source(Box::new(TestSource(Rc::clone(&files))));
        realms.set_grant_set(|_, allowed| GrantSet::unreviewed(allowed));
        let server_tap = Rc::new(Tap::default());
        let mut server = RpcLink::new(
            table,
            Side::Server,
            Some(CLIENT),
            NetWidths::engine_default(),
            Rc::clone(&server_tap) as Rc<dyn RpcCarrier>,
            0,
        );
        server.set_connection_rate(1_000_000, 1_000_000);
        let mut server_vm = Vm::new();
        for stage in ServerStage::ALL {
            let _ = engine.replay(*stage, &mut server_vm);
        }
        let mut world = World {
            engine,
            vm,
            gui,
            rpc,
            channels,
            lanes,
            realms,
            server,
            server_tap,
            server_vm,
            asked,
            files,
            script,
            spawns,
            got,
            tick: 1,
            now_us: 1_000_000,
        };
        // The link exists once the client has pumped with an active connection.
        world.pump();
        assert!(world.rpc.is_ready(), "the client has a link");
        world
    }

    /// The client starts a new connection in one go, as a server switch does: the channels start over and the RPC layer follows
    /// at its next tick, with no frame in between that sees the link down.
    pub fn new_connection(&mut self) {
        self.channels.reset();
        self.channels.set_ghost_bits(8);
        self.channels.set_active(true);
        self.pump();
        assert!(self.rpc.is_ready(), "the new connection has a link");
    }

    /// One client tick of the RPC layer.
    pub fn pump(&mut self) {
        self.tick += 1;
        self.rpc.pump(&mut self.vm, self.tick);
        self.server.tick(self.tick);
    }

    /// One frame of the realms.
    pub fn frame(&mut self) {
        self.now_us += 16_667;
        self.pump();
        self.realms
            .frame(&mut self.vm, &self.gui, self.now_us, FRAME);
        self.pump();
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    /// What the client queued reaches the server and runs there.
    pub fn up(&mut self) {
        for (batch, _) in self.lanes.take_out(None) {
            self.server.receive(&batch).expect("the server reads it");
        }
        self.run_server();
    }

    pub fn run_server(&mut self) {
        let mut hooks = NoHooks;
        let mut env = RpcEnv {
            vm: &mut self.server_vm,
            tick: self.tick,
            conn: Some(CLIENT),
            perms: &NoPermissions,
            ghosts: &NoGhosts,
            hooks: &mut hooks,
        };
        self.server.dispatch(&mut env);
        self.server.flush();
    }

    /// What the server queued reaches the client.
    pub fn down(&mut self) {
        let batches: Vec<Vec<u8>> = std::mem::take(&mut *self.server_tap.0.borrow_mut());
        for batch in batches {
            self.lanes.received(None, batch);
        }
        self.pump();
    }

    /// The server sends the offer of one mod.
    pub fn offer(&mut self, tiers: &[&str]) {
        use torque_plugin_client_script::messages::{OFFER, ScriptOffer};
        let mods = [OfferedMod {
            id: "hud".into(),
            hash: [7; 32],
            entry: "client/main.lua".into(),
            tiers: tiers.iter().map(|t| t.to_string()).collect(),
        }];
        let handle = self
            .server
            .rpc::<ScriptOffer, ()>(OFFER)
            .expect("the offer rpc");
        self.server
            .notify(&handle, ScriptOffer::of(&mods))
            .expect("queued");
        self.server.flush();
        self.down();
    }

    /// The server sends the realm a value.
    pub fn send_down(&mut self, value: &Value) {
        use torque_plugin_client_script::messages::{DOWN, ScriptDown};
        let handle = self
            .server
            .rpc::<ScriptDown, ()>(DOWN)
            .expect("the down rpc");
        self.server
            .notify(&handle, ScriptDown::of(value).expect("fits"))
            .expect("queued");
        self.server.flush();
        self.down();
    }

    /// The server state the plugin keeps.
    pub fn script_server(&self) -> Rc<torque_plugin_client_script::server::ScriptServer> {
        self.server_vm
            .extension::<torque_plugin_client_script::server::ScriptServer>()
            .expect("the server state")
    }

    /// The abilities named in the player's answer, as the server has them.
    pub fn status(&self) -> Option<torque_plugin_client_script::messages::Status> {
        self.script_server().status_of(CLIENT)
    }
}
