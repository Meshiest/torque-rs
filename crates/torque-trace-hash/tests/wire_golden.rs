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
//! `TORQUE_BLESS_GOLDEN=1 cargo test -p torque-trace-hash --test main wire_golden::` writes the file again.
//!
//! A server of a net game of its own made of Torque's parts (Torque's table and ghosts, a connection part of its own, a
//! payload layer that counts what passes through Torque's event layer) writes the same bytes: its plain and gated sessions are
//! held to the same goldens (`net_game_*`).

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use torque::app::client::GameClient;
use torque::app::config::{EngineConfig, GameConfig};
use torque::app::files::GameFiles;
use torque::app::net_game::{NetCodecsPlugin, NetGameParts, NetGamePlugin, WrapPayload};
use torque::app::plugins::{DefaultPlugins, ModsPlugin};
use torque::app::server::{GameServer, Listen, ServerStart};
use torque::engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginId};
use torque::mods::CacheChoice;
use torque::net::connect::SharedSecret;
use torque_app_server::plugins::set_protocol;
use torque_app_server::testing::TestNet;
use torque_engine::ProtocolKind;
use torque_example_minimal_engine::{ExamplePlugin, MISSION, START, files, server};
use torque_plugin_rope::RopePlugin;
use torque_server::net::NetGame;
use torque_server::net::codecs::{NetCodecs, TorqueCodecs};
use torque_server::net::payload::{ConnCtx, PacketOutcome, PayloadLayer};
use torque_server::net::torque::{TgeGhostGame, TorqueNet};
use torque_trace_hash::rig::{Control, Mode, Rig, fix_server_keys};
use torque_trace_hash::scenarios::Baseline;

use crate::common;

use common::{Link, SERVER, Wire, client};

const GOLDEN_FILE: &str = "tests/golden/wire_sessions.txt";
const GOLDEN: &str = include_str!("golden/wire_sessions.txt");

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
    engine_with(files, extra, |_| {})
}

/// [`engine`], with `more` added to the builder last.
fn engine_with(files: &GameFiles, extra: &Extra, more: impl FnOnce(&mut EngineBuilder)) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    match extra {
        Extra::Mods(make) => {
            builder.add_plugins((DefaultPlugins.set(make()), ExamplePlugin(files.clone())))
        }
        _ => builder.add_plugins((DefaultPlugins, ExamplePlugin(files.clone()))),
    };
    // Torque's protocol whatever `TORQUE_NET_PROTOCOL` says: the goldens pin this one.
    set_protocol(&mut builder, ProtocolKind::Tge);
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
    more(&mut builder);
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

/// A folder of this process alone: the tests that run the same session run side by side in their own processes.
fn cache_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("wire-golden-{name}-{}", std::process::id()))
}

fn server_mods() -> ModsPlugin {
    ModsPlugin::new().required_mods()
}

fn client_mods() -> ModsPlugin {
    let dir = cache_dir("download");
    let _ = std::fs::remove_dir_all(&dir);
    ModsPlugin::new().cache(CacheChoice::Folder(dir))
}

/// A server of the example game and one engine client over the memory link, every datagram between them recorded. The server
/// speaks net game `G`.
struct Pair<G: NetGame = TorqueNet> {
    server: GameServer<G>,
    client: GameClient,
    link: Link,
}

impl Pair {
    fn new(
        server_files: &GameFiles,
        server_extra: &Extra,
        client_files: &GameFiles,
        client_extra: &Extra,
    ) -> Pair {
        let server_engine = engine(server_files, server_extra);
        let server = server(&server_engine, Listen::Memory(SERVER)).expect("the server starts");
        Pair::over(server, server_extra, client_files, client_extra)
    }
}

impl<G: NetGame> Pair<G> {
    /// A client of `client_files` and `client_extra` joining `server` (made with `server_extra`).
    fn over(
        mut server: GameServer<G>,
        server_extra: &Extra,
        client_files: &GameFiles,
        client_extra: &Extra,
    ) -> Pair<G> {
        fix_server_keys(&server).expect("the network host");
        if matches!(server_extra, Extra::Rope) {
            server
                .vm_mut()
                .eval(
                    b"datablock RopeData(Cable) { segments = 8; thickness = 0.05; color = \"200 160 90 255\"; };",
                )
                .expect("the rope's datablock");
        }
        let (mut client, link) = client(&engine(client_files, client_extra), client_files);
        client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        Pair {
            server,
            client,
            link,
        }
    }

    /// One tick of the server, then of the client, the link carried around each.
    fn step(&mut self) {
        let Self {
            server,
            client,
            link,
        } = self;
        link.step(server, || client.step(32));
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Steps until `done` holds, at most `limit` ticks.
    fn run_until(&mut self, limit: u32, done: impl Fn(&Self) -> bool) -> bool {
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
    plain_on(Rig::new(Baseline, Control::Figure).expect("the rig"))
}

fn plain_on<G: NetGame>(mut rig: Rig<G>) -> Session {
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
    gated_on(Pair::new(&files, &Extra::Rope, &files, &Extra::Rope))
}

fn gated_on<G: NetGame>(mut pair: Pair<G>) -> Session {
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
        wire: pair.link.wire,
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
        wire: pair.link.wire,
    }
}

fn manifest_reject() -> Session {
    let files = game(false);
    let mut pair = Pair::new(&files, &Extra::Rope, &files, &Extra::Nothing);
    pair.run(60);
    Session {
        ghosts: pair.client.ghost_count(),
        why: pair.client.global("$Why"),
        wire: pair.link.wire,
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
        wire: pair.link.wire,
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
        out.push_str(&common::render(name, &session.wire));
    }
    out
}

/// The two tests run the sessions one at a time: the download session clears and fills one cache folder, and a client of
/// one test finding the other's cached mod would send no download.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn the_wire_is_the_bytes_it_was() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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

// ---- a net game of its own, made of Torque's parts

/// Torque's event layer behind a layer that counts the packets it writes and reads, passing every call on.
struct Counting {
    inner: Box<dyn PayloadLayer>,
    packets: Rc<Cell<u32>>,
}

impl PayloadLayer for Counting {
    fn on_game_packet(
        &mut self,
        ctx: &mut ConnCtx<'_>,
        received: &torque::net::protocol::Received,
        payload: &mut torque::net::bits::BitReader<'_>,
    ) -> PacketOutcome {
        self.packets.set(self.packets.get() + 1);
        self.inner.on_game_packet(ctx, received, payload)
    }

    fn on_notify(&mut self, ctx: &mut ConnCtx<'_>, notify: torque::net::protocol::Notify) {
        self.inner.on_notify(ctx, notify);
    }

    fn on_accept(
        &mut self,
        conn: torque::net::handshake::ConnId,
        obj: torque_ts_vm::sim::ObjId,
        ghost_bits: u8,
    ) {
        self.inner.on_accept(conn, obj, ghost_bits);
    }

    fn mark_peer(&mut self, conn: torque::net::handshake::ConnId) {
        self.inner.mark_peer(conn);
    }

    fn mark_channels(&mut self, conn: torque::net::handshake::ConnId) {
        self.inner.mark_channels(conn);
    }

    fn write(
        &mut self,
        ctx: &mut ConnCtx<'_>,
        w: &mut torque::net::bits::BitWriter,
    ) -> Result<(), torque::net::NetError> {
        self.packets.set(self.packets.get() + 1);
        self.inner.write(ctx, w)
    }

    fn busy(&self, conn: torque::net::handshake::ConnId) -> bool {
        self.inner.busy(conn)
    }

    fn take_calls(
        &mut self,
        conn: torque::net::handshake::ConnId,
    ) -> Vec<torque_server::net::payload::Followup> {
        self.inner.take_calls(conn)
    }

    fn take_work(&mut self, conn: torque::net::handshake::ConnId) -> u32 {
        self.inner.take_work(conn)
    }

    fn on_drop(&mut self, conn: torque::net::handshake::ConnId, vm: &mut torque_ts_vm::vm::Vm) {
        self.inner.on_drop(conn, vm);
    }
}

/// The test net game's plugin: its parts, with the payload layer counting into `packets`.
fn test_net(packets: &Rc<Cell<u32>>) -> NetGamePlugin<TestNet> {
    let packets = Rc::clone(packets);
    NetGamePlugin::new(move |_: &mut _, _: &_| {
        let packets = Rc::clone(&packets);
        let payload: WrapPayload = Box::new(move |inner| Box::new(Counting { inner, packets }));
        Ok(NetGameParts {
            game: TestNet::default(),
            ghosts: TgeGhostGame,
            static_writes: None,
            payload: Some(payload),
        })
    })
}

/// The sessions' lines of `names` in the goldens' text `text`.
fn lines_of(text: &str, names: &[&str]) -> String {
    text.lines()
        .filter(|l| l.split(' ').next().is_some_and(|n| names.contains(&n)))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn net_game_a_net_game_of_torques_parts_writes_the_goldens_bytes() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let plain_packets = Rc::new(Cell::new(0));
    let rig = Rig::<TestNet>::with_net_game(
        Mode::Tge,
        (Baseline, test_net(&plain_packets)),
        Control::Figure,
        |_| Ok(()),
    )
    .expect("the rig");
    let plain = plain_on(rig);

    let gated_packets = Rc::new(Cell::new(0));
    let files = game(false);
    let server_engine = engine_with(&files, &Extra::Rope, |builder| {
        builder.add_plugins(test_net(&gated_packets));
    });
    let start = ServerStart {
        listen: Listen::Memory(SERVER),
        globals: &[],
        start: START,
        args: &[MISSION],
    };
    let server = GameServer::<TestNet>::start_with_net(&server_engine, files.mem_files(), &start)
        .expect("the server starts");
    let gated = gated_on(Pair::over(server, &Extra::Rope, &files, &Extra::Rope));

    assert!(plain_packets.get() > 100, "{}", plain_packets.get());
    assert!(gated_packets.get() > 50, "{}", gated_packets.get());
    assert!(gated.ghosts >= 4, "the rope's ghost reached the client");
    let text = render(&[("plain", plain), ("gated", gated)]);
    assert_eq!(
        lines_of(&text, &["plain", "gated"]),
        lines_of(GOLDEN, &["plain", "gated"]),
        "the test net game's sessions differ from the goldens"
    );
}

#[test]
fn net_game_codecs_that_do_not_carry_the_plugin_classes_are_warned_about() {
    let files = game(false);
    let warning = |codecs: Option<fn() -> Rc<dyn NetCodecs>>| {
        let engine = engine_with(&files, &Extra::Rope, |builder| {
            if let Some(make) = codecs {
                builder.add_plugins(NetCodecsPlugin::new(move |_| make()));
            }
        });
        let mut server = server(&engine, Listen::Memory(SERVER)).expect("the server starts");
        server
            .drain_log()
            .into_iter()
            .find(|l| l.contains("reach no client"))
    };
    assert_eq!(warning(None), None, "the engine's codecs carry them");
    let text = warning(Some(|| Rc::new(TorqueCodecs))).expect("warned");
    assert!(
        text.contains("Rope") && text.contains("its codecs do not"),
        "{text}"
    );
}
