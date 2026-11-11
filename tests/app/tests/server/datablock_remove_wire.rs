//! Datablock removal reaching a real client over the memory transport in the engine's protocol: the client purges the record
//! and its script object once no ghost names the datablock, a datablock deleted over a ghost's object stays until the ghost
//! leaves and goes within two ticks after, a hostile remove of an unknown id is counted, one of a datablock in use stays
//! retired and a body sent for it is refused, more retired datablocks than the cap end the connection with the reason, and in
//! Torque's protocol a removal leaves the client its datablock. No clock is read.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_client::conn::tge::RETIRED_DATABLOCKS_REASON;
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_engine::EngineBuilder;
use torque_ghost_world::world::MAX_RETIRED_DATABLOCKS;
use torque_ghosts::TgeClass;
use torque_net::handshake::ConnId;
use torque_ts_vm::vm::Vm;

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_320);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Tge,
    Engine,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Tge => "tge",
            Mode::Engine => "engine",
        }
    }
}

struct Rig {
    server: GameServer,
    client: GameClient,
    addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
}

impl Rig {
    fn new(mode: Mode) -> Rig {
        let mut files = GameFiles::default();
        files.insert(
            "server.cs",
            include_bytes!("datablock_remove_wire/server.cs"),
        );
        files.insert("main.cs", include_bytes!("datablock_remove_wire/main.cs"));
        let b = [-1.0, -1.0, 0.0, 1.0, 1.0, 1.0];
        let shape = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &["ambient"]);
        files.insert("a.dts", &torque_shapes::write::write_dts(&shape));
        let toml = format!(
            "[game]\nname = \"Datablock removal test\"\nserver = \"server.cs\"\nclient = \"main.cs\"\n\n[net]\nprotocol = \"{}\"\n\n[plugins]\ndisable = [\"torque.audio\", \"torque.water\", \"torque.terrain\", \"torque.server-query\"]\n",
            mode.label()
        );
        files.insert("game.toml", toml.as_bytes());
        let config = GameConfig::load(&files).expect("game.toml");
        let mut builder = EngineBuilder::new();
        builder.add_plugins(DefaultPlugins).config(&config);
        let engine = builder.build().expect("the engine builds");
        let server = GameServer::start_engine(
            &engine,
            files.mem_files(),
            &ServerStart {
                listen: Listen::Memory(SERVER),
                globals: &[],
                start: "startIt",
                args: &[],
            },
        )
        .expect("the server starts");
        let mut client_builder = EngineBuilder::new();
        client_builder.add_plugins(DefaultPlugins).config(&config);
        let client_engine = client_builder.build().expect("the client's engine builds");
        let port = 50_321;
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let mem = MemoryDatagrams {
            local_port: port,
            ..MemoryDatagrams::default()
        };
        let (sent, incoming) = (mem.sent.clone(), mem.incoming.clone());
        let mut builder = ClientBuilder::from_engine(
            &client_engine,
            Some(files.mem_files()),
            ClientNet::External,
        )
        .expect("the client builds");
        builder.set_socket(Box::new(mem), NetPolicy::Loopback);
        builder.exec_entry().expect("the client's script");
        let client: GameClient = builder.build();
        Rig {
            server,
            client,
            addr,
            sent,
            incoming,
        }
    }

    /// A rig whose client has connected, has its mission's datablocks and ghosts from the camera.
    fn connected(mode: Mode) -> Rig {
        let mut rig = Rig::new(mode);
        rig.client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        assert!(
            rig.run_until(400, |r| r.global("$Done") == "1"),
            "{mode:?}: the mission chain ends: {:?}",
            rig.server.drain_log()
        );
        rig.run(5);
        assert_eq!(rig.client_ids(), rig.server_ids(), "{mode:?}");
        rig
    }

    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SERVER {
                end.inject(SocketAddr::V4(self.addr), &data);
            }
        }
        for (to, data) in end.take_sent() {
            if SocketAddr::V4(self.addr) == to {
                let SocketAddr::V4(from) = SERVER else {
                    continue;
                };
                self.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        self.client.step(32);
        self.carry();
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }

    fn run_until(&mut self, limit: usize, mut done: impl FnMut(&Rig) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    fn global(&self, name: &str) -> String {
        self.server
            .vm()
            .global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }

    fn eval(&mut self, code: &str) -> String {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_else(|e| panic!("{code}: {e}"))
    }

    fn server_id(&mut self, name: &str) -> u32 {
        self.eval(&format!("return {name}.getId();"))
            .parse()
            .expect("an id")
    }

    /// Runs until the server has heard the next round's answer.
    fn round(&mut self) {
        let want = (self.global("$Updated").parse::<usize>().unwrap_or(0) + 1).to_string();
        assert_eq!(self.eval("return updateDataBlocks();"), "1");
        assert!(
            self.run_until(600, |r| r.global("$Updated") == want),
            "the round ends: {:?}",
            self.server.drain_log()
        );
    }

    fn server_ids(&self) -> BTreeSet<u32> {
        datablock_ids(self.server.vm())
    }

    /// The client's datablocks: the members of its connection object in the datablock id range.
    fn client_ids(&self) -> BTreeSet<u32> {
        let st = self.client.host.client.borrow();
        let Some(conn) = st.conn.conn.as_ref() else {
            return BTreeSet::new();
        };
        let objects = self.client.vm().objects();
        objects
            .members(conn.obj)
            .map(|m| {
                (0..m.len())
                    .filter_map(|i| m.get(i))
                    .filter(|&id| objects.is_datablock_id(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The client's record of datablock `id`: whether the world holds it, and whether it is retired.
    fn record(&self, id: u32) -> (bool, bool) {
        self.client
            .with_world(|w| {
                (
                    w.records.datablocks.contains_key(&id),
                    w.records.is_datablock_retired(id),
                )
            })
            .unwrap_or((false, false))
    }

    fn shapes(&self) -> usize {
        self.client
            .with_world(|w| {
                w.records
                    .ghosts
                    .values()
                    .filter(|g| g.class == Some(TgeClass::StaticShape))
                    .count()
            })
            .unwrap_or(0)
    }

    fn refused(&self) -> u64 {
        self.client.host.client.borrow().datablocks_refused
    }

    fn removes_ignored(&self) -> u64 {
        self.client.host.client.borrow().datablock_removes_ignored
    }

    fn still_connected(&self) -> bool {
        self.client.host.client.borrow().conn.conn.is_some()
    }

    /// The server's connection id of the client.
    fn conn(&self) -> ConnId {
        let shared = self.server.shared().expect("the network host").borrow();
        let rec = shared
            .conns
            .values()
            .find(|r| r.addr == SocketAddr::V4(self.addr))
            .expect("the connection");
        rec.conn.expect("accepted")
    }

    /// A server that is not the engine's: removals posted whatever it sent.
    fn hostile_removes(&self, ids: &[u32]) {
        let conn = self.conn();
        self.server
            .shared()
            .expect("the network host")
            .borrow()
            .events
            .post_datablock_removes(conn, ids)
            .expect("posted");
    }
}

fn datablock_ids(vm: &Vm) -> BTreeSet<u32> {
    let objects = vm.objects();
    objects
        .members(objects.datablock_group())
        .map(|m| (0..m.len()).filter_map(|i| m.get(i)).collect())
        .unwrap_or_default()
}

/// Declares `StaticShapeData(name)` and, when `shape` says so, a StaticShape `S<name>` on it; the datablock's id.
fn declare(rig: &mut Rig, name: &str, shape: bool, at: usize) -> u32 {
    let mut code = format!("datablock StaticShapeData({name}) {{ shapeFile = \"a.dts\"; }};");
    if shape {
        code.push_str(&format!(
            "new StaticShape(S{name}) {{ dataBlock = {name}; position = \"{} {} 0\"; }};",
            at % 16,
            at / 16
        ));
    }
    rig.eval(&code);
    rig.server_id(name)
}

#[test]
fn a_removal_reaches_the_client_which_purges_the_record_and_the_object_once_no_ghost_names_it() {
    let mut rig = Rig::connected(Mode::Engine);
    let lone = declare(&mut rig, "Lone", false, 0);
    let used = declare(&mut rig, "Used", true, 1);
    rig.round();
    assert!(rig.run_until(100, |r| r.shapes() == 1 && r.client_ids() == r.server_ids()));
    assert!(rig.client.vm().objects().contains(lone));
    // No user: gone from the server, the client's record and its object follow within a few ticks.
    assert_eq!(rig.eval("return removeDataBlock(Lone);"), "1");
    assert!(
        rig.run_until(20, |r| !r.record(lone).0
            && !r.client.vm().objects().contains(lone)),
        "the client let the datablock go: {:?}",
        rig.record(lone)
    );
    assert_eq!(rig.client_ids(), rig.server_ids());
    // Deleted over its shape: the server keeps it retiring and tells nobody; the client keeps it.
    rig.eval("Used.delete();");
    rig.run(10);
    assert_eq!(rig.record(used), (true, false));
    assert!(rig.client.vm().objects().contains(used));
    assert_eq!(rig.client_ids(), rig.server_ids());
    // The shape goes: the server frees the datablock and the client purges it within two ticks of the ghost leaving.
    rig.eval("SUsed.delete();");
    assert!(rig.run_until(100, |r| r.shapes() == 0), "the ghost leaves");
    assert!(
        rig.run_until(3, |r| !r.record(used).0
            && !r.client.vm().objects().contains(used)),
        "purged within two ticks: {:?}",
        rig.record(used)
    );
    assert_eq!(rig.client_ids(), rig.server_ids());
    assert_eq!(rig.refused(), 0);
    assert_eq!(rig.removes_ignored(), 0);
    assert!(rig.still_connected());
}

#[test]
fn a_hostile_remove_of_an_unknown_id_is_counted_and_one_in_use_stays_retired_with_its_body_refused()
{
    let mut rig = Rig::connected(Mode::Engine);
    let used = declare(&mut rig, "Used", true, 0);
    rig.round();
    assert!(rig.run_until(100, |r| r.shapes() == 1 && r.client_ids().contains(&used)));
    rig.hostile_removes(&[used + 700]);
    assert!(
        rig.run_until(50, |r| r.removes_ignored() == 1),
        "the unknown id is counted"
    );
    assert!(rig.still_connected());
    rig.hostile_removes(&[used]);
    assert!(
        rig.run_until(50, |r| r.record(used) == (true, true)),
        "retired, kept for its ghost"
    );
    rig.run(10);
    assert_eq!(rig.record(used), (true, true), "still in use");
    assert!(rig.client.vm().objects().contains(used));
    // A body for the retired id (the server edits and resends it) is refused and counted.
    rig.eval("Used.mass = 7;");
    let refused_before = rig.refused();
    rig.round();
    assert_eq!(
        rig.refused(),
        refused_before + 1,
        "the body for a retired id"
    );
    assert_eq!(rig.record(used), (true, true));
    assert!(rig.still_connected());
    // The ghost leaves: purged within two ticks, object and all.
    rig.eval("SUsed.delete();");
    assert!(rig.run_until(100, |r| r.shapes() == 0));
    assert!(rig.run_until(3, |r| r.record(used) == (false, false)));
    assert!(!rig.client.vm().objects().contains(used));
    assert_eq!(rig.removes_ignored(), 1);
    assert!(rig.still_connected());
}

#[test]
fn more_retired_datablocks_than_the_cap_end_the_connection_with_the_reason() {
    let mut rig = Rig::connected(Mode::Engine);
    let n = MAX_RETIRED_DATABLOCKS + 1;
    let ids: Vec<u32> = (0..n)
        .map(|i| declare(&mut rig, &format!("D{i}"), true, i))
        .collect();
    rig.round();
    assert!(
        rig.run_until(400, |r| r.shapes() == n && r.client_ids() == r.server_ids()),
        "{} shapes, {} ids",
        rig.shapes(),
        rig.client_ids().len()
    );
    rig.hostile_removes(&ids);
    assert!(
        rig.run_until(400, |r| !r.still_connected()),
        "the connection ends"
    );
    assert_eq!(
        rig.client.global("$DropReason"),
        RETIRED_DATABLOCKS_REASON,
        "the reason reaches the scripts"
    );
}

#[test]
fn in_torques_protocol_a_removal_leaves_the_client_its_datablock() {
    let mut rig = Rig::connected(Mode::Tge);
    let lone = declare(&mut rig, "Lone", false, 0);
    rig.round();
    assert!(rig.run_until(50, |r| r.client_ids().contains(&lone)));
    assert_eq!(rig.eval("return removeDataBlock(Lone);"), "1");
    rig.run(30);
    assert!(!rig.server_ids().contains(&lone));
    assert!(rig.client_ids().contains(&lone), "a stock client keeps it");
    assert_eq!(rig.record(lone), (true, false));
    assert_eq!(rig.removes_ignored(), 0);
    assert!(rig.still_connected());
}
