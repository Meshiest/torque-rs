//! The client's side of ghosting over TGE's records: a recorded stream of ghost sections (about a thousand updates of
//! 1,024 static shapes, written by a real hub), read into the client's table by `GhostClient<TgeCodec>`, and the world applying
//! what changed. One iteration reads the whole stream.

use std::cell::RefCell;
use std::hint::black_box;
use std::rc::Rc;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use ghost_world::ClientWorld;
use ghost_world::tge::{TgeClientSet, TgeCodec};
use net::bits::{BitReader, BitWriter};
use net::classes::Kind;
use net::events::ConnectionMessage;
use net::ghost::GhostClient;
use net::handshake::ConnId;
use net::huffman::Table;
use net::profile::DEFAULT_DATABLOCK_BITS;
use torque_game::scene::container::{Container, ContainerConfig};
use torque_scene::BinGeometry;
use torque_server::net::codecs::net_codecs;
use torque_server::net::ghosts::SourceEnv;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use ts_vm::host::MemFiles;
use ts_vm::vm::Vm;

const OBJECTS: usize = 1_024;
const GHOST_BITS: u8 = 11;
const PACKET_BYTES: usize = 1_000;

/// One ghost section as the server wrote it.
struct Packet {
    bytes: Vec<u8>,
    bits: usize,
}

struct Recording {
    created: Vec<Packet>,
    updates: Vec<Packet>,
    update_records: usize,
    class_names: Vec<String>,
    class_bits: u8,
}

/// A hub with one connection and `OBJECTS` static shapes: the sections that create every ghost, then the sections of every object
/// marked dirty, each pass acknowledged.
fn record() -> Recording {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    let files = MemFiles::new();
    let b = [-1.0, -2.0, 0.0, 1.0, 2.0, 1.0];
    let car = shapes::synth::hull_shape(b, &[("detail2", b)], &[], &[]);
    files.insert("car.dts", &shapes::write::write_dts(&car));
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
            step_limit: None,
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
    for i in 0..OBJECTS {
        script += &format!(
            "new StaticShape(Prop{i}) {{ dataBlock = PropData; position = \"{} {} 0\"; }};\n",
            i % 32,
            i / 32
        );
    }
    server.vm.eval(script.as_bytes()).expect("the scene");
    let hub = shared.borrow().ghosts.clone().expect("a hub");
    let find = |s: &str| server.vm.objects().find(s.as_bytes()).expect("object");
    let conn = ConnId(1);
    hub.connect(conn, find("Client"), GHOST_BITS);
    hub.set_control(conn, Some(find("Eye")));
    let table = Table::torque();
    let env = |table| SourceEnv {
        vm: &server.vm,
        table,
        ghost_bits: GHOST_BITS,
    };
    hub.activate(env(None), conn).expect("activates");
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
    let mut seq = 1;
    let mut pass = || -> Option<(Packet, usize)> {
        hub.collapse();
        hub.begin_pass();
        let mut w = BitWriter::with_limit(PACKET_BYTES * 8);
        w.enable_string_buffer();
        let stats = hub
            .write_section(env(Some(&table)), conn, &mut w, PACKET_BYTES * 8, seq)
            .expect("a section");
        hub.on_notify(conn, seq, true);
        hub.end_pass(0);
        seq += 1;
        (stats.records > 0).then(|| {
            (
                Packet {
                    bits: w.bits(),
                    bytes: w.into_bytes(),
                },
                stats.records,
            )
        })
    };
    let mut created = Vec::new();
    while let Some((p, _)) = pass() {
        created.push(p);
    }
    let mut updates = Vec::new();
    let mut update_records = 0;
    for i in 0..OBJECTS {
        hub.set_mask(find(&format!("Prop{i}")), 0x02);
    }
    while let Some((p, n)) = pass() {
        updates.push(p);
        update_records += n;
    }
    let codecs = net_codecs(&server.vm);
    let classes = codecs.object_classes();
    Recording {
        created,
        updates,
        update_records,
        class_names: classes
            .classes(Kind::Object)
            .iter()
            .map(|c| c.name.clone())
            .collect(),
        class_bits: classes.id_bits(Kind::Object),
    }
}

fn client(rec: &Recording) -> GhostClient<TgeCodec> {
    let widths = ghosts::NetWidths::new(GHOST_BITS, DEFAULT_DATABLOCK_BITS);
    let names = rec.class_names.iter().map(String::as_str);
    let codec = TgeCodec::new(names, widths, Table::torque());
    let mut client = GhostClient::new(codec, rec.class_bits, GHOST_BITS);
    client.enable_log();
    client
}

fn read(client: &mut GhostClient<TgeCodec>, packets: &[Packet]) {
    for p in packets {
        let mut r = BitReader::with_bit_len(&p.bytes, p.bits);
        client.read_section(&mut r).expect("a section");
    }
}

fn reads(c: &mut Criterion) {
    let rec = record();
    assert!(
        rec.update_records >= 1_000,
        "{} updates",
        rec.update_records
    );
    println!(
        "ghost_read: {} creating sections, {} update sections with {} update records",
        rec.created.len(),
        rec.updates.len(),
        rec.update_records
    );
    let mut group = c.benchmark_group("ghost_read");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));
    group.bench_function("read_section_updates", |b| {
        let mut client = client(&rec);
        read(&mut client, &rec.created);
        client.take_log();
        b.iter(|| {
            read(&mut client, &rec.updates);
            black_box(client.take_log())
        });
    });
    group.bench_function("read_and_apply_updates", |b| {
        let mut client = client(&rec);
        let mut world = ClientWorld::<TgeClientSet>::default();
        read(&mut client, &rec.created);
        let changes = client.take_log();
        world.apply_ghosts(1, &changes, &client, [0.0; 3], &mut ());
        let mut record = 2;
        b.iter(|| {
            read(&mut client, &rec.updates);
            let changes = client.take_log();
            world.apply_ghosts(record, &changes, &client, [0.0; 3], &mut ());
            record += 1;
        });
    });
    group.finish();
}

criterion_group!(benches, reads);
criterion_main!(benches);
