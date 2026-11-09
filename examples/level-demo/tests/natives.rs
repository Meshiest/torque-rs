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

/// The client's list: the rope plugin's natives are in `client-rope.txt` (the `rope` feature), the default list is `client.txt`.
const CLIENT: &str = if cfg!(feature = "rope") {
    "client-rope.txt"
} else {
    "client.txt"
};

/// The server's list: the rope plugin's networked functions are in `server-rope.txt` (the `rope` feature), the default list is
/// `server.txt`. The demo speaks the engine's protocol whatever `TORQUE_NET_PROTOCOL` says, so both have the scene hierarchy's
/// natives, and the props' (`prop*`, `joint*`).
fn server_listing() -> PathBuf {
    listing(if cfg!(feature = "rope") {
        "server-rope.txt"
    } else {
        "server.txt"
    })
}

#[test]
fn the_client_has_these_natives() {
    let game = LevelDemo::new(GameFiles::bundled().unwrap()).unwrap();
    check_listing(&listing(CLIENT), &game.vm().native_listing());
}

#[test]
fn the_server_has_these_natives() {
    let game = LevelDemo::new(GameFiles::bundled().unwrap()).unwrap();
    let map = maps(&game.files).into_iter().next().expect("a map");
    let server =
        WorldServer::start(game.engine(), &game.files, &map.path).expect("the server starts");
    check_listing(&server_listing(), &server.vm().native_listing());
}
