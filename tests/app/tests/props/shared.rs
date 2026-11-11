//! The props plugin on a script engine a client shares with its server (`ClientBuilder::over_server`): each side keeps its
//! own tether sources, so the server's Player step and the client's prediction never ask each other's.

use std::net::{Ipv4Addr, SocketAddr};

use torque_app::plugins::{DefaultPlugins, set_protocol};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_app::{ClientBuilder, ClientNet};
use torque_client::conn::tge::TgeConn;
use torque_engine::{EngineBuilder, ProtocolKind};
use torque_game::player_tether::{self, Side};
use torque_plugin_props::PropsPlugin;
use torque_ts_vm::host::MemFiles;

#[test]
fn a_client_over_its_server_keeps_each_sides_tether_sources_apart() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins((DefaultPlugins, PropsPlugin));
    set_protocol(&mut builder, ProtocolKind::Tge);
    let engine = builder.build().expect("the engine builds");
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    let server = GameServer::start_engine(
        &engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts");
    let count =
        |vm: &torque_ts_vm::vm::Vm, side| player_tether::of(vm, side).map_or(0, |s| s.len());
    assert_eq!(count(server.vm(), Side::Server), 1);
    assert_eq!(count(server.vm(), Side::Client), 0);
    let client = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::None)
        .expect("the client over the server")
        .build::<GameServer>();
    assert_eq!(
        count(client.vm(), Side::Server),
        1,
        "the server's step asks only the server's"
    );
    assert_eq!(
        count(client.vm(), Side::Client),
        1,
        "the prediction asks only the client's"
    );
}
