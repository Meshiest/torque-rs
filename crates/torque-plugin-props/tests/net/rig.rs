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

pub struct Rig {
    pub server: GameServer,
    /// The server's engine (its resources are the server's props).
    pub engine: Engine,
    pub peers: Vec<Peer>,
}

impl Rig {
    /// A server of the props engine and no client; `globals` are set before its scripts run.
    pub fn new(files: &GameFiles, globals: &[(&str, &str)]) -> Rig {
        let engine = engine(files, true);
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
