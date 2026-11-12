//! A script language on a script engine a client shares with its server (`ClientBuilder::over_server`): the client's
//! recipes do not change the side the server's set.

use std::net::{Ipv4Addr, SocketAddr};

use torque_app::plugins::DefaultPlugins;
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_app::{ClientBuilder, ClientNet};
use torque_client::conn::tge::TgeConn;
use torque_engine::EngineBuilder;
use torque_plugin_loader::web_lua::{LuaHost, LuaModule, WebLuaPlugin};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::{Vm, VmError};

/// A module whose every file writes the side of its realm into `$Side`.
struct Side;

impl LuaModule for Side {
    fn run_file(&self, host: &mut dyn LuaHost, _path: &str, _source: &[u8]) -> Result<(), VmError> {
        let (_, side) = host.realm();
        host.set_global("$Side", format!("{side:?}").as_bytes());
        Ok(())
    }

    fn call_defined(&self, _: &mut dyn LuaHost, _: u64, _: &[Vec<u8>]) -> Result<Vec<u8>, VmError> {
        Ok(Vec::new())
    }
}

fn side(vm: &mut Vm) -> String {
    vm.exec_file(b"probe.lua", false).expect("the file runs");
    String::from_utf8_lossy(&vm.global("$Side").unwrap_or_default()).into_owned()
}

#[test]
fn a_client_over_its_server_keeps_the_servers_side() {
    // Torque's protocol: a client over its server carries no RPCs in the engine's (`ClientError::SharedRpcLanes`).
    let mut builder = EngineBuilder::new();
    builder.add_plugins((DefaultPlugins, WebLuaPlugin::new(Side)));
    torque_app::plugins::set_protocol(&mut builder, torque_engine::ProtocolKind::Tge);
    let engine = builder.build().expect("the engine builds");
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    files.insert("probe.lua", b"-- side");
    let mut server = GameServer::start_engine(
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
    assert_eq!(side(server.vm_mut()), "Server");
    let mut client = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::None)
        .expect("the client over the server")
        .build::<GameServer>();
    assert_eq!(side(client.vm_mut()), "Server");
    // A client of its own is the client's side.
    let mut own = ClientBuilder::from_engine(&engine, None, ClientNet::None)
        .expect("a client of its own")
        .build::<GameServer>();
    let probe = MemFiles::new();
    probe.insert("probe.lua", b"-- side");
    own.vm_mut().set_file_host(probe);
    assert_eq!(side(own.vm_mut()), "Client");
}
