//! A ShapeBase's dissolve end to end, in each network protocol: a server script's `dissolve` reaches an engine client of the
//! engine's protocol with the ghost's cloak group (its serial, way, time, edge, emitter and look), the client's runner takes it
//! with its clock where the server's is, and `clearDissolve` takes it away. Over Torque's protocol the record carries none and
//! the client draws the statue whole. A real server and client over the memory transport, stepped by hand.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient, carry_between};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::{DefaultPlugins, set_protocol};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_engine::{Engine, EngineBuilder, ProtocolKind};
use torque_ghost_world::tge::shape_part;
use torque_ghosts::TgeClass;
use torque_ghosts::shapebase::DissolveField;
use torque_render_scene::dissolve::{Direction, Dissolves};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_010);

fn files() -> GameFiles {
    let mut files = GameFiles::default();
    files.insert("server.cs", include_bytes!("dissolve_wire/server.cs"));
    files.insert("main.cs", include_bytes!("dissolve_wire/main.cs"));
    files.insert("game.toml", include_bytes!("dissolve_wire/game.toml"));
    let b = [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5];
    files.insert(
        "art/box.dts",
        &torque_shapes::synth::solid_dts(b, &[("detail32", b)], [0, 0, 0, 32767], [0.0; 3], false),
    );
    files
}

fn engine(files: &GameFiles, protocol: ProtocolKind) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    set_protocol(&mut builder, protocol);
    builder.build().expect("the engine builds")
}

struct Rig {
    server: GameServer,
    client: GameClient,
}

impl Rig {
    fn new(protocol: ProtocolKind) -> Rig {
        let files = files();
        let server = GameServer::start_engine(
            &engine(&files, protocol),
            files.mem_files(),
            &ServerStart {
                listen: Listen::Memory(SERVER),
                globals: &[],
                start: "startIt",
                args: &[],
            },
        )
        .expect("the server starts");
        let client_engine = engine(&files, protocol);
        let mut builder = ClientBuilder::from_engine(
            &client_engine,
            Some(files.mem_files()),
            ClientNet::Memory {
                client: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50_011),
                server: SERVER,
            },
        )
        .expect("the client builds");
        builder.exec_entry().expect("the client's script");
        let mut client: GameClient = builder.build();
        client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        let mut rig = Rig { server, client };
        assert!(
            rig.run_until(400, |r| r.global("$Connected") == "1"),
            "the client joins"
        );
        rig
    }

    fn step(&mut self) {
        carry_between(&self.server, &[&self.client]);
        self.server.tick();
        carry_between(&self.server, &[&self.client]);
        self.client.step(32);
        carry_between(&self.server, &[&self.client]);
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

    fn eval(&mut self, code: &str) {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .unwrap_or_else(|e| panic!("{code}: {e}"));
    }

    /// The client's ghost of the statue (the one StaticShape).
    fn statue(&self) -> Option<u32> {
        self.client
            .with_world(|w| w.of_class(TgeClass::StaticShape).map(|(i, _)| i).next())
            .flatten()
    }

    /// The dissolve the statue's record carries on the client.
    fn dissolve(&self) -> Option<DissolveField> {
        let ghost = self.statue()?;
        self.client
            .with_world(|w| {
                let record = w.records.ghosts.get(&ghost)?.record.as_ref()?;
                shape_part(record)?
                    .body
                    .as_ref()?
                    .rare
                    .as_ref()?
                    .cloak?
                    .dissolve
            })
            .flatten()
    }

    /// The client's id of the server's datablock `name` (its id there: the wire's).
    fn datablock(&self, name: &str) -> u32 {
        let id = self
            .server
            .vm()
            .objects()
            .find(name.as_bytes())
            .expect("the datablock");
        assert!(
            self.client
                .with_world(|w| w.records.datablocks.contains_key(&id))
                .unwrap_or(false),
            "the client holds {name}"
        );
        id
    }
}

#[test]
fn a_dissolve_reaches_an_engine_client_with_its_look_and_clock_and_clears() {
    let mut rig = Rig::new(ProtocolKind::Engine);
    rig.eval("makeStatue();");
    assert!(
        rig.run_until(50, |r| r.statue().is_some()),
        "the statue is ghosted"
    );
    assert_eq!(rig.dissolve(), None);
    rig.eval("$Statue.dissolve(2000, 1, \"1 0.5 0 6\", 0.05, SandEmitter, 1);");
    assert!(
        rig.run_until(30, |r| r.dissolve().is_some()),
        "the dissolve arrives"
    );
    let d = rig.dissolve().expect("the dissolve");
    assert_eq!((d.serial, d.out, d.duration_ms), (1, true, 2000));
    assert!(d.elapsed_ms < 500, "{}", d.elapsed_ms);
    assert_eq!((d.edge, d.glow, d.edge_share), ([255, 128, 0], 48, 13));
    assert_eq!(d.emitter, Some(rig.datablock("SandEmitter")));
    assert!(d.tint);
    // The client's runner starts the clock where the server's is and makes the sand's emitter of the client's datablocks.
    let ghost = rig.statue().expect("the statue");
    let mut runner = Dissolves::default();
    rig.client
        .with_world(|w| runner.sync_world(&w.records))
        .expect("a world");
    let eaten = runner.eaten(ghost).expect("dissolving");
    assert!(
        (eaten - d.elapsed_ms as f32 / 2000.0).abs() < 1e-3,
        "{eaten}"
    );
    // A materialize is a new serial; a clear sends the group without one.
    rig.eval("$Statue.setDissolveLook(0.5, 1, 100); $Statue.dissolve(1000, 0);");
    assert!(
        rig.run_until(30, |r| r.dissolve().is_some_and(|d| d.serial == 2)),
        "the second dissolve arrives"
    );
    let d = rig.dissolve().expect("the dissolve");
    let look = torque_render_scene::dissolve::Look::of_field(&d);
    assert_eq!(look.direction, Direction::In);
    assert_eq!((look.cell, look.sweep, look.density), (0.5, 1.0, 100.0));
    rig.eval("$Statue.clearDissolve();");
    assert!(
        rig.run_until(30, |r| r.dissolve().is_none()),
        "the clear arrives"
    );
    rig.client
        .with_world(|w| runner.sync_world(&w.records))
        .expect("a world");
    assert_eq!(runner.eaten(ghost), None);
}

#[test]
fn torques_protocol_carries_no_dissolve() {
    let mut rig = Rig::new(ProtocolKind::Tge);
    rig.eval("makeStatue();");
    assert!(
        rig.run_until(50, |r| r.statue().is_some()),
        "the statue is ghosted"
    );
    rig.eval("$Statue.dissolve(2000, 1);");
    for _ in 0..30 {
        rig.step();
        assert_eq!(rig.dissolve(), None);
    }
}
