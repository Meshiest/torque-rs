//! The natives of the app layer's bare script engines, compared with the snapshot files in `tests/natives/`: a client made from
//! `ClientSetup::default()`, a server over a script that starts nothing, and the page bridge's. A change to what scripts can
//! call shows as a diff here; `TORQUE_BLESS=1 cargo test -p torque-app --test natives` rewrites the snapshot files.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use torque_app::client::{ClientBuilder, ClientSetup};
use torque_app::page_bridge;
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::listing::check_listing;
use torque_ts_vm::vm::Vm;

fn listing(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/natives")
        .join(name)
}

#[test]
fn the_bare_client_has_these_natives() {
    let builder = ClientBuilder::new(ClientSetup::default()).expect("the client");
    check_listing(&listing("client.txt"), &builder.vm.native_listing());
}

#[test]
fn the_bare_server_has_these_natives() {
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    let server = GameServer::start(
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts");
    check_listing(&listing("server.txt"), &server.vm().native_listing());
}

/// A web build installs these on its client's engine when the window is made; the snapshot is made natively.
#[test]
fn the_page_bridge_has_these_natives() {
    let mut vm = Vm::new();
    let before = vm.native_listing();
    page_bridge::install(&mut vm);
    let added: String = vm
        .native_listing()
        .lines()
        .filter(|line| !before.lines().any(|b| b == *line))
        .map(|line| format!("{line}\n"))
        .collect();
    check_listing(&listing("page-bridge.txt"), &added);
}
