//! The natives the vehicle demo's client and server script engines have, compared with the snapshot files in `tests/natives/`. A
//! change to what scripts can call shows as a diff here; `TORQUE_BLESS=1 cargo test -p torque-example-vehicle-demo --test
//! natives` rewrites the snapshot files.

use std::path::{Path, PathBuf};

use torque::script::vm::listing::check_listing;
use torque_example_vehicle_demo::client::{ClientOptions, NetMode, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_example_vehicle_demo::server::WorldServer;

fn listing(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/natives")
        .join(name)
}

#[test]
fn the_client_has_these_natives() {
    let game = VehicleDemo::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            port: 0,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    check_listing(&listing("client.txt"), &game.vm().native_listing());
}

#[test]
fn the_server_has_these_natives() {
    let files = GameFiles::bundled().unwrap();
    let server = WorldServer::start_alone(&files, "natives").expect("the server starts");
    check_listing(&listing("server.txt"), &server.vm().native_listing());
}
