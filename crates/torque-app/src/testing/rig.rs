//! A real server and engine clients in one process over the memory transport, stepped by hand: one 32 ms tick of the server, then
//! one of each client, the datagrams carried across around each. No clock is read.

use std::marker::PhantomData;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_engine::{Engine, EngineBuilder};

use crate::GameFiles;
use crate::client::{ClientBuilder, ClientNet, GameClient};
use crate::config::{EngineConfig, GameConfig};
use crate::plugins::DefaultPlugins;
use crate::server::{GameServer, Listen, ServerStart};

/// Where the rig's server listens.
pub const SERVER: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

/// The plugin a [`Rig`] is for.
pub trait RigPlugin {
    /// The script function of the game's `main.cs` that starts the server.
    const START: &'static str;

    /// Adds the plugin to an engine's builder.
    fn add(builder: &mut EngineBuilder);

    /// Sets up a client the rig made, before it connects (the default does nothing).
    fn client(client: &mut GameClient, files: &GameFiles) {
        let _ = (client, files);
    }
}

/// An engine of the default plugins (the game's `game.toml` applied).
pub fn plain_engine(files: &GameFiles) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    builder.build().expect("the engine builds")
}

/// An engine of the default plugins (the game's `game.toml` applied) and the plugin `P`.
pub fn engine<P: RigPlugin>(files: &GameFiles) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    P::add(&mut builder);
    builder.build().expect("the engine builds")
}

/// One client of a [`Rig`].
pub struct Peer {
    /// The client.
    pub client: GameClient,
    /// Its engine (its resources are the client's).
    pub engine: Engine,
    /// Where it sends from.
    pub addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
}

impl Peer {
    /// Ghosts of every class the client holds.
    #[must_use]
    pub fn ghosts(&self) -> usize {
        self.client.ghost_count()
    }

    /// The client's script global `name`, empty when it has none.
    #[must_use]
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
    /// The ticks a datagram takes to the server.
    pub up: u64,
    /// The ticks a datagram takes to a client.
    pub down: u64,
    /// The datagrams lost, per thousand, each way.
    pub loss_per_mille: u32,
    /// The state of the loss sequence.
    pub seed: u64,
}

impl Link {
    /// About 100 ms there and back (one tick up, two down) and 2% of the datagrams lost each way.
    #[must_use]
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

/// A server of the plugin `P` and its clients.
pub struct Rig<P> {
    /// The server.
    pub server: GameServer,
    /// The server's engine (its resources are the server's).
    pub engine: Engine,
    /// The clients, in the order they joined.
    pub peers: Vec<Peer>,
    /// The link the datagrams cross.
    pub link: Link,
    /// The steps run, and the datagrams on their way up (from a client) and down (to one), with the step each arrives at.
    now: u64,
    up: Vec<InFlight>,
    down: Vec<InFlight>,
    plugin: PhantomData<P>,
}

impl<P: RigPlugin> Rig<P> {
    /// A server of the plugin's engine and no client; `globals` are set before its scripts run.
    pub fn new(files: &GameFiles, globals: &[(&str, &str)]) -> Rig<P> {
        Rig::with_engine(files, globals, engine::<P>(files))
    }

    /// A server of `engine` and no client; `globals` are set before its scripts run.
    pub fn with_engine(files: &GameFiles, globals: &[(&str, &str)], engine: Engine) -> Rig<P> {
        let server = GameServer::start_engine(
            &engine,
            files.mem_files(),
            &ServerStart {
                listen: Listen::Memory(SERVER),
                globals,
                start: P::START,
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
            plugin: PhantomData,
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
        P::client(&mut client, files);
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

    /// Runs `ticks` steps.
    pub fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Runs until `done` holds, at most `limit` ticks; false when it never did.
    pub fn run_until(&mut self, limit: usize, mut done: impl FnMut(&Rig<P>) -> bool) -> bool {
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

    /// The server's objects of `class`.
    #[must_use]
    pub fn objects_of(&self, class: &str) -> usize {
        let objects = self.server.vm().objects();
        objects
            .iter()
            .filter(|&(id, _)| objects.is_class_named(id, class))
            .count()
    }
}
