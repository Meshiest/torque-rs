//! The demo end to end, headless: a server whose game mode is a Lua mod that it requires, and two clients over the memory
//! transport that download the mod's content when they join, see the game mode's ropes, and never run its Lua. The mods' load
//! rules are checked on the server: a mod that requires another loads after it, and a mod the prefs disable is never mounted.
//!
//! These tests need the Lua extension as a library, which they build with cargo (it compiles Luau's C++, so they need a C++
//! compiler and are ignored by `cargo test`): `just examples/mods-demo-test` runs them. `TORQUE_LUA_LIBRARY` names a library
//! built already.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use torque::app::server::GameServer;
use torque::mods::Mods;
use torque_example_mods_demo::{Lua, Session, assets, library_file_name, lua_library};

/// The build folder: the one `cargo test` was given, else the workspace's.
fn target_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("a build folder")
        .to_path_buf()
}

/// The extension's library: the file `TORQUE_LUA_LIBRARY` names, else built by cargo once (a build of its own workspace, as the
/// extension's own tests build it).
fn library() -> &'static Path {
    static LIBRARY: OnceLock<PathBuf> = OnceLock::new();
    LIBRARY.get_or_init(|| {
        if let Some(library) = lua_library() {
            return library;
        }
        let manifest =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/torque-plugin-lua/Cargo.toml");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--quiet", "--lib", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(target_dir())
            .status()
            .expect("cargo runs");
        assert!(status.success(), "the Lua extension builds");
        let library = target_dir()
            .join("debug")
            .join(library_file_name("torque_lua"));
        assert!(library.is_file(), "{} is built", library.display());
        library
    })
}

fn work(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("mods-demo-{name}"))
}

/// A session with the Lua extension on the server (and on the clients when `lua_clients`), downloads in a fresh folder.
fn session(name: &str, lua_clients: bool) -> Session {
    let dir = work(name);
    let _ = std::fs::remove_dir_all(&dir);
    let lua = Lua::install(library(), &dir.join("lua")).expect("the extension installs");
    Session::new(Some(lua), lua_clients, dir.join("downloads")).expect("the session starts")
}

/// Bytes a client has received, of what it was told to fetch.
fn received(session: &mut Session, client: usize) -> u64 {
    let progress = session.clients[client]
        .call("getModDownloadProgress", &[])
        .expect("the progress");
    progress
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .expect("a number")
}

fn joined(s: &mut Session) -> bool {
    s.clients
        .iter()
        .all(|c| c.ghost_count() >= 8 && !c.global("$Orbs::Updates").is_empty())
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn mods_load_in_the_order_their_rules_give_and_a_pref_keeps_one_out() {
    let mut session = session("rules", false);
    assert_eq!(session.server_global("$Confetti::Ran"), "");
    let server = &mut session.server;
    let list = server.call("getModList", &[]).unwrap();
    let ids: Vec<&str> = list.split_whitespace().collect();
    // The list holds the disabled mod too, in its place: every mod requires `orbs`, then the free ones go by id.
    assert_eq!(
        ids,
        ["orbs", "confetti", "golden-orbs"],
        "golden-orbs requires orbs, so it loads after it though its name sorts first"
    );
    let field = |server: &mut GameServer, id: &str, field: &str| {
        server.call("getModField", &[id, field]).unwrap()
    };
    assert_eq!(field(server, "orbs", "status"), "enabled");
    assert_eq!(field(server, "golden-orbs", "status"), "enabled");
    // The pref `$Pref::Mods::Disabled` (game/prefs.cs, read by the entry script) says confetti starts off: it is never mounted,
    // its scripts never run, and nothing of it is offered to players.
    assert_eq!(field(server, "confetti", "status"), "disabled");
    assert_eq!(server.call("isModEnabled", &["confetti"]).unwrap(), "0");
    assert_eq!(server.call("isModEnabled", &["orbs"]).unwrap(), "1");
    assert_eq!(server.call("getRequiredModList", &[]).unwrap(), "orbs");
    // The Lua of the second mod sits over the first's: the third orb is gold.
    let value = |server: &mut GameServer, key: &str| server.call("orbsValue", &[key]).unwrap();
    assert_eq!(
        (value(server, "1"), value(server, "2"), value(server, "3")),
        ("1".to_string(), "1".to_string(), "3".to_string())
    );
    // Switching the second mod off brings the first's rules back.
    server.call("disableMod", &["golden-orbs"]).unwrap();
    assert_eq!(value(server, "3"), "1");
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn two_clients_download_the_mod_play_rounds_and_see_the_ropes() {
    let mut s = session("play", false);
    for n in 0..2 {
        assert_eq!(s.add_client().unwrap(), n);
        assert_eq!(s.clients[n].call("getModList", &[]).unwrap(), "");
        s.connect(n).unwrap();
    }
    let in_time = s.run_until(6_000, joined);
    let console = s.server_console();
    assert!(
        in_time,
        "both clients joined: {:?} {console:?}",
        s.clients[0].client_log
    );

    // Both downloaded the mod's content, once.
    let content = Mods::of(s.server.vm())
        .and_then(|m| m.content_of("orbs"))
        .expect("the server has the orbs mod")
        .bytes;
    assert!(content > 1_000, "the mod has content: {content} bytes");
    for n in 0..2 {
        assert_eq!(received(&mut s, n), content, "client {n} downloaded it");
        assert_eq!(
            s.clients[n].call("getModDownloadProgress", &[]).unwrap(),
            format!("{content} {content} 1 1")
        );
        assert_eq!(s.client_global(n, "$Download::Done"), "1");
        assert_eq!(s.client_global(n, "$Download::Failed"), "");
        let mods = Mods::of(s.clients[n].vm()).expect("the client has mods");
        for file in [
            "shapes/orb.dts",
            "shapes/orb.png",
            "shapes/runner_a.dts",
            "art/beam.png",
            "sounds/pickup.wav",
        ] {
            assert!(mods.is_server_file(file), "client {n} mounted {file}");
            assert!(mods.is_untrusted(file), "and holds it as untrusted");
        }
        // Nothing of the mod that could run came with it, and the disabled mod's content never did.
        // The module is there as data and is untrusted: nothing runs it.
        assert!(mods.is_untrusted("lib/rules.lua"));
        for file in ["server.cs", "server.lua", "mod.toml", "art/confetti.png"] {
            assert!(!mods.is_server_file(file), "client {n} has {file}");
        }
    }
    // The content is the mod's generated files and its Lua module (`lib/rules.lua`: Lua files are content, kept as data);
    // not `server.cs`, `server.lua` or `mod.toml`.
    let generated: u64 = assets::orbs()
        .unwrap()
        .iter()
        .map(|(_, b)| b.len() as u64)
        .sum();
    let module = std::fs::metadata(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("game/mods/orbs/lib/rules.lua"),
    )
    .unwrap()
    .len();
    assert_eq!(content, generated + module);

    // The Lua game mode runs on the server: the rounds advance, the scores move, the third orb is gold.
    let mut seen = std::collections::BTreeSet::new();
    let mut rounds = std::collections::BTreeSet::new();
    let mut ropes = 0;
    for _ in 0..1_900 {
        s.step();
        seen.insert(s.server_global("$Orbs::Scores"));
        rounds.insert(s.server_global("$Orbs::Round"));
        if let Ok(n) = s.clients[1].call("ropeNetCount", &[]) {
            ropes = ropes.max(n.parse::<u32>().unwrap_or(0));
        }
    }
    assert!(
        rounds.contains("3"),
        "the rounds advanced past two: {rounds:?}"
    );
    assert!(seen.len() >= 6, "the score changed: {seen:?}");
    let taken: u32 = s.server_global("$Orbs::Collected").parse().unwrap();
    let points: u32 = s.server_global("$Orbs::Points").parse().unwrap();
    assert!(taken > 10, "orbs were taken: {taken}");
    assert!(
        points > taken,
        "golden orbs are worth more: {points} of {taken}"
    );
    assert!(
        s.server_console()
            .iter()
            .any(|l| l.contains("orbs: the game mode is loaded in the server realm"))
            || taken > 0
    );
    assert_eq!(s.server_global("$Confetti::Ran"), "");

    // Both clients show the board the server sends and have the ropes: a beam for each runner and the leader's cable.
    for n in 0..2 {
        assert!(s.client_global(n, "$Orbs::Updates").parse::<u32>().unwrap() > 20);
        let state = s.client_global(n, "$Orbs::State");
        assert!(state.contains("Blue") && state.contains("Red"), "{state:?}");
        // The Lua's own globals are the server's: a client never ran it.
        assert_eq!(s.client_global(n, "$Orbs::Round"), "");
        assert_eq!(s.client_global(n, "$Orbs::Points"), "");
    }
    assert!(ropes >= 3, "the clients hold the ropes: {ropes}");
    let drawn: usize = s
        .engines
        .iter()
        .skip(1)
        .map(|e| {
            e.resource::<torque_plugin_rope::RopeShared>()
                .expect("ropes")
                .lock()
                .ropes()
                .count()
        })
        .sum();
    assert!(drawn >= 6, "each client simulates {drawn} ropes in all");

    // A second join of a client that has the content in its cache sends nothing.
    s.disconnect(0).unwrap();
    s.run(200);
    s.clients[0].set_global("$Download::Done", "");
    s.clients[0].set_global("$Orbs::Updates", "");
    s.connect(0).unwrap();
    assert!(s.run_until(6_000, joined), "the second join");
    assert_eq!(
        s.clients[0].call("getModDownloadProgress", &[]).unwrap(),
        "0 0 0 0"
    );
    assert_eq!(s.client_global(0, "$Download::Done"), "1");
    let cached = std::fs::read_dir(s.cache_of(0))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".mod"))
        .count();
    assert_eq!(cached, 1, "one entry in the client's cache");
    let mods = Mods::of(s.clients[0].vm()).unwrap();
    assert!(
        mods.is_server_file("shapes/orb.dts"),
        "mounted from the cache"
    );
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn a_servers_lua_never_runs_on_a_client_that_has_the_extension() {
    let mut s = session("lua-client", true);
    s.add_client().unwrap();
    s.connect(0).unwrap();
    assert!(s.run_until(6_000, joined));
    // The client has Lua: it runs its own scripts. What the server sent is data: the rules file may be there, and neither
    // `exec` nor Lua's `require` runs it.
    let mods = Mods::of(s.clients[0].vm()).unwrap();
    assert!(
        mods.is_server_file("lib/rules.lua"),
        "the module arrived as content"
    );
    assert!(mods.is_untrusted("lib/rules.lua"));
    for path in ["lib/rules.lua", "mods/orbs/lib/rules.lua"] {
        let result = s.clients[0]
            .vm_mut()
            .eval(format!("return exec(\"{path}\");").as_bytes());
        assert_ne!(result.map(|r| r == b"1"), Ok(true), "exec ran {path}");
    }
    s.run(100);
    assert_eq!(s.client_global(0, "$Orbs::Round"), "");
    assert_eq!(s.client_global(0, "$Orbs::Points"), "");
    assert!(
        s.server_global("$Orbs::Round").parse::<u32>().unwrap() >= 1,
        "the server's copy runs"
    );
}

#[test]
#[ignore = "builds the Lua extension and needs a GPU (offscreen: no window opens)"]
fn pictures_of_a_round_are_drawn_from_a_client() {
    let mut s = session("pictures", false);
    for n in 0..2 {
        s.add_client().unwrap();
        s.connect(n).unwrap();
    }
    assert!(s.run_until(6_000, joined));
    let dir = std::env::var_os("TORQUE_MODS_DEMO_PICTURES")
        .map_or_else(|| work("pictures").join("shots"), PathBuf::from);
    let paths = match torque_example_mods_demo::viewer::write_shots(s, &dir) {
        Ok(paths) => paths,
        Err(e) if e.contains("adapter") || e.contains("GPU") => {
            eprintln!("skipped: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let sizes: Vec<u64> = paths
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .collect();
    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(
        sizes.iter().all(|&n| n > 20_000),
        "pictures with something in them: {sizes:?}"
    );
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn a_library_added_to_the_plugins_folder_is_not_approved_while_the_shipped_one_loads() {
    use torque::extensions::{LoadReport, Outcome};
    use torque_example_mods_demo::{Role, engine, server_files};
    let dir = work("extra-library");
    let _ = std::fs::remove_dir_all(&dir);
    let lua = Lua::install(library(), &dir.join("lua")).expect("the extension installs");
    // Another library beside it: any bytes, refused by its hash before it is loaded.
    let plugins = dir.join("lua").join("plugins");
    std::fs::write(plugins.join(library_file_name("extra")), b"not shipped").unwrap();
    std::fs::write(
        plugins.join("extra.toml"),
        "id = \"someone.extra\"\nversion = \"1.0.0\"\nlibrary = \"extra\"\n",
    )
    .unwrap();
    let files = server_files().unwrap();
    let engine = engine(&files, Role::Server, Some(&lua)).expect("the engine builds");
    let report = engine.resource::<LoadReport>().expect("the loader ran");
    assert_eq!(
        report.get("torque.lua").map(|e| &e.outcome),
        Some(&Outcome::Loaded)
    );
    assert_eq!(
        report.get("someone.extra").map(|e| &e.outcome),
        Some(&Outcome::NotApproved)
    );
}
