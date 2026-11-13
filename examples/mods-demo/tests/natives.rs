//! The natives of the demo's script engines (the engine's plugins, the props plugin's and the mods'), compared with the snapshot
//! files in `tests/natives/`. `TORQUE_BLESS=1 cargo test -p torque-example-mods-demo --test main natives::` rewrites them. The Lua
//! extension is not loaded here, so the mods that need it are skipped and their functions are not listed. The demo speaks the
//! engine's own protocol whatever `TORQUE_NET_PROTOCOL` says (the props need it), so there is one listing a side.

use std::path::{Path, PathBuf};

use torque::app::client::ClientNet;
use torque::app::server::Listen;
use torque::mods::CacheChoice;
use torque::script::vm::listing::check_listing;
use torque_example_mods_demo::{
    Role, SERVER, client, client_files, engine, server_files, start_server,
};

/// The snapshot of `side`.
fn listing(side: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/natives")
        .join(format!("{side}.txt"))
}

#[test]
fn the_server_has_these_natives() {
    let engine = engine(&server_files().unwrap(), Role::Server, None).unwrap();
    let server = start_server(&engine, Listen::Memory(SERVER), 0).expect("the server starts");
    check_listing(&listing("server"), &server.vm().native_listing());
}

#[test]
fn the_client_has_these_natives() {
    let cache = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mods-demo-natives");
    let cache = CacheChoice::Folder(cache);
    let engine = engine(&client_files().unwrap(), Role::Client { cache }, None).unwrap();
    let client = client(&engine, ClientNet::None).expect("the client");
    check_listing(&listing("client"), &client.vm().native_listing());
}
