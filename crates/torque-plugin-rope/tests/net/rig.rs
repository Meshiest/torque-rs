//! A real server and engine clients in one process over the memory transport, stepped by hand: one 32 ms tick of the server, then one
//! of each client, the datagrams carried across around each. No clock is read.

// Each test file uses some of what is here.
#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
use torque_app::server::{GameServer, Listen};
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_rope::{NetRopes, Rope, RopePlugin, RopeShared};
use torque_ts_vm::sim::ObjId;

pub const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

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
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    if rope {
        builder.add_plugins(RopePlugin::default());
    }
    builder.build().expect("the engine builds")
}

/// An engine of the default plugins (the game's `game.toml` applied) and the rope plugin `plugin`.
pub fn engine_with(files: &GameFiles, plugin: impl torque_engine::PluginType) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    builder.add_plugins(plugin);
    builder.build().expect("the engine builds")
}

pub struct Peer {
    pub client: GameClient,
    pub engine: Engine,
    pub addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
}

impl Peer {
    pub fn ropes(&self) -> RopeShared {
        self.engine.resource::<RopeShared>().expect("ropes").clone()
    }

    pub fn book<R>(&self, f: impl FnOnce(&NetRopes) -> R) -> R {
        let book = self
            .engine
            .resource::<torque_plugin_rope::RopeNet>()
            .expect("book");
        f(&book.lock())
    }

    /// The client's rope ids, in order.
    pub fn rope_ids(&self) -> Vec<u32> {
        self.ropes().lock().ropes().map(|(id, _)| id).collect()
    }

    /// Ghosts of every class the client holds.
    pub fn ghosts(&self) -> usize {
        self.client.ghost_count()
    }
}

pub struct Rig {
    pub server: GameServer,
    pub peers: Vec<Peer>,
}

impl Rig {
    /// A server of the rope engine and no client.
    pub fn new(files: &GameFiles) -> Rig {
        let engine = engine(files, true);
        let server = GameServer::start_engine(
            &engine,
            files.mem_files(),
            &torque_app::server::ServerStart {
                listen: Listen::Memory(SERVER),
                globals: &[],
                start: "startRopes",
                args: &[],
            },
        )
        .expect("the server starts");
        Rig {
            server,
            peers: Vec::new(),
        }
    }

    /// Adds a client of `engine` at port `port` and has it connect.
    pub fn join(&mut self, files: &GameFiles, engine: Engine, port: u16) -> usize {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let mem = MemoryDatagrams {
            local_port: port,
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
        self.peers.push(Peer {
            client,
            engine,
            addr,
            sent,
            incoming,
        });
        self.peers.len() - 1
    }

    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        for peer in &self.peers {
            for (to, data) in peer.sent.borrow_mut().drain(..) {
                if SocketAddr::V4(to) == SERVER {
                    end.inject(SocketAddr::V4(peer.addr), &data);
                }
            }
        }
        for (to, data) in end.take_sent() {
            if let Some(peer) = self.peers.iter().find(|p| SocketAddr::V4(p.addr) == to) {
                let SocketAddr::V4(from) = SERVER else {
                    continue;
                };
                peer.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    /// One tick of the server, then of every client.
    pub fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        for peer in &mut self.peers {
            peer.client.step(32);
        }
        self.carry();
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

    /// Moves the server's static shape `obj` by `setTransform`, which marks its record's position as in TGE.
    pub fn move_static(&mut self, obj: ObjId, pose: &str) {
        self.eval(&format!("{obj}.setTransform(\"{pose}\");"));
    }

    /// Moves the server's `Mover` to `x` along the x axis, one unit up.
    pub fn move_mover(&mut self, x: f32) {
        let mover = self
            .server
            .vm()
            .objects()
            .find(b"Mover")
            .expect("the mover");
        self.move_static(mover, &format!("{x} 0 1 0 0 1 0"));
    }

    /// Server objects of a class.
    pub fn objects(&self, class: &str) -> usize {
        let objects = self.server.vm().objects();
        objects
            .iter()
            .filter(|(_, o)| objects.classes.name(o.class()) == class.as_bytes())
            .count()
    }
}

/// A server and two clients, connected and ghosted.
pub fn two() -> (Rig, torque_app::GameFiles) {
    let files = files();
    let mut rig = Rig::new(&files);
    rig.join(&files, engine(&files, true), 50_001);
    rig.join(&files, engine(&files, true), 50_002);
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
