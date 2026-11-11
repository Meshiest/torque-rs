//! The server's transport registry: the engine's own transports by id, a plugin's own, and the refusals.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_engine::{Engine, EngineBuilder, Plugin, PluginGroup};
use torque_server::net::transport::{MemoryTransport, SharedTransport};
use torque_ts_vm::host::MemFiles;

use crate::plugins::MinimalPlugins;
use crate::server::{GameServer, Listen, ServerStart};
use crate::transports::{
    EngineServerTransports, MemoryServerTransportPlugin, ServerLink, UdpServerTransportPlugin,
    server_transport_ids,
};

const LOCAL: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

fn start(engine: &Engine, listen: Listen) -> Result<GameServer, String> {
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen,
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
}

fn udp_loopback() -> Listen {
    Listen::Udp {
        address: Ipv4Addr::LOCALHOST,
        port: 0,
        public: false,
    }
}

fn engine(group: impl PluginGroup) -> Engine {
    EngineBuilder::new()
        .add_plugins(group)
        .build()
        .expect("the engine builds")
}

/// A plugin's own transport: an in-memory one under the id `test.link`, asked for with a request of its own.
struct LinkRequest(SocketAddr);

#[derive(torque_engine::PluginId)]
#[plugin(id = "test.link-transport")]
struct LinkTransportPlugin;

impl Plugin for LinkTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport("test.link", |listen| {
            let Listen::Custom(custom) = listen else {
                return Err("not a custom listen".to_string());
            };
            let LinkRequest(at) = custom
                .request::<LinkRequest>()
                .ok_or("the link takes a LinkRequest")?;
            let (transport, end) = MemoryTransport::new();
            let transport: SharedTransport = Rc::new(RefCell::new(transport));
            transport
                .borrow_mut()
                .bind(at.ip(), at.port())
                .map_err(|e| format!("{e}"))?;
            Ok(ServerLink {
                transport,
                end: Some(end),
            })
        });
    }
}

#[test]
fn the_group_registers_the_transports_of_the_build_by_id() {
    let mut expected = vec!["udp", "memory"];
    if cfg!(feature = "web") {
        expected.push("web");
    }
    if cfg!(feature = "p2p") {
        expected.extend(["rooms", "relay"]);
    }
    if cfg!(feature = "quic") {
        expected.push("quic");
    }
    assert_eq!(server_transport_ids(&engine(MinimalPlugins)), expected);
}

#[test]
fn a_server_listens_on_udp_and_in_memory_by_default() {
    let engine = engine(MinimalPlugins);
    let udp = start(&engine, udp_loopback()).expect("udp");
    assert!(udp.address().is_some_and(|a| a.port() != 0));
    assert!(udp.memory_end().is_none());
    let memory = start(&engine, Listen::Memory(LOCAL)).expect("memory");
    assert_eq!(memory.address(), Some(LOCAL));
    assert!(memory.memory_end().is_some());
}

#[test]
fn turning_a_transport_off_refuses_its_id_with_the_plugin_to_add() {
    let off = engine(MinimalPlugins.disable::<UdpServerTransportPlugin>());
    let error = start(&off, udp_loopback()).err().expect("no udp");
    assert!(error.contains("\"udp\""), "{error}");
    assert!(error.contains("torque.transport-udp-server"), "{error}");
    assert!(error.contains("registered: memory"), "{error}");
    assert!(start(&off, Listen::Memory(LOCAL)).is_ok());

    let off = engine(MinimalPlugins.disable::<MemoryServerTransportPlugin>());
    let error = start(&off, Listen::Memory(LOCAL)).err().expect("no memory");
    assert!(error.contains("torque.transport-memory-server"), "{error}");
}

#[test]
fn an_id_nobody_registered_is_refused_naming_the_registered_ones() {
    let engine = engine(MinimalPlugins);
    let error = start(&engine, Listen::custom("test.link", LinkRequest(LOCAL)))
        .err()
        .expect("unregistered");
    assert!(error.contains("\"test.link\""), "{error}");
    assert!(
        error.contains("add the plugin that registers it"),
        "{error}"
    );
    assert!(error.contains("udp, memory"), "{error}");
}

#[test]
fn a_plugins_transport_serves_the_listen_that_names_its_id() {
    let engine = EngineBuilder::new()
        .add_plugins((MinimalPlugins, LinkTransportPlugin))
        .build()
        .expect("the engine builds");
    let server = start(&engine, Listen::custom("test.link", LinkRequest(LOCAL))).expect("link");
    assert_eq!(server.address(), Some(LOCAL));
    assert!(server.memory_end().is_some());
    // The request must be the plugin's own type.
    let error = start(&engine, Listen::custom("test.link", 7u8))
        .err()
        .expect("wrong request");
    assert!(error.contains("LinkRequest"), "{error}");
}

#[test]
fn a_second_transport_under_an_id_is_skipped_with_a_warning() {
    let engine = EngineBuilder::new()
        .add_plugins((MinimalPlugins, LinkTransportPlugin))
        .add_plugins(DuplicatePlugin)
        .build()
        .expect("the engine builds");
    let mut server = start(&engine, Listen::custom("test.link", LinkRequest(LOCAL))).expect("link");
    let log = server.drain_log().join("\n");
    assert!(
        log.contains("a second transport named \"test.link\""),
        "{log}"
    );
    // The first one registered serves the id.
    assert!(server.memory_end().is_some());
}

#[derive(torque_engine::PluginId)]
#[plugin(id = "test.duplicate-transport")]
struct DuplicatePlugin;

impl Plugin for DuplicatePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport("test.link", |_| Err("the second one".to_string()));
    }
}

/// A plugin that gives the engine a sign-in provider, which binds tickets to the server's address.
#[derive(torque_engine::PluginId)]
#[plugin(id = "test.sign-in")]
struct SignInPlugin;

impl Plugin for SignInPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .net()
            .auth(torque_net::connect::SharedSecret::new([3; 16], "host"));
    }
}

fn sign_in_engine() -> Engine {
    EngineBuilder::new()
        .add_plugins((MinimalPlugins, SignInPlugin, LinkTransportPlugin))
        .build()
        .expect("the engine builds")
}

fn link_at(address: [u8; 4], port: u16) -> Listen {
    Listen::custom("test.link", LinkRequest(SocketAddr::from((address, port))))
}

fn warned(server: &GameServer) -> Vec<String> {
    server
        .vm()
        .log_texts()
        .into_iter()
        .filter(|line| line.contains("set_public_address"))
        .collect()
}

#[test]
fn a_server_with_sign_in_warns_when_it_cannot_know_the_address_players_join_by() {
    let engine = sign_in_engine();
    // Bound to every interface, and the relay's shape (loopback, port 0): the address players dial is not the transport's.
    for listen in [link_at([0, 0, 0, 0], 28_000), link_at([127, 0, 0, 1], 0)] {
        let server = start(&engine, listen).expect("link");
        let lines = warned(&server);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("refuse every sign-in"), "{}", lines[0]);
    }
    // A specific address with a real port, and the memory transport, are what clients dial: no warning.
    assert!(warned(&start(&engine, link_at([192, 168, 1, 5], 28_000)).expect("link")).is_empty());
    assert!(warned(&start(&engine, udp_loopback()).expect("udp")).is_empty());
    assert!(warned(&start(&engine, Listen::Memory(LOCAL)).expect("memory")).is_empty());
    // An engine with no sign-in has nothing to refuse.
    let plain = EngineBuilder::new()
        .add_plugins((MinimalPlugins, LinkTransportPlugin))
        .build()
        .expect("the engine builds");
    assert!(warned(&start(&plain, link_at([0, 0, 0, 0], 28_000)).expect("link")).is_empty());
}

#[test]
fn an_address_that_is_none_or_port_zero_is_not_one_players_join_by() {
    use crate::server::sign_in_address_warning as warn;
    let relay = SocketAddr::from(([127, 0, 0, 1], 0));
    assert!(warn(None, false).is_some());
    assert!(warn(Some(relay), false).is_some());
    assert!(warn(Some(SocketAddr::from(([0, 0, 0, 0], 28_000))), false).is_some());
    assert!(warn(Some(SocketAddr::from(([192, 168, 1, 5], 28_000))), false).is_none());
    assert!(warn(Some(LOCAL), true).is_none());
}

#[test]
fn a_public_bind_of_plain_engine_udp_is_refused() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    crate::plugins::set_protocol(&mut builder, torque_engine::ProtocolKind::Engine);
    let engine = builder.build().expect("the engine builds");
    for address in [Ipv4Addr::UNSPECIFIED, Ipv4Addr::new(203, 0, 113, 7)] {
        let listen = Listen::Udp {
            address,
            port: 0,
            public: true,
        };
        let refused = start(&engine, listen)
            .err()
            .expect("a public plain bind is refused");
        assert!(
            refused.contains("private addresses only"),
            "{address}: {refused}"
        );
    }
    // The same engine listens on a private address.
    assert!(start(&engine, udp_loopback()).is_ok());
}
