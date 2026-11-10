//! The natives of the example's script engines, compared with the snapshot files in `tests/natives/` and with the hand-wired
//! example's: the engine's server and client have every native that example's have, and the snapshots here say what else
//! `DefaultPlugins` adds. `TORQUE_BLESS=1 cargo test -p torque-example-minimal-engine --test natives` rewrites the snapshot files.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use torque::app::client::{ClientBuilder, ClientNet};
use torque::app::plugins::{DefaultPlugins, protocol_from_env};
use torque::app::server::Listen;
use torque::engine::{Engine, EngineBuilder, ProtocolKind};
use torque::script::vm::listing::check_listing;
use torque_example_minimal_engine::{ExamplePlugin, builder, client, files, server};

fn listing(folder: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(folder)
        .join("tests/natives")
        .join(name)
}

fn engine() -> Engine {
    builder(&files().unwrap()).unwrap().build().unwrap()
}

/// The server's snapshot for the protocol in use: the engine's has the scene hierarchy's natives too, in `server-engine.txt`.
fn server_listing() -> PathBuf {
    listing(
        ".",
        match protocol_from_env() {
            ProtocolKind::Engine => "server-engine.txt",
            _ => "server.txt",
        },
    )
}

/// The window client's snapshot for the protocol in use: the engine's has the RPC natives, in `client-engine.txt`.
fn client_listing() -> PathBuf {
    listing(
        ".",
        match protocol_from_env() {
            ProtocolKind::Engine => "client-engine.txt",
            _ => "client.txt",
        },
    )
}

/// The natives (name and arguments) of the hand-wired example's snapshot `name` that `actual` lacks; a native's usage text may differ.
fn missing(name: &str, actual: &str) -> Vec<String> {
    let key = |line: &str| line.split(' ').take(2).collect::<Vec<_>>().join(" ");
    let original = std::fs::read_to_string(listing("../minimal", name)).unwrap();
    original
        .lines()
        .filter(|line| !actual.lines().any(|l| key(l) == key(line)))
        .map(String::from)
        .collect()
}

#[test]
fn the_server_has_these_natives() {
    let at = SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000));
    let server = server(&engine(), Listen::Memory(at)).expect("the server starts");
    let actual = server.vm().native_listing();
    check_listing(&server_listing(), &actual);
    assert_eq!(missing("server.txt", &actual), Vec::<String>::new());
}

#[test]
fn the_window_client_has_these_natives() {
    let client = client(&engine(), ClientNet::None).expect("the client");
    let actual = client.vm().native_listing();
    check_listing(&client_listing(), &actual);
    assert_eq!(missing("client.txt", &actual), Vec::<String>::new());
}

#[test]
fn game_toml_turns_off_the_plugins_it_names_and_their_natives_go() {
    let engine = engine();
    for id in [
        "torque.audio",
        "torque.water",
        "torque.terrain",
        "torque.server-query",
    ] {
        assert!(!engine.is_plugin_added_id(id), "{id} is off");
    }
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());
    let natives = client(&engine, ClientNet::None)
        .expect("the client")
        .vm()
        .native_listing();
    assert!(!natives.contains("queryLanServers"));

    // The same plugins without the config keep their natives.
    let plain = EngineBuilder::new()
        .add_plugins((DefaultPlugins, ExamplePlugin(files().unwrap())))
        .build()
        .unwrap();
    assert!(plain.is_plugin_added_id("torque.server-query"));
    let natives = ClientBuilder::from_engine(&plain, None, ClientNet::None)
        .expect("the client")
        .vm
        .native_listing();
    assert!(natives.contains("queryLanServers"));
}
