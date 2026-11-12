//! A real server and engine clients in one process over the memory transport, stepped by hand: one 32 ms tick of the server, then one
//! of each client, the datagrams carried across around each. No clock is read.

// Each test file uses some of what is here.
#![allow(dead_code)]

use torque_app::GameFiles;
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
pub use torque_app::testing::Peer;
use torque_app::testing::RigPlugin;
#[allow(unused_imports)]
pub use torque_app::testing::SERVER;
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_rope::{NetRopes, Rope, RopePlugin, RopeShared};
use torque_ts_vm::sim::ObjId;

/// The rope plugin as the rig's subject.
pub struct Ropes;

impl RigPlugin for Ropes {
    const START: &'static str = "startRopes";

    fn add(builder: &mut EngineBuilder) {
        builder.add_plugins(RopePlugin::default());
    }
}

/// A server of the rope engine and its clients.
pub type Rig = torque_app::testing::Rig<Ropes>;

pub fn files() -> GameFiles {
    let mut files = GameFiles::default();
    files.insert("server.cs", include_bytes!("server.cs"));
    files.insert("main.cs", include_bytes!("main.cs"));
    files.insert("game.toml", include_bytes!("game.toml"));
    // A picture for ropes to be drawn with: two red texels and two blue.
    let texels: Vec<u8> = [
        [255, 0, 0, 255],
        [0, 0, 255, 255],
        [0, 0, 255, 255],
        [255, 0, 0, 255],
    ]
    .into_iter()
    .flatten()
    .collect();
    files.insert(
        "art/stripe.png",
        &torque_plugin_rope::encode_png(2, 2, &texels).expect("a png"),
    );
    files
}

/// An engine of the default plugins (the game's `game.toml` applied) and, when `rope` is true, the rope plugin.
pub fn engine(files: &GameFiles, rope: bool) -> Engine {
    if rope {
        torque_app::testing::engine::<Ropes>(files)
    } else {
        torque_app::testing::plain_engine(files)
    }
}

/// An engine of the default plugins (the game's `game.toml` applied) and the rope plugin `plugin`.
pub fn engine_with(files: &GameFiles, plugin: impl torque_engine::PluginType) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    builder.add_plugins(plugin);
    builder.build().expect("the engine builds")
}

/// What a test reads off a client of the rope engine.
pub trait PeerRopes {
    /// The client's ropes.
    fn ropes(&self) -> RopeShared;
    /// The client's book of ropes.
    fn book<R>(&self, f: impl FnOnce(&NetRopes) -> R) -> R;
    /// The client's rope ids, in order.
    fn rope_ids(&self) -> Vec<u32>;
}

impl PeerRopes for Peer {
    fn ropes(&self) -> RopeShared {
        self.engine.resource::<RopeShared>().expect("ropes").clone()
    }

    fn book<R>(&self, f: impl FnOnce(&NetRopes) -> R) -> R {
        let book = self
            .engine
            .resource::<torque_plugin_rope::RopeNet>()
            .expect("book");
        f(&book.lock())
    }

    fn rope_ids(&self) -> Vec<u32> {
        self.ropes().lock().ropes().map(|(id, _)| id).collect()
    }
}

/// What a test does to the server of the rope engine.
pub trait RigRopes {
    /// Moves the server's static shape `obj` by `setTransform`, which marks its record's position as in TGE.
    fn move_static(&mut self, obj: ObjId, pose: &str);
    /// Moves the server's `Mover` to `x` along the x axis, one unit up.
    fn move_mover(&mut self, x: f32);
}

impl RigRopes for Rig {
    fn move_static(&mut self, obj: ObjId, pose: &str) {
        self.eval(&format!("{obj}.setTransform(\"{pose}\");"));
    }

    fn move_mover(&mut self, x: f32) {
        let mover = self
            .server
            .vm()
            .objects()
            .find(b"Mover")
            .expect("the mover");
        self.move_static(mover, &format!("{x} 0 1 0 0 1 0"));
    }
}

/// A server and two clients, connected and ghosted.
pub fn two() -> (Rig, torque_app::GameFiles) {
    let files = files();
    let mut rig = Rig::new(&files, &[]);
    rig.join(&files, engine(&files, true), 50_001, &[]);
    rig.join(&files, engine(&files, true), 50_002, &[]);
    assert!(
        rig.run_until(400, |r| r.peers.iter().all(|p| p.ghosts() >= 3)),
        "{:?}",
        rig.peers[0].client.client_log
    );
    (rig, files)
}

/// The first rope of a client's world.
pub fn rope(peer: &Peer) -> (u32, Rope) {
    let ropes = peer.ropes();
    let world = ropes.lock();
    let (id, rope) = world.ropes().next().expect("a rope");
    (id, rope.clone())
}

pub fn create(rig: &mut Rig, style: &str) -> i32 {
    let id = rig.eval(&format!(
        "return ropeNetCreate(\"0 0 3\", \"6 0 3\", {style});"
    ));
    id.parse().expect("a rope id")
}

/// How far the points of a rope are from the line through its ends.
pub fn off_the_line(rope: &Rope) -> f32 {
    let p: Vec<[f32; 3]> = rope.points().collect();
    let (a, b) = (p[0], p[p.len() - 1]);
    let d: Vec<f32> = (0..3).map(|k| b[k] - a[k]).collect();
    let len = d.iter().map(|x| x * x).sum::<f32>().sqrt();
    p.iter()
        .map(|q| {
            let r: Vec<f32> = (0..3).map(|k| q[k] - a[k]).collect();
            let along = r.iter().zip(&d).map(|(x, y)| x * y).sum::<f32>() / len;
            (0..3)
                .map(|k| (r[k] - d[k] / len * along).powi(2))
                .sum::<f32>()
                .sqrt()
        })
        .fold(0.0, f32::max)
}
