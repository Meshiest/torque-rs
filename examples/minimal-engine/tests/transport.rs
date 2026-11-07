//! A transport a plugin registers carries a whole session: the example server listens on it and the client connects over it,
//! by its id, with no socket opened. And turning a transport plugin off makes listening and connecting on its id fail with
//! the reason.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque::app::client::{ClientBuilder, ClientNet, GameClient};
use torque::app::plugins::DefaultPlugins;
use torque::app::server::Listen;
use torque::app::transports::{
    ClientLink, EngineClientTransports, EngineServerTransports, ServerLink,
    UdpClientTransportPlugin, UdpServerTransportPlugin,
};
use torque::client::net::{Datagrams, NetPolicy};
use torque::engine::{EngineBuilder, Plugin, PluginGroup, PluginId};
use torque::server::net::transport::{BindError, RECV_BUFFER_BYTES, Recv, Transport};
use torque_example_minimal_engine::{ExamplePlugin, client, files, ghosts, server};

const SERVER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);

/// The two queues between the client's end and the server's.
#[derive(Default)]
struct Wire {
    to_server: VecDeque<Vec<u8>>,
    to_client: VecDeque<Vec<u8>>,
    up: usize,
    down: usize,
}

type Shared = Rc<RefCell<Wire>>;

struct ServerEnd {
    wire: Shared,
    bound: Option<SocketAddr>,
}

impl Transport for ServerEnd {
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        self.bound = Some(SocketAddr::new(ip, port));
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        assert!(buf.len() >= RECV_BUFFER_BYTES);
        let Some(data) = self.wire.borrow_mut().to_server.pop_front() else {
            return Recv::Empty;
        };
        buf[..data.len()].copy_from_slice(&data);
        Recv::Datagram {
            len: data.len(),
            from: SocketAddr::V4(CLIENT),
        }
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        assert_eq!(to, SocketAddr::V4(CLIENT));
        let mut wire = self.wire.borrow_mut();
        wire.down += 1;
        wire.to_client.push_back(data.to_vec());
        true
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.bound
    }

    fn is_real(&self) -> bool {
        false
    }
}

struct ClientEnd(Shared);

impl Datagrams for ClientEnd {
    fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
        assert_eq!(to, SERVER);
        let mut wire = self.0.borrow_mut();
        wire.up += 1;
        wire.to_server.push_back(bytes.to_vec());
        true
    }

    fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
        self.0
            .borrow_mut()
            .to_client
            .pop_front()
            .map(|d| (SERVER, d))
    }

    fn port(&self) -> u16 {
        CLIENT.port()
    }
}

/// What a client names to connect over the channel (the channel has one server).
struct ChannelTarget;

/// A transport of its own, `test.channel`: both ends are queues in this process.
#[derive(PluginId)]
#[plugin(crate = torque::engine, id = "test.channel-transport")]
struct ChannelTransportPlugin(Shared);

impl Plugin for ChannelTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let wire = Rc::clone(&self.0);
        engine.add_server_transport("test.channel", move |_| {
            let mut end = ServerEnd {
                wire: Rc::clone(&wire),
                bound: None,
            };
            end.bind(IpAddr::V4(*SERVER.ip()), SERVER.port())
                .map_err(|e| e.to_string())?;
            Ok(ServerLink::new(Rc::new(RefCell::new(end))))
        });
        let wire = Rc::clone(&self.0);
        engine.add_client_transport("test.channel", move |_: &ChannelTarget| {
            Ok(ClientLink::new(
                Box::new(ClientEnd(Rc::clone(&wire))),
                NetPolicy::Loopback,
            ))
        });
    }
}

fn engine_with(wire: &Shared) -> EngineBuilder {
    let mut engine = EngineBuilder::new();
    engine.add_plugins((
        DefaultPlugins,
        ExamplePlugin(files().unwrap()),
        ChannelTransportPlugin(Rc::clone(wire)),
    ));
    engine
}

#[test]
fn a_session_runs_over_a_transport_a_plugin_registered() {
    let wire = Shared::default();
    let engine = engine_with(&wire).build().unwrap();
    let mut server = server(&engine, Listen::custom("test.channel", ())).expect("the server");
    assert_eq!(server.address(), Some(SocketAddr::V4(SERVER)));

    let game = engine.resource::<torque::app::files::GameFiles>().unwrap();
    let mut builder =
        ClientBuilder::from_engine(&engine, Some(game.mem_files()), ClientNet::External)
            .expect("the client");
    builder
        .connect("test.channel", &ChannelTarget)
        .expect("the client connects over the channel");
    builder.exec("client.cs").unwrap();
    let mut client: GameClient = builder.build();
    client.call("connectTo", &[&SERVER.to_string()]).unwrap();

    for _ in 0..600 {
        server.tick();
        client.step(32);
        if client.ghost_count() == 3 {
            break;
        }
    }
    let seen = ghosts(&client);
    assert_eq!(client.ghost_count(), 3, "{seen:?}\n{:?}", client.client_log);
    for class in ["StaticShape", "Item", "Camera"] {
        assert_eq!(
            seen.iter().filter(|(c, _)| *c == class).count(),
            1,
            "{class}"
        );
    }
    // The session went over the channel alone: datagrams both ways, no socket of the client's.
    let (up, down) = (wire.borrow().up, wire.borrow().down);
    assert!(up > 3 && down > 3, "up {up}, down {down}");
    assert_eq!(
        client.host.client.borrow().net.borrow().port(),
        CLIENT.port()
    );
}

#[test]
fn turning_a_transport_plugin_off_fails_listen_and_connect_on_its_id() {
    let off = EngineBuilder::new()
        .add_plugins((
            DefaultPlugins
                .disable::<UdpServerTransportPlugin>()
                .disable::<UdpClientTransportPlugin>(),
            ExamplePlugin(files().unwrap()),
        ))
        .build()
        .unwrap();
    let listen = Listen::Udp {
        address: Ipv4Addr::LOCALHOST,
        port: 0,
        public: false,
    };
    let error = server(&off, listen).err().expect("udp is off");
    assert!(error.contains("listen on the transport \"udp\""), "{error}");
    assert!(error.contains("torque.transport-udp-server"), "{error}");
    let error = client(&off, ClientNet::Loopback).err().expect("udp is off");
    assert!(
        error.contains("connect over the transport \"udp\""),
        "{error}"
    );
    assert!(error.contains("torque.transport-udp-client"), "{error}");

    // The custom id is refused the same way when its plugin is not added.
    let plain = EngineBuilder::new()
        .add_plugins((DefaultPlugins, ExamplePlugin(files().unwrap())))
        .build()
        .unwrap();
    let error = server(&plain, Listen::custom("test.channel", ()))
        .err()
        .expect("no channel");
    assert!(error.contains("\"test.channel\""), "{error}");
    assert!(error.contains("registered: udp, memory"), "{error}");
}
