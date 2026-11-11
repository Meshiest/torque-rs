//! Live datablock rounds on a real server and client over the memory transport, stepped by hand, once with Torque's protocol
//! and once with the engine's: datablocks added and edited mid-game reach the client's script group with their values, the
//! server's `onDataBlocksUpdated` and the client's `onDataBlocksChanged` run once per round, and a server that sends far more
//! than the client allows leaves the client's table at its cap with the rest counted and the connection up. No clock is read.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_engine::EngineBuilder;
use torque_ts_vm::vm::Vm;

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_310);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Tge,
    Engine,
}

impl Mode {
    const ALL: [Mode; 2] = [Mode::Tge, Mode::Engine];

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
        files.insert("server.cs", include_bytes!("datablock_live/server.cs"));
        files.insert("main.cs", include_bytes!("datablock_live/main.cs"));
        let toml = format!(
            "[game]\nname = \"Datablock round test\"\nserver = \"server.cs\"\nclient = \"main.cs\"\n\n[net]\nprotocol = \"{}\"\n\n[plugins]\ndisable = [\"torque.audio\", \"torque.water\", \"torque.terrain\", \"torque.server-query\"]\n",
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
        let port = 50_311;
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

    /// A rig whose client has connected and has its mission's datablocks.
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

    fn client_global(&self, name: &str) -> String {
        self.client.global(name)
    }

    fn eval(&mut self, code: &str) -> String {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_else(|e| panic!("{code}: {e}"))
    }

    fn client_eval(&mut self, code: &str) -> String {
        self.client.call("eval", &[code]).expect("client eval")
    }

    fn update(&mut self) -> usize {
        self.eval("return updateDataBlocks();")
            .parse()
            .expect("a count")
    }

    /// Runs until the server has heard the next round's answer.
    fn round(&mut self, mode: Mode) {
        let want = (self.global("$Updated").parse::<usize>().unwrap_or(0) + 1).to_string();
        assert!(
            self.run_until(600, |r| r.global("$Updated") == want),
            "{mode:?}: the round ends: {:?}",
            self.server.drain_log()
        );
    }

    fn server_ids(&self) -> BTreeSet<u32> {
        datablock_ids(self.server.vm())
    }

    /// The client's datablocks: the members of its connection object in the datablock id range (TGE's client keeps them
    /// there, not in DataBlockGroup).
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

    fn refused(&self) -> u64 {
        self.client.host.client.borrow().datablocks_refused
    }

    fn still_connected(&self) -> bool {
        self.client.host.client.borrow().conn.conn.is_some()
    }
}

fn datablock_ids(vm: &Vm) -> BTreeSet<u32> {
    let objects = vm.objects();
    objects
        .members(objects.datablock_group())
        .map(|m| (0..m.len()).filter_map(|i| m.get(i)).collect())
        .unwrap_or_default()
}

/// Declares `n` AudioDescriptions `Desc{from}`.. with `referenceDistance` (a raw float on the wire) a quarter of the index.
fn declare(rig: &mut Rig, from: usize, n: usize) -> Vec<u32> {
    let mut code = String::new();
    for i in from..from + n {
        code.push_str(&format!(
            "datablock AudioDescription(Desc{i}) {{ is3D = true; referenceDistance = {}; }};\n",
            i as f32 * 0.25
        ));
    }
    rig.eval(&code);
    (from..from + n)
        .map(|i| {
            rig.eval(&format!("return Desc{i}.getId();"))
                .parse()
                .expect("an id")
        })
        .collect()
}

#[test]
fn datablocks_added_and_edited_mid_game_reach_a_real_client_in_both_protocols() {
    for mode in Mode::ALL {
        let mut rig = Rig::connected(mode);
        let mut declared = 0;
        let mut seed = 1u64;
        for round in 0..50usize {
            // The edits of the round: a few of the client's datablocks, with values a seed picks.
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let edits = (seed >> 33) as usize % 4;
            let mut edited = Vec::new();
            for k in 0..edits.min(declared) {
                let i = (seed as usize >> (8 * k)) % declared;
                let v = 100.0 + round as f32 * 0.5;
                rig.eval(&format!("Desc{i}.referenceDistance = {v};"));
                let id: u32 = rig
                    .eval(&format!("return Desc{i}.getId();"))
                    .parse()
                    .unwrap();
                edited.push((id, v));
            }
            let new = declare(&mut rig, declared, 5);
            declared += 5;
            assert_eq!(
                rig.update(),
                1,
                "{mode:?} round {round}: one client reached"
            );
            rig.round(mode);
            assert_eq!(rig.client_ids(), rig.server_ids(), "{mode:?} round {round}");
            for (i, id) in new.iter().enumerate() {
                let want = (declared - 5 + i) as f32 * 0.25;
                let got: f32 = rig
                    .client_eval(&format!("return {id}.referenceDistance;"))
                    .parse()
                    .unwrap();
                assert!(
                    (got - want).abs() < 1e-6,
                    "{mode:?} round {round}: new {id}: {got} vs {want}"
                );
            }
            for (id, v) in &edited {
                let got: f32 = rig
                    .client_eval(&format!("return {id}.referenceDistance;"))
                    .parse()
                    .unwrap();
                assert!(
                    (got - v).abs() < 1e-6,
                    "{mode:?} round {round}: edited {id}: {got} vs {v}"
                );
            }
            assert_eq!(
                rig.client_global("$Changed"),
                (round + 1).to_string(),
                "{mode:?}: onDataBlocksChanged once per round"
            );
        }
        assert_eq!(rig.global("$Done"), "1", "{mode:?}: onDataBlocksDone once");
        assert_eq!(rig.global("$Updated"), "50", "{mode:?}");
        assert_eq!(rig.refused(), 0, "{mode:?}");
        assert!(rig.still_connected(), "{mode:?}");
        assert_eq!(rig.client_ids().len(), 3 + 250, "{mode:?}");
    }
}

#[test]
fn a_server_that_sends_past_the_clients_cap_leaves_its_table_at_the_cap_and_the_rest_counted() {
    for mode in Mode::ALL {
        let mut rig = Rig::connected(mode);
        rig.client_eval("$Pref::Client::MaxDataBlocks = 64;");
        rig.run(2);
        // 300 new datablocks in one round, then nine rounds that edit every one of them: 3,000 events.
        declare(&mut rig, 0, 300);
        rig.update();
        rig.round(mode);
        assert_eq!(
            rig.client_ids().len(),
            64,
            "{mode:?}: the table is at its cap"
        );
        assert_eq!(rig.refused(), 300 - 61, "{mode:?}: the adds past the cap");
        for round in 1..10 {
            let mut code = String::new();
            for i in 0..300 {
                code.push_str(&format!(
                    "Desc{i}.referenceDistance = {}.5;",
                    round * 10 + i % 10
                ));
            }
            rig.eval(&code);
            rig.update();
            rig.round(mode);
            assert_eq!(rig.client_ids().len(), 64, "{mode:?} round {round}");
            assert!(
                rig.still_connected(),
                "{mode:?} round {round}: a server's rate is under the live limit"
            );
        }
        assert_eq!(
            rig.refused(),
            (300 - 61) * 10,
            "{mode:?}: every refused event counted"
        );
        assert_eq!(rig.client_global("$Changed"), "10", "{mode:?}");
        assert_eq!(rig.global("$Counts"), "300,".repeat(10), "{mode:?}");
        // The datablocks the client kept carry the last round's values.
        let kept: Vec<u32> = rig.client_ids().into_iter().collect();
        let server_vm_value =
            |rig: &mut Rig, id: u32| rig.eval(&format!("return {id}.referenceDistance;"));
        for id in kept {
            let want = server_vm_value(&mut rig, id);
            let got = rig.client_eval(&format!("return {id}.referenceDistance;"));
            assert_eq!(got, want, "{mode:?}: {id}");
        }
        assert!(rig.still_connected(), "{mode:?}");
    }
}
