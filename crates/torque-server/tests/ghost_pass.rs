//! The hub's ghost pass over TGE's objects once it has settled: a full query that finds the world as it left it only counts its
//! objects, and anything that changes the world still reaches the connection.

use std::cell::RefCell;
use std::rc::Rc;

use torque_game::scene::container::{Container, ContainerConfig};
use torque_net::bits::BitWriter;
use torque_net::events::ConnectionMessage;
use torque_net::handshake::ConnId;
use torque_net::huffman::Table;
use torque_scene::BinGeometry;
use torque_server::net::ghosts::{GhostHub, SourceEnv};
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;

const GHOST_BITS: u8 = 8;
const CONN: ConnId = ConnId(1);

struct Scene {
    server: Server<HostParts<TorqueNet>>,
    hub: GhostHub<TgeGhostGame>,
    table: Table,
    seq: u32,
}

impl Scene {
    fn new(props: usize) -> Scene {
        let mut vm = torque_game::testing::vm();
        let files = MemFiles::new();
        let b = [-1.0, -2.0, 0.0, 1.0, 2.0, 1.0];
        let car = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &[]);
        files.insert("car.dts", &torque_shapes::write::write_dts(&car));
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
            "datablock CameraData(EyeData) {};\ndatablock StaticShapeData(PropData) { shapeFile = \"car.dts\"; };\nnew Camera(Eye) { dataBlock = EyeData; };\nnew ScriptObject(Client);\n",
        );
        for i in 0..props {
            script += &format!("new StaticShape(Prop{i}) {{ dataBlock = PropData; }};\n");
        }
        server.vm.eval(script.as_bytes()).expect("the scene");
        let hub = shared.borrow().ghosts.clone().expect("a hub");
        let client = server.vm.objects().find(b"Client").expect("client");
        let eye = server.vm.objects().find(b"Eye").expect("eye");
        hub.connect(CONN, client, GHOST_BITS);
        hub.set_control(CONN, Some(eye));
        let env = SourceEnv {
            vm: &server.vm,
            table: None,
            ghost_bits: GHOST_BITS,
        };
        hub.activate(env, CONN).expect("activates");
        let seq = hub.with(|h| h.conns[&CONN].mgr.sequence());
        hub.on_message(
            CONN,
            &ConnectionMessage {
                seq,
                message: 1,
                count: 0,
            },
        )
        .expect("ready");
        Scene {
            server,
            hub,
            table: Table::torque(),
            seq: 1,
        }
    }

    /// One send pass: one acknowledged packet; its records.
    fn pass(&mut self) -> usize {
        self.hub.collapse();
        self.hub.begin_pass();
        let env = SourceEnv {
            vm: &self.server.vm,
            table: Some(&self.table),
            ghost_bits: GHOST_BITS,
        };
        let mut w = BitWriter::with_limit(8_000);
        w.enable_string_buffer();
        let stats = self
            .hub
            .write_section(env, CONN, &mut w, 8_000, self.seq)
            .expect("a section");
        self.hub.on_notify(CONN, self.seq, true);
        self.hub.end_pass(0);
        self.seq += 1;
        stats.records
    }

    fn settle(&mut self) {
        for _ in 0..50 {
            if self.pass() == 0 {
                return;
            }
        }
        panic!("the ghosts never settled");
    }

    fn ghosts(&self) -> usize {
        self.hub.with(|h| h.conns[&CONN].mgr.ghost_count())
    }

    fn scan_units(&self) -> u64 {
        self.hub.totals(CONN).expect("totals").scan_units
    }
}

#[test]
fn a_settled_pass_costs_the_same_scan_work_and_sends_nothing() {
    let mut s = Scene::new(40);
    s.settle();
    assert!(s.ghosts() > 40);
    // The first quiet pass may still be the one that settles; a few later ones cost the same to the unit and send nothing.
    s.pass();
    let before = s.scan_units();
    assert_eq!(s.pass(), 0);
    let one_pass = s.scan_units() - before;
    assert!(one_pass > 40, "{one_pass} units");
    let before = s.scan_units();
    for _ in 0..5 {
        assert_eq!(s.pass(), 0);
    }
    assert_eq!(s.scan_units() - before, 5 * one_pass);
}

#[test]
fn objects_added_or_removed_after_the_pass_settled_still_reach_the_connection() {
    let mut s = Scene::new(40);
    s.settle();
    let held = s.ghosts();
    s.server
        .vm
        .eval(b"new StaticShape(Late) { dataBlock = PropData; };")
        .unwrap();
    s.settle();
    assert_eq!(s.ghosts(), held + 1, "the new object is ghosted");
    let late = s.server.vm.objects().find(b"Late").expect("late");
    assert!(s.hub.ghost_index(CONN, late).is_some());
    s.server.vm.eval(b"Prop3.delete(); Late.delete();").unwrap();
    s.settle();
    assert_eq!(s.ghosts(), held - 1, "both are let go of and freed");
}
