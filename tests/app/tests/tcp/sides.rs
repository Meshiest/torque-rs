//! The plugin in a real server and a real client: both have the classes, and their own ticks service the sockets.

#[path = "../../../../crates/torque-plugin-tcp/tests/common/mod.rs"]
mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::thread;
use std::time::{Duration, Instant};

use common::{answer, listener};
use torque_app::plugins::DefaultPlugins;
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_app::{ClientBuilder, ClientNet};
use torque_engine::EngineBuilder;
use torque_plugin_tcp::TcpPlugin;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

/// A peer that answers one connection with `line`.
fn peer(line: &'static [u8]) -> u16 {
    let (listen, port) = listener();
    thread::spawn(move || {
        if let Ok((stream, _)) = listen.accept() {
            answer(stream, line);
        }
    });
    port
}

const SCRIPT: &str = r#"
    function Probe::onLine(%this, %line) { $got = %line; }
    new TCPObject(Probe);
"#;

fn got(vm: &Vm) -> String {
    vm.global("$got")
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default()
}

#[test]
fn a_server_and_a_client_each_service_their_own_sockets_on_their_ticks() {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, TcpPlugin::default()))
        .build()
        .expect("the engine builds");
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
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
    let mut client = ClientBuilder::from_engine(&engine, None, ClientNet::None)
        .expect("the client")
        .build::<GameServer>();

    let to_server = peer(b"for the server\n");
    let to_client = peer(b"for the client\n");
    server
        .vm_mut()
        .eval(SCRIPT.as_bytes())
        .expect("the server's script");
    client
        .vm_mut()
        .eval(SCRIPT.as_bytes())
        .expect("the client's script");
    server
        .vm_mut()
        .eval(format!(r#"Probe.connect("127.0.0.1:{to_server}");"#).as_bytes())
        .expect("the server connects");
    client
        .vm_mut()
        .eval(format!(r#"Probe.connect("127.0.0.1:{to_client}");"#).as_bytes())
        .expect("the client connects");

    let start = Instant::now();
    while (got(server.vm()).is_empty() || got(client.vm()).is_empty())
        && start.elapsed() < common::WAIT
    {
        server.tick();
        client.step(32);
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(got(server.vm()), "for the server");
    assert_eq!(got(client.vm()), "for the client");
}

#[test]
fn a_client_over_its_server_shares_the_servers_classes_and_sockets() {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, TcpPlugin::default()))
        .build()
        .expect("the engine builds");
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
    let mut client = ClientBuilder::<torque_client::conn::tge::TgeConn>::over_server(
        &engine,
        server,
        ClientNet::None,
    )
    .expect("the client over the server")
    .build::<GameServer>();
    let log = client.host.server.drain_log().join("\n");
    assert!(!log.contains("TCPObject"), "{log}");

    let port = peer(b"for the shared engine\n");
    client.vm_mut().eval(SCRIPT.as_bytes()).expect("the script");
    client
        .vm_mut()
        .eval(format!(r#"Probe.connect("127.0.0.1:{port}");"#).as_bytes())
        .expect("it connects");
    let start = Instant::now();
    while got(client.vm()).is_empty() && start.elapsed() < common::WAIT {
        client.step(32);
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(got(client.vm()), "for the shared engine");
}

#[test]
fn a_client_over_its_server_polls_the_one_host_once_a_tick() {
    let limits = torque_plugin_tcp::TcpLimits {
        callbacks_per_tick: 1,
        ..Default::default()
    };
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, TcpPlugin::default().with_limits(limits)))
        .build()
        .expect("the engine builds");
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
    let mut client = ClientBuilder::<torque_client::conn::tge::TgeConn>::over_server(
        &engine,
        server,
        ClientNet::None,
    )
    .expect("the client over the server")
    .build::<GameServer>();
    let port = peer(b"1\n2\n3\n4\n5\n6\n7\n8\n");
    client
        .vm_mut()
        .eval(
            format!(
                "function Counter::onLine(%this, %line) {{ $lines++; }}\n\
                 new TCPObject(Counter); Counter.connect(\"127.0.0.1:{port}\");"
            )
            .as_bytes(),
        )
        .expect("it connects");
    let lines =
        |client: &torque_app::GameClient<GameServer, _, GameServer>| client.number("$lines") as u32;
    let start = Instant::now();
    while lines(&client) == 0 && start.elapsed() < common::WAIT {
        client.step(32);
        thread::sleep(Duration::from_millis(2));
    }
    let first = lines(&client);
    assert!(first >= 1, "no line came");
    // One callback a tick: the server's tick hook polls, the client's does not poll it again.
    for _ in 0..3 {
        client.step(32);
    }
    assert_eq!(lines(&client), first + 3);
}
