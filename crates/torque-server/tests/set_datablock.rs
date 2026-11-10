//! `GameBase::setDataBlock` on a server with TGE's game classes: the pointer changes, the classes' `onNewDataBlock` work and the
//! new datablock's script callback run, a name that is no datablock fails with TGE's message and changes nothing, and a client
//! that ghosts the object reads the new datablock.

mod net_client;

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use net_client::{NetServer, TestClient, connection_of, eval, run};
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryEnd, MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);

/// The scene every test starts from: two StaticShape datablocks that record their callbacks, a PlayerData, a camera for a
/// connection to scope from, and a StaticShape on the first datablock.
const SCENE: &str = r#"
new SimGroup(ClientGroup);
datablock CameraData(EyeData) {};
new Camera(Eye) { dataBlock = EyeData; };
datablock StaticShapeData(First) { shapeFile = "a.dts"; };
datablock StaticShapeData(Second) { shapeFile = "b.dts"; };
datablock PlayerData(Body) { shapeFile = "a.dts"; };
function First::onNewDataBlock(%this, %obj) { $calls = $calls @ "first:" @ %obj @ ";"; }
function Second::onNewDataBlock(%this, %obj) { $calls = $calls @ "second:" @ %obj @ ";"; }
new StaticShape(Prop) { dataBlock = First; };
"#;

fn game_server() -> (NetServer, Rc<RefCell<NetShared<TorqueNet>>>, MemoryEnd) {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    let files = MemFiles::new();
    for (name, half) in [("a.dts", 1.0), ("b.dts", 2.0)] {
        let b = [-half, -half, 0.0, half, half, half];
        let shape = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &[]);
        files.insert(name, &torque_shapes::write::write_dts(&shape));
    }
    vm.set_file_host(files);
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
    state.borrow_mut().allow_connections = true;
    s.run(TICK_MS, |_| false);
    (s, shared, end)
}

fn global(s: &NetServer, name: &str) -> String {
    String::from_utf8_lossy(&s.vm.global(name).unwrap_or_default()).into_owned()
}

fn answer(s: &mut NetServer, code: &str) -> String {
    String::from_utf8(s.vm.eval(code.as_bytes()).unwrap()).unwrap()
}

fn id_of(s: &NetServer, name: &str) -> u32 {
    s.vm.objects().find(name.as_bytes()).expect(name)
}

fn errors(s: &mut NetServer) -> Vec<String> {
    s.vm.drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .filter(|l| l.contains("Could not find"))
        .collect()
}

#[test]
fn the_datablock_changes_and_the_new_datablocks_callback_runs() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    let (prop, first, second) = (id_of(&s, "Prop"), id_of(&s, "First"), id_of(&s, "Second"));
    assert_eq!(
        global(&s, "$calls"),
        format!("first:{prop};"),
        "the add ran the first datablock's callback"
    );
    assert_eq!(
        answer(&mut s, "return Prop.getDataBlock();"),
        first.to_string()
    );

    assert_eq!(answer(&mut s, "return Prop.setDataBlock(Second);"), "1");
    assert_eq!(
        answer(&mut s, "return Prop.getDataBlock();"),
        second.to_string()
    );
    assert_eq!(global(&s, "$calls"), format!("first:{prop};second:{prop};"));

    // The datablock it has already: true, and no second callback. An id works as a name does.
    assert_eq!(
        answer(&mut s, &format!("return Prop.setDataBlock({second});")),
        "1"
    );
    assert_eq!(global(&s, "$calls"), format!("first:{prop};second:{prop};"));
}

#[test]
fn the_shape_follows_the_new_datablock() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    let before = answer(&mut s, "return Prop.getWorldBox();");
    eval(&mut s, "Prop.setDataBlock(Second);");
    let after = answer(&mut s, "return Prop.getWorldBox();");
    assert_ne!(before, after, "the object box is the new shape's");
}

#[test]
fn a_name_that_is_no_datablock_fails_as_tge_does_and_changes_nothing() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    let first = id_of(&s, "First");
    s.vm.drain_log();
    for name in ["NoSuchDatablock", "Prop", "\"\""] {
        assert_eq!(
            answer(&mut s, &format!("return Prop.setDataBlock({name});")),
            "0",
            "{name}"
        );
    }
    let shown = errors(&mut s);
    assert_eq!(
        shown,
        [
            "Could not find data block \"NoSuchDatablock\"",
            "Could not find data block \"Prop\"",
            "Could not find data block \"\"",
        ]
    );
    assert_eq!(
        answer(&mut s, "return Prop.getDataBlock();"),
        first.to_string()
    );
    assert!(!global(&s, "$calls").contains("second"));
}

#[test]
fn a_datablock_of_another_class_is_refused_and_changes_nothing() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    let first = id_of(&s, "First");
    assert_eq!(answer(&mut s, "return Prop.setDataBlock(Body);"), "0");
    assert_eq!(
        answer(&mut s, "return Prop.getDataBlock();"),
        first.to_string()
    );
    assert_eq!(
        global(&s, "$calls"),
        format!("first:{};", id_of(&s, "Prop"))
    );
}

#[test]
fn a_player_takes_the_new_datablocks_products_and_keeps_running() {
    let (mut s, _, _) = game_server();
    eval(
        &mut s,
        r#"
datablock PlayerData(Slow) { shapeFile = "a.dts"; maxDamage = 100; maxForwardSpeed = 4; };
datablock PlayerData(Fast) { shapeFile = "a.dts"; maxDamage = 200; maxForwardSpeed = 12; };
new Player(Runner) { dataBlock = Slow; position = "0 0 5"; };
"#,
    );
    let fast = id_of(&s, "Fast");
    s.run(TICK_MS, |_| false);
    assert_eq!(answer(&mut s, "return Runner.setDataBlock(Fast);"), "1");
    assert_eq!(
        answer(&mut s, "return Runner.getDataBlock();"),
        fast.to_string()
    );
    for _ in 0..4 {
        s.run(TICK_MS, |_| false);
    }
    assert_eq!(
        answer(&mut s, "return Runner.getDataBlock();"),
        fast.to_string()
    );
    assert!(
        s.vm.drain_log()
            .iter()
            .all(|l| !String::from_utf8_lossy(&l.text).contains("rror")),
    );
}

#[test]
fn before_the_object_is_added_it_only_takes_the_pointer() {
    let (mut s, _, _) = game_server();
    eval(
        &mut s,
        r#"
datablock StaticShapeData(One) { shapeFile = "a.dts"; };
datablock StaticShapeData(Other) { shapeFile = "b.dts"; };
function One::onNewDataBlock(%this, %obj) { $inner = %obj.setDataBlock(Other); $seen = %obj.getDataBlock(); }
function Other::onNewDataBlock(%this, %obj) { $otherCalled = 1; }
new StaticShape(Early) { dataBlock = One; };
"#,
    );
    assert_eq!(global(&s, "$inner"), "1");
    assert_eq!(global(&s, "$seen"), id_of(&s, "Other").to_string());
    assert_eq!(
        global(&s, "$otherCalled"),
        "",
        "the add did not run Other's callback"
    );
}

#[test]
fn a_ghosted_objects_client_reads_the_new_datablock() {
    let (mut s, shared, end) = game_server();
    eval(&mut s, SCENE);
    let (first, second) = (id_of(&s, "First"), id_of(&s, "Second"));
    let mut client = TestClient::new(50_010, 7, Default::default());
    client.start(&end);
    run(&mut s, &end, &mut [&mut client], 10);
    assert!(client.connected());
    let conn = connection_of(&shared, client.addr);
    eval(
        &mut s,
        &format!("{conn}.setControlObject(Eye); {conn}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut client], 20);
    let prop = id_of(&s, "Prop");
    let hub = shared.borrow().ghosts.clone().expect("a hub");
    let conn_id = shared.borrow().conns[&conn].conn.expect("accepted");
    let index = hub
        .ghost_index(conn_id, prop)
        .expect("the prop has a ghost");
    assert_eq!(client.world.ghosts[&index].datablock(), Some(first));

    eval(&mut s, "Prop.setDataBlock(Second);");
    run(&mut s, &end, &mut [&mut client], 10);
    assert_eq!(client.world.ghosts[&index].datablock(), Some(second));
    assert!(client.errors.is_empty(), "{:?}", client.errors);

    // A ghost whose object was refused a datablock is not sent another.
    let updates = client.world.ghosts[&index].updates;
    eval(&mut s, "Prop.setDataBlock(Body);");
    run(&mut s, &end, &mut [&mut client], 10);
    assert_eq!(client.world.ghosts[&index].datablock(), Some(second));
    assert_eq!(client.world.ghosts[&index].updates, updates);
}
