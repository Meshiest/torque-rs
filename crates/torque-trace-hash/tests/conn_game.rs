//! A conn game of the test's own on the engine's client host: TGE's feed ([`start_feed`]) with parts, a local test name and a
//! native of its own. A client of it is made by the same plugins as a client of TGE's
//! ([`ClientBuilder::from_engine_with_conn`]) and joins the example game's server over the memory link.
//!
//! The wire check: a session of it writes the bytes a client of TGE's conn game writes, in either protocol, and TGE's bytes are
//! pinned in `golden/conn_game_sessions.txt` (recorded with TGE's client before the conn game was a type parameter). After a
//! change that is meant to alter the wire, `TORQUE_BLESS_GOLDEN=1 cargo test -p torque-trace-hash --test conn_game` writes
//! the file again.

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use sha2::{Digest, Sha256};
use torque::app::client::{ClientBuilder, ClientError, ClientNet, GameClient};
use torque::app::config::{EngineConfig, GameConfig};
use torque::app::files::GameFiles;
use torque::app::hosting::HostingPlugin;
use torque::app::plugins::{
    ClientHostPlugin, ClientParts, ClientWiring, DefaultPlugins, ZoomPlugin,
};
use torque::app::server::{GameServer, Listen};
use torque::client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque::engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginId};
use torque_app_server::plugins::set_protocol;
use torque_client::conn::engine_wire::EngineNonceKey;
use torque_client::conn::tge::{TgeConn, TgeFeed, start_feed};
use torque_client::conn::{ConnGame, Connect, Started};
use torque_client::host::{ClientPass, EnginePass};
use torque_client::state::ClientState;
use torque_engine::ProtocolKind;
use torque_example_minimal_engine::{ExamplePlugin, files, server};
use torque_gui::Gui;
use torque_server::net::engine::ensure_host;
use torque_trace_hash::rig::CHALLENGE_KEY;
use torque_ts_vm::vm::{Native, Vm};

const GOLDEN_FILE: &str = "tests/golden/conn_game_sessions.txt";
const GOLDEN: &str = include_str!("golden/conn_game_sessions.txt");
/// Datagrams kept verbatim.
const VERBATIM: usize = 8;
const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);
const PROTOCOLS: [(ProtocolKind, &str); 2] =
    [(ProtocolKind::Tge, "tge"), (ProtocolKind::Engine, "engine")];

/// The test's conn game: TGE's feed and world, its own parts, a second local test name and a native that counts its calls.
#[derive(Debug, Default)]
struct TestConn;

/// The test conn game's part of the natives' state.
#[derive(Debug, Default)]
struct TestParts {
    calls: i32,
}

impl ConnGame for TestConn {
    type Feed = TgeFeed;
    type Parts = TestParts;

    const HANDSHAKE_WAIT_MS: u32 = TgeConn::HANDSHAKE_WAIT_MS;
    const HANDSHAKE_TRIES: u32 = TgeConn::HANDSHAKE_TRIES;

    fn local_test_names() -> &'static [&'static str] {
        &["isLocalConnection", "isTestLocal"]
    }

    fn register_natives(vm: &mut Vm, client: &Rc<RefCell<ClientState<TestConn>>>, gui: &Gui) {
        let _ = gui;
        let st = Rc::clone(client);
        vm.register("testConnCalls", 1, 1, "testConnCalls();", move |_, _| {
            let mut s = st.borrow_mut();
            s.parts.calls += 1;
            Native::Int(s.parts.calls)
        });
    }

    fn start(
        vm: &mut Vm,
        client: &Rc<RefCell<ClientState<TestConn>>>,
        connect: Connect,
    ) -> Result<Started<TestConn>, String> {
        let _ = client;
        start_feed(vm, connect)
    }
}

/// The example game's engine in `protocol`, with `extra` added.
fn engine_with(
    files: &GameFiles,
    protocol: ProtocolKind,
    extra: impl FnOnce(&mut EngineBuilder),
) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins((DefaultPlugins, ExamplePlugin(files.clone())));
    set_protocol(&mut builder, protocol);
    extra(&mut builder);
    builder.config(&config);
    builder.build().expect("the engine builds")
}

fn engine(files: &GameFiles, protocol: ProtocolKind) -> Engine {
    engine_with(files, protocol, |_| {})
}

/// A client of conn game `C` made by `engine`, its scripts run, on an in-memory socket whose two queues are returned.
fn client<C: ConnGame>(
    engine: &Engine,
    files: &GameFiles,
) -> (GameClient<GameServer, C>, DatagramQueue, DatagramQueue) {
    let mem = MemoryDatagrams {
        local_port: CLIENT.port(),
        ..MemoryDatagrams::default()
    };
    let (sent, incoming) = (mem.sent.clone(), mem.incoming.clone());
    let mut builder = ClientBuilder::<C>::from_engine_with_conn(
        engine,
        Some(files.mem_files()),
        ClientNet::External,
    )
    .expect("the client builds");
    builder.set_socket(Box::new(mem), NetPolicy::Loopback);
    builder.exec_entry().expect("the client's script");
    let mut client = builder.build();
    // The client draws its connect sequence and nonces at random; fixed, the datagrams are a function of the inputs.
    client
        .vm_mut()
        .set_extension(Rc::new(EngineNonceKey(CHALLENGE_KEY)));
    (client, sent, incoming)
}

/// Every datagram of a session, in the order carried, and which way (true: to the server).
type Wire = Vec<(bool, Vec<u8>)>;

/// A server of the example game and a client of conn game `C` over the memory link, every datagram between them recorded.
struct Pair<C: ConnGame> {
    server: GameServer,
    client: GameClient<GameServer, C>,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    wire: Wire,
}

impl<C: ConnGame<Feed = TgeFeed>> Pair<C> {
    fn new(protocol: ProtocolKind, extra: impl FnOnce(&mut EngineBuilder)) -> Pair<C> {
        let files = files().expect("the example game");
        let server =
            server(&engine(&files, protocol), Listen::Memory(SERVER)).expect("the server starts");
        // The server draws the key of its challenges at random; fixed, the datagrams are a function of the inputs.
        server
            .shared()
            .expect("the network host")
            .borrow_mut()
            .hs
            .set_key(CHALLENGE_KEY);
        if let Some(host) = ensure_host(
            server.vm(),
            &mut server.shared().expect("the network host").borrow_mut(),
        ) {
            host.table.rekey(CHALLENGE_KEY);
        }
        let (mut client, sent, incoming) =
            client::<C>(&engine_with(&files, protocol, extra), &files);
        client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        Pair {
            server,
            client,
            sent,
            incoming,
            wire: Vec::new(),
        }
    }

    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SERVER {
                self.wire.push((true, data.clone()));
                end.inject(SocketAddr::V4(CLIENT), &data);
            }
        }
        for (to, data) in end.take_sent() {
            if to == SocketAddr::V4(CLIENT) {
                self.wire.push((false, data.clone()));
                let SocketAddr::V4(from) = SERVER else {
                    continue;
                };
                self.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    /// One tick of the server, then of the client, the link carried around each.
    fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        self.client.step(32);
        self.carry();
    }

    /// Steps until the client holds the mission's ghosts (at most 400 ticks), then 60 ticks more.
    fn session(mut self) -> Pair<C> {
        for _ in 0..400 {
            if self.client.ghost_count() >= 3 {
                break;
            }
            self.step();
        }
        assert!(
            self.client.ghost_count() >= 3,
            "{:?}",
            self.client.client_log
        );
        for _ in 0..60 {
            self.step();
        }
        self
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn digest(wire: &Wire, up: bool) -> String {
    let mut h = Sha256::new();
    for (_, bytes) in wire.iter().filter(|(d, _)| *d == up) {
        h.update((bytes.len() as u32).to_le_bytes());
        h.update(bytes);
    }
    hex(&h.finalize())
}

/// The golden file's lines of the session `name`: the count and a digest each way, and the first datagrams.
fn render(name: &str, wire: &Wire) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{name} datagrams {}", wire.len());
    let _ = writeln!(out, "{name} to-server {}", digest(wire, true));
    let _ = writeln!(out, "{name} to-client {}", digest(wire, false));
    for (i, (up, bytes)) in wire.iter().take(VERBATIM).enumerate() {
        let way = if *up { "to-server" } else { "to-client" };
        let _ = writeln!(out, "{name} {i} {way} {}", hex(bytes));
    }
    out
}

#[test]
fn conn_game_client_joins_the_example_server() {
    let mut pair = Pair::<TestConn>::new(ProtocolKind::Tge, |_| {}).session();
    let connected = pair
        .client
        .call(
            "eval",
            &["return ServerConnection.isConnected() SPC ServerConnection.isTestLocal();"],
        )
        .expect("the connection's natives");
    assert_eq!(connected, "1 0", "connected over the network, not locally");
}

#[test]
fn conn_game_writes_torques_bytes_in_both_protocols() {
    let mut text = String::new();
    for (protocol, label) in PROTOCOLS {
        let tge = render(
            label,
            &Pair::<TgeConn>::new(protocol, |_| {}).session().wire,
        );
        let test = Pair::<TestConn>::new(protocol, |_| {}).session();
        assert!(
            test.wire.len() > 60,
            "{label}: {} datagrams",
            test.wire.len()
        );
        assert_eq!(
            render(label, &test.wire),
            tge,
            "{label}: the test conn game's session differs from TGE's"
        );
        text.push_str(&tge);
    }
    if std::env::var_os("TORQUE_BLESS_GOLDEN").is_some() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_FILE);
        std::fs::write(path, &text).expect("writes the goldens");
        return;
    }
    assert_eq!(text, GOLDEN, "TGE's sessions differ from the goldens");
}

/// The natives of a client of `C` with a connection, one line each.
fn native_listing<C: ConnGame>(engine: &Engine) -> String {
    let builder = ClientBuilder::<C>::from_engine_with_conn(engine, None, ClientNet::External)
        .expect("the client builds");
    builder.vm.native_listing()
}

#[test]
fn conn_game_natives_are_the_engines_and_its_own() {
    let files = files().expect("the example game");
    let engine = engine(&files, ProtocolKind::Tge);
    let tge = native_listing::<TgeConn>(&engine);
    let test = native_listing::<TestConn>(&engine);
    let added: Vec<&str> = test
        .lines()
        .filter(|l| !tge.lines().any(|t| t == *l))
        .collect();
    let missing: Vec<&str> = tge
        .lines()
        .filter(|l| !test.lines().any(|t| t == *l))
        .collect();
    assert!(missing.is_empty(), "{missing:?}");
    assert_eq!(added.len(), 2, "{added:?}");
    assert!(
        added.iter().any(|l| l.starts_with("testConnCalls ")),
        "{added:?}"
    );
    assert!(
        added
            .iter()
            .any(|l| l.starts_with("GameConnection::isTestLocal ")),
        "{added:?}"
    );
}

#[test]
fn conn_game_parts_are_its_own() {
    let files = files().expect("the example game");
    let (mut client, _, _) = client::<TestConn>(&engine(&files, ProtocolKind::Tge), &files);
    assert_eq!(client.call("testConnCalls", &[]).as_deref(), Ok("1"));
    assert_eq!(client.call("testConnCalls", &[]).as_deref(), Ok("2"));
    assert_eq!(client.host.client.borrow().parts.calls, 2);
}

#[test]
fn conn_game_wiring_is_typed_and_the_parts_are_not() {
    let files = files().expect("the example game");
    let engine = engine(&files, ProtocolKind::Tge);
    let builder =
        ClientBuilder::<TestConn>::from_engine_with_conn(&engine, None, ClientNet::External)
            .expect("the client builds");
    let wiring = builder
        .vm
        .extension::<ClientWiring<TestConn>>()
        .expect("the typed wiring");
    assert!(Rc::ptr_eq(wiring.state(), &builder.state));
    assert!(
        builder.vm.extension::<ClientWiring>().is_none(),
        "no wiring of TGE's conn game"
    );
    let parts = builder.vm.extension::<ClientParts>().expect("the parts");
    assert_eq!(parts.net(), ClientNet::External);
    assert!(parts.gui().is_some());
    assert!(ClientWiring::<TestConn>::of(&builder.vm, "the test").is_ok());
    let err = ClientWiring::<TgeConn>::of(&builder.vm, "the test")
        .err()
        .expect("another conn game");
    assert!(
        matches!(
            &err,
            ClientError::WrongConnGame { plugin, wanted, found }
                if plugin == "the test"
                    && *wanted == std::any::type_name::<TgeConn>()
                    && *found == std::any::type_name::<TestConn>()
        ),
        "{err}"
    );
    let err = ClientWiring::<TestConn>::of(&Vm::new(), "the test")
        .err()
        .expect("no client");
    assert!(
        matches!(&err, ClientError::NotAClient { plugin } if plugin == "the test"),
        "{err}"
    );
}

/// A pass of the test conn game's own: the engine's, counted.
struct CountingPass(Rc<Cell<u32>>);

impl ClientPass<TestConn> for CountingPass {
    fn net(&self, vm: &mut Vm, client: &Rc<RefCell<ClientState<TestConn>>>) {
        self.0.set(self.0.get() + 1);
        EnginePass.net(vm, client);
    }

    fn other(&self, vm: &mut Vm, client: &Rc<RefCell<ClientState<TestConn>>>) {
        EnginePass.other(vm, client);
    }
}

#[derive(PluginId)]
#[plugin(crate = torque::engine)]
struct CountingPassPlugin(Rc<Cell<u32>>);

impl Plugin for CountingPassPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let pass: Rc<dyn ClientPass<TestConn>> = Rc::new(CountingPass(Rc::clone(&self.0)));
        engine.set_slot::<dyn ClientPass<TestConn>>(pass);
    }
}

#[test]
fn conn_game_pass_is_the_games_when_it_sets_one() {
    let passes = Rc::new(Cell::new(0));
    let counter = Rc::clone(&passes);
    let pair = Pair::<TestConn>::new(ProtocolKind::Tge, move |b| {
        b.add_plugins(CountingPassPlugin(counter));
    })
    .session();
    assert!(passes.get() >= 60, "{} passes", passes.get());
    assert!(pair.client.ghost_count() >= 3);
}

#[test]
fn conn_game_needs_the_client_host() {
    let files = files().expect("the example game");
    let engine = engine(&files, ProtocolKind::Tge);
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins.disable::<ClientHostPlugin>());
    let bare = builder.build().expect("the engine builds");
    assert!(
        ClientBuilder::<TestConn>::from_engine_with_conn(&engine, None, ClientNet::None).is_ok()
    );
    let err = ClientBuilder::<TestConn>::from_engine_with_conn(&bare, None, ClientNet::None)
        .err()
        .expect("no client host");
    assert!(matches!(err, ClientError::NoClientHost), "{err}");
}

#[test]
fn conn_game_gets_the_opt_in_plugins_natives() {
    let files = files().expect("the example game");
    let engine = engine_with(&files, ProtocolKind::Tge, |b| {
        b.add_plugins((ZoomPlugin, HostingPlugin::default()));
    });
    let (mut client, _, _) = client::<TestConn>(&engine, &files);
    assert_eq!(
        client
            .call("checkJoinAddress", &["127.0.0.1:28000"])
            .as_deref(),
        Ok("127.0.0.1:28000")
    );
    client.call("setFov", &["60"]).expect("the zoom's natives");
    assert!((client.host.client.borrow().zoom.target - 60.0).abs() < 1e-3);
}
