//! The client's transport registry beside the server's: the engine's own transports by id, what turning one off does to a
//! listen and to a connect, and a client's own transport.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_client::net::{MemoryDatagrams, NetPolicy};
use torque_engine::{Engine, EngineBuilder, Plugin, PluginGroup};
use torque_ts_vm::host::MemFiles;

use crate::client::{ClientBuilder, ClientError, ClientNet};
use crate::plugins::DefaultPlugins;
use crate::server::{GameServer, Listen, ServerStart};
use crate::transports::{
    ClientLink, EngineClientTransports, MemoryClientTransportPlugin, MemoryServerTransportPlugin,
    UdpClientTransportPlugin, UdpServerTransportPlugin, client_transport_ids,
};
use torque_app_server::transports::server_transport_ids;

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);

fn engine(group: impl PluginGroup) -> Engine {
    EngineBuilder::new()
        .add_plugins(group)
        .build()
        .expect("the engine builds")
}

/// [`engine`] in Torque's protocol, whose UDP is the `udp` transports' on both sides (the engine's is the QUIC link's where
/// the build has it).
fn tge_engine(group: impl PluginGroup) -> Engine {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(group);
    crate::plugins::set_protocol(&mut builder, torque_engine::ProtocolKind::Tge);
    builder.build().expect("the engine builds")
}

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

fn builder(engine: &Engine, net: ClientNet) -> Result<ClientBuilder, ClientError> {
    ClientBuilder::from_engine(engine, None, net)
}

/// The transport's message of a client whose socket did not open.
fn socket_error(made: Result<ClientBuilder, ClientError>) -> String {
    match made {
        Err(ClientError::Socket(message)) => message,
        Err(other) => panic!("not a socket error: {other}"),
        Ok(_) => panic!("the client's socket opened"),
    }
}

#[test]
fn the_default_plugins_register_the_transports_of_the_build_on_both_sides() {
    let engine = engine(DefaultPlugins);
    let mut server = vec!["udp", "memory"];
    let mut client = vec!["udp", "memory"];
    if cfg!(feature = "web") {
        server.push("web");
        client.push("web");
    }
    if cfg!(feature = "p2p") {
        server.extend(["rooms", "relay"]);
    }
    if cfg!(feature = "quic") {
        server.push("quic");
        client.extend(["quic", "quic-udp"]);
    }
    assert_eq!(server_transport_ids(&engine), server);
    assert_eq!(client_transport_ids(&engine), client);
}

#[test]
fn the_default_transports_connect_a_client_to_a_server_in_memory_and_over_udp() {
    let engine = engine(DefaultPlugins);
    let memory = start(&engine, Listen::Memory(SERVER)).expect("the memory server");
    assert!(memory.memory_end().is_some());
    let udp = start(
        &engine,
        Listen::Udp {
            address: Ipv4Addr::LOCALHOST,
            port: 0,
            public: false,
        },
    )
    .expect("the udp server");
    assert!(udp.address().is_some());
    builder(&engine, ClientNet::Loopback).expect("the udp client");
    builder(
        &engine,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    )
    .expect("the memory client");
}

#[test]
fn turning_a_transport_plugin_off_refuses_that_id_on_its_side_only() {
    let no_udp_server = tge_engine(DefaultPlugins.disable::<UdpServerTransportPlugin>());
    let listen = Listen::Udp {
        address: Ipv4Addr::LOCALHOST,
        port: 0,
        public: false,
    };
    let error = start(&no_udp_server, listen).err().expect("no udp server");
    assert!(error.contains("listen on the transport \"udp\""), "{error}");
    assert!(error.contains("torque.transport-udp-server"), "{error}");
    // Its client half is still there.
    builder(&no_udp_server, ClientNet::Loopback).expect("the udp client");

    let no_udp_client = tge_engine(DefaultPlugins.disable::<UdpClientTransportPlugin>());
    let error = socket_error(builder(&no_udp_client, ClientNet::Loopback));
    assert!(
        error.contains("connect over the transport \"udp\""),
        "{error}"
    );
    assert!(error.contains("torque.transport-udp-client"), "{error}");
    assert!(error.contains("registered: memory"), "{error}");
    let listen = Listen::Udp {
        address: Ipv4Addr::LOCALHOST,
        port: 0,
        public: false,
    };
    assert!(start(&no_udp_client, listen).is_ok());
}

#[test]
fn a_game_played_alone_needs_the_memory_transport() {
    let engine = engine(
        DefaultPlugins
            .disable::<MemoryClientTransportPlugin>()
            .disable::<MemoryServerTransportPlugin>(),
    );
    let error = start(&engine, Listen::Memory(SERVER))
        .err()
        .expect("no memory");
    assert!(error.contains("torque.transport-memory-server"), "{error}");
    let error = socket_error(builder(
        &engine,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    ));
    assert!(error.contains("torque.transport-memory-client"), "{error}");
}

struct Dummy;

#[derive(torque_engine::PluginId)]
#[plugin(id = "test.dummy-transport")]
struct DummyTransportPlugin;

impl Plugin for DummyTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_client_transport("test.dummy", |_: &Dummy| {
            Ok(ClientLink::new(
                Box::new(MemoryDatagrams::default()),
                NetPolicy::Loopback,
            ))
        });
    }
}

#[test]
fn a_client_connects_over_a_plugins_transport_by_id_with_its_own_target() {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, DummyTransportPlugin))
        .build()
        .expect("the engine builds");
    let mut client = builder(&engine, ClientNet::External).expect("the client");
    assert!(client.connect("test.dummy", &Dummy).is_ok());
    // The target must be the type the transport takes.
    let error = client
        .connect("test.dummy", &7u8)
        .expect_err("wrong target");
    assert!(error.contains("test.dummy"), "{error}");
    assert!(error.contains("Dummy"), "{error}");
    // An id nobody registered.
    let error = client
        .connect("test.none", &Dummy)
        .expect_err("unregistered");
    assert!(error.contains("\"test.none\""), "{error}");
    assert!(
        error.contains("add the plugin that registers it"),
        "{error}"
    );
    assert!(error.contains("test.dummy"), "{error}");
}

#[test]
fn the_hosting_plugin_adds_the_join_natives_on_top_of_the_rooms_transport() {
    use crate::hosting::HostingPlugin;
    let plain = builder(&engine(DefaultPlugins), ClientNet::None).expect("the client");
    assert!(!plain.vm.native_listing().contains("checkJoinAddress"));

    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, HostingPlugin::default()))
        .build()
        .expect("the engine builds");
    assert!(engine.is_plugin_added::<HostingPlugin>());
    let hosted = builder(&engine, ClientNet::None).expect("the client");
    let listing = hosted.vm.native_listing();
    for name in [
        "checkJoinAddress",
        "joinLookupResult",
        "cancelJoinLookup",
        "armPublicJoin",
    ] {
        assert!(listing.contains(name), "{name}");
    }
}

#[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
#[test]
fn hosting_is_turned_off_with_a_warning_when_the_rooms_transport_is() {
    use crate::hosting::HostingPlugin;
    use torque_app_server::transports::RoomsServerTransportPlugin;
    let engine = EngineBuilder::new()
        .add_plugins((
            DefaultPlugins.disable::<RoomsServerTransportPlugin>(),
            HostingPlugin::default(),
        ))
        .build()
        .expect("the engine builds");
    assert!(!engine.is_plugin_added::<HostingPlugin>());
    assert!(
        engine
            .warnings()
            .iter()
            .any(|w| w.to_string().contains("torque.hosting")),
        "{:?}",
        engine.warnings()
    );
}

#[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
#[test]
fn hosting_brings_the_rooms_transport_with_it() {
    use crate::hosting::HostingPlugin;
    let engine = EngineBuilder::new()
        .add_plugins(HostingPlugin::default())
        .build()
        .expect("the engine builds");
    assert_eq!(server_transport_ids(&engine), ["rooms"]);
}

#[cfg(feature = "quic")]
#[test]
fn a_server_is_dialed_again_after_a_doubling_wait_and_at_once_once_connected() {
    use crate::transports::Redial;
    use std::time::{Duration, Instant};
    let a = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_000);
    let b = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);
    let t0 = Instant::now();
    let mut redial = Redial::default();
    assert!(redial.may_dial(a, t0));
    // Every tick before the wait is refused.
    assert!(!redial.may_dial(a, t0 + Duration::from_millis(32)));
    assert!(!redial.may_dial(a, t0 + Redial::FIRST - Duration::from_millis(1)));
    // Another server is dialed at once.
    assert!(redial.may_dial(b, t0 + Duration::from_millis(1)));
    let mut now = t0 + Duration::from_millis(1);
    let mut wait = Redial::FIRST;
    for _ in 0..8 {
        now += wait;
        assert!(redial.may_dial(b, now));
        wait = (wait * 2).min(Redial::MOST);
        assert!(!redial.may_dial(b, now + wait - Duration::from_millis(1)));
    }
    assert_eq!(wait, Redial::MOST);
    // Connected: the next dial goes at once, and the wait starts over.
    redial.connected();
    assert!(redial.may_dial(b, now));
    assert!(!redial.may_dial(b, now + Redial::FIRST - Duration::from_millis(1)));
    assert!(redial.may_dial(b, now + Redial::FIRST));
}
