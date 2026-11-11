//! The natives the level demo's client and server script engines have, compared with the snapshot files in `tests/natives/`. A
//! change to what scripts can call shows as a diff here; `TORQUE_BLESS=1 cargo test -p torque-example-level-demo --test
//! natives` rewrites the snapshot files.

use std::path::{Path, PathBuf};

use torque::script::vm::listing::check_listing;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::{GameFiles, maps};
use torque_example_level_demo::server::WorldServer;

fn listing(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/natives")
        .join(name)
}

/// The client's list: the rope plugin's natives are in `client-rope.txt` (the `rope` feature), the default list is `client.txt`. The
/// demo speaks the engine's protocol whatever `TORQUE_NET_PROTOCOL` says, so both have the RPC natives.
fn client_listing() -> PathBuf {
    listing(if cfg!(feature = "rope") {
        "client-rope.txt"
    } else {
        "client.txt"
    })
}

/// The server's list: the rope plugin's networked functions are in `server-rope.txt` (the `rope` feature), the default list is
/// `server.txt`. The demo speaks the engine's protocol whatever `TORQUE_NET_PROTOCOL` says, so both have the scene hierarchy's
/// natives, and the props' (`prop*`, `joint*`). The props' `jointRope` is there when the props have their `rope` feature, which a
/// build can have on without the demo's (another crate asks for it): that list is `server-joint-rope.txt`.
fn server_listing() -> PathBuf {
    listing(match (cfg!(feature = "rope"), torque_plugin_props::ROPE) {
        (true, _) => "server-rope.txt",
        (false, true) => "server-joint-rope.txt",
        (false, false) => "server.txt",
    })
}

#[test]
fn the_client_has_these_natives() {
    let game = LevelDemo::new(GameFiles::bundled().unwrap()).unwrap();
    check_listing(&client_listing(), &game.vm().native_listing());
}

#[test]
fn the_server_has_these_natives() {
    let game = LevelDemo::new(GameFiles::bundled().unwrap()).unwrap();
    let map = maps(&game.files).into_iter().next().expect("a map");
    let server =
        WorldServer::start(game.engine(), &game.files, &map.path).expect("the server starts");
    check_listing(&server_listing(), &server.vm().native_listing());
}

/// The office played with others (`office_engine`, the window's engine): the client's and the server's lists, with the mods,
/// the list of matches, the join screen's and the server scripts' natives. Checked in the default build only (the rope
/// feature's natives are the other lists').
#[test]
fn the_office_engines_client_and_server_have_these_natives() {
    use torque::mods::CacheChoice;
    use torque_example_level_demo::client::MatchOptions;
    use torque_example_level_demo::office::{ServerScripts, office_engine, start_office};
    use torque_plugin_client_script::Choices;

    if cfg!(feature = "rope") || torque_plugin_props::ROPE {
        return;
    }
    let engine = office_engine(
        GameFiles::bundled().unwrap(),
        Vec::new(),
        CacheChoice::Memory,
    )
    .unwrap()
    .build()
    .unwrap();
    let options = MatchOptions {
        net: torque_example_level_demo::client::NetMode::Loopback,
        ..MatchOptions::default()
    };
    let scripts = ServerScripts::new(Choices::in_memory(), None);
    let game = LevelDemo::for_matches(&engine, options, None, scripts).unwrap();
    check_listing(&listing("office-client.txt"), &game.vm().native_listing());
    let listen =
        torque::app::server::Listen::Memory(torque_example_level_demo::server::SERVER_ADDR);
    let start = torque_example_level_demo::office::OfficeStart {
        name: "Office",
        dedicated: true,
    };
    let server = start_office(&engine, listen, &start).expect("the server starts");
    check_listing(&listing("office-server.txt"), &server.vm().native_listing());
}
