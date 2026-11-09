//! A real server and engine clients in one process over the memory transport, stepped by hand: one 32 ms tick of the server, then
//! one of each client, the datagrams carried across around each. No clock is read.

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
use torque_plugin_props::{ClientProps, PropsPlugin, ServerProps};

pub const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

pub fn files() -> GameFiles {
    let mut files = GameFiles::default();
    files.insert("server.cs", include_bytes!("server.cs"));
    files.insert("main.cs", include_bytes!("main.cs"));
    files.insert("game.toml", include_bytes!("game.toml"));
    let dts = torque_shapes::write::write_dts;
    files.insert("kit/cube.dts", &dts(&torque_example_assets::shapes::cube()));
    files.insert(
        "kit/figure.dts",
        &dts(&torque_example_assets::shapes::figure()),
    );
    files.insert(
        "kit/buggy.dts",
        &dts(&torque_example_assets::buggy::buggy()),
    );
    files.insert(
        "kit/buggy_wheel.dts",
        &dts(&torque_example_assets::buggy::wheel()),
    );
    files
}

/// An engine of the default plugins (the game's `game.toml` applied) and, when `props` is true, the props plugin.
pub fn engine(files: &GameFiles, props: bool) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    if props {
        builder.add_plugins(PropsPlugin);
    }
    builder.build().expect("the engine builds")
}

/// Torque's answers, with the game's files for the shapes the client reads.
#[derive(Debug)]
struct FilesGame(torque_ts_vm::host::MemFiles);

impl torque_client::game::ClientGame for FilesGame {
    fn file_reader(&self) -> Option<torque_client::predict::FileReader> {
        let files = self.0.clone();
        Some(std::rc::Rc::new(
            move |path: &[u8]| match torque_ts_vm::host::FileHost::read(&files, path) {
                torque_ts_vm::host::FileRead::Found(bytes) => Some(bytes),
                _ => None,
            },
        ))
    }
}

pub struct Peer {
    pub client: GameClient,
    pub engine: Engine,
    pub addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
}

impl Peer {
    pub fn book(&self) -> ClientProps {
        self.engine
            .resource::<ClientProps>()
            .expect("the book")
            .clone()
    }

    /// The client's props by the server's slot: (slot, position, asleep).
    pub fn props(&self) -> Vec<(u16, [f32; 3], bool)> {
        let book = self.book();
        let b = book.0.borrow();
        let mut out: Vec<_> = b
            .props()
            .values()
            .map(|p| (p.slot, p.q.position(), p.asleep))
            .collect();
        out.sort_by_key(|p| p.0);
        out
    }

    pub fn ghosts(&self) -> usize {
        self.client.ghost_count()
    }

    pub fn global(&self, name: &str) -> String {
        self.client
            .vm()
            .global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }
}

/// The link between the server and its clients: the ticks a datagram takes each way and the share lost (per thousand, a
/// seeded sequence, the same every run). The default carries every datagram at once.
#[derive(Debug, Clone, Copy, Default)]
pub struct Link {
    pub up: u64,
    pub down: u64,
    pub loss_per_mille: u32,
    pub seed: u64,
}

impl Link {
    /// About 100 ms there and back (one tick up, two down) and 2% of the datagrams lost each way.
    pub fn lossy() -> Link {
        Link {
            up: 1,
            down: 2,
            loss_per_mille: 20,
            seed: 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn lost(&mut self) -> bool {
        if self.loss_per_mille == 0 {
            return false;
        }
        // xorshift64
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        (x % 1000) < u64::from(self.loss_per_mille)
    }
}

type InFlight = (u64, SocketAddrV4, Vec<u8>);

pub struct Rig {
    pub server: GameServer,
    /// The server's engine (its resources are the server's props).
    pub engine: Engine,
    pub peers: Vec<Peer>,
    pub link: Link,
    /// The steps run, and the datagrams on their way up (from a client) and down (to one), with the step each arrives at.
    now: u64,
    up: Vec<InFlight>,
    down: Vec<InFlight>,
}

impl Rig {
    /// A server of the props engine and no client; `globals` are set before its scripts run.
    pub fn new(files: &GameFiles, globals: &[(&str, &str)]) -> Rig {
        Rig::with_engine(files, globals, engine(files, true))
    }

    /// A server of `engine` and no client; `globals` are set before its scripts run.
    pub fn with_engine(files: &GameFiles, globals: &[(&str, &str)], engine: Engine) -> Rig {
        let server = GameServer::start_engine(
            &engine,
            files.mem_files(),
            &torque_app::server::ServerStart {
                listen: Listen::Memory(SERVER),
                globals,
                start: "startProps",
                args: &[],
            },
        )
        .expect("the server starts");
        Rig {
            server,
            engine,
            peers: Vec::new(),
            link: Link::default(),
            now: 0,
            up: Vec::new(),
            down: Vec::new(),
        }
    }

    /// Adds a client of `engine` at port `port` and has it connect; `prefs` are set on it first.
    pub fn join(
        &mut self,
        files: &GameFiles,
        engine: Engine,
        port: u16,
        prefs: &[(&str, &str)],
    ) -> usize {
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
        // The game's files for the shapes the client's prediction reads (the floor it stands on), as a game gives them.
        if torque_client::game::installed_game(client.vm()).is_none() {
            torque_client::game::set_client_game(
                client.vm_mut(),
                std::rc::Rc::new(FilesGame(files.mem_files())),
            );
        }
        for (name, value) in prefs {
            client.vm_mut().set_global(name, value);
        }
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
        let now = self.now;
        for peer in &self.peers {
            for (to, data) in peer.sent.borrow_mut().drain(..) {
                if SocketAddr::V4(to) == SERVER && !self.link.lost() {
                    self.up.push((now + self.link.up, peer.addr, data));
                }
            }
        }
        for (to, data) in end.take_sent() {
            if let SocketAddr::V4(to) = to
                && !self.link.lost()
            {
                self.down.push((now + self.link.down, to, data));
            }
        }
        let (due, later): (Vec<InFlight>, Vec<InFlight>) = std::mem::take(&mut self.up)
            .into_iter()
            .partition(|d| d.0 <= now);
        self.up = later;
        for (_, from, data) in due {
            end.inject(SocketAddr::V4(from), &data);
        }
        let (due, later): (Vec<InFlight>, Vec<InFlight>) = std::mem::take(&mut self.down)
            .into_iter()
            .partition(|d| d.0 <= now);
        self.down = later;
        let SocketAddr::V4(from) = SERVER else {
            return;
        };
        for (_, to, data) in due {
            if let Some(peer) = self.peers.iter().find(|p| p.addr == to) {
                peer.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    /// One tick of the server, then of every client.
    pub fn step(&mut self) {
        self.now += 1;
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

    pub fn props(&self) -> ServerProps {
        self.engine
            .resource::<ServerProps>()
            .expect("the server's props")
            .clone()
    }

    /// The server's objects of `class`.
    pub fn objects_of(&self, class: &str) -> usize {
        let objects = self.server.vm().objects();
        objects
            .iter()
            .filter(|(_, o)| objects.classes.name(o.class()) == class.as_bytes())
            .count()
    }

    /// The server's props by slot: (slot, position, asleep).
    pub fn server_props(&self) -> Vec<(u16, [f32; 3], bool)> {
        let props = self.props();
        let s = props.0.borrow();
        let mut out: Vec<_> = s
            .world
            .as_ref()
            .map(|w| {
                w.props()
                    .values()
                    .map(|p| (p.slot, p.q.position(), p.rest.is_asleep()))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|p| p.0);
        out
    }

    /// The ghost section totals of each client's connection, in join order.
    pub fn totals(&self) -> Vec<torque_server::net::ghosts::Totals> {
        let shared = self.server.shared.borrow();
        let hub = shared.ghosts.clone().expect("a ghost hub");
        let mut conns: Vec<_> = shared.conns.values().filter_map(|c| c.conn).collect();
        conns.sort_unstable();
        conns.iter().filter_map(|&c| hub.totals(c)).collect()
    }
}

/// A server and `n` clients of the props engine, connected and ghosted; `prefs` set on each client.
pub fn clients(n: usize, globals: &[(&str, &str)], prefs: &[(&str, &str)]) -> (Rig, GameFiles) {
    let files = files();
    let mut rig = Rig::new(&files, globals);
    for k in 0..n {
        rig.join(&files, engine(&files, true), 50_001 + k as u16, prefs);
    }
    assert!(
        rig.run_until(400, |r| r.peers.iter().all(|p| p.ghosts() >= 2)),
        "{:?}",
        rig.peers[0].client.client_log
    );
    (rig, files)
}
