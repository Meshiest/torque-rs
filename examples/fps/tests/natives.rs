//! The natives the arena shooter's client and server script engines have, compared with the snapshot files in `tests/natives/`.
//! A change to what scripts can call shows as a diff here; `TORQUE_BLESS=1 cargo test -p torque-example-fps --test natives`
//! rewrites the snapshot files.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use torque::script::vm::listing::check_listing;
use torque::server::net::transport::{MemoryTransport, SharedTransport, Transport};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;
use torque_example_fps::server::{ArenaServer, ServerOptions};

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

/// The server's list: the rope plugin's networked functions are in `server-rope.txt` (the `rope` feature), the default list is `server.txt`.
const SERVER: &str = if cfg!(feature = "rope") {
    "server-rope.txt"
} else {
    "server.txt"
};

#[test]
fn the_client_has_these_natives() {
    let game = ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            port: 0,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    check_listing(&listing(CLIENT), &game.vm().native_listing());
}

#[test]
fn the_server_has_these_natives() {
    let files = GameFiles::bundled().unwrap();
    let (mut transport, _end) = MemoryTransport::new();
    transport
        .bind(Ipv4Addr::LOCALHOST.into(), 28_000)
        .expect("the in-memory address binds");
    let transport: SharedTransport = Rc::new(RefCell::new(transport));
    let options = ServerOptions {
        address: Ipv4Addr::LOCALHOST,
        port: 0,
        name: "natives".to_string(),
        mission: files.maps().first().expect("a map").clone(),
        public: false,
    };
    let server = ArenaServer::start_bound(&files, &options, transport).expect("the server starts");
    assert_eq!(
        server.address(),
        Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000)))
    );
    check_listing(&listing(SERVER), &server.vm().native_listing());
}
