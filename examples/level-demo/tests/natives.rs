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

#[test]
fn the_client_has_these_natives() {
    let game = LevelDemo::new(GameFiles::bundled().unwrap()).unwrap();
    check_listing(&listing("client.txt"), &game.vm().native_listing());
}

#[test]
fn the_server_has_these_natives() {
    let files = GameFiles::bundled().unwrap();
    let map = maps(&files).into_iter().next().expect("a map");
    let server = WorldServer::start(&files, &map.path).expect("the server starts");
    check_listing(&listing("server.txt"), &server.vm().native_listing());
}
