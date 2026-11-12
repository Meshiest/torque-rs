//! A real server and an engine client in one process over the memory transport, stepped by hand: one 32 ms tick of the
//! server, then one of the client, the datagrams carried across around each, a share of them dropped when the rig is lossy.
//! No clock is read.

// Each test file uses some of what is here.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
use torque_app::server::{GameServer, Listen};
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_collide::math::Matrix;
use torque_engine::{Engine, EngineBuilder, PluginGroup};
use torque_game::scene::access::matrix_of;
use torque_shapes::instance::ShapeInstance;
use torque_testkit::Rng;
use torque_ts_vm::sim::ObjId;

pub const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CLIENT_PORT: u16 = 50_001;

pub fn files() -> GameFiles {
    let mut files = GameFiles::default();
    files.insert("server.cs", include_bytes!("server.cs"));
    files.insert("main.cs", include_bytes!("main.cs"));
    files.insert("game.toml", include_bytes!("game.toml"));
    let dts = torque_shapes::write::write_dts;
    files.insert("kit/cube.dts", &dts(&torque_example_assets::shapes::cube()));
    files.insert(
        "kit/pistol.dts",
        &dts(&torque_example_assets::shapes::pistol()),
    );
    files
}

/// A plugin that asks for the scope control, as one that keeps objects out of a connection's ghosts does.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "test.scope")]
pub struct WantsScope;

impl torque_engine::Plugin for WantsScope {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().scope_control();
    }
}

/// An engine of the default plugins with the game's `game.toml` (the engine's own protocol: the hierarchy is on).
pub fn engine(files: &GameFiles) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    builder.add_plugins(WantsScope);
    builder.build().expect("the engine builds")
}

/// The same engine without the hierarchy plugin.
pub fn engine_without_hierarchy(files: &GameFiles) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins(DefaultPlugins.disable::<torque_plugin_hierarchy::HierarchyPlugin>())
        .config(&config);
    builder.build().expect("the engine builds")
}

pub struct Rig {
    pub server: GameServer,
    pub client: GameClient,
    pub engine: Engine,
    addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    /// The share of datagrams dropped each way, and the source of the drops.
    loss: f64,
    rng: Rng,
    pub ticks: u64,
}

impl Rig {
    /// A server and a client connected to it, with `loss` of the datagrams dropped each way.
    pub fn new(loss: f64) -> Rig {
        let files = files();
        let mut rig = Rig::start(loss, engine(&files));
        assert!(
            rig.run_until(600, |r| r.client.ghost_count() >= 1),
            "the client never got its camera: {:?}",
            rig.client.client_log
        );
        rig
    }

    /// A server and a client of `engine` that asked to connect.
    pub fn start(loss: f64, engine: Engine) -> Rig {
        let files = files();
        let server_engine = self::engine(&files);
        let server = GameServer::start_engine(
            &server_engine,
            files.mem_files(),
            &torque_app::server::ServerStart {
                listen: Listen::Memory(SERVER),
                globals: &[],
                start: "startHierarchy",
                args: &[],
            },
        )
        .expect("the server starts");
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, CLIENT_PORT);
        let mem = MemoryDatagrams {
            local_port: CLIENT_PORT,
            ..MemoryDatagrams::default()
        };
        let (sent, incoming) = (mem.sent.clone(), mem.incoming.clone());
        let mut builder =
            ClientBuilder::from_engine(&engine, Some(files.mem_files()), ClientNet::External)
                .expect("the client builds");
        builder.set_socket(Box::new(mem), NetPolicy::Loopback);
        builder.exec_entry().expect("the client's script");
        let mut client: GameClient = builder.build();
        client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        Rig {
            server,
            client,
            engine,
            addr,
            sent,
            incoming,
            loss,
            rng: Rng::new(0x41e7_a2c4),
            ticks: 0,
        }
    }

    fn dropped(loss: f64, rng: &mut Rng) -> bool {
        loss > 0.0 && (rng.below(1_000_000) as f64) < loss * 1_000_000.0
    }

    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        let up: Vec<_> = self.sent.borrow_mut().drain(..).collect();
        for (to, data) in up {
            if SocketAddr::V4(to) == SERVER && !Rig::dropped(self.loss, &mut self.rng) {
                end.inject(SocketAddr::V4(self.addr), &data);
            }
        }
        let SocketAddr::V4(from) = SERVER else {
            return;
        };
        for (to, data) in end.take_sent() {
            if to == SocketAddr::V4(self.addr) && !Rig::dropped(self.loss, &mut self.rng) {
                self.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    /// One tick of the server, then of the client.
    pub fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        self.client.step(32);
        self.carry();
        self.ticks += 1;
    }

    pub fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Runs until `done` holds, at most `limit` ticks; false when it never did.
    pub fn run_until(&mut self, limit: usize, mut done: impl FnMut(&Rig) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    /// Runs script `code` on the server.
    pub fn eval(&mut self, code: &str) -> String {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_else(|e| panic!("{code}: {e}"))
    }

    /// Moves StaticShape `obj` to `pose` (`setTransform` text); its `setTransform` marks the record's position, so clients see it move.
    pub fn move_to(&mut self, obj: ObjId, pose: &str) {
        self.eval(&format!("{obj}.setTransform(\"{pose}\");"));
    }

    /// The object `code` returns.
    pub fn object(&mut self, code: &str) -> ObjId {
        let text = self.eval(code);
        text.parse()
            .unwrap_or_else(|_| panic!("{code} gave {text:?}, not an object"))
    }

    /// Server object `obj`'s ghost index on the client's connection, once its first record was acknowledged.
    pub fn ghost(&mut self, obj: ObjId) -> Option<u32> {
        u32::try_from(
            self.eval(&format!("return ghostOf({obj});"))
                .parse::<i64>()
                .ok()?,
        )
        .ok()
    }

    /// Server object `obj`'s world transform.
    pub fn server_world(&self, obj: ObjId) -> Matrix {
        matrix_of(self.server.vm(), obj).expect("a scene object")
    }

    /// Where the client draws ghost `index` ([`torque_ghost_world::ClientWorld::world_pose`]), its nodes those of the
    /// game's shapes at rest.
    pub fn client_world(&self, index: u32) -> Option<Matrix> {
        self.client
            .with_world(|w| {
                let records = &w.records;
                let nodes = |ghost: u32, node: u16| rest_node(records, ghost, node);
                records.world_pose(index, &nodes).map(|(m, _)| m)
            })
            .flatten()
    }

    /// Where ghost `index`'s own record puts it.
    pub fn record_world(&self, index: u32) -> Option<Matrix> {
        self.client
            .with_world(|w| {
                let pose = w.records.ghosts.get(&index)?.pose()?;
                Some(torque_ghost_world::links::record_matrix(&pose))
            })
            .flatten()
    }

    /// The client's links, read.
    pub fn links<R>(&self, f: impl FnOnce(&torque_ghost_world::links::ClientLinks) -> R) -> R {
        self.client
            .with_world(|w| f(&w.records.links))
            .expect("connected")
    }

    /// The update records the client applied to ghost `index`.
    pub fn updates(&self, index: u32) -> u32 {
        self.client
            .with_world(|w| w.records.ghosts.get(&index).map_or(0, |g| g.updates))
            .unwrap_or(0)
    }
}

/// The game's files as the renderer reads them.
pub struct Kit(pub GameFiles);

impl torque_render_scene::Assets for Kit {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.file(path).map(<[u8]>::to_vec)
    }
}

impl Rig {
    /// Where the client's renderer places each ghost's shape this frame (`torque_render_scene::moving::draws_culled`): the
    /// transforms of its draws, by ghost.
    pub fn drawn(&self) -> BTreeMap<u32, Matrix> {
        let kit = Kit(files());
        self.client
            .with_world(|w| {
                let mut textures = torque_render_scene::textures::TextureSet::new(&kit);
                let draws = torque_render_scene::moving::draws_culled(
                    &w.records,
                    &(),
                    &kit,
                    &mut textures,
                    &mut Vec::new(),
                    torque_render_scene::shapes::DetailPolicy::Highest,
                    torque_render_scene::moving::ShapeSelect::All,
                    None,
                );
                draws.transforms.iter().map(|(&g, m)| (g, m.m)).collect()
            })
            .unwrap_or_default()
    }
}

/// Node `node` of the shape ghost `ghost`'s datablock names, at rest, in the shape's space.
fn rest_node(
    records: &torque_ghost_world::ClientWorld<torque_ghost_world::tge::TgeClientSet>,
    ghost: u32,
    node: u16,
) -> Option<Matrix> {
    let db = records.ghosts.get(&ghost)?.datablock()?;
    let file = records.datablock_text(db, "shapeFile")?;
    let model = match file {
        b"kit/cube.dts" => torque_example_assets::shapes::cube(),
        b"kit/pistol.dts" => torque_example_assets::shapes::pistol(),
        _ => return None,
    };
    let instance = ShapeInstance::new(&model.shape, &mut |_| true).ok()?;
    instance.node_transform(usize::from(node)).copied()
}

/// The largest difference of the translations of `a` and `b`, and of their rotation parts.
pub fn error(a: &Matrix, b: &Matrix) -> (f32, f32) {
    let translation = [3, 7, 11]
        .iter()
        .map(|&i| (a[i] - b[i]).abs())
        .fold(0.0, f32::max);
    let rotation = [0, 1, 2, 4, 5, 6, 8, 9, 10]
        .iter()
        .map(|&i| (a[i] - b[i]).abs())
        .fold(0.0, f32::max);
    (translation, rotation)
}

/// The moving root's pose at tick `t` as `setTransform` text: it slides, rises and turns by whole steps, so every tick's
/// translation differs.
pub fn root_pose(t: u64) -> String {
    let t = t as f32;
    format!(
        "{} {} {} 0 0 1 {}",
        t * 0.04,
        t * 0.01,
        2.0 + (t * 0.05).sin(),
        t * 0.02
    )
}

/// Waits until the client draws each of `children` on the ghost of its parent; the children's ghosts.
pub fn linked(rig: &mut Rig, children: &[(ObjId, ObjId)]) -> Vec<u32> {
    for _ in 0..900 {
        let ghosts: Option<Vec<(u32, u32)>> = children
            .iter()
            .map(|&(c, p)| Some((rig.ghost(c)?, rig.ghost(p)?)))
            .collect();
        if let Some(ghosts) = ghosts
            && ghosts
                .iter()
                .all(|&(c, p)| rig.links(|l| l.parent(c).is_some_and(|q| q.object.0 == p)))
        {
            return ghosts.into_iter().map(|(c, _)| c).collect();
        }
        rig.step();
    }
    panic!("the client never drew every child on its parent");
}

/// The tree of the pictures and of the renderer's check: a cube on the root's origin, turned; a pistol eight times its size
/// on that cube; a small cube just past the pistol's muzzle node (its local translation scaled by the pistol's scale). The
/// root, and each child with its parent.
pub fn three_levels(rig: &mut Rig) -> (ObjId, Vec<(ObjId, ObjId)>) {
    let root = rig.object("return cube(\"0 0 1.5\", \"1 1 1\");");
    let a = rig.object("return cube(\"1.6 0 1.5\", \"0.7 0.7 0.7\");");
    let gun = rig.object("return pistol(\"1.6 0 2.4\", \"8 8 8\");");
    let b = rig.object("return cube(\"1.6 1 2.6\", \"0.4 0.4 0.4\");");
    let script = format!(
        "return {a}.setParent({root}) && {a}.setLocalTransform(\"1.6 0 0 0 0 1 0.6\")          && {gun}.setParent({a}) && {gun}.setLocalTransform(\"0 0 0.9 0 0 1 1.57\")          && {b}.setParent({gun}, 0, \"muzzlePoint\") && {b}.setLocalTransform(\"0 0.05 0 0 0 1 0.4\");"
    );
    assert_eq!(
        rig.eval(&script),
        "1",
        "{}",
        rig.eval("return getParentError();")
    );
    (root, vec![(a, root), (gun, a), (b, gun)])
}
