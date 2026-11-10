//! The natives the vehicle demo's client and server script engines have, compared with the snapshot files in `tests/natives/`. A
//! change to what scripts can call shows as a diff here; `TORQUE_BLESS=1 cargo test -p torque-example-vehicle-demo --test
//! natives` rewrites the snapshot files.

use std::path::{Path, PathBuf};

use torque::app::plugins::protocol_from_env;
use torque::engine::ProtocolKind;
use torque::script::vm::listing::check_listing;
use torque_example_vehicle_demo::client::{ClientOptions, NetMode, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_example_vehicle_demo::seats::server_engine;
use torque_example_vehicle_demo::server::WorldServer;

fn listing(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/natives")
        .join(name)
}

/// The server's list for the protocol in use: the engine's has the scene hierarchy's natives too, in `server-engine.txt`.
fn server_listing() -> PathBuf {
    listing(match protocol_from_env() {
        ProtocolKind::Engine => "server-engine.txt",
        _ => "server.txt",
    })
}

/// The client's list for the protocol in use: the engine's has the RPC natives too, in `client-engine.txt`.
fn client_listing() -> PathBuf {
    listing(match protocol_from_env() {
        ProtocolKind::Engine => "client-engine.txt",
        _ => "client.txt",
    })
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
    check_listing(&client_listing(), &game.vm().native_listing());
}

#[test]
fn the_server_has_these_natives() {
    let files = GameFiles::bundled().unwrap();
    let server = WorldServer::start_alone(&server_engine().unwrap(), &files, "natives")
        .expect("the server starts");
    check_listing(&server_listing(), &server.vm().native_listing());
}
