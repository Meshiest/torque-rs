//! The natives of the app layer's bare script engines, compared with the snapshot files in `tests/natives/`: a client made from
//! `ClientSetup::default()`, a server over a script that starts nothing, and the page bridge's. A change to what scripts can
//! call shows as a diff here; `TORQUE_BLESS=1 cargo test -p torque-app --test main natives::` rewrites the snapshot files.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use torque_app::client::{ClientBuilder, ClientNet, ClientSetup};
use torque_app::page_bridge;
use torque_app::plugins::{
    AudioPlugin, DefaultPlugins, GameClassesPlugin, ServerQueryPlugin, protocol_from_env,
};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_engine::{EngineBuilder, PluginGroup, ProtocolKind};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::listing::check_listing;
use torque_ts_vm::vm::Vm;

fn listing(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/natives")
        .join(name)
}

/// The snapshot for the protocol in use: the engine's has the natives of its RPCs too, in `<stem>-engine.txt`.
fn listing_for_protocol(stem: &str) -> PathBuf {
    listing(&match protocol_from_env() {
        ProtocolKind::Engine => format!("{stem}-engine.txt"),
        _ => format!("{stem}.txt"),
    })
}

#[test]
fn the_bare_client_has_these_natives() {
    let builder = ClientBuilder::new(ClientSetup::default()).expect("the client");
    check_listing(
        &listing_for_protocol("client"),
        &builder.vm.native_listing(),
    );
}

/// With the audio plugin turned off, the client has the natives it had before audio joined the defaults.
#[test]
fn the_bare_client_without_audio_has_these_natives() {
    let mut engine = EngineBuilder::new();
    engine.add_plugins(
        DefaultPlugins
            .disable::<GameClassesPlugin>()
            .disable::<ServerQueryPlugin>()
            .disable::<AudioPlugin>(),
    );
    let engine = engine.build().expect("the engine builds");
    let builder = ClientBuilder::from_engine(&engine, None, ClientNet::None).expect("the client");
    check_listing(
        &listing_for_protocol("client-no-audio"),
        &builder.vm.native_listing(),
    );
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
    check_listing(
        &listing_for_protocol("server"),
        &server.vm().native_listing(),
    );
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
