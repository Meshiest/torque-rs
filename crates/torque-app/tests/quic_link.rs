//! The QUIC link end to end on real sockets (loopback, port 0): an engine-mode server on `Listen::Quic` and a client over
//! `quic` connect and exchange ghosts; the server's identity pinned, wrong, and trusted on first use then changed; a ticket
//! bound to one connection's channel binding refused on another; a client whose socket moves keeps its connection.

#![cfg(feature = "quic")]

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::{DefaultPlugins, set_protocol};
use torque_app::server::{GameServer, IdentitySource, Listen, QuicListen, ServerStart};
use torque_app::transports::QuicTarget;
use torque_client::net::Datagrams;
use torque_engine::{Engine, EngineBuilder, Plugin, ProtocolKind};
use torque_net::connect::{AuthProvider, ConnectBlock, SharedSecret};
use torque_net::engine::bridge::{connect_of, ticket_context};
use torque_net::engine::{
    ChannelBinding, CloseReason, Event, Link, LinkFacts, NoRoom, NoStreams, Session, SessionConfig,
};
use torque_net::profile::HandshakeProfile;
use torque_net_quic::{
    QuicDatagrams, QuicLimits, QuicWatch, ServerIdentity, ServerKey, SystemClock, Trust, UdpIo,
};

const SECRET: [u8; 16] = [5; 16];
const TICK: Duration = Duration::from_millis(2);

fn files() -> GameFiles {
    let mut files = GameFiles::default();
    files.insert("server.cs", include_bytes!("quic_link/server.cs"));
    files.insert("main.cs", include_bytes!("quic_link/main.cs"));
    files.insert("game.toml", include_bytes!("quic_link/game.toml"));
    files
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "torque-app-quic-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ))
}

/// An engine-mode engine of the default plugins (and `extra`), over the test game's config.
fn engine<M>(files: &GameFiles, extra: impl torque_engine::Plugins<M>) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins(DefaultPlugins)
        .config(&config)
        .add_plugins(extra);
    set_protocol(&mut builder, ProtocolKind::Engine);
    builder.build().expect("the engine builds")
}

#[derive(torque_engine::PluginId)]
#[plugin(id = "test.nothing")]
struct Nothing;

impl Plugin for Nothing {
    fn build(&self, _engine: &mut EngineBuilder) {}
}

/// Tickets bound to the connection by the shared secret.
#[derive(torque_engine::PluginId)]
#[plugin(id = "test.ticket-auth")]
struct TicketAuth;

impl Plugin for TicketAuth {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().auth(SharedSecret::new(SECRET, "player"));
    }
}

/// A server of `engine` on the QUIC link at `port` (0: any), on `key`.
fn start(engine: &Engine, files: &GameFiles, key: &Rc<ServerKey>, port: u16) -> GameServer {
    GameServer::start_engine(
        engine,
        files.mem_files(),
        &ServerStart {
            listen: Listen::Quic {
                address: Ipv4Addr::LOCALHOST,
                port,
                public: false,
                quic: Rc::new(QuicListen {
                    identity: IdentitySource::Key(Rc::clone(key)),
                    limits: QuicLimits::default(),
                }),
            },
            globals: &[],
            start: "startIt",
            args: &[],
        },
    )
    .expect("the server starts")
}

fn addr_of(server: &GameServer) -> SocketAddrV4 {
    match server.address().expect("bound") {
        SocketAddr::V4(a) => a,
        SocketAddr::V6(_) => panic!("an IPv4 server"),
    }
}

/// A client of `engine` over `quic` to `target`, told to connect; its link's watch.
fn client(engine: &Engine, files: &GameFiles, target: &QuicTarget) -> (GameClient, QuicWatch) {
    let mut builder =
        ClientBuilder::from_engine(engine, Some(files.mem_files()), ClientNet::External)
            .expect("the client builds");
    let info = builder
        .connect("quic", target)
        .expect("the quic link opens");
    let watch = info.take::<QuicWatch>().expect("the link's watch");
    builder.exec_entry().expect("the client's script");
    let mut client: GameClient = builder.build();
    client
        .call(
            "connectTo",
            &[&SocketAddrV4::new(target.address, target.port).to_string()],
        )
        .expect("connect");
    (client, watch)
}

fn global(server: &GameServer, name: &str) -> String {
    server
        .vm()
        .global(name)
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default()
}

/// Ticks `server` and steps `client` until `done` (at most `limit` ticks), on the real clock.
fn run_until(
    server: &mut GameServer,
    client: &mut GameClient,
    limit: usize,
    mut done: impl FnMut(&GameServer, &GameClient) -> bool,
) -> bool {
    for _ in 0..limit {
        if done(server, client) {
            return true;
        }
        server.tick();
        client.step(32);
        std::thread::sleep(TICK);
    }
    done(server, client)
}

fn target(at: SocketAddrV4, identity: Option<ServerIdentity>) -> QuicTarget {
    QuicTarget {
        address: *at.ip(),
        port: at.port(),
        identity,
        known_servers: None,
    }
}

#[test]
fn a_client_joins_an_engine_mode_server_over_quic_and_gets_its_ghosts() {
    let files = files();
    let engine = engine(&files, Nothing);
    let key = Rc::new(ServerKey::generate().expect("a key"));
    let mut server = start(&engine, &files, &key, 0);
    let at = addr_of(&server);
    let (mut client, watch) = client(&engine, &files, &target(at, Some(key.identity())));
    assert!(
        run_until(&mut server, &mut client, 600, |s, c| {
            global(s, "$Connected") == "1" && c.ghost_count() >= 1
        }),
        "server {:?}; client {:?}; link {:?}",
        server.drain_log(),
        client.client_log,
        watch.state()
    );
    let state = watch.state();
    assert!(state.connected && state.error.is_none(), "{state:?}");
    assert_eq!(state.identity, Some(key.identity()));
    assert_eq!(client.global("$Accepted"), "1");
    let status = server.transport_status().join("\n");
    assert!(
        status.contains("quic:") && status.contains("1 connections"),
        "{status}"
    );
}

#[test]
fn a_server_whose_identity_is_not_the_one_given_is_refused_with_both_hashes() {
    let files = files();
    let engine = engine(&files, Nothing);
    let key = Rc::new(ServerKey::generate().expect("a key"));
    let mut server = start(&engine, &files, &key, 0);
    let at = addr_of(&server);
    let wanted = ServerIdentity([0x42; 32]);
    let (mut client, watch) = client(&engine, &files, &target(at, Some(wanted)));
    assert!(run_until(&mut server, &mut client, 600, |_, _| {
        watch.state().error.is_some()
    }));
    let error = watch.state().error.expect("refused");
    assert!(error.contains(&wanted.to_string()), "{error}");
    assert!(error.contains(&key.identity().to_string()), "{error}");
    assert!(error.contains("not the identity given"), "{error}");
    assert_eq!(global(&server, "$Connected"), "");
}

#[test]
fn a_server_is_trusted_on_first_use_and_refused_once_its_key_changes() {
    let dir = scratch("known");
    let known = dir.join(torque_net_quic::KNOWN_SERVERS_FILE);
    let files = files();
    let engine = engine(&files, Nothing);
    let first = Rc::new(ServerKey::generate().expect("a key"));
    let mut server = start(&engine, &files, &first, 0);
    let at = addr_of(&server);
    let mut target = target(at, None);
    target.known_servers = Some(known.clone());
    {
        let (mut client, watch) = client(&engine, &files, &target);
        assert!(run_until(&mut server, &mut client, 600, |s, _| {
            global(s, "$Connected") == "1"
        }));
        assert_eq!(watch.state().identity, Some(first.identity()));
        let text = std::fs::read_to_string(&known).expect("the known servers file");
        assert_eq!(text.trim(), format!("{at} {}", first.identity()));
    }
    server.close();
    drop(server);
    // Another key at the same address.
    let second = Rc::new(ServerKey::generate().expect("another key"));
    let mut server = start(&engine, &files, &second, at.port());
    let (mut client, watch) = client(&engine, &files, &target);
    assert!(run_until(&mut server, &mut client, 600, |_, _| {
        watch.state().error.is_some()
    }));
    let error = watch.state().error.expect("refused");
    assert!(error.contains("not the identity remembered"), "{error}");
    assert!(error.contains(&first.identity().to_string()), "{error}");
    assert!(error.contains(&second.identity().to_string()), "{error}");
    assert_eq!(global(&server, "$Connected"), "");
    std::fs::remove_dir_all(&dir).expect("removed");
}

// ---- raw sessions over the link: tickets and migration
// -----------------------------------------------------------------------------------

/// An engine session's link over a [`QuicDatagrams`].
struct QuicLink<'a> {
    socket: &'a mut QuicDatagrams,
    server: SocketAddrV4,
}

impl Link for QuicLink<'_> {
    fn max_datagram(&self) -> usize {
        self.socket.link_kind().max_datagram
    }

    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        if self.socket.send_to(self.server, datagram) {
            Ok(())
        } else {
            Err(NoRoom)
        }
    }

    fn facts(&self) -> LinkFacts {
        self.socket.link_kind().facts
    }
}

/// A QUIC socket connected to `server`, pinned to `identity`, with its handshake done against the ticking `host`.
fn quic_socket(
    host: &mut GameServer,
    server: SocketAddrV4,
    identity: ServerIdentity,
) -> QuicDatagrams {
    let io = UdpIo::open(IpAddr::V4(Ipv4Addr::LOCALHOST), 0).expect("a socket");
    let mut socket = QuicDatagrams::connect(
        io,
        SystemClock,
        server,
        Trust::Identity(identity),
        &QuicLimits::default(),
        None,
    )
    .expect("a connection");
    for _ in 0..600 {
        if socket.is_connected() {
            return socket;
        }
        host.tick();
        socket.pump();
        std::thread::sleep(TICK);
    }
    panic!("the QUIC handshake never finished: {:?}", socket.error());
}

/// What a raw client's connect ended with: accepted (none), or the close.
fn connect_raw(
    host: &mut GameServer,
    engine: &Engine,
    socket: &mut QuicDatagrams,
    server: SocketAddrV4,
    answer: &dyn Fn(u64, Option<ChannelBinding>) -> Vec<u8>,
) -> Option<CloseReason> {
    let profile = HandshakeProfile::default();
    let block = ConnectBlock {
        manifest: Some(engine.net_manifest(&profile)),
        ..Default::default()
    };
    let connect = connect_of(&profile, Some(&block), b"", &[]).expect("a connect");
    let game = String::from_utf8_lossy(&profile.game).into_owned();
    let facts = socket.link_kind().facts;
    let mut session =
        Session::client(SessionConfig::new(&game), facts, connect, 7, 0).expect("a client");
    for tick in 0..600u32 {
        let now = tick * 32;
        session.poll(&mut QuicLink { socket, server }, &mut NoStreams, now);
        host.tick();
        std::thread::sleep(TICK);
        while let Some((_, d)) = socket.recv() {
            session.receive(&d, now);
        }
        while let Some(event) = session.next_event() {
            match event {
                Event::Challenge(n) => session
                    .answer(answer(n, socket.channel_binding()))
                    .expect("an answer"),
                Event::Accepted(_) => return None,
                Event::Closed(why) => return Some(why),
                _ => {}
            }
        }
    }
    panic!("the connect neither ended nor was accepted");
}

fn refused(why: Option<CloseReason>) -> String {
    match why {
        Some(CloseReason::Rejected { text, .. }) => text,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_ticket_is_bound_to_its_connections_channel_binding() {
    let files = files();
    let engine = engine(&files, TicketAuth);
    let key_a = Rc::new(ServerKey::generate().expect("a key"));
    let key_b = Rc::new(ServerKey::generate().expect("another key"));
    let mut a = start(&engine, &files, &key_a, 0);
    let mut b = start(&engine, &files, &key_b, 0);
    let (at_a, at_b) = (addr_of(&a), addr_of(&b));
    let secret = SharedSecret::new(SECRET, "player");
    let captured = RefCell::new(Vec::new());

    // An honest ticket for A's address, challenge, identity and this connection's binding.
    let mut socket = quic_socket(&mut a, at_a, key_a.identity());
    let binding_a = socket.channel_binding().expect("the binding");
    let honest = |n: u64, binding: Option<ChannelBinding>| {
        let ticket = secret
            .ticket(&ticket_context(
                SocketAddr::V4(at_a),
                n,
                Some(key_a.identity()),
                binding,
            ))
            .expect("a ticket");
        captured.borrow_mut().clone_from(&ticket);
        ticket
    };
    assert_eq!(
        connect_raw(&mut a, &engine, &mut socket, at_a, &honest),
        None
    );
    let stolen = captured.borrow().clone();
    assert!(!stolen.is_empty(), "a ticket was made");

    // The same ticket relayed to B fails, as does one made for B in every way but the binding, which is A's.
    let mut socket_b = quic_socket(&mut b, at_b, key_b.identity());
    assert_ne!(socket_b.channel_binding(), Some(binding_a));
    let relayed = |_: u64, _: Option<ChannelBinding>| stolen.clone();
    let why = refused(connect_raw(&mut b, &engine, &mut socket_b, at_b, &relayed));
    assert!(!why.is_empty(), "{why}");
    let mut socket_b = quic_socket(&mut b, at_b, key_b.identity());
    let wrong_binding = |n: u64, _: Option<ChannelBinding>| {
        secret
            .ticket(&ticket_context(
                SocketAddr::V4(at_b),
                n,
                Some(key_b.identity()),
                Some(binding_a),
            ))
            .expect("a ticket")
    };
    refused(connect_raw(
        &mut b,
        &engine,
        &mut socket_b,
        at_b,
        &wrong_binding,
    ));
    // A ticket made without any binding fails too: the server's context has one.
    let mut socket_b = quic_socket(&mut b, at_b, key_b.identity());
    let unbound = |n: u64, _: Option<ChannelBinding>| {
        secret
            .ticket(&ticket_context(
                SocketAddr::V4(at_b),
                n,
                Some(key_b.identity()),
                None,
            ))
            .expect("a ticket")
    };
    refused(connect_raw(&mut b, &engine, &mut socket_b, at_b, &unbound));
    // An honest ticket for B is accepted: the binding read from the socket is what the server derived.
    let mut socket_b = quic_socket(&mut b, at_b, key_b.identity());
    let honest_b = |n: u64, binding: Option<ChannelBinding>| {
        secret
            .ticket(&ticket_context(
                SocketAddr::V4(at_b),
                n,
                Some(key_b.identity()),
                binding,
            ))
            .expect("a ticket")
    };
    assert_eq!(
        connect_raw(&mut b, &engine, &mut socket_b, at_b, &honest_b),
        None
    );
}

#[test]
fn a_client_whose_socket_moves_keeps_its_connection() {
    let files = files();
    let engine = engine(&files, Nothing);
    let key = Rc::new(ServerKey::generate().expect("a key"));
    let mut server = start(&engine, &files, &key, 0);
    let at = addr_of(&server);
    let mut socket = quic_socket(&mut server, at, key.identity());
    let no_ticket = |_: u64, _: Option<ChannelBinding>| Vec::new();
    assert_eq!(
        connect_raw(&mut server, &engine, &mut socket, at, &no_ticket),
        None
    );
    let before = server.transport_status().join("\n");
    assert!(before.contains("0 migrations"), "{before}");
    let binding = socket.channel_binding();
    // A new socket, so a new address: the connection moves with it.
    socket.rebind(UdpIo::open(IpAddr::V4(Ipv4Addr::LOCALHOST), 0).expect("a socket"));
    for _ in 0..40 {
        assert!(socket.send_to(at, b"\x00still here"));
        server.tick();
        socket.pump();
        std::thread::sleep(TICK);
    }
    assert!(socket.is_connected(), "{:?}", socket.error());
    assert_eq!(socket.channel_binding(), binding, "the same connection");
    let after = server.transport_status().join("\n");
    assert!(after.contains("1 migrations (0 refused)"), "{after}");
    assert!(after.contains("1 connections"), "{after}");
}
