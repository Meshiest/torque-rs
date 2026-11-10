//! The natives of the demo's script engines (the engine's plugins, the rope plugin's and the mods'), compared with the snapshot
//! files in `tests/natives/`. `TORQUE_BLESS=1 cargo test -p torque-example-mods-demo --test natives` rewrites them. The Lua
//! extension is not loaded here, so the mods that need it are skipped and their functions are not listed. In the engine's
//! protocol the mods' request is an RPC, so the RPC natives are there too: those listings are `*-engine.txt`.

use std::path::{Path, PathBuf};

use torque::app::client::ClientNet;
use torque::script::vm::listing::check_listing;
use torque_example_mods_demo::{Role, client, client_files, engine, server, server_files};

/// The snapshot of `side` for the protocol `engine` speaks.
fn listing(side: &str, engine: &torque::engine::Engine) -> PathBuf {
    let name = match engine.protocol() {
        torque::engine::ProtocolKind::Engine => format!("{side}-engine.txt"),
        _ => format!("{side}.txt"),
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/natives")
        .join(name)
}

#[test]
fn the_server_has_these_natives() {
    let engine = engine(&server_files().unwrap(), Role::Server, None).unwrap();
    let server = server(&engine).expect("the server starts");
    check_listing(&listing("server", &engine), &server.vm().native_listing());
}

#[test]
fn the_client_has_these_natives() {
    let cache = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mods-demo-natives");
    let engine = engine(&client_files().unwrap(), Role::Client { cache }, None).unwrap();
    let client = client(&engine, ClientNet::None).expect("the client");
    check_listing(&listing("client", &engine), &client.vm().native_listing());
}
