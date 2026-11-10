//! A server with TGE's game classes ([`torque_server::game::install`]) and a network host: a loopback client joins, and the
//! scripts start its ghosting and give it its control object through the connection's methods.

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use torque_net::client::{Client, ClientEvent, ClientState};
use torque_net::handshake::ConnectFields;
use torque_net::huffman::Table;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryEnd, MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

type NetServer = Server<HostParts<TorqueNet>>;

const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);
const CLIENT_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 50_000);

/// A server with the game classes and a network host on a memory transport, taking connections.
fn game_server() -> (NetServer, Rc<RefCell<NetShared<TorqueNet>>>, MemoryEnd) {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
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
    state.borrow_mut().allow_connections = true;
    (s, shared, end)
}

/// Runs ticks until the client is connected, answering the server's datagrams.
fn join(s: &mut NetServer, end: &MemoryEnd) -> Client {
    let table = Table::torque();
    let mut client = Client::new(7, ConnectFields::standard(b"", Vec::new()));
    end.inject(CLIENT_ADDR, &client.challenge_request());
    for _ in 0..10 {
        s.run(TICK_MS, |_| false);
        let now = s.vm.sim_time();
        for (_, datagram) in end.take_sent() {
            if let ClientEvent::Send(bytes) = client.on_datagram(&table, &datagram, now) {
                end.inject(CLIENT_ADDR, &bytes);
            }
        }
        if client.state() == &ClientState::Connected {
            break;
        }
    }
    assert_eq!(client.state(), &ClientState::Connected);
    client
}

fn global(s: &NetServer, name: &str) -> String {
    String::from_utf8_lossy(&s.vm.global(name).unwrap_or_default()).into_owned()
}

#[test]
fn the_scripts_give_a_connection_its_control_object_and_start_and_stop_its_ghosting() {
    let (mut s, shared, end) = game_server();
    s.vm.eval(
        b"new SimGroup(ClientGroup);
datablock CameraData(EyeData) {};
new Camera(Eye) { dataBlock = EyeData; };
new Sun(TheSun) { azimuth = 30; elevation = 45; };",
    )
    .unwrap();
    let _client = join(&mut s, &end);
    let obj = shared
        .borrow()
        .conns
        .values()
        .find(|r| r.addr == CLIENT_ADDR)
        .map(|r| r.obj)
        .expect("the connection's object");
    let conn = shared.borrow().conns[&obj].conn.expect("accepted");
    let hub = shared.borrow().ghosts.clone().expect("a hub");
    let eye = s.vm.objects().find(b"Eye").unwrap();
    let sun = s.vm.objects().find(b"TheSun").unwrap();
    // The `NetObject` behaviour entered both objects in the hub as they were added.
    assert!(hub.knows(eye) && hub.knows(sun));

    // A ShapeBase becomes the control object and the scope object; anything else is refused and changes nothing.
    let script = format!(
        "$none = {obj}.getControlObject(); $set = {obj}.setControlObject(Eye); $got = {obj}.getControlObject(); \
         $bad = {obj}.setControlObject(TheSun); $missing = {obj}.setControlObject(NoSuchObject); $after = {obj}.getControlObject();"
    );
    s.vm.eval(script.as_bytes()).unwrap();
    assert_eq!(global(&s, "$none"), "0");
    assert_eq!(global(&s, "$set"), "1");
    assert_eq!(global(&s, "$got"), eye.to_string());
    assert_eq!(
        (global(&s, "$bad"), global(&s, "$missing")),
        ("0".into(), "0".into())
    );
    assert_eq!(global(&s, "$after"), eye.to_string());
    assert_eq!(hub.control_of(conn), Some(eye));
    assert_eq!(hub.scope_object_of(conn), Some(eye));

    // `activateGhosting` posts the ghost-always objects (the Sun is ScopeAlways); `resetGhosting` drops every ghost.
    s.vm.eval(format!("$before = {obj}.getGhostID(TheSun);").as_bytes())
        .unwrap();
    assert_eq!(
        global(&s, "$before"),
        "-1",
        "no ghost before ghosting starts"
    );
    s.vm.eval(format!("{obj}.activateGhosting();").as_bytes())
        .unwrap();
    assert!(hub.ghost_index(conn, sun).is_some(), "the sun has a ghost");
    s.vm.eval(
        format!("$index = {obj}.getGhostID(TheSun); $missing = {obj}.getGhostID(NoSuchObject);")
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(
        global(&s, "$index"),
        hub.ghost_index(conn, sun).unwrap().to_string()
    );
    assert_eq!(global(&s, "$missing"), "-1");
    s.vm.eval(format!("{obj}.resetGhosting();").as_bytes())
        .unwrap();
    assert!(hub.ghost_index(conn, sun).is_none(), "ghosting was reset");

    // A control object that is deleted leaves the connection without one.
    s.vm.eval(b"Eye.delete();").unwrap();
    s.vm.eval(format!("$gone = {obj}.getControlObject();").as_bytes())
        .unwrap();
    assert_eq!(global(&s, "$gone"), "0");
    assert_eq!(hub.control_of(conn), None);
}

#[test]
fn the_methods_answer_their_neutral_values_on_a_connection_the_host_did_not_make() {
    let (mut s, _shared, _end) = game_server();
    s.vm.eval(
        b"datablock CameraData(EyeData) {};
new Camera(Eye) { dataBlock = EyeData; };
new GameConnection(Stranger);
$set = Stranger.setControlObject(Eye);
$got = Stranger.getControlObject();
Stranger.activateGhosting();
Stranger.resetGhosting();
$command = isFunction(commandToClient);",
    )
    .unwrap();
    assert_eq!(global(&s, "$set"), "0");
    assert_eq!(global(&s, "$got"), "0");
    // The command natives come with the host.
    assert_eq!(global(&s, "$command"), "1");
}

#[test]
fn torques_codecs_move_a_shapebases_simulation_bits_to_tges_record_masks() {
    use torque_ghosts::tge::TgeClass;
    use torque_server::net::codecs::{NetCodecs, TorqueCodecs};
    let player = TgeClass::Player.code();
    let item = TgeClass::Item.code();
    // A Player's MoveMask and ImpactMask as the simulation marks them, then as TGE's Player record reads them.
    assert_eq!(
        TorqueCodecs.sim_mask(player, 0x0200_0000 | 0x0400_0000),
        torque_ghosts::player::mask::MOVE | torque_ghosts::player::mask::IMPACT
    );
    // An Item's position and rotation bits.
    assert_eq!(
        TorqueCodecs.sim_mask(item, 0x0400_0000 | 0x0800_0000),
        torque_ghosts::shapes::item_mask::POSITION | torque_ghosts::shapes::item_mask::ROTATION
    );
    // The ShapeBase part's bits move up too: the damage, a mounted image, the scale bit stays.
    use torque_ghosts::shapebase::mask as sb;
    assert_eq!(
        TorqueCodecs.sim_mask(player, 0x0000_0001 | 0x0000_0020 | 0x0010_0000),
        0x0000_0001 | sb::DAMAGE | sb::IMAGE_N
    );
    let shape = TgeClass::StaticShape.code();
    assert_eq!(TorqueCodecs.sim_mask(shape, 0x0001_0000), sb::THREAD_N);
    // A class without a ShapeBase part keeps every bit (a Projectile's bounce and explosion bits are TGE's).
    let projectile = TgeClass::Projectile.code();
    assert_eq!(TorqueCodecs.sim_mask(projectile, 0x60), 0x60);
}
