//! A server's ghost pass over TGE's objects: the hub, `TgeSource` and the codecs of the classes, 64 connections with 1,024
//! ghostable objects each, one send pass a tick (collapse, then one section per connection, each acknowledged). The dirty
//! objects are marked the way the scene classes mark them.
//!
//! Cases: `static` (nothing marked), `dirty_100` and `dirty_all` (1,024 marked). One iteration is a tick of all 64 connections.

use std::cell::RefCell;
use std::hint::black_box;
use std::rc::Rc;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use torque_game::scene::container::{Container, ContainerConfig};
use torque_net::bits::BitWriter;
use torque_net::events::ConnectionMessage;
use torque_net::handshake::ConnId;
use torque_net::huffman::Table;
use torque_scene::BinGeometry;
use torque_server::net::ghosts::SourceEnv;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

const CONNECTIONS: u32 = 64;
const OBJECTS: usize = 1_024;
const GHOST_BITS: u8 = 11;
const PACKET_BYTES: usize = 1_000;

type NetServer = Server<HostParts<TorqueNet>>;

struct Scene {
    server: NetServer,
    shared: Rc<RefCell<NetShared<TorqueNet>>>,
    table: Table,
    props: Vec<ObjId>,
    conns: Vec<ConnId>,
    seq: u32,
    rng: u64,
}

impl Scene {
    fn new() -> Scene {
        let mut vm = Vm::new();
        torque_game::specs::install(&mut vm);
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
            "datablock CameraData(EyeData) {};\ndatablock StaticShapeData(PropData) { shapeFile = \"car.dts\"; };\n",
        );
        for i in 0..CONNECTIONS {
            script += &format!(
                "new Camera(Eye{i}) {{ dataBlock = EyeData; }};\nnew ScriptObject(Client{i});\n"
            );
        }
        for i in 0..OBJECTS {
            script += &format!(
                "new StaticShape(Prop{i}) {{ dataBlock = PropData; position = \"{} {} 0\"; }};\n",
                i % 32,
                i / 32
            );
        }
        server.vm.eval(script.as_bytes()).expect("the scene");
        let hub = shared.borrow().ghosts.clone().expect("a hub");
        let find = |s: &str| {
            server
                .vm
                .objects()
                .find(s.as_bytes())
                .unwrap_or_else(|| panic!("no object {s}"))
        };
        let props: Vec<ObjId> = (0..OBJECTS).map(|i| find(&format!("Prop{i}"))).collect();
        let table = Table::torque();
        let mut conns = Vec::new();
        for i in 0..CONNECTIONS {
            let conn = ConnId(i + 1);
            let client = find(&format!("Client{i}"));
            let eye = find(&format!("Eye{i}"));
            hub.connect(conn, client, GHOST_BITS);
            hub.set_control(conn, Some(eye));
            let env = SourceEnv {
                vm: &server.vm,
                table: None,
                ghost_bits: GHOST_BITS,
            };
            hub.activate(env, conn).expect("activates");
            let seq = hub.with(|h| h.conns[&conn].mgr.sequence());
            hub.on_message(
                conn,
                &ConnectionMessage {
                    seq,
                    message: 1,
                    count: 0,
                },
            )
            .expect("ready");
            conns.push(conn);
        }
        let mut scene = Scene {
            server,
            shared,
            table,
            props,
            conns,
            seq: 1,
            rng: 12_345,
        };
        for _ in 0..400 {
            if scene.pass() == 0 {
                break;
            }
        }
        let ghosts = hub.with(|h| h.conns[&scene.conns[0]].mgr.ghost_count());
        assert!(ghosts > OBJECTS, "{ghosts} ghosts");
        scene
    }

    /// One send pass: a packet to every connection, acknowledged. The records written.
    fn pass(&mut self) -> usize {
        let hub = self.shared.borrow().ghosts.clone().expect("a hub");
        hub.collapse();
        hub.begin_pass();
        let mut records = 0;
        for &conn in &self.conns {
            let env = SourceEnv {
                vm: &self.server.vm,
                table: Some(&self.table),
                ghost_bits: GHOST_BITS,
            };
            let mut w = BitWriter::with_limit(PACKET_BYTES * 8);
            w.enable_string_buffer();
            let stats = hub
                .write_section(env, conn, &mut w, PACKET_BYTES * 8, self.seq)
                .expect("a section");
            black_box(&w);
            hub.on_notify(conn, self.seq, true);
            self.seq += 1;
            records += stats.records;
        }
        hub.end_pass(0);
        records
    }

    fn mark(&mut self, dirty: usize) {
        let hub = self.shared.borrow().ghosts.clone().expect("a hub");
        for k in 0..dirty {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let at = if dirty >= OBJECTS {
                k
            } else {
                (self.rng % OBJECTS as u64) as usize
            };
            hub.set_mask(self.props[at], 0x02);
        }
    }
}

fn ticks(c: &mut Criterion) {
    let mut group = c.benchmark_group("ghost_tick");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));
    for (name, dirty) in [
        ("static", 0usize),
        ("dirty_100", 100),
        ("dirty_all", OBJECTS),
    ] {
        group.bench_function(format!("{name}/tick_64_conns"), |b| {
            let mut scene = Scene::new();
            let mut records = 0;
            b.iter_custom(|iters| {
                let start = Instant::now();
                for _ in 0..iters {
                    scene.mark(dirty);
                    records += scene.pass();
                }
                start.elapsed()
            });
            black_box(records);
        });
    }
    group.finish();
}

criterion_group!(benches, ticks);
criterion_main!(benches);
