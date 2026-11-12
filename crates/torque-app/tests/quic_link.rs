//! The QUIC link end to end on real sockets (loopback, port 0): an engine-mode server on `Listen::Quic` and a client over
//! `quic` connect and exchange ghosts; the server's identity pinned, wrong, and trusted on first use then changed; a ticket
//! bound to one connection's channel binding refused on another; a client whose socket moves keeps its connection; a
//! desktop asked to speak WebTransport (the path browsers take) joins a server that takes sessions. In the engine's protocol
//! `Listen::Udp` and a client's `ClientNet::Loopback` socket are the QUIC link, the LAN list still answered on its port; with
//! the QUIC transports turned off, and in Torque's protocol, they are plain UDP.

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
    start_with(engine, files, key, port, false)
}

/// [`start`], taking WebTransport sessions too when `web_transport` says so.
fn start_with(
    engine: &Engine,
    files: &GameFiles,
    key: &Rc<ServerKey>,
    port: u16,
    web_transport: bool,
) -> GameServer {
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
                    web_transport: web_transport.then(torque_net_quic::WebTransportSetup::default),
                    lan_list: false,
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
        web_transport: false,
    }
}

#[test]
fn a_desktop_asked_to_speaks_webtransport_and_gets_its_ghosts_as_over_the_raw_protocol() {
    let files = files();
    let engine = engine(&files, Nothing);
    let key = Rc::new(ServerKey::generate().expect("a key"));
    let mut server = start_with(&engine, &files, &key, 0, true);
    let at = addr_of(&server);
    let mut over_session = target(at, Some(key.identity()));
    over_session.web_transport = true;
    let (mut page, watch) = client(&engine, &files, &over_session);
    assert!(
        run_until(&mut server, &mut page, 600, |s, c| {
            global(s, "$Connected") == "1" && c.ghost_count() >= 1
        }),
        "server {:?}; client {:?}; link {:?}",
        server.drain_log(),
        page.client_log,
        watch.state()
    );
    let state = watch.state();
    assert!(state.connected && state.error.is_none(), "{state:?}");
    assert_eq!(page.global("$Accepted"), "1");
    let status = server.transport_status().join("\n");
    assert!(
        status.contains("webtransport: 1 sessions, 0 failed"),
        "{status}"
    );
    // A raw desktop joins the same server beside it.
    let (mut raw, raw_watch) = client(&engine, &files, &target(at, Some(key.identity())));
    assert!(
        run_until(&mut server, &mut raw, 600, |_, c| c.ghost_count() >= 1),
        "{:?}",
        raw_watch.state()
    );
    let status = server.transport_status().join("\n");
    assert!(status.contains("2 established now"), "{status}");
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

// ---- UDP in the engine's protocol
// -----------------------------------------------------------------------------------

/// A server of `engine` on `Listen::Udp` at the loopback address, any port.
fn start_udp(engine: &Engine, files: &GameFiles) -> GameServer {
    GameServer::start_engine(
        engine,
        files.mem_files(),
        &ServerStart {
            listen: Listen::Udp {
                address: Ipv4Addr::LOCALHOST,
                port: 0,
                public: false,
            },
            globals: &[],
            start: "startIt",
            args: &[],
        },
    )
    .expect("the server starts")
}

/// A client of `engine` on its UDP socket at the loopback address, told to connect to `at`.
fn udp_client(engine: &Engine, files: &GameFiles, at: SocketAddrV4) -> GameClient {
    let mut builder =
        ClientBuilder::from_engine(engine, Some(files.mem_files()), ClientNet::Loopback)
            .expect("the client builds");
    builder.exec_entry().expect("the client's script");
    let mut client: GameClient = builder.build();
    client
        .call("connectTo", &[&at.to_string()])
        .expect("connect");
    client
}

#[test]
fn in_the_engines_protocol_udp_is_the_quic_link_on_both_ends_and_the_lan_list_is_answered_on_its_port()
 {
    let files = files();
    let engine = engine(&files, Nothing);
    let mut server = start_udp(&engine, &files);
    let at = addr_of(&server);
    let status = server.transport_status().join("\n");
    assert!(status.contains("quic:"), "{status}");
    let mut client = udp_client(&engine, &files, at);
    assert!(
        run_until(&mut server, &mut client, 600, |s, c| {
            global(s, "$Connected") == "1" && c.ghost_count() >= 1
        }),
        "server {:?}; client {:?}",
        server.drain_log(),
        client.client_log
    );
    assert_eq!(client.global("$Accepted"), "1");
    let status = server.transport_status().join("\n");
    assert!(status.contains("1 established now"), "{status}");

    // The LAN list's ping, which is not QUIC, is answered on the same port.
    use torque_net::serverquery::{GAME_PING_REQUEST, GAME_PING_RESPONSE, padded_request};
    let ask = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a socket");
    ask.set_nonblocking(true).expect("non-blocking");
    let request = padded_request(GAME_PING_REQUEST, 0, 7, 9, 64);
    ask.send_to(&request, at).expect("sent");
    let mut buf = [0u8; 2048];
    let mut answer = None;
    for _ in 0..200 {
        server.tick();
        if let Ok((len, _)) = ask.recv_from(&mut buf) {
            answer = Some(buf[..len].to_vec());
            break;
        }
        std::thread::sleep(TICK);
    }
    let answer = answer.expect("the ping was answered");
    assert_eq!(answer.first(), Some(&GAME_PING_RESPONSE));
}

#[test]
fn with_the_quic_transports_turned_off_udp_in_the_engines_protocol_is_plain_on_both_ends() {
    let files = files();
    let config = GameConfig::load(&files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    set_protocol(&mut builder, ProtocolKind::Engine);
    builder
        .disable_id("torque.transport-quic-server")
        .disable_id("torque.transport-quic-client");
    let engine = builder.build().expect("the engine builds");
    let mut server = start_udp(&engine, &files);
    let at = addr_of(&server);
    let status = server.transport_status().join("\n");
    assert!(!status.contains("quic:"), "{status}");
    let mut client = udp_client(&engine, &files, at);
    assert!(
        run_until(&mut server, &mut client, 600, |s, c| {
            global(s, "$Connected") == "1" && c.ghost_count() >= 1
        }),
        "server {:?}; client {:?}",
        server.drain_log(),
        client.client_log
    );
}

#[test]
fn in_torques_protocol_udp_stays_plain_udp() {
    let files = files();
    let config = GameConfig::load(&files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    set_protocol(&mut builder, ProtocolKind::Tge);
    let engine = builder.build().expect("the engine builds");
    let mut server = start_udp(&engine, &files);
    let status = server.transport_status().join("\n");
    assert!(!status.contains("quic:"), "{status}");
}

#[test]
fn a_client_joins_a_loopback_server_hosted_again_on_the_same_port_in_one_run() {
    let files = files();
    let engine = engine(&files, Nothing);
    let mut server = start_udp(&engine, &files);
    let at = addr_of(&server);
    let mut client = udp_client(&engine, &files, at);
    assert!(
        run_until(&mut server, &mut client, 600, |s, c| {
            global(s, "$Connected") == "1" && c.global("$Accepted") == "1"
        }),
        "first: server {:?}; client {:?}",
        server.drain_log(),
        client.client_log
    );
    client.call("leave", &[]).expect("leave");
    server.close();
    drop(server);
    // The same port, a new server with a new key (a loopback server's key lasts as long as the server).
    let mut server = GameServer::start_engine(
        &engine,
        files.mem_files(),
        &ServerStart {
            listen: Listen::Udp {
                address: Ipv4Addr::LOCALHOST,
                port: at.port(),
                public: false,
            },
            globals: &[],
            start: "startIt",
            args: &[],
        },
    )
    .expect("the server starts again");
    client
        .call("connectTo", &[&at.to_string()])
        .expect("connect again");
    assert!(
        run_until(&mut server, &mut client, 600, |s, c| {
            global(s, "$Connected") == "1" && c.global("$Accepted") == "2"
        }),
        "second: server {:?}; client {:?}",
        server.drain_log(),
        client.client_log
    );
}

/// The count a status line gives before `what` (`"3 accepted"` -> 3).
fn count_before(status: &str, what: &str) -> u64 {
    let at = status.find(what).expect("the count is in the status");
    status[..at]
        .split_whitespace()
        .last()
        .and_then(|n| n.parse().ok())
        .expect("a number")
}

#[test]
fn a_loopback_server_whose_key_changes_within_a_session_is_refused_on_the_redial() {
    let files = files();
    let engine = engine(&files, Nothing);
    let mut server = start_udp(&engine, &files);
    let at = addr_of(&server);
    let mut client = udp_client(&engine, &files, at);
    assert!(
        run_until(&mut server, &mut client, 600, |s, c| {
            global(s, "$Connected") == "1" && c.global("$Accepted") == "1"
        }),
        "first: server {:?}; client {:?}",
        server.drain_log(),
        client.client_log
    );
    // Another server takes the same port number with another key while the client is still in its session: no new connect.
    server.close();
    drop(server);
    let mut other = GameServer::start_engine(
        &engine,
        files.mem_files(),
        &ServerStart {
            listen: Listen::Udp {
                address: Ipv4Addr::LOCALHOST,
                port: at.port(),
                public: false,
            },
            globals: &[],
            start: "startIt",
            args: &[],
        },
    )
    .expect("another server starts");
    // The client's session resends and redials on its own; past the dial, a while longer for a handshake to finish.
    let mut dialed = None;
    for tick in 0..400 {
        other.tick();
        client.step(32);
        std::thread::sleep(TICK);
        let status = other.transport_status().join("\n");
        if dialed.is_none() && count_before(&status, " accepted") >= 1 {
            dialed = Some(tick);
        }
        if dialed.is_some_and(|d| tick > d + 100) {
            break;
        }
    }
    let status = other.transport_status().join("\n");
    assert!(dialed.is_some(), "the client never dialed again: {status}");
    assert!(status.contains("0 established in all"), "{status}");
    assert_eq!(global(&other, "$Connected"), "");
    assert_eq!(client.global("$Accepted"), "1");
}
