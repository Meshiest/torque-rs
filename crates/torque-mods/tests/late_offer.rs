//! Late content: a required mod's files change while clients play. The server re-reads the mod (`refreshRequiredMods()`),
//! sends the connected clients the new offer on the mods channel, and holds back the live datablock rounds of each until its
//! ready word; a client fetches only what it lacks under what its caps have left, a client that has the content fetches
//! nothing, a repeated offer costs nothing, and a script in a late offer is refused as at the accept. The server and its
//! clients are whole engines over the memory transport, with the datagrams carried by the test and a fifth of them dropped.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;
use torque_testkit::Rng;

use torque::app::client::{ClientBuilder, ClientNet, GameClient};
use torque::app::config::{EngineConfig, GameConfig};
use torque::app::files::GameFiles;
use torque::app::plugins::{DefaultPlugins, ModsPlugin, set_protocol};
use torque::app::server::{GameServer, Listen, ServerStart};
use torque::client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque::engine::{Engine, EngineBuilder, PluginGroup, ProtocolKind};
use torque::mods::offer::{FileOffer, ModOffer, encode_offer};
use torque::mods::{CacheChoice, DownloadLimits, Mods};
use torque::net::chunk::Kind;
use torque::script::vm::host::MemFiles;
use torque::server::net::channels::ChannelHub;
use torque_example_minimal_engine::{ExamplePlugin, MISSION, START, files};

const SERVER_V4: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_300);
const SERVER: SocketAddr = SocketAddr::V4(SERVER_V4);

/// The one file the mod has at the start.
const CRATE_BYTES: usize = 3_000;
/// The file added mid-session.
const BIG_BYTES: usize = 3 << 20;
const BIG: &str = "mods/props/sounds/big.ogg";

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i * 31 + usize::from(seed) * 17) % 251) as u8)
        .collect()
}

fn big() -> Vec<u8> {
    pattern(BIG_BYTES, 7)
}

/// The game's files: the example with the server counting `onConnect` and the client keeping what it hears.
fn game() -> GameFiles {
    let mut files = files().unwrap();
    let mut server_cs = String::from_utf8(files.file("server.cs").unwrap().to_vec()).unwrap();
    server_cs.push_str(
        "\nfunction GameConnection::onConnect(%client)\n{\n   $Test::Connected++;\n   %client.transmitDataBlocks(1);\n}\n\
         function GameConnection::onDataBlocksUpdated(%client, %count)\n{\n   $Test::Updated++;\n}\n\
         function clientCount()\n{\n   return isObject(ClientGroup) ? ClientGroup.getCount() : 0;\n}\n",
    );
    files.insert("server.cs", server_cs.as_bytes());
    let mut client_cs = String::from_utf8(files.file("client.cs").unwrap().to_vec()).unwrap();
    client_cs.push_str(
        "\nfunction GameConnection::onConnectRequestRejected(%this, %why)\n{\n   $Why = %why;\n}\n\
         function onModDownloadFailed(%why)\n{\n   $Failed = %why;\n}\n\
         function onModDownloadDone()\n{\n   $Done++;\n}\n\
         function onDataBlockObjectReceived(%index, %total)\n{\n   $DbEvents++;\n}\n\
         function onDataBlocksChanged()\n{\n   $Rounds++;\n}\n",
    );
    files.insert("client.cs", client_cs.as_bytes());
    files
}

/// Puts the mod `props` (served) into `files`, with `art` as its picture and, when given, the big sound too.
fn put_mod(files: &mut GameFiles, art: &[u8], big: Option<&[u8]>) {
    files.insert(
        "mods/props/mod.toml",
        b"id = \"props\"\nversion = \"1.0\"\nshare = \"download\"\n",
    );
    files.insert("mods/props/server.cs", b"$Props::ServerRan = 1;");
    files.insert("mods/props/art/crate.png", art);
    if let Some(big) = big {
        files.insert(BIG, big);
    }
}

fn engine_of(files: &GameFiles, mods: ModsPlugin, protocol: Option<ProtocolKind>) -> Engine {
    let config = GameConfig::load(files).unwrap();
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins((DefaultPlugins.set(mods), ExamplePlugin(files.clone())))
        .config(&config);
    if let Some(protocol) = protocol {
        set_protocol(&mut builder, protocol);
    }
    builder.build().expect("the engine builds")
}

fn client_plugin() -> ModsPlugin {
    ModsPlugin::new().cache(CacheChoice::Memory)
}

/// A client on a socket of the test's own, so the test carries its datagrams.
struct Peer {
    client: GameClient,
    addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
}

impl Peer {
    fn call(&mut self, function: &str, args: &[&str]) -> String {
        self.client.call(function, args).unwrap()
    }

    fn state(&mut self) -> String {
        self.call("getModDownloadState", &[])
    }

    /// `received total mounted fetching` of the client's download in hand.
    fn progress(&mut self) -> String {
        self.call("getModDownloadProgress", &[])
    }

    /// Whether the client has the object with the server's `id` (a client's datablocks have no names).
    fn has_object(&mut self, id: &str) -> bool {
        self.call("isObject", &[id]) == "1"
    }

    fn number(&self, name: &str) -> u32 {
        self.client.global(name).parse().unwrap_or(0)
    }

    fn mods(&self) -> Rc<Mods> {
        Mods::of(self.client.vm()).expect("the client has mods")
    }
}

/// A server and its clients, ticked by hand, over datagrams the test carries and drops.
struct Link {
    server: GameServer,
    /// The server's files, shared with its script engine: a file put here is one the server has.
    server_files: MemFiles,
    peers: Vec<Peer>,
    rng: Rng,
    /// The datagrams dropped, in percent, each way.
    loss: u64,
    ticks: u32,
    protocol: Option<ProtocolKind>,
}

impl Link {
    /// A server of `server_files` (with `globals` set before its entry script) and no clients yet.
    fn new(
        server_files: &GameFiles,
        globals: &[(&str, &str)],
        loss: u64,
        protocol: Option<ProtocolKind>,
    ) -> Link {
        let engine = engine_of(server_files, ModsPlugin::new().required_mods(), protocol);
        let mem = server_files.mem_files();
        let start = ServerStart {
            listen: Listen::Memory(SERVER),
            globals,
            start: START,
            args: &[MISSION],
        };
        let server =
            GameServer::start_engine(&engine, mem.clone(), &start).expect("the server starts");
        Link {
            server,
            server_files: mem,
            peers: Vec::new(),
            rng: Rng::star(loss + 1),
            loss,
            ticks: 0,
            protocol,
        }
    }

    /// Adds a client with `files` and `mods`, connected to the server; its index.
    fn join(&mut self, files: &GameFiles, mods: ModsPlugin) -> usize {
        let n = self.peers.len();
        let addr = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 28_400 + n as u16);
        let engine = engine_of(files, mods, self.protocol);
        let mem = MemoryDatagrams {
            local_port: addr.port(),
            ..MemoryDatagrams::default()
        };
        let (sent, incoming) = (mem.sent.clone(), mem.incoming.clone());
        let mut builder =
            ClientBuilder::from_engine(&engine, Some(files.mem_files()), ClientNet::External)
                .expect("the client builds");
        builder.set_socket(Box::new(mem), NetPolicy::Loopback);
        builder.exec_entry().expect("the client's script");
        let mut client = builder.build();
        client.call("connectTo", &[&SERVER.to_string()]).unwrap();
        self.peers.push(Peer {
            client,
            addr,
            sent,
            incoming,
        });
        n
    }

    fn dropped(&mut self) -> bool {
        self.loss > 0 && self.rng.below(100) < self.loss
    }

    /// Carries the datagrams each way, dropping the share the loss says.
    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        let end = end.clone();
        for i in 0..self.peers.len() {
            let outgoing: Vec<(SocketAddrV4, Vec<u8>)> =
                self.peers[i].sent.borrow_mut().drain(..).collect();
            let from = SocketAddr::V4(self.peers[i].addr);
            for (to, data) in outgoing {
                if SocketAddr::V4(to) == SERVER && !self.dropped() {
                    end.inject(from, &data);
                }
            }
        }
        for (to, data) in end.take_sent() {
            if self.dropped() {
                continue;
            }
            if let Some(peer) = self.peers.iter().find(|p| SocketAddr::V4(p.addr) == to) {
                peer.incoming.borrow_mut().push_back((SERVER_V4, data));
            }
        }
    }

    /// One tick of the server, then of every client.
    fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        for peer in &mut self.peers {
            peer.client.step(32);
        }
        self.carry();
        self.ticks += 1;
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Steps until `done` holds, at most `limit` ticks; how many it took.
    fn run_until(
        &mut self,
        limit: u32,
        what: &str,
        mut done: impl FnMut(&mut Link) -> bool,
    ) -> u32 {
        for tick in 0..limit {
            if done(self) {
                return tick;
            }
            self.step();
        }
        let states: Vec<String> = (0..self.peers.len())
            .map(|i| {
                format!(
                    "{}: state {} failed {:?} why {:?}",
                    i,
                    self.peers[i].state(),
                    self.peers[i].client.global("$Failed"),
                    self.peers[i].client.global("$Why")
                )
            })
            .collect();
        panic!("not {what} in {limit} ticks: {states:?}");
    }

    fn server_number(&self, name: &str) -> u32 {
        String::from_utf8_lossy(&self.server.vm().global(name).unwrap_or_default())
            .parse()
            .unwrap_or(0)
    }

    fn eval(&mut self, code: &str) -> String {
        String::from_utf8_lossy(
            &self
                .server
                .vm_mut()
                .eval(code.as_bytes())
                .expect("the code runs"),
        )
        .into_owned()
    }

    /// Whether client `i` has joined: its mods are mounted, the mission's three datablocks came and a ghost is in.
    fn joined(&mut self, i: usize) -> bool {
        self.peers[i].state() == "done"
            && self.peers[i].number("$DbEvents") >= 3
            && self.peers[i].client.ghost_count() >= 1
    }

    /// Queues `message` on the mods channel for client `i`, as a server that speaks out of turn would.
    fn inject_offer(&mut self, i: usize, message: &[u8]) {
        let hub = self
            .server
            .vm()
            .extension::<ChannelHub>()
            .expect("the server has channels");
        let conn = hub.clients()[i];
        for _ in 0..1_000 {
            if hub
                .send_to(
                    conn,
                    torque::mods::CHANNEL,
                    Kind::Snapshot,
                    message.to_vec(),
                )
                .is_ok()
            {
                return;
            }
            // The channel's queue is full: let the client take some.
            self.step();
        }
        panic!("the offer never queued");
    }

    /// An offer message of the mod `props` as the server requires it now, listing `files` as its content.
    fn offer_message(&self, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let required = Mods::of(self.server.vm()).expect("mods").required();
        let props = required
            .iter()
            .find(|m| m.id == "props")
            .expect("the server requires props");
        let offer = ModOffer {
            id: props.id.clone(),
            name: props.name.clone(),
            version: props.version.clone(),
            hash: props.hash,
            download: true,
            files: files
                .iter()
                .map(|(path, data)| FileOffer::of(path, data).unwrap())
                .collect(),
        };
        encode_offer(&[offer])
    }
}

#[test]
fn a_file_added_to_a_required_mod_reaches_the_clients_before_the_datablock_naming_it() {
    let art = pattern(CRATE_BYTES, 1);
    let mut server_files = game();
    put_mod(&mut server_files, &art, None);
    // The speed of a late download is the engine protocol's: its burst sends as many packets a tick as the byte rate allows.
    let mut link = Link::new(&server_files, &[], 20, Some(ProtocolKind::Engine));
    link.server
        .vm_mut()
        .set_global_uint(b"$Pref::Server::MaxClientBytesPerSecond", 4 << 20);
    // A lacks the mod; B has it installed, with the file the server is about to add.
    let a = link.join(&game(), client_plugin());
    let mut b_files = game();
    put_mod(&mut b_files, &art, Some(&big()));
    let b = link.join(&b_files, client_plugin());
    link.run_until(3_000, "both joined", |l| l.joined(a) && l.joined(b));
    assert_eq!(link.server_number("$Test::Connected"), 2);
    link.run(50);
    let events_a = link.peers[a].number("$DbEvents");
    let done_a = link.peers[a].number("$Done");
    assert!(events_a >= 3, "the mission's datablocks came: {events_a}");

    // The server gets a new file in the mod, re-reads it, and declares a datablock that names it, in one tick.
    link.server_files.insert(BIG, &big());
    let changed = link.eval("return refreshRequiredMods();");
    assert_eq!(changed, "1", "one mod's content changed");
    link.eval(
        "datablock AudioProfile(LateSound) { filename = \"mods/props/sounds/big.ogg\"; }; $Test::Reached = updateDataBlocks();",
    );
    assert_eq!(
        link.server_number("$Test::Reached"),
        2,
        "both clients are past their mission chain"
    );
    let late_id = link.eval("return LateSound.getId();");
    let from = link.ticks;

    // A fetches the file; until its ready word no datablock reaches it, and B, which has the file, gets the datablock at once.
    let mut a_was_downloading = false;
    let fetched = link.run_until(300, "A's late download done", |l| {
        let state = l.peers[a].state();
        if state == "downloading" {
            a_was_downloading = true;
            assert!(
                !l.peers[a].has_object(&late_id),
                "a datablock reached A before its content"
            );
            assert_eq!(
                l.peers[a].number("$DbEvents"),
                events_a,
                "a datablock event reached A before its content"
            );
        }
        a_was_downloading && state == "done"
    });
    assert!(a_was_downloading, "A downloaded");
    eprintln!("{BIG_BYTES} bytes reached A in {fetched} ticks at 20% loss");
    assert_eq!(
        link.peers[a].progress(),
        format!("{BIG_BYTES} {BIG_BYTES} 1 1"),
        "A fetched the whole new file and nothing else"
    );
    assert_eq!(
        link.peers[a].number("$Done"),
        done_a + 1,
        "A's scripts heard the download end"
    );
    assert_eq!(
        link.peers[b].progress(),
        "0 0 0 0",
        "B had the content: nothing fetched"
    );
    let arrived = link.run_until(200, "the datablock on A", |l| {
        l.peers[a].has_object(&late_id)
    });
    eprintln!("the datablock reached A {arrived} ticks after its ready word");
    link.run_until(200, "the datablock on B", |l| {
        l.peers[b].has_object(&late_id)
    });
    assert_eq!(
        link.peers[a].number("$DbEvents"),
        events_a + 1,
        "one event: the late datablock"
    );
    assert!(link.ticks - from <= 300 + 200, "within the budget");
    assert!(
        link.peers[a].mods().is_server_file("sounds/big.ogg"),
        "mounted as the server's content"
    );
    assert_eq!(
        link.server_number("$Test::Connected"),
        2,
        "onConnect never ran again"
    );
    assert_eq!(link.eval("return clientCount();"), "2");
    // Each client's round is answered once; A's answer is a round trip behind its datablock.
    link.run_until(200, "both rounds answered", |l| {
        l.server_number("$Test::Updated") == 2
    });
    link.run(50);
    assert_eq!(link.server_number("$Test::Updated"), 2, "and never again");
}

#[test]
fn a_late_offer_over_the_cumulative_caps_is_refused_with_the_reason_and_nothing_mounted() {
    let art = pattern(150_000, 2);
    let mut server_files = game();
    put_mod(&mut server_files, &art, None);
    let mut link = Link::new(&server_files, &[], 0, None);
    let limits = DownloadLimits {
        total_bytes: 200_000,
        ..DownloadLimits::default()
    };
    let a = link.join(
        &game(),
        ModsPlugin::new()
            .downloads(limits)
            .cache(CacheChoice::Memory),
    );
    link.run_until(5_000, "joined", |l| l.joined(a));
    assert_eq!(link.peers[a].progress(), "150000 150000 1 1");
    // A second file: 100 KB more than the 50 KB the connection's cap has left.
    link.server_files
        .insert("mods/props/sounds/more.ogg", &pattern(100_000, 3));
    assert_eq!(link.eval("return refreshRequiredMods();"), "1");
    link.run_until(600, "refused", |l| l.peers[a].state() == "failed");
    let why = link.peers[a].client.global("$Failed");
    assert!(
        why.contains("needs 97.7 KiB of your 195.3 KiB download limit"),
        "{why}"
    );
    let mods = link.peers[a].mods();
    assert!(
        !mods.is_server_file("sounds/more.ogg"),
        "nothing of the late offer is mounted"
    );
    assert!(
        !mods.is_server_file("art/crate.png"),
        "a failed connection keeps no server content"
    );
    assert_eq!(
        link.peers[a].progress(),
        "150000 150000 1 1",
        "nothing more was fetched: the refusal came before a block was asked for"
    );
    link.run_until(600, "the client left", |l| {
        l.eval("return clientCount();") == "0"
    });
}

#[test]
fn a_repeated_offer_costs_the_client_nothing() {
    let art = pattern(CRATE_BYTES, 1);
    let mut server_files = game();
    put_mod(&mut server_files, &art, None);
    let mut link = Link::new(&server_files, &[], 0, None);
    let a = link.join(&game(), client_plugin());
    link.run_until(5_000, "joined", |l| l.joined(a));
    link.run(20);
    let done = link.peers[a].number("$Done");
    // The offer the client has, a thousand times over, as a server that repeats itself: every copy is a repeat of the offer
    // taken at the accept, so nothing is read, fetched or told to scripts.
    let message = link.offer_message(&[("art/crate.png", art.clone())]);
    for _ in 0..1_000 {
        link.inject_offer(a, &message);
    }
    link.run(400);
    assert_eq!(link.peers[a].state(), "done");
    assert_eq!(
        link.peers[a].progress(),
        format!("{CRATE_BYTES} {CRATE_BYTES} 1 1"),
        "the repeats are the offer the client has, byte for byte: not one was even matched"
    );
    assert_eq!(link.peers[a].number("$Done"), done, "scripts heard nothing");
    assert!(
        link.peers[a].mods().is_server_file("art/crate.png"),
        "the mount stays"
    );
    assert_eq!(
        link.eval("return clientCount();"),
        "1",
        "the client is still playing"
    );
}

#[test]
fn a_script_in_a_late_offer_is_refused_as_at_the_accept() {
    let art = pattern(CRATE_BYTES, 1);
    let mut server_files = game();
    put_mod(&mut server_files, &art, None);
    let mut link = Link::new(&server_files, &[], 0, None);
    let a = link.join(&game(), client_plugin());
    link.run_until(5_000, "joined", |l| l.joined(a));
    let message = link.offer_message(&[
        ("art/crate.png", art),
        ("scripts/evil.cs", b"$pwned = 1;".to_vec()),
    ]);
    link.inject_offer(a, &message);
    link.run_until(600, "refused", |l| l.peers[a].state() == "failed");
    let why = link.peers[a].client.global("$Failed");
    assert!(
        why.contains("a server sends content, never scripts"),
        "{why}"
    );
    assert!(!link.peers[a].mods().is_server_file("scripts/evil.cs"));
    assert_eq!(link.peers[a].client.global("$pwned"), "");
}

/// A server that stops requiring a mod, or requires another, sends the change the same way.
#[test]
fn enabling_a_mod_mid_game_sends_its_content_as_a_late_offer() {
    let art = pattern(CRATE_BYTES, 1);
    let mut server_files = game();
    put_mod(&mut server_files, &art, None);
    server_files.insert(
        "mods/extra/mod.toml",
        b"id = \"extra\"\nversion = \"2.0\"\nshare = \"download\"\n",
    );
    server_files.insert("mods/extra/art/extra.png", &pattern(40_000, 9));
    let mut link = Link::new(
        &server_files,
        &[("$Pref::Mods::Disabled", "extra")],
        0,
        None,
    );
    assert_eq!(link.eval("return getRequiredModList();"), "props");
    let a = link.join(&game(), client_plugin());
    link.run_until(5_000, "joined", |l| l.joined(a));
    link.eval("enableMod(\"extra\");");
    assert_eq!(link.eval("return getRequiredModCount();"), "2");
    link.run_until(600, "extra fetched", |l| {
        l.peers[a].mods().is_server_file("art/extra.png")
    });
    assert_eq!(
        link.peers[a].progress(),
        "40000 40000 1 1",
        "the new mod alone"
    );
    assert_eq!(link.eval("return clientCount();"), "1");
}

/// The server's content changes while a client is still fetching the offer it was sent: the download goes on from the layout
/// the client was offered, completes, and the new offer follows its ready word.
#[test]
fn a_download_in_flight_survives_a_change_of_the_servers_content() {
    let art = pattern(600_000, 4);
    let mut server_files = game();
    put_mod(&mut server_files, &art, None);
    let mut link = Link::new(&server_files, &[], 0, None);
    // Slow enough to be mid-download for a while in either protocol.
    link.server
        .vm_mut()
        .set_global_uint(b"$Pref::Server::MaxClientBytesPerSecond", 24 << 10);
    let a = link.join(&game(), client_plugin());
    link.run_until(2_000, "downloading", |l| {
        l.peers[a].state() == "downloading"
    });
    link.run(20);
    // The mod gains a file and the server re-reads it while the first download is in flight.
    link.server_files
        .insert("mods/props/sounds/more.ogg", &pattern(20_000, 6));
    assert_eq!(link.eval("return refreshRequiredMods();"), "1");
    link.run_until(20_000, "joined after the change", |l| l.joined(a));
    assert_eq!(
        link.server_number("$Test::Connected"),
        1,
        "the client joined once"
    );
    link.run_until(3_000, "the new file", |l| {
        l.peers[a].mods().is_server_file("sounds/more.ogg")
    });
    assert!(link.peers[a].mods().is_server_file("art/crate.png"));
    assert_eq!(link.eval("return clientCount();"), "1", "never kicked");
}
