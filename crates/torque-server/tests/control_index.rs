//! The hub finds the connection that controls an object by an index of control objects, not by walking the connections: it
//! stays right through a respawn, a switch of control object, a control object that controls another, a released control and
//! a disconnect, and an object nothing controls (a bot) costs no chain walk however many connections there are.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use torque_game::scene::camera::{ControlChain, TorqueChain, set_control_chain};
use torque_game::scene::container::{Container, ContainerConfig};
use torque_game::sim::player::methods::set_control_object;
use torque_net::handshake::ConnId;
use torque_scene::BinGeometry;
use torque_server::net::ghosts::GhostHub;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

const GHOST_BITS: u8 = 8;

struct Scene {
    server: Server<HostParts<TorqueNet>>,
    hub: GhostHub<TgeGhostGame>,
}

impl Scene {
    /// A server with `clients` connections, each with a `Client<i>` object and no control object, and a `Player` datablock.
    fn new(clients: u32) -> Scene {
        let mut vm = torque_game::testing::vm();
        let files = MemFiles::new();
        let b = [-0.5, -0.5, 0.0, 0.5, 0.5, 2.0];
        let mut body = torque_shapes::synth::hull_shape(
            b,
            &[("detail2", b)],
            &["eye", "cam"],
            &["root", "look", "hold"],
        );
        body.shape.names[1] = b"base".to_vec();
        files.insert("body.dts", &torque_shapes::write::write_dts(&body));
        vm.set_file_host(files);
        let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
        torque_server::game::install(&mut vm, &state);
        let mut server = Server::with_state(vm, Rc::clone(&state));
        let (transport, _end) = MemoryTransport::new();
        let transport: SharedTransport = Rc::new(RefCell::new(transport));
        let container = Rc::new(Container::new(ContainerConfig {
            bins: BinGeometry::default(),
            indexes: Vec::new(),
            candidate_order: None,
        }));
        let shared = attach_host(
            &mut server.vm,
            Host {
                transport,
                game: TorqueNet::default(),
                engine: Rc::clone(&state),
                clock: NetClock::Sim,
                ghosts: TgeGhostGame,
                container,
                static_writes: None,
            },
        );
        server
            .attach_net(Rc::new(NetPasses(Rc::clone(&shared))))
            .expect("a net host");
        server.run(TICK_MS, |_| false);
        let mut script = String::from(
            "datablock PlayerData(Body) { shapeFile = \"body.dts\"; boundingBox = \"1 1 2\"; maxDamage = 100; };\n\
             datablock CameraData(EyeData) {};\n",
        );
        for i in 0..clients {
            script += &format!("new ScriptObject(Client{i});\n");
        }
        server.vm.eval(script.as_bytes()).expect("the scene");
        let hub = shared.borrow().ghosts.clone().expect("a hub");
        let scene = Scene { server, hub };
        for i in 0..clients {
            let client = scene.find(&format!("Client{i}"));
            scene.hub.connect(ConnId(i + 1), client, GHOST_BITS);
        }
        scene
    }

    fn find(&self, name: &str) -> ObjId {
        self.server
            .vm
            .objects()
            .find(name.as_bytes())
            .unwrap_or_else(|| panic!("no object {name}"))
    }

    fn spawn(&mut self, class: &str, name: &str) -> ObjId {
        let block = if class == "Camera" { "EyeData" } else { "Body" };
        let script = format!("new {class}({name}) {{ dataBlock = {block}; }};");
        self.server.vm.eval(script.as_bytes()).expect("spawns");
        self.find(name)
    }

    fn client_of(&self, obj: ObjId) -> Option<ObjId> {
        self.hub.controlling_client_of(&self.server.vm, obj)
    }
}

#[test]
fn the_index_follows_respawn_switch_chain_release_and_disconnect() {
    let mut s = Scene::new(2);
    let (conn_a, conn_b) = (ConnId(1), ConnId(2));
    let (client_a, client_b) = (s.find("Client0"), s.find("Client1"));
    let first = s.spawn("Player", "First");
    let bot = s.spawn("Player", "Bot");
    assert_eq!(s.client_of(first), None, "nobody controls it yet");

    assert!(s.hub.adopt_control(conn_a, first));
    assert_eq!(s.client_of(first), Some(client_a));
    assert_eq!(s.client_of(bot), None, "a bot has no client");

    // Respawn: the Player is deleted, a new one takes over the connection.
    s.server.vm.eval(b"First.delete();").unwrap();
    assert_eq!(s.hub.control_of(conn_a), None);
    let second = s.spawn("Player", "Second");
    assert!(s.hub.adopt_control(conn_a, second));
    assert_eq!(s.client_of(second), Some(client_a));

    // Switch: the old control object is let go, and a connection that takes an object takes it from the other.
    let third = s.spawn("Player", "Third");
    assert!(s.hub.adopt_control(conn_a, third));
    assert_eq!(
        (s.client_of(second), s.client_of(third)),
        (None, Some(client_a))
    );
    assert!(s.hub.adopt_control(conn_b, third));
    assert_eq!(s.hub.control_of(conn_a), None);
    assert_eq!(s.client_of(third), Some(client_b));

    // A control object that controls another hands its client to it, and takes it back on letting go.
    let cam = s.spawn("Camera", "Eye");
    set_control_object(&mut s.server.vm, third, Some(cam));
    assert_eq!(s.client_of(cam), Some(client_b));
    set_control_object(&mut s.server.vm, third, None);
    assert_eq!(s.client_of(cam), None);

    // `set_control` and `release_control` keep the index too.
    s.hub.set_control(conn_a, Some(second));
    assert_eq!(s.client_of(second), Some(client_a));
    s.hub.set_control(conn_a, Some(bot));
    assert_eq!(
        (s.client_of(second), s.client_of(bot)),
        (None, Some(client_a))
    );
    s.hub.release_control(conn_a);
    assert_eq!(s.client_of(bot), None);

    // A connection that goes takes its control object's client with it; the object itself stays.
    assert_eq!(s.client_of(third), Some(client_b));
    s.hub.remove_conn(conn_b);
    assert_eq!(s.client_of(third), None);
    assert_eq!(s.hub.controlling_camera_pos(&s.server.vm, third), None);
}

/// TGE's chain, counting how often the hub asks it for a link.
struct Counting(Rc<Cell<usize>>);

impl ControlChain for Counting {
    fn control_object(&self, vm: &Vm, id: ObjId) -> Option<ObjId> {
        self.0.set(self.0.get() + 1);
        TorqueChain.control_object(vm, id)
    }
}

#[test]
fn a_lookup_does_not_walk_the_connections() {
    const CLIENTS: u32 = 64;
    let mut s = Scene::new(CLIENTS);
    let links = Rc::new(Cell::new(0));
    set_control_chain(&mut s.server.vm, Rc::new(Counting(Rc::clone(&links))));
    let players: Vec<ObjId> = (0..CLIENTS)
        .map(|i| s.spawn("Player", &format!("P{i}")))
        .collect();
    for (i, p) in (1..).zip(&players) {
        s.hub.adopt_control(ConnId(i), *p);
    }
    let bots: Vec<ObjId> = (0..CLIENTS)
        .map(|i| s.spawn("Player", &format!("B{i}")))
        .collect();
    let cam = s.spawn("Camera", "Eye");
    set_control_object(&mut s.server.vm, players[7], Some(cam));
    links.set(0);

    for (i, p) in (0..).zip(&players) {
        assert_eq!(s.client_of(*p), Some(s.find(&format!("Client{i}"))));
    }
    for b in &bots {
        assert_eq!(s.client_of(*b), None);
    }
    assert_eq!(
        links.get(),
        0,
        "a control object and a bot ask the chain for nothing"
    );

    assert_eq!(s.client_of(cam), Some(s.find("Client7")));
    assert!(
        links.get() <= 2,
        "an object further down is checked against one chain, not one per connection: {}",
        links.get()
    );
}

#[test]
fn set_control_takes_an_object_from_the_connection_that_holds_it() {
    let mut s = Scene::new(2);
    let (conn_a, conn_b) = (ConnId(1), ConnId(2));
    let (client_a, client_b) = (s.find("Client0"), s.find("Client1"));
    let player = s.spawn("Player", "Held");
    s.hub.set_control(conn_a, Some(player));
    assert_eq!(s.client_of(player), Some(client_a));

    s.hub.set_control(conn_b, Some(player));
    assert_eq!(s.hub.control_of(conn_a), None, "the first holder lets go");
    assert_eq!(s.hub.scope_object_of(conn_a), None);
    assert_eq!(s.hub.control_of(conn_b), Some(player));
    assert_eq!(s.client_of(player), Some(client_b));

    // The index kept no entry for the first connection: letting go of the object leaves nobody with it.
    s.hub.set_control(conn_b, None);
    assert_eq!(s.client_of(player), None);
    // Taking its own object again changes nothing.
    s.hub.set_control(conn_a, Some(player));
    s.hub.set_control(conn_a, Some(player));
    assert_eq!(s.client_of(player), Some(client_a));
}

/// Chains whose links are objects named in a table: `controller` is the table read backwards when the chain says so.
struct Linked {
    next: Rc<RefCell<Vec<(ObjId, ObjId)>>>,
    mirrored: bool,
}

impl ControlChain for Linked {
    fn control_object(&self, _vm: &Vm, id: ObjId) -> Option<ObjId> {
        self.next
            .borrow()
            .iter()
            .find_map(|(from, to)| (*from == id).then_some(*to))
    }

    fn controller(&self, vm: &Vm, id: ObjId) -> Option<ObjId> {
        if !self.mirrored {
            return TorqueChain.controller(vm, id);
        }
        self.next
            .borrow()
            .iter()
            .find_map(|(from, to)| (*to == id).then_some(*from))
    }
}

#[test]
fn a_chain_that_overrides_both_links_is_found_and_one_that_does_not_finds_nothing() {
    for mirrored in [true, false] {
        let mut s = Scene::new(2);
        let (client_a, client_b) = (s.find("Client0"), s.find("Client1"));
        let head_a = s.spawn("Player", "HeadA");
        let head_b = s.spawn("Player", "HeadB");
        s.server
            .vm
            .eval(b"new ScriptObject(MidA); new ScriptObject(TailA);")
            .unwrap();
        let (mid, tail) = (s.find("MidA"), s.find("TailA"));
        let next = Rc::new(RefCell::new(vec![(head_a, mid), (mid, tail)]));
        set_control_chain(
            &mut s.server.vm,
            Rc::new(Linked {
                next: Rc::clone(&next),
                mirrored,
            }),
        );
        s.hub.set_control(ConnId(1), Some(head_a));
        s.hub.set_control(ConnId(2), Some(head_b));

        // The control objects themselves are found whichever way the chain is declared.
        assert_eq!(s.client_of(head_a), Some(client_a));
        assert_eq!(s.client_of(head_b), Some(client_b));
        // The objects further down are found when `controller` mirrors the links, and are `None` when it does not: never
        // another connection's client.
        let found = mirrored.then_some(client_a);
        assert_eq!(s.client_of(mid), found, "mirrored: {mirrored}");
        assert_eq!(s.client_of(tail), found, "mirrored: {mirrored}");
    }
}
