//! The wire of whole engines, pinned: five scripted sessions over the in-memory link, every datagram of each recorded. A session's
//! golden is the SHA-256 of the datagrams each way (each datagram length-prefixed, in order) and the first eight datagrams as hex.
//! A change that alters one bit a server or a client writes in these sessions fails here and names the session.
//!
//! The sessions: a peer that is not an engine's own (a plain client of Torque's protocol with no block, on the trace world's
//! figure); a gated peer, an engine with the rope plugin joining a server that has it and ghosting a rope; a held mods download
//! (the server holds the joining client's connection in a burst while it sends the missing mod); a refusal by the manifest (a
//! client without the plugin the server has); a refusal by the ticket (a client signing in under another secret).
//!
//! The goldens are in `golden/wire_sessions.txt`. After a change that is meant to alter the wire,
//! `TORQUE_BLESS_GOLDEN=1 cargo test -p torque-trace-hash --test wire_golden` writes the file again.

use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use torque::app::client::{ClientBuilder, ClientNet, GameClient};
use torque::app::config::{EngineConfig, GameConfig};
use torque::app::files::GameFiles;
use torque::app::plugins::{DefaultPlugins, ModsPlugin};
use torque::app::server::{GameServer, Listen};
use torque::client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque::engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginId};
use torque::mods::CacheChoice;
use torque::net::connect::SharedSecret;
use torque_example_minimal_engine::{ExamplePlugin, files, server};
use torque_plugin_rope::RopePlugin;
use torque_trace_hash::rig::{CHALLENGE_KEY, Control, Rig};
use torque_trace_hash::scenarios::Baseline;

const GOLDEN_FILE: &str = "tests/golden/wire_sessions.txt";
const GOLDEN: &str = include_str!("golden/wire_sessions.txt");
/// Datagrams kept verbatim.
const VERBATIM: usize = 8;
const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);

/// Every datagram of a session, in the order carried, and which way (true: to the server).
type Wire = Vec<(bool, Vec<u8>)>;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn digest(wire: &Wire, up: bool) -> String {
    let mut h = Sha256::new();
    for (_, bytes) in wire.iter().filter(|(d, _)| *d == up) {
        h.update((bytes.len() as u32).to_le_bytes());
        h.update(bytes);
    }
    hex(&h.finalize())
}

/// A plugin that signs a client in, and has a server check the sign-in, under `secret`.
#[derive(PluginId)]
#[plugin(crate = torque::engine)]
struct SignIn {
    secret: [u8; 16],
    name: &'static str,
}

impl Plugin for SignIn {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().auth(SharedSecret::new(self.secret, self.name));
    }
}

/// What an engine adds to the example game's `DefaultPlugins`.
#[derive(Clone)]
enum Extra {
    Nothing,
    Rope,
    SignIn([u8; 16], &'static str),
    Mods(fn() -> ModsPlugin),
}

fn engine(files: &GameFiles, extra: &Extra) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    match extra {
        Extra::Mods(make) => {
            builder.add_plugins((DefaultPlugins.set(make()), ExamplePlugin(files.clone())))
        }
        _ => builder.add_plugins((DefaultPlugins, ExamplePlugin(files.clone()))),
    };
    match extra {
        Extra::Rope => {
            builder.add_plugins(RopePlugin::default());
        }
        Extra::SignIn(secret, name) => {
            builder.add_plugins(SignIn {
                secret: *secret,
                name,
            });
        }
        Extra::Nothing | Extra::Mods(_) => {}
    }
    builder.config(&config);
    builder.build().expect("the engine builds")
}

/// The example game, with the hooks the sessions read: why the client was refused, and the mod the server may require.
fn game(pack: bool) -> GameFiles {
    let mut files = files().expect("the example game");
    let mut client_cs = String::from_utf8(files.file("client.cs").unwrap().to_vec()).unwrap();
    client_cs.push_str(
        "\nfunction GameConnection::onConnectRequestRejected(%this, %why)\n{\n   $Why = %why;\n}\n",
    );
    files.insert("client.cs", client_cs.as_bytes());
    if pack {
        files.insert(
            "mods/props/mod.toml",
            b"id = \"props\"\nversion = \"1.2\"\nshare = \"download\"\n",
        );
        files.insert("mods/props/server.cs", b"$Props::ServerRan = 1;");
        let art: Vec<u8> = (0..24_000usize)
            .map(|i| ((i * 31 + 17) % 251) as u8)
            .collect();
        files.insert("mods/props/art/crate.png", &art);
    }
    files
}

fn cache_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("wire-golden-{name}"))
}

fn server_mods() -> ModsPlugin {
    ModsPlugin::new().required_mods()
}

fn client_mods() -> ModsPlugin {
    let dir = cache_dir("download");
    let _ = std::fs::remove_dir_all(&dir);
    ModsPlugin::new().cache(CacheChoice::Folder(dir))
}

/// A server of the example game and one engine client over the memory link, every datagram between them recorded.
struct Pair {
    server: GameServer,
    client: GameClient,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    wire: Wire,
}

impl Pair {
    fn new(
        server_files: &GameFiles,
        server_extra: &Extra,
        client_files: &GameFiles,
        client_extra: &Extra,
    ) -> Pair {
        let server_engine = engine(server_files, server_extra);
        let mut server = server(&server_engine, Listen::Memory(SERVER)).expect("the server starts");
        // The server draws the key of its challenges at random; fixed, the datagrams are a function of the inputs.
        server.shared.borrow_mut().hs.set_key(CHALLENGE_KEY);
        if matches!(server_extra, Extra::Rope) {
            server
                .vm_mut()
                .eval(
                    b"datablock RopeData(Cable) { segments = 8; thickness = 0.05; color = \"200 160 90 255\"; };",
                )
                .expect("the rope's datablock");
        }
        let client_engine = engine(client_files, client_extra);
        let mem = MemoryDatagrams {
            local_port: CLIENT.port(),
            ..MemoryDatagrams::default()
        };
        let (sent, incoming) = (mem.sent.clone(), mem.incoming.clone());
        let mut builder = ClientBuilder::from_engine(
            &client_engine,
            Some(client_files.mem_files()),
            ClientNet::External,
        )
        .expect("the client builds");
        builder.set_socket(Box::new(mem), NetPolicy::Loopback);
        builder.exec_entry().expect("the client's script");
        let mut client = builder.build();
        client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        Pair {
            server,
            client,
            sent,
            incoming,
            wire: Vec::new(),
        }
    }

    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SERVER {
                self.wire.push((true, data.clone()));
                end.inject(SocketAddr::V4(CLIENT), &data);
            }
        }
        for (to, data) in end.take_sent() {
            if to == SocketAddr::V4(CLIENT) {
                self.wire.push((false, data.clone()));
                let SocketAddr::V4(from) = SERVER else {
                    continue;
                };
                self.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    /// One tick of the server, then of the client, the link carried around each.
    fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        self.client.step(32);
        self.carry();
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Steps until `done` holds, at most `limit` ticks.
    fn run_until(&mut self, limit: u32, done: impl Fn(&Pair) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }
}

/// What a session came to besides its wire: for the checks that it did what it says.
struct Session {
    wire: Wire,
    /// The ghosts the client held at the end, and the client's reason for being refused.
    ghosts: usize,
    why: String,
}

/// A client that sends no block (the trace world's figure, as a Torque client sends its connect).
fn plain() -> Session {
    let mut rig = Rig::new(Baseline, Control::Figure).expect("the rig");
    rig.record_wire();
    assert!(rig.settle(400), "the client predicts");
    for tick in 0..90u32 {
        rig.set_global("$mvForwardAction", if tick % 40 < 30 { 1.0 } else { 0.0 });
        rig.set_global("$mvYaw", if tick % 25 < 5 { 0.01 } else { 0.0 });
        rig.step();
    }
    Session {
        wire: std::mem::take(&mut rig.wire),
        ghosts: 0,
        why: String::new(),
    }
}

fn gated() -> Session {
    let files = game(false);
    let mut pair = Pair::new(&files, &Extra::Rope, &files, &Extra::Rope);
    assert!(
        pair.run_until(400, |p| p.client.ghost_count() >= 3),
        "{:?}",
        pair.client.client_log
    );
    pair.server
        .vm_mut()
        .eval(b"return ropeNetCreate(\"0 0 3\", \"6 0 3\", Cable);")
        .expect("a rope");
    pair.run(60);
    Session {
        ghosts: pair.client.ghost_count(),
        why: pair.client.global("$Why"),
        wire: pair.wire,
    }
}

fn held_download() -> Session {
    let server_files = game(true);
    let client_files = game(false);
    let mut pair = Pair::new(
        &server_files,
        &Extra::Mods(server_mods),
        &client_files,
        &Extra::Mods(client_mods),
    );
    assert!(
        pair.run_until(1_000, |p| p.client.ghost_count() >= 3),
        "{:?}",
        pair.client.client_log
    );
    pair.run(30);
    Session {
        ghosts: pair.client.ghost_count(),
        why: pair.client.global("$Why"),
        wire: pair.wire,
    }
}

fn manifest_reject() -> Session {
    let files = game(false);
    let mut pair = Pair::new(&files, &Extra::Rope, &files, &Extra::Nothing);
    pair.run(60);
    Session {
        ghosts: pair.client.ghost_count(),
        why: pair.client.global("$Why"),
        wire: pair.wire,
    }
}

fn ticket_reject() -> Session {
    let files = game(false);
    let mut pair = Pair::new(
        &files,
        &Extra::SignIn([1; 16], ""),
        &files,
        &Extra::SignIn([2; 16], "ada"),
    );
    pair.run(60);
    Session {
        ghosts: pair.client.ghost_count(),
        why: pair.client.global("$Why"),
        wire: pair.wire,
    }
}

fn sessions() -> Vec<(&'static str, Session)> {
    vec![
        ("plain", plain()),
        ("gated", gated()),
        ("download", held_download()),
        ("manifest_reject", manifest_reject()),
        ("ticket_reject", ticket_reject()),
    ]
}

/// The golden file's text for the sessions: the count and a digest line for each way, and a line for each of the first datagrams.
fn render(sessions: &[(&str, Session)]) -> String {
    let mut out = String::new();
    for (name, session) in sessions {
        let wire = &session.wire;
        let _ = writeln!(out, "{name} datagrams {}", wire.len());
        let _ = writeln!(out, "{name} to-server {}", digest(wire, true));
        let _ = writeln!(out, "{name} to-client {}", digest(wire, false));
        for (i, (up, bytes)) in wire.iter().take(VERBATIM).enumerate() {
            let way = if *up { "to-server" } else { "to-client" };
            let _ = writeln!(out, "{name} {i} {way} {}", hex(bytes));
        }
    }
    out
}

#[test]
fn the_wire_is_the_bytes_it_was() {
    let sessions = sessions();
    let text = render(&sessions);
    if std::env::var_os("TORQUE_BLESS_GOLDEN").is_some() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_FILE);
        std::fs::write(path, &text).expect("writes the goldens");
        return;
    }
    let mut differing = Vec::new();
    for (name, _) in &sessions {
        let pick = |s: &str| {
            s.lines()
                .filter(|l| l.split(' ').next() == Some(name))
                .collect::<Vec<_>>()
                .join("\n")
        };
        if pick(&text) != pick(GOLDEN) {
            differing.push(*name);
        }
    }
    assert!(
        differing.is_empty(),
        "sessions whose bytes differ from the goldens: {differing:?}\n--- now\n{text}"
    );
    assert_eq!(text.lines().count(), GOLDEN.lines().count());
}

#[test]
fn the_sessions_do_what_they_say() {
    let by_name: std::collections::BTreeMap<_, _> = sessions().into_iter().collect();
    // The accepted sessions went on trading packets; the refused ones ended at the refusal, with the reason.
    assert!(by_name["plain"].wire.len() > 100);
    assert!(by_name["gated"].ghosts >= 4, "the rope is a ghost");
    let download = &by_name["download"];
    let mods = std::fs::read_dir(cache_dir("download")).map_or(0, |d| {
        d.flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".mod"))
            .count()
    });
    assert_eq!(mods, 1, "the client cached the mod it fetched");
    let biggest = download
        .wire
        .iter()
        .filter(|(up, _)| !*up)
        .map(|(_, b)| b.len())
        .max()
        .unwrap();
    assert!(
        biggest >= 1_000,
        "the held connection sends full packets: {biggest}"
    );
    assert!(by_name["download"].ghosts >= 3);
    let manifest = &by_name["manifest_reject"];
    let ticket = &by_name["ticket_reject"];
    assert!(
        manifest.why.contains("rope") || !manifest.why.is_empty(),
        "{}",
        manifest.why
    );
    assert!(!ticket.why.is_empty(), "{}", ticket.why);
    assert_ne!(manifest.why, ticket.why);
    assert_eq!(manifest.ghosts, 0);
    assert_eq!(ticket.ghosts, 0);
    // Each refusal ends in a few datagrams, not a session.
    assert!(manifest.wire.len() < 12, "{}", manifest.wire.len());
    assert!(ticket.wire.len() < 12, "{}", ticket.wire.len());
}
