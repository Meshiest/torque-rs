//! Servers that require mods, through the engine's own server and client over the memory transport. The server sends what it
//! requires and the client fetches what it lacks: a client with the mod joins at once, one that lacks it fetches, verifies, caches
//! and mounts it before the server's `onConnect` runs, a second join uses the cache, an update sends only the blocks that changed,
//! a client that cannot or will not fetch leaves with the reason, a hostile client's requests are refused, and nothing a server
//! sends ever runs.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::rc::Rc;

use torque::app::client::{ClientNet, GameClient, carry_between};
use torque::app::config::{EngineConfig, GameConfig};
use torque::app::files::GameFiles;
use torque::app::plugins::{DefaultPlugins, ModsPlugin};
use torque::app::server::{GameServer, Listen};
use torque::engine::{Engine, EngineBuilder, PluginGroup};
use torque::mods::offer::BLOCK_BYTES;
use torque::mods::{CacheChoice, DownloadLimits, Mods};
use torque::net::connect::{AuthContext, ConnectBlock};
use torque::net::profile::HandshakeProfile;
use torque_example_minimal_engine::{ExamplePlugin, client, files, server};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);

fn cache_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("mods-required-{name}"))
}

/// A cache folder with nothing in it.
fn fresh_cache(name: &str) -> PathBuf {
    let dir = cache_dir(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn cached(dir: &PathBuf) -> usize {
    std::fs::read_dir(dir).map_or(0, |d| {
        d.flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".mod"))
            .count()
    })
}

/// What a test varies about a mod: its version, the bytes of its art and what `mod.toml` says to share.
#[derive(Clone)]
struct Pack {
    version: &'static str,
    art: Vec<u8>,
    share: &'static str,
}

const BOOM: [u8; 1_500] = [9; 1_500];

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i * 31 + usize::from(seed) * 17) % 251) as u8)
        .collect()
}

impl Pack {
    fn new(version: &'static str) -> Pack {
        Pack {
            version,
            art: pattern(3_000, 1),
            share: "download",
        }
    }

    fn art(mut self, art: Vec<u8>) -> Pack {
        self.art = art;
        self
    }

    fn share(mut self, share: &'static str) -> Pack {
        self.share = share;
        self
    }

    /// The mod's files under `mods/props`: content (a Lua file among it), scripts (entry scripts, a library, compiled code, a
    /// menu) and the manifest.
    fn put(&self, files: &mut GameFiles) {
        let toml = format!(
            "id = \"props\"\nversion = \"{}\"\nshare = \"{}\"\n",
            self.version, self.share
        );
        files.insert("mods/props/mod.toml", toml.as_bytes());
        files.insert("mods/props/server.cs", b"$Props::ServerRan = 1;");
        files.insert("mods/props/client.cs", b"$Props::ClientRan = 1;");
        files.insert("mods/props/lib/evil.cs", b"$Props::EvilRan = 1;");
        files.insert("mods/props/lib/evil.cs.dso", &[1, 2, 3, 4]);
        files.insert("mods/props/gui/menu.gui", b"$Props::GuiRan = 1;");
        files.insert(
            "mods/props/ui/panel.lua",
            b"$Props::LuaRan = 1; os.execute('x')",
        );
        files.insert("mods/props/art/crate.png", &self.art);
        files.insert("mods/props/sounds/boom.ogg", &BOOM);
    }
}

/// The game's files, with the server's `onConnect` counting and the client's hooks keeping what they hear.
fn game(pack: Option<&Pack>) -> GameFiles {
    let mut files = files().unwrap();
    let mut server_cs = String::from_utf8(files.file("server.cs").unwrap().to_vec()).unwrap();
    server_cs.push_str(
        "\nfunction GameConnection::onConnect(%client)\n{\n   $Test::Connected++;\n   %client.transmitDataBlocks(1);\n}\n\
         function serverCmdHostile(%client)\n{\n   $Test::Hostile++;\n}\n\
         function clientCount()\n{\n   return isObject(ClientGroup) ? ClientGroup.getCount() : 0;\n}\n",
    );
    files.insert("server.cs", server_cs.as_bytes());
    let mut client_cs = String::from_utf8(files.file("client.cs").unwrap().to_vec()).unwrap();
    client_cs.push_str(
        "\nfunction GameConnection::onConnectRequestRejected(%this, %why)\n{\n   $Why = %why;\n}\n\
         function onModDownloadFailed(%why)\n{\n   $Failed = %why;\n}\n\
         function onModDownloadProgress(%id, %got, %total, %at, %of)\n{\n   $Progress = %id SPC %got SPC %total SPC %at SPC %of;\n   $ProgressCalls++;\n}\n\
         function onModDownloadDone()\n{\n   $Done++;\n}\n",
    );
    files.insert("client.cs", client_cs.as_bytes());
    if let Some(pack) = pack {
        pack.put(&mut files);
    }
    files
}

fn engine(files: &GameFiles, mods: ModsPlugin) -> Engine {
    let config = GameConfig::load(files).unwrap();
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins((DefaultPlugins.set(mods), ExamplePlugin(files.clone())))
        .config(&config);
    builder.build().expect("the engine builds")
}

fn server_plugin() -> ModsPlugin {
    ModsPlugin::new().required_mods()
}

/// A client whose downloads go to the cache folder `name` (emptied first).
fn client_plugin(name: &str) -> ModsPlugin {
    ModsPlugin::new().cache(CacheChoice::Folder(fresh_cache(name)))
}

/// A client that keeps what is in the cache folder `name`.
fn client_plugin_keeping(name: &str) -> ModsPlugin {
    ModsPlugin::new().cache(CacheChoice::Folder(cache_dir(name)))
}

/// A server hosted by a client, not yet connected.
struct Pair {
    client: GameClient,
}

fn pair(
    server_pack: Option<&Pack>,
    server_mods: ModsPlugin,
    client_pack: Option<&Pack>,
    client_mods: ModsPlugin,
) -> Pair {
    let server_files = game(server_pack);
    let server_engine = engine(&server_files, server_mods);
    let server = server(&server_engine, Listen::Memory(SERVER)).expect("the server starts");
    let client_files = game(client_pack);
    let client_engine = engine(&client_files, client_mods);
    let mut client = client(
        &client_engine,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    )
    .expect("the client");
    client.start_hosting(server);
    Pair { client }
}

impl Pair {
    fn server_global(&self, name: &str) -> String {
        let server = self.client.server.as_ref().expect("a hosted server");
        String::from_utf8_lossy(&server.vm().global(name).unwrap_or_default()).into_owned()
    }

    fn connect(&mut self) {
        self.client
            .call("connectTo", &[&SERVER.to_string()])
            .unwrap();
    }

    fn mods(&self) -> Rc<Mods> {
        Mods::of(self.client.vm()).expect("the client has mods")
    }

    fn state(&mut self) -> String {
        self.client.call("getModDownloadState", &[]).unwrap()
    }

    fn progress(&mut self) -> String {
        self.client.call("getModDownloadProgress", &[]).unwrap()
    }

    fn connected(&self) -> u32 {
        self.server_global("$Test::Connected").parse().unwrap_or(0)
    }

    /// Steps until `done` holds, at most `limit` ticks; after each, the server's `onConnect` must not have run before the client
    /// had every required mod mounted (when `held_back`).
    fn run_until(&mut self, limit: u32, held_back: bool, done: impl Fn(&mut Pair) -> bool) -> u32 {
        let before = self.connected();
        for tick in 0..limit {
            self.client.step(32);
            if held_back && self.connected() > before {
                assert_eq!(
                    self.state(),
                    "done",
                    "onConnect ran at tick {tick} before the content was mounted"
                );
            }
            if done(self) {
                return tick;
            }
        }
        panic!(
            "not done in {limit} ticks: state {} why {:?} failed {:?} log {:?}",
            self.state(),
            self.client.global("$Why"),
            self.client.global("$Failed"),
            self.client.client_log
        );
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.client.step(32);
        }
    }

    /// The client's reason for leaving.
    fn failed(&mut self) -> String {
        self.client.global("$Failed")
    }

    fn lost(&self) -> bool {
        self.client
            .client_log
            .iter()
            .any(|l| l.contains("Connection with server lost"))
    }
}

fn joined(p: &mut Pair) -> bool {
    p.client.ghost_count() == 3
}

fn left(p: &mut Pair) -> bool {
    !p.client.global("$Failed").is_empty() || !p.client.global("$Why").is_empty()
}

#[test]
fn a_client_with_the_mod_joins_without_a_download() {
    let pack = Pack::new("1.2");
    let mut p = pair(
        Some(&pack),
        server_plugin(),
        Some(&pack),
        client_plugin("match"),
    );
    p.connect();
    p.run_until(1_000, true, joined);
    assert_eq!(p.state(), "done");
    assert_eq!(p.progress(), "0 0 0 0");
    assert_eq!(p.client.call("getRequiredModList", &[]).unwrap(), "props");
    assert_eq!(p.connected(), 1);
    assert!(
        !p.mods().is_server_file("art/crate.png"),
        "the local copy is the client's own mod"
    );
    assert_eq!(p.server_global("$Props::ServerRan"), "1");
    assert_eq!(cached(&cache_dir("match")), 0);
}

#[test]
fn the_server_lists_what_it_requires_for_its_scripts() {
    let pack = Pack::new("1.2");
    let mut p = pair(
        Some(&pack),
        server_plugin(),
        Some(&pack),
        client_plugin("list"),
    );
    let server = p.client.server.as_mut().unwrap();
    assert_eq!(server.call("getRequiredModCount", &[]).unwrap(), "1");
    assert_eq!(server.call("getRequiredModList", &[]).unwrap(), "props");
    assert_eq!(
        server
            .call("getRequiredModField", &["props", "version"])
            .unwrap(),
        "1.2"
    );
    assert_eq!(
        server
            .call("getRequiredModField", &["props", "hash"])
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        server
            .call("getRequiredModField", &["props", "files"])
            .unwrap(),
        "3"
    );
    assert_eq!(
        server
            .call("getRequiredModField", &["props", "download"])
            .unwrap(),
        "1"
    );
    // Switching the mod off takes it off the list.
    server.call("disableMod", &["props"]).unwrap();
    assert_eq!(server.call("getRequiredModCount", &[]).unwrap(), "0");
}

#[test]
fn the_server_never_receives_what_a_client_has_installed_or_cached() {
    let pack = Pack::new("1.2");
    let p = pair(
        Some(&pack),
        server_plugin(),
        Some(&pack),
        client_plugin("private"),
    );
    // What the client's connect block carries about mods, with a mod installed.
    let extras = p
        .client
        .vm()
        .extension::<torque::engine::net::ConnectExtras>()
        .expect("the client adds to its connect block");
    let section = (extras.0)(p.client.vm()).expect("a section");
    let block = ConnectBlock {
        mods: Some(section.clone()),
        ..ConnectBlock::default()
    };
    let mut w = torque::net::bits::BitWriter::new();
    block.write(&mut w).unwrap();
    let wire = w.into_bytes();
    // The section is a format and two caps; nothing in the block can be a mod's id, version or hash.
    assert_eq!(section.encode().len(), 13);
    let hash = p.mods().content_of("props").unwrap().hash;
    assert!(
        !wire
            .windows(8)
            .any(|window| hash.windows(8).any(|h| h == window))
    );
    assert!(!wire.windows(5).any(|window| window == b"props"));
    assert_eq!(section.max_bytes, DownloadLimits::default().total_bytes);
    // With a fetched mod in the cache the block is the same.
    let mut q = pair(
        Some(&pack),
        server_plugin(),
        None,
        client_plugin("private2"),
    );
    q.connect();
    q.run_until(3_000, true, joined);
    assert_eq!(cached(&cache_dir("private2")), 1);
    let extras = q
        .client
        .vm()
        .extension::<torque::engine::net::ConnectExtras>()
        .unwrap();
    assert_eq!((extras.0)(q.client.vm()), Some(section));
}

#[test]
fn a_client_that_will_not_fetch_leaves_with_the_reason_naming_the_mod() {
    let pack = Pack::new("1.2");
    // Downloads off in its settings.
    let mut p = pair(Some(&pack), server_plugin(), None, client_plugin("off"));
    p.client.set_global("$Pref::Mods::Download", "0");
    p.connect();
    p.run_until(600, false, left);
    let why = p.failed();
    assert!(
        why.contains("props 1.2")
            && why.contains("KiB")
            && why.contains("turned off in your settings"),
        "{why}"
    );
    p.run(100);
    assert_eq!(p.client.ghost_count(), 0);
    assert_eq!(p.connected(), 0, "the server never started the join");
    assert_eq!(cached(&cache_dir("off")), 0);
    // A server that does not serve the mod (marked required only, or downloads off on its side).
    let required = Pack::new("1.2").share("required");
    let mut p = pair(
        Some(&required),
        server_plugin(),
        None,
        client_plugin("off-required"),
    );
    p.connect();
    p.run_until(600, false, left);
    assert!(
        p.failed().contains("props 1.2") && p.failed().contains("you do not have it"),
        "{}",
        p.failed()
    );
    let mut p = pair(
        Some(&pack),
        server_plugin().allow_downloads(false),
        None,
        client_plugin("off-noserve"),
    );
    p.connect();
    p.run_until(600, false, left);
    assert!(p.failed().contains("you do not have it"), "{}", p.failed());
    // Downloads off do not matter to a client that has the mod already.
    let mut p = pair(
        Some(&pack),
        server_plugin(),
        Some(&pack),
        client_plugin("off-have"),
    );
    p.client.set_global("$Pref::Mods::Download", "0");
    p.connect();
    p.run_until(1_000, true, joined);
}

#[test]
fn downloads_fetch_verify_cache_mount_and_join_and_a_second_join_uses_the_cache() {
    let pack = Pack::new("1.2");
    let dir = fresh_cache("fetch");
    let mut p = pair(
        Some(&pack),
        server_plugin(),
        None,
        client_plugin_keeping("fetch"),
    );
    assert_eq!(p.client.call("getModList", &[]).unwrap(), "");
    p.connect();
    let ticks = p.run_until(3_000, true, joined);
    eprintln!("4.4 KB downloaded and joined in {ticks} ticks");
    assert_eq!(p.state(), "done");
    assert_eq!(p.connected(), 1);
    assert_eq!(p.client.global("$Done"), "1");
    assert!(p.client.global("$Failed").is_empty());
    let total = pack.art.len() + BOOM.len() + b"$Props::LuaRan = 1; os.execute('x')".len();
    assert_eq!(
        p.client.global("$Progress"),
        format!("props {total} {total} 1 1")
    );
    assert_eq!(p.progress(), format!("{total} {total} 1 1"));
    // The content is mounted as a server's files; nothing that could be TorqueScript is.
    let mods = p.mods();
    assert!(mods.is_server_file("art/crate.png"));
    assert!(mods.is_server_file("sounds/boom.ogg"));
    for script in [
        "mod.toml",
        "server.cs",
        "client.cs",
        "lib/evil.cs",
        "lib/evil.cs.dso",
        "gui/menu.gui",
        "mods/props/client.cs",
    ] {
        assert!(!mods.is_server_file(script), "{script}");
    }
    assert_eq!(p.client.global("$Props::ClientRan"), "");
    assert_eq!(p.client.global("$Props::EvilRan"), "");
    assert_eq!(p.client.global("$Props::GuiRan"), "");
    // The Lua file was sent: it is there as data, untrusted, and nothing runs it.
    assert!(mods.is_server_file("ui/panel.lua"));
    assert!(mods.is_untrusted("ui/panel.lua"));
    assert_eq!(
        p.client.call("exec", &["ui/panel.lua"]).unwrap_or_default(),
        "0"
    );
    assert_eq!(p.client.global("$Props::LuaRan"), "");
    // The server's own copy ran its server script.
    assert_eq!(p.server_global("$Props::ServerRan"), "1");
    // One entry in the cache, named for the hash.
    assert_eq!(cached(&dir), 1);
    let name = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .file_name();
    let hash = p
        .client
        .server
        .as_mut()
        .unwrap()
        .call("getRequiredModField", &["props", "hash"])
        .unwrap();
    assert_eq!(name.to_string_lossy(), format!("{hash}.mod"));

    // Leave: the mounted content goes with the connection, the cache stays.
    p.client.call("disconnect", &[]).unwrap();
    p.run(200);
    assert!(!p.mods().is_server_file("art/crate.png"));
    assert_eq!(cached(&dir), 1);
    // Join again: the server offers the same content, the client has it in its cache, nothing is sent.
    p.connect();
    p.run_until(3_000, true, |p| p.connected() == 2 && joined(p));
    assert_eq!(p.state(), "done");
    assert_eq!(p.progress(), "0 0 0 0");
    assert!(p.mods().is_server_file("art/crate.png"));
    assert_eq!(cached(&dir), 1);
    // Clearing the cache empties it.
    assert!(
        p.client
            .call("clearModDownloadCache", &[])
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 4_500
    );
    assert_eq!(cached(&dir), 0);
}

#[test]
fn an_update_sends_only_the_blocks_that_changed() {
    // Three blocks of art; the update changes a few bytes in the middle one.
    let art = pattern(BLOCK_BYTES * 2 + 5_000, 3);
    let v1 = Pack::new("1.0").art(art.clone());
    let mut changed = art;
    for b in &mut changed[BLOCK_BYTES + 100..BLOCK_BYTES + 140] {
        *b ^= 0xff;
    }
    let v2 = Pack::new("1.1").art(changed);
    let dir = fresh_cache("update");
    {
        let mut p = pair(
            Some(&v1),
            server_plugin(),
            None,
            client_plugin_keeping("update"),
        );
        p.connect();
        p.run_until(20_000, true, joined);
        let sent = p.progress();
        eprintln!("the first version: {sent}");
        assert_eq!(cached(&dir), 1);
    }
    // The server now runs the update; the same client cache joins it.
    let mut p = pair(
        Some(&v2),
        server_plugin(),
        None,
        client_plugin_keeping("update"),
    );
    p.connect();
    p.run_until(20_000, true, joined);
    assert_eq!(p.state(), "done");
    // One block of the changed file, and nothing of the files that did not change.
    let block = BLOCK_BYTES;
    assert_eq!(p.progress(), format!("{block} {block} 1 1"));
    assert_eq!(
        cached(&dir),
        2,
        "the update is kept beside the version it was built from"
    );
    assert!(p.mods().is_server_file("art/crate.png"));
    // A third join of the update sends nothing.
    p.client.call("disconnect", &[]).unwrap();
    p.run(200);
    p.connect();
    p.run_until(5_000, true, |p| p.connected() == 2 && joined(p));
    assert_eq!(p.progress(), "0 0 0 0");
}

/// A client in the middle of a download, whose connection a test can speak on.
fn downloading(name: &str) -> Pair {
    let pack = Pack::new("1.2").art(pattern(BLOCK_BYTES * 7, 5));
    let mut p = pair(Some(&pack), server_plugin(), None, client_plugin(name));
    p.connect();
    p.run_until(2_000, false, |p| p.state() == "downloading");
    p
}

#[test]
fn a_hostile_clients_requests_are_refused_and_it_is_dropped() {
    for (name, request) in [
        ("count", "commandToServer('TorqueModsGet', 0, 0, 0, 999);"),
        (
            "start",
            "commandToServer('TorqueModsGet', 0, 0, 1000000, 1);",
        ),
        ("mod", "commandToServer('TorqueModsGet', 7, 0, 0, 1);"),
        ("file", "commandToServer('TorqueModsGet', 0, 99, 0, 1);"),
        ("negative", "commandToServer('TorqueModsGet', 0, 0, 0, -1);"),
        ("zero", "commandToServer('TorqueModsGet', 0, 0, 0, 0);"),
        (
            "wrap",
            "commandToServer('TorqueModsGet', 0, 0, 2147483647, 4);",
        ),
        (
            "flood",
            "for (%i = 0; %i < 60; %i++) commandToServer('TorqueModsGet', 0, 0, 0, 1);",
        ),
    ] {
        let mut p = downloading(&format!("hostile-{name}"));
        assert_eq!(p.connected(), 0);
        p.client.vm_mut().eval(request.as_bytes()).unwrap();
        p.run(200);
        assert!(p.lost(), "{name}: {:?}", p.client.client_log);
        assert_eq!(p.client.ghost_count(), 0, "{name}");
        assert_eq!(p.connected(), 0, "{name}: the join never started");
    }
}

#[test]
fn a_client_that_never_asks_for_anything_is_dropped_as_stalled() {
    // The client holds still: nothing it can do after the offer moves the server's count.
    let limits = DownloadLimits {
        idle_seconds: 2,
        ..DownloadLimits::default()
    };
    let pack = Pack::new("1.2").art(pattern(BLOCK_BYTES * 7, 5));
    let mut p = pair(
        Some(&pack),
        server_plugin().downloads(limits),
        None,
        client_plugin("stalled"),
    );
    p.connect();
    // Take the client's way of asking away: its downloads never send a command.
    p.client
        .vm_mut()
        .eval(b"function commandToServer() {}")
        .unwrap();
    p.run_until(2_000, false, |p| p.lost());
    assert_eq!(p.connected(), 0);
}

#[test]
fn the_caps_stop_a_download_with_the_reason() {
    let pack = Pack::new("1.2");
    // The client allows less than the package needs sent.
    let small = DownloadLimits {
        total_bytes: 1_000,
        ..DownloadLimits::default()
    };
    let mut p = pair(
        Some(&pack),
        server_plugin(),
        None,
        client_plugin("cap1").downloads(small),
    );
    p.connect();
    p.run_until(600, false, left);
    assert!(
        p.failed().contains("props") && p.failed().contains("download limit"),
        "{}",
        p.failed()
    );
    assert!(!p.mods().is_server_file("art/crate.png"));
    // The player's own pref lowers the cap too.
    let big = Pack::new("1.2").art(vec![3; 2 << 20]);
    let mut p = pair(Some(&big), server_plugin(), None, client_plugin("cap2"));
    p.client.set_global("$Pref::Mods::DownloadMegabytes", "1");
    p.connect();
    p.run_until(600, false, left);
    assert!(
        p.failed().contains("props") && p.failed().contains("download limit"),
        "{}",
        p.failed()
    );
    // The server will not serve what is over its own caps: the mod is required but not offered.
    let tight = DownloadLimits {
        mod_bytes: 2_000,
        ..DownloadLimits::default()
    };
    let mut p = pair(
        Some(&pack),
        server_plugin().downloads(tight),
        None,
        client_plugin("cap3"),
    );
    p.connect();
    p.run_until(600, false, left);
    assert!(p.failed().contains("you do not have it"), "{}", p.failed());
    let tight = DownloadLimits {
        file_bytes: 2_000,
        ..DownloadLimits::default()
    };
    let mut p = pair(
        Some(&pack),
        server_plugin().downloads(tight),
        None,
        client_plugin("cap4"),
    );
    p.connect();
    p.run_until(600, false, left);
    assert!(p.failed().contains("you do not have it"), "{}", p.failed());
}

#[test]
fn a_download_that_takes_too_long_ends_the_connection() {
    // A few megabytes cannot arrive in three seconds.
    let pack = Pack::new("1.2").art(pattern(3 << 20, 8));
    let limits = DownloadLimits {
        seconds: 3,
        ..DownloadLimits::default()
    };
    let mut p = pair(
        Some(&pack),
        server_plugin().downloads(limits),
        None,
        client_plugin("slow").downloads(limits),
    );
    p.connect();
    p.run_until(2_000, true, |p| {
        !p.client.global("$Failed").is_empty() || p.lost()
    });
    p.run(200);
    assert_ne!(p.state(), "done");
    assert_eq!(p.client.ghost_count(), 0);
    assert_eq!(p.connected(), 0);
    assert!(!p.mods().is_server_file("art/crate.png"));
    assert_eq!(cached(&cache_dir("slow")), 0);
}

#[test]
fn a_server_with_no_required_mods_speaks_torques_handshake() {
    // The default plugins say nothing on the wire: no gate, no channel, no connect block.
    let files = game(None);
    let plain = engine(&files, ModsPlugin::new());
    assert!(plain.connect_gate(&HandshakeProfile::default()).is_none());
    assert!(!plain.is_networked());
    assert!(plain.net_channels().unwrap().is_empty());
    let cx = AuthContext {
        server: SERVER,
        sequence: 1,
        challenge: [1, 2, 3, 4],
    };
    assert!(plain.connect_block(&cx).unwrap().is_none());
    // Two such engines join as ever.
    let mut p = pair(None, ModsPlugin::new(), None, ModsPlugin::new());
    p.connect();
    p.run_until(1_000, false, joined);
    assert_eq!(p.connected(), 1);
    // A server that opted in and has nothing to require takes an opted-in client at once.
    let mut p = pair(None, server_plugin(), None, client_plugin("none-needed"));
    p.connect();
    p.run_until(1_000, false, joined);
    assert_eq!(p.connected(), 1);
    // A client that did not opt in cannot join a server that did, and is told why.
    let mut p = pair(None, server_plugin(), None, ModsPlugin::new());
    p.connect();
    p.run_until(300, false, left);
    assert_eq!(
        p.client.global("$Why"),
        "server needs plugin torque.mods 1, you do not have it"
    );
}

#[test]
fn a_larger_mod_arrives_at_the_speed_of_a_burst() {
    // 400 KB: how long it takes is printed. Before the burst (the channel took half of each of 10 packets of 200 bytes a second)
    // the same download ran at about 0.7 KB a second.
    let size = 400_000;
    let pack = Pack::new("1.2").art(pattern(size, 6));
    let mut p = pair(Some(&pack), server_plugin(), None, client_plugin("large"));
    p.connect();
    p.run_until(2_000, false, |p| p.state() == "downloading");
    let start = p.client.global("$ProgressCalls");
    let _ = start;
    let ticks = p.run_until(20_000, true, |p| p.state() == "done");
    let kbs = size as f64 / 1024.0 / (f64::from(ticks) * 0.032);
    eprintln!(
        "{size} bytes after {ticks} ticks ({} s): {kbs:.1} KB/s",
        ticks * 32 / 1_000
    );
    assert!(kbs > 15.0, "{kbs} KB/s");
    p.run_until(2_000, true, joined);
    assert!(p.mods().is_server_file("art/crate.png"));
    // The packets went back to the connection's rate after the download.
    p.run(100);
}

#[test]
fn a_held_client_can_run_no_game_command_and_is_in_no_group_until_it_joins() {
    let mut p = downloading("held-command");
    for _ in 0..5 {
        p.client
            .vm_mut()
            .eval(b"commandToServer('Hostile');")
            .unwrap();
    }
    p.run(60);
    assert_eq!(p.state(), "downloading");
    assert_eq!(
        p.server_global("$Test::Hostile"),
        "",
        "no game command ran while the client was held"
    );
    let members = |p: &mut Pair| {
        let server = p.client.server.as_mut().unwrap();
        server
            .call("clientCount", &[])
            .unwrap_or_else(|e| panic!("{e}"))
    };
    assert_eq!(members(&mut p), "0", "a held client is not in ClientGroup");
    // Once it has joined, its commands run as ever.
    p.run_until(5_000, true, joined);
    assert_eq!(members(&mut p), "1");
    p.client
        .vm_mut()
        .eval(b"commandToServer('Hostile');")
        .unwrap();
    p.run(30);
    assert_eq!(p.server_global("$Test::Hostile"), "1");
}

#[test]
fn ready_before_fetching_only_hurries_the_client_that_says_it_and_a_request_after_ready_is_refused()
{
    let mut p = downloading("early-ready");
    assert_eq!(p.connected(), 0);
    p.client
        .vm_mut()
        .eval(b"commandToServer('TorqueModsReady');")
        .unwrap();
    p.run(20);
    assert_eq!(p.connected(), 1, "the server took the word of the client");
    // The server no longer holds it: a request now drops it.
    p.client
        .vm_mut()
        .eval(b"commandToServer('TorqueModsGet', 0, 0, 0, 1);")
        .unwrap();
    p.run(100);
    assert!(
        p.lost(),
        "state {} failed {:?} {:?}",
        p.state(),
        p.failed(),
        p.client.client_log
    );
}

/// Several clients of one server that a test ticks by hand.
struct Crowd {
    server: GameServer,
    clients: Vec<GameClient>,
}

impl Crowd {
    fn new(pack: &Pack, mods: ModsPlugin) -> Crowd {
        let files = game(Some(pack));
        let engine = engine(&files, mods);
        Crowd {
            server: server(&engine, Listen::Memory(SERVER)).expect("the server starts"),
            clients: Vec::new(),
        }
    }

    /// Adds a client; `stalled` takes away its way of asking the server for anything, so it holds its place.
    fn add(&mut self, stalled: bool) -> usize {
        let n = self.clients.len();
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_100 + n as u16);
        let files = game(None);
        let engine = engine(&files, client_plugin(&format!("crowd-{n}")));
        let mut client = client(
            &engine,
            ClientNet::Memory {
                client: address,
                server: SERVER,
            },
        )
        .expect("the client");
        if stalled {
            client
                .vm_mut()
                .eval(b"function commandToServer() {}")
                .unwrap();
        }
        self.clients.push(client);
        n
    }

    fn carry(&self) {
        let refs: Vec<&GameClient> = self.clients.iter().collect();
        carry_between(&self.server, &refs);
    }

    fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        for client in &mut self.clients {
            client.step(32);
        }
        self.carry();
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    fn connected(&self) -> u32 {
        String::from_utf8_lossy(
            &self
                .server
                .vm()
                .global("$Test::Connected")
                .unwrap_or_default(),
        )
        .parse()
        .unwrap_or(0)
    }

    /// Connects client `i` and says why it was refused: empty when the server took it.
    fn try_join(&mut self, i: usize) -> String {
        let client = &mut self.clients[i];
        client.set_global("$Why", "");
        client.call("connectTo", &[&SERVER.to_string()]).unwrap();
        self.run(60);
        self.clients[i].global("$Why")
    }
}

const BUSY: &str = "Server busy, try again.";

fn big_pack() -> Pack {
    Pack::new("1.2").art(pattern(BLOCK_BYTES * 7, 5))
}

#[test]
fn the_seventh_held_client_is_turned_away_with_server_busy() {
    let mut crowd = Crowd::new(&big_pack(), server_plugin());
    for i in 0..6 {
        let n = crowd.add(false);
        assert_eq!(crowd.try_join(n), "", "client {i} is taken");
    }
    assert_eq!(crowd.connected(), 0, "all six are still held");
    let n = crowd.add(false);
    assert_eq!(crowd.try_join(n), BUSY);
}

#[test]
fn a_held_place_frees_when_the_client_is_ready_leaves_or_stalls() {
    let two = DownloadLimits {
        max_clients: 2,
        idle_seconds: 100,
        ..DownloadLimits::default()
    };
    // Ready: one honest client and one that holds its place.
    let mut crowd = Crowd::new(&big_pack(), server_plugin().downloads(two));
    let honest = crowd.add(false);
    let stalled = crowd.add(true);
    assert_eq!(crowd.try_join(honest), "");
    assert_eq!(crowd.try_join(stalled), "");
    let third = crowd.add(false);
    assert_eq!(crowd.try_join(third), BUSY);
    for _ in 0..3_000 {
        if crowd.connected() >= 1 {
            break;
        }
        crowd.step();
    }
    assert_eq!(crowd.connected(), 1, "the honest client was released");
    assert_eq!(crowd.try_join(third), "", "its place was free");
    // The cap still holds with the new client in it.
    let fourth = crowd.add(false);
    assert_eq!(crowd.try_join(fourth), BUSY);

    // Leaving.
    let mut crowd = Crowd::new(&big_pack(), server_plugin().downloads(two));
    let (a, b) = (crowd.add(true), crowd.add(true));
    assert_eq!(crowd.try_join(a), "");
    assert_eq!(crowd.try_join(b), "");
    let c = crowd.add(false);
    assert_eq!(crowd.try_join(c), BUSY);
    crowd.clients[a].call("disconnect", &[]).unwrap();
    crowd.run(100);
    assert_eq!(
        crowd.try_join(c),
        "",
        "the place of the client that left was free"
    );

    // Standing still: both stalled clients are dropped after the idle time.
    let quick = DownloadLimits {
        idle_seconds: 6,
        ..two
    };
    let mut crowd = Crowd::new(&big_pack(), server_plugin().downloads(quick));
    let (a, b) = (crowd.add(true), crowd.add(true));
    assert_eq!(crowd.try_join(a), "");
    assert_eq!(crowd.try_join(b), "");
    let c = crowd.add(false);
    assert_eq!(crowd.try_join(c), BUSY);
    crowd.run(300);
    assert!(
        crowd.clients[a]
            .client_log
            .iter()
            .any(|l| l.contains("Connection with server lost"))
    );
    assert_eq!(
        crowd.try_join(c),
        "",
        "the stalled clients' places were free"
    );
}

#[test]
fn a_server_with_nothing_required_holds_nobody_and_counts_nobody() {
    let one = DownloadLimits {
        max_clients: 1,
        ..DownloadLimits::default()
    };
    let mut crowd = Crowd::new(
        &Pack::new("1.2").share("none"),
        server_plugin().downloads(one),
    );
    for i in 0..3 {
        let n = crowd.add(false);
        assert_eq!(crowd.try_join(n), "", "client {i}");
    }
    crowd.run(200);
    assert_eq!(
        crowd.connected(),
        3,
        "every client joined at once, past the cap of one"
    );
}
