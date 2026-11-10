//! The demo end to end, headless: a server whose game mode is a Lua mod that it requires, and two clients over the memory
//! transport that download the mod's content when they join, play the match (each steered by its bot), and never run its Lua.
//! The mods' load rules are checked on the server: a mod that requires another loads after it, and a mod the prefs disable is
//! never mounted.
//!
//! These tests need the Lua extension as a library, which they build with cargo (it compiles Luau's C++, so they need a C++
//! compiler and are ignored by `cargo test`): `just examples mods-demo test` runs them. `TORQUE_LUA_LIBRARY` names a library
//! built already.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use torque::app::server::GameServer;
use torque::mods::Mods;
use torque_example_mods_demo::{Lua, Session, assets, library_file_name, lua_library};
use torque_plugin_props::data::PropShape;

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
    s.joined()
}

fn number(s: &Session, name: &str) -> u32 {
    s.server_global(name).parse().unwrap_or(0)
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn mods_load_in_the_order_their_rules_give_and_a_pref_keeps_one_out() {
    let mut session = session("rules", false);
    assert_eq!(session.server_global("$Confetti::Ran"), "");
    let server = &mut session.server;
    let list = server.call("getModList", &[]).unwrap();
    let ids: Vec<&str> = list.split_whitespace().collect();
    // The list holds the disabled mod too, in its place: every mod requires `soccer`, then the free ones go by id.
    assert_eq!(
        ids,
        ["soccer", "big-ball", "confetti"],
        "big-ball requires soccer, so it loads after it though its name sorts first"
    );
    let field = |server: &mut GameServer, id: &str, field: &str| {
        server.call("getModField", &[id, field]).unwrap()
    };
    assert_eq!(field(server, "soccer", "status"), "enabled");
    assert_eq!(field(server, "big-ball", "status"), "enabled");
    // The pref `$Pref::Mods::Disabled` (game/prefs.cs, read by the entry script) says confetti starts off: it is never mounted,
    // its scripts never run, and nothing of it is offered to players.
    assert_eq!(field(server, "confetti", "status"), "disabled");
    assert_eq!(server.call("isModEnabled", &["confetti"]).unwrap(), "0");
    assert_eq!(server.call("isModEnabled", &["soccer"]).unwrap(), "1");
    assert_eq!(server.call("getRequiredModList", &[]).unwrap(), "soccer");
    // The second mod's script ran after the first's (its ball copies the soccer mod's), and its Lua sits over the first's: the
    // match is played with the big ball.
    assert_eq!(
        session.server_eval("return BigBallData.radius;").unwrap(),
        "0.75"
    );
    assert_eq!(
        session
            .server_eval("return BigBallData.restitution;")
            .unwrap(),
        session.server_eval("return BallData.restitution;").unwrap()
    );
    let server = &mut session.server;
    assert_eq!(server.call("soccerBallData", &[]).unwrap(), "BigBallData");
    // Switching the second mod off brings the first's rules back.
    server.call("disableMod", &["big-ball"]).unwrap();
    assert_eq!(server.call("soccerBallData", &[]).unwrap(), "BallData");
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn two_clients_download_the_mod_and_play_a_match() {
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
        .and_then(|m| m.content_of("soccer"))
        .expect("the server has the soccer mod")
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
            "shapes/pitch.dts",
            "shapes/pitch.png",
            "shapes/player_blue.dts",
            "shapes/ball.dts",
            "sounds/kick.wav",
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
    let generated: u64 = assets::soccer()
        .unwrap()
        .iter()
        .map(|(_, b)| b.len() as u64)
        .sum();
    let module = std::fs::metadata(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("game/mods/soccer/lib/rules.lua"),
    )
    .unwrap()
    .len();
    assert_eq!(content, generated + module);
    // One player a team.
    assert_eq!(
        [
            s.client_global(0, "$Match::Team"),
            s.client_global(1, "$Match::Team")
        ],
        ["Blue", "Red"]
    );
    // Entering twice gives a client no second player.
    let figures = "%n = 0; for (%i = 0; %i < MissionCleanup.getCount(); %i++) \
                   if (MissionCleanup.getObject(%i).getClassName() $= \"Player\") %n++; return %n;";
    assert_eq!(s.server_eval(figures).unwrap(), "2");
    s.server_eval("ClientGroup.getObject(0).onEnterGame();")
        .unwrap();
    assert_eq!(s.server_eval(figures).unwrap(), "2");

    // The Lua game mode runs on the server and the bots play: the ball is kicked, goals are scored, a round ends and the next
    // starts.
    let mut scores = BTreeSet::new();
    let mut shown = BTreeSet::new();
    let done = s.run_until(9_000, |s| {
        scores.insert(s.server_global("$Soccer::Score"));
        shown.insert(s.client_global(1, "$Match::Phase"));
        number(s, "$Soccer::Round") >= 2 && number(s, "$Soccer::Goals") >= 2
    });
    let (goals, kicks) = (number(&s, "$Soccer::Goals"), number(&s, "$Soccer::Kicks"));
    assert!(
        done,
        "a round ended and goals were scored: {goals} goals, round {}, {scores:?}",
        s.server_global("$Soccer::Round")
    );
    assert!(kicks > goals, "the ball was kicked: {kicks}");
    assert!(scores.len() >= 3, "the score changed: {scores:?}");
    assert!(
        ["play", "goal", "over"].iter().all(|p| shown.contains(*p)),
        "the clients were shown play, a goal and the end of a round: {shown:?}"
    );
    assert_eq!(s.server_global("$Soccer::Ball"), "BigBallData");
    assert_eq!(s.server_global("$Confetti::Ran"), "");

    // Both clients show the board the server sends and hold the ball, the big one, as a prop.
    for n in 0..2 {
        assert!(
            s.client_global(n, "$Match::Updates")
                .parse::<u32>()
                .unwrap()
                > 20
        );
        let state = s.client_global(n, "$Match::State");
        assert_eq!(state.split('\t').count(), 6, "{state:?}");
        // The Lua's own globals are the server's: a client never ran it.
        assert_eq!(s.client_global(n, "$Soccer::Round"), "");
        assert_eq!(s.client_global(n, "$Soccer::Goals"), "");
        let ball = s.ball(n).and_then(|b| b.params).map(|p| p.shape);
        assert_eq!(
            ball,
            Some(PropShape::Ball(0.75)),
            "client {n} holds the big ball"
        );
        // The kicks are heard: the server asks each client to play the kick, and the client loads it from the mod's content.
        assert!(
            s.client_global(n, "$Sound::Played")
                .parse::<u32>()
                .unwrap_or(0)
                > 0,
            "client {n} heard no kick"
        );
        assert_ne!(
            s.client_global(n, "$Sound::Last"),
            "0",
            "client {n} could not play the kick"
        );
    }

    // A second join of a client that has the content in its cache sends nothing.
    s.disconnect(0).unwrap();
    s.run(200);
    s.clients[0].set_global("$Download::Done", "");
    s.clients[0].set_global("$Match::Updates", "");
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
        mods.is_server_file("shapes/ball.dts"),
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
    for path in ["lib/rules.lua", "mods/soccer/lib/rules.lua"] {
        let result = s.clients[0]
            .vm_mut()
            .eval(format!("return exec(\"{path}\");").as_bytes());
        assert_ne!(result.map(|r| r == b"1"), Ok(true), "exec ran {path}");
    }
    s.run(100);
    assert_eq!(s.client_global(0, "$Soccer::Round"), "");
    assert_eq!(s.client_global(0, "$Soccer::Goals"), "");
    assert!(number(&s, "$Soccer::Round") >= 1, "the server's copy runs");
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn a_client_plays_only_the_kick_and_only_on_the_pitch() {
    let mut s = session("sound", false);
    s.add_client().unwrap();
    s.connect(0).unwrap();
    assert!(s.run_until(6_000, joined));
    let client = &mut s.clients[0];
    let mut play = |args: &[&str]| {
        let before = client.global("$Sound::Played");
        client.call("clientCmdPlaySound", args).unwrap();
        (client.global("$Sound::Played") != before).then(|| client.global("$Sound::At"))
    };
    // The kick on the pitch plays where it was asked.
    assert_eq!(
        play(&["sounds/kick.wav", "1", "-2", "0.5"]).as_deref(),
        Some("1 -2 0.5")
    );
    // Any other file plays nothing, even one the client has.
    for file in [
        "shapes/ball.png",
        "sounds/../sounds/kick.wav",
        "SOUNDS/KICK.WAV",
        "client.cs",
    ] {
        assert_eq!(play(&[file, "0", "0", "0"]), None, "{file}");
    }
    // A place that is not three plain numbers plays nothing.
    for place in [
        ["nan", "0", "0"],
        ["1", "inf", "0"],
        ["1", "2", ""],
        ["1;2", "0", "0"],
        ["1 2", "0", "0"],
    ] {
        let [x, y, z] = place;
        assert_eq!(play(&["sounds/kick.wav", x, y, z]), None, "{place:?}");
    }
    // A place past the pitch is moved onto its edge.
    assert_eq!(
        play(&["sounds/kick.wav", "1e30", "-99999", "-5"]).as_deref(),
        Some("8 -13.5 0")
    );
}

#[test]
#[ignore = "builds the Lua extension and needs a GPU (offscreen: no window opens)"]
fn pictures_of_a_match_are_drawn_from_a_client() {
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
    assert_eq!(paths.len(), 5, "{paths:?}");
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
