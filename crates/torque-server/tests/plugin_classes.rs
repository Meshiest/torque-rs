//! A plugin's ghost class end to end: an engine whose plugin adds a console class and its ghost record, a server with TGE's
//! game classes and a network host on the memory transport, and two clients that read the server's packets with TGE's
//! client codec and the same plugin classes. Each client's world gets the plugin's ghost with its fields, follows its updates
//! (a field sent only with its mask bit is kept until the next one), loses it when its ghosting is reset (the other keeps
//! it) and when the object is deleted; a TGE client, which cannot read the record, is never sent it.

use crate::net_client;

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use torque_engine::{
    EngineBuilder, EngineClass, GhostRecord, Plugin, PluginId, Replicate, ServerStage,
};
use torque_ghosts::plugin::PluginClasses;
use torque_ghosts::{TgeClass, TgeRecord};
use torque_net::classes::Kind;
use torque_net::huffman::Table;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryEnd, MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::Server;
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use net_client::{NetServer, TestClient, connection_of, eval, run};

const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);

/// The console class of the plugin's objects (its fields are read by the ghost's packer, not here).
#[allow(dead_code)]
#[derive(EngineClass)]
#[class(name = "Rope", parent = "SceneObject")]
struct Rope {
    segments: i32,
    length: f32,
    label: String,
    color: [f32; 3],
}

/// What a client knows of a rope.
#[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
struct RopeGhost {
    #[net(bits = 6)]
    segments: u32,
    length: f32,
    #[net(max_len = 12)]
    label: String,
    #[net(mask = 0x2)]
    color: Option<[f32; 3]>,
}

#[derive(PluginId)]
#[plugin(id = "test.ropes")]
struct RopePlugin;

impl Plugin for RopePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.server_script().class::<Rope>();
        engine
            .net()
            .ghost::<RopeGhost>("Rope", Replicate::Interpolated);
    }
}

/// A server with the game classes, the plugin's class and set, and a network host on a memory transport.
fn game_server() -> (NetServer, Rc<RefCell<NetShared<TorqueNet>>>, MemoryEnd) {
    let engine = EngineBuilder::new()
        .add_plugins(RopePlugin)
        .build()
        .expect("the engine builds");
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    engine
        .replay(ServerStage::Classes, &mut vm)
        .expect("the plugin's classes");
    vm.set_file_host(MemFiles::new());
    let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
    let game = torque_server::game::install(&mut vm, &state);
    let mut s = Server::with_state(vm, Rc::clone(&state));
    let (transport, end) = MemoryTransport::new();
    let transport: SharedTransport = Rc::new(RefCell::new(transport));
    transport
        .borrow_mut()
        .bind(SERVER_ADDR.ip(), SERVER_ADDR.port())
        .unwrap();
    let shared = attach_host(
        &mut s.vm,
        Host {
            transport,
            game: TorqueNet::default(),
            engine: Rc::clone(&state),
            clock: NetClock::Sim,
            ghosts: TgeGhostGame,
            container: game.container,
            static_writes: None,
        },
    );
    s.attach_net(Rc::new(NetPasses(Rc::clone(&shared))))
        .unwrap();
    // The script registry's stage: the plugin's console class.
    engine
        .replay(ServerStage::Net, &mut s.vm)
        .expect("the plugin's script registry");
    state.borrow_mut().allow_connections = true;
    (s, shared, end)
}

/// What a client of the plugin sees of its ropes.
impl TestClient {
    /// The client's ropes: (ghost index, record).
    fn ropes(&self) -> Vec<(u32, RopeGhost)> {
        self.world
            .ghosts
            .iter()
            .filter_map(|(&index, g)| match (&g.class, &g.record) {
                (Some(TgeClass::Plugin(c)), Some(record)) if c.name() == "Rope" => {
                    Some((index, record.plugin::<RopeGhost>()?.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn rope(&self) -> Option<RopeGhost> {
        self.ropes().into_iter().next().map(|(_, r)| r)
    }
}

#[test]
fn a_plugin_ghost_reaches_two_clients_follows_its_updates_and_goes_with_its_scope_and_its_object() {
    let (mut s, shared, end) = game_server();
    let plugins =
        (*torque_server::net::codecs::plugin_classes(&s.vm).expect("the plugin set")).clone();
    assert_eq!(plugins.names(Kind::Object).collect::<Vec<_>>(), ["Rope"]);
    eval(
        &mut s,
        "new SimGroup(ClientGroup);
datablock CameraData(EyeData) {};
new Camera(EyeA) { dataBlock = EyeData; };
new Camera(EyeB) { dataBlock = EyeData; };
new Rope(Line) { segments = 70; length = 2.5; label = \"twisted hemp line\"; color = \"1 0.5 0.25\"; };",
    );
    let line = s.vm.objects().find(b"Line").unwrap();
    let mut a = TestClient::new(50_001, 7, plugins.clone());
    let mut b = TestClient::new(50_002, 9, plugins);
    a.start(&end);
    b.start(&end);
    run(&mut s, &end, &mut [&mut a, &mut b], 10);
    assert!(a.connected() && b.connected());
    let conn_a = connection_of(&shared, a.addr);
    let conn_b = connection_of(&shared, b.addr);
    // Each connection scopes from its camera.
    eval(
        &mut s,
        &format!("{conn_a}.setControlObject(EyeA); {conn_b}.setControlObject(EyeB);"),
    );

    // Neither client is the engine's own (no handshake extension): with the server's switch off, the rope is not sent.
    eval(
        &mut s,
        &format!("{conn_a}.activateGhosting(); {conn_b}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut a, &mut b], 20);
    assert!(a.ropes().is_empty() && b.ropes().is_empty());
    assert!(
        a.errors.is_empty() && b.errors.is_empty(),
        "{:?} {:?}",
        a.errors,
        b.errors
    );

    // With every client the engine's own, both get it, packed with its widths and caps.
    eval(&mut s, "$Server::EngineClasses = 1;");
    eval(
        &mut s,
        &format!("{conn_a}.resetGhosting(); {conn_b}.resetGhosting();"),
    );
    run(&mut s, &end, &mut [&mut a, &mut b], 5);
    eval(
        &mut s,
        &format!("{conn_a}.activateGhosting(); {conn_b}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut a, &mut b], 20);
    let first = RopeGhost {
        segments: 70 & 0x3f,
        length: 2.5,
        label: "twisted hemp".to_string(),
        color: Some([1.0, 0.5, 0.25]),
    };
    for c in [&a, &b] {
        assert_eq!(c.rope(), Some(first.clone()), "{:?}", c.errors);
        let (index, _) = c.ropes()[0];
        let class = c.world.ghosts[&index].class.unwrap();
        assert_eq!(class.name(), "Rope");
        assert_eq!(class.net_flags(), torque_net::ghost::net_flags::GHOSTABLE);
    }

    // An update of the always-sent fields: the color is not sent (its mask bit is clear) and the client keeps it.
    eval(&mut s, "Line.segments = 9; Line.color = \"0 0 1\";");
    torque_game::scene::ghost::mark(&s.vm, line, 0x1);
    run(&mut s, &end, &mut [&mut a, &mut b], 10);
    for c in [&a, &b] {
        let rope = c.rope().unwrap();
        assert_eq!((rope.segments, rope.color), (9, Some([1.0, 0.5, 0.25])));
        let (index, _) = c.ropes()[0];
        assert!(c.world.ghosts[&index].updates >= 1);
    }
    // Its mask bit set, the color goes out.
    torque_game::scene::ghost::mark(&s.vm, line, 0x2);
    run(&mut s, &end, &mut [&mut a, &mut b], 10);
    for c in [&a, &b] {
        assert_eq!(c.rope().unwrap().color, Some([0.0, 0.0, 1.0]));
    }

    // Scoping: the second client's ghosting is reset, so it loses its ghosts while the first keeps its own; a new rope reaches
    // only the client that is ghosting.
    eval(&mut s, &format!("{conn_b}.resetGhosting();"));
    eval(
        &mut s,
        "new Rope(Spare) { segments = 3; label = \"backup\"; };",
    );
    run(&mut s, &end, &mut [&mut a, &mut b], 20);
    assert!(b.ropes().is_empty());
    let labels: Vec<String> = a.ropes().into_iter().map(|(_, r)| r.label).collect();
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert!(labels.contains(&"backup".to_string()), "{:?}", a.ropes());

    // Deletion: the object's ghost goes from the client that had it.
    eval(&mut s, "Line.delete();");
    run(&mut s, &end, &mut [&mut a, &mut b], 20);
    let left: Vec<String> = a.ropes().into_iter().map(|(_, r)| r.label).collect();
    assert_eq!(left, ["backup"]);
    assert!(
        a.errors.is_empty() && b.errors.is_empty(),
        "{:?} {:?}",
        a.errors,
        b.errors
    );
}

#[test]
fn a_client_without_the_plugin_refuses_its_class_id_cleanly() {
    let (mut s, shared, end) = game_server();
    eval(
        &mut s,
        "new SimGroup(ClientGroup); $Server::EngineClasses = 1; new Rope(Line) { segments = 1; };
datablock CameraData(EyeData) {}; new Camera(Eye) { dataBlock = EyeData; };",
    );
    let mut c = TestClient::new(50_003, 11, PluginClasses::default());
    c.start(&end);
    run(&mut s, &end, &mut [&mut c], 10);
    let conn = connection_of(&shared, c.addr);
    eval(
        &mut s,
        &format!("{conn}.setControlObject(Eye); {conn}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut c], 20);
    // The packet that carries the rope does not read: the class id is past every class the client knows.
    assert!(
        c.errors.iter().any(|e| e.contains("object class")),
        "{:?}",
        c.errors
    );
    assert!(c.ropes().is_empty());
    assert!(
        c.world
            .ghosts
            .values()
            .all(|g| !matches!(g.record, Some(TgeRecord::Plugin(..))))
    );
}

/// A plugin's datablock class: the console class and its record in one struct.
#[derive(Debug, Clone, PartialEq, Default, EngineClass, torque_engine::DatablockCodec)]
#[class(name = "PropData", parent = "SimDataBlock")]
struct PropData {
    mass: f32,
    #[net(max_len = 32)]
    model: String,
}

#[derive(PluginId)]
#[plugin(id = "test.props")]
struct PropsPlugin;

impl Plugin for PropsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.server_script().class::<PropData>();
        engine.net().datablock::<PropData>("PropData");
    }
}

#[test]
fn a_plugins_datablock_goes_to_the_engines_own_clients_and_as_one_bit_to_the_others() {
    use torque_server::net::codecs::{PluginDbRecord, net_codecs};
    use torque_server::net::datablocks::{ConnDatablocks, DatablockKeys, DbPost, net_classes_of};
    let engine = EngineBuilder::new()
        .add_plugins(PropsPlugin)
        .build()
        .expect("the engine builds");
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).unwrap();
    }
    vm.eval(b"datablock PropData(Crate) { mass = 3; model = \"crate.glb\"; };")
        .unwrap();
    let id = vm.objects().find(b"Crate").unwrap();
    let codecs = net_codecs(&vm);
    let classes = Rc::new(net_classes_of(&*codecs));
    let keys = DatablockKeys::new();
    // Keys start at 0, the key of a connection that has nothing: the datablock's is above it.
    keys.advance();
    keys.bump(id);
    let table = Table::torque();
    let rendered = |peer: bool| {
        let mut chain = ConnDatablocks::default();
        chain.set_plugin_classes(peer);
        let (_, posts, _) = chain.call(vm.objects(), &keys, &classes, 1);
        chain.render(vm.objects(), &keys, &table, &*codecs).unwrap();
        let [DbPost::Cell(cell)] = posts.as_slice() else {
            panic!("one datablock event: {posts:?}");
        };
        cell.0.rendered().expect("rendered")
    };
    // A client that is not the engine's own: the event of a datablock it has, one 0 bit.
    assert!(rendered(false).body.is_none());
    // The engine's own client: the record, its class id past TGE's 35 classes.
    let event = rendered(true);
    let body = event.body.expect("a body");
    assert_eq!(body.class, "PropData");
    let record = body
        .record
        .as_any()
        .downcast_ref::<PluginDbRecord>()
        .unwrap();
    assert_eq!(record.class_id, 35);
    assert_eq!(
        record.record.downcast_ref::<PropData>(),
        Some(&PropData {
            mass: 3.0,
            model: "crate.glb".to_string()
        })
    );
}
