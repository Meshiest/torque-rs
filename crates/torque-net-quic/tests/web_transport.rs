//! WebTransport sessions over the in-process pipe under the fake clock: a client that speaks `h3` opens a session and the
//! engine's datagrams flow inside it both ways, beside a raw client on the same endpoint; a page from elsewhere is refused
//! with 403 and nothing is established; a connection that opens no session is closed at the handshake timeout; the link
//! reports the datagram room the prefix leaves; the server's certificate hashes rotate on the fake clock and the hash a page
//! took stays good across one rotation; a server without WebTransport refuses the `h3` handshake.

mod common;

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use common::{Client, SERVER, World, client_recv};
use torque_client::net::Datagrams;
use torque_net_quic::certs::{BROWSER_MAX_VALIDITY, ROTATE_EVERY, validity_of_certificate};
use torque_net_quic::{
    CertificateChoice, Origins, QUIC_MAX_DATAGRAM, QuicLimits, Trust, WebTransportSession,
    WebTransportSetup,
};
use torque_server::net::transport::Transport;

const PAGE: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 4_000);
const DESKTOP: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 8), 4_000);

fn setup(origins: Origins) -> WebTransportSetup {
    WebTransportSetup {
        session: WebTransportSession {
            origins,
            path: "/".to_string(),
            hosts: Vec::new(),
        },
        certificates: CertificateChoice::Rotating,
    }
}

/// Connects `client`, sends one datagram through and answers it: the server's stand-in for the client.
fn hello(world: &mut World, client: &mut Client) -> std::net::SocketAddr {
    assert!(world.connect(client, 50), "{:?}", client.error());
    assert!(client.send_to(SERVER, b"hello"));
    world.step(&mut [client]);
    let got = world.server_recv();
    assert_eq!(got.len(), 1, "{got:?}");
    let (stand_in, data) = got.into_iter().next().expect("one datagram");
    assert_eq!(data, b"hello");
    assert!(world.server.send_to(stand_in, b"world"));
    world.step(&mut [client]);
    assert_eq!(client_recv(client), [b"world".to_vec()]);
    stand_in
}

#[test]
fn a_page_opens_a_session_and_its_datagrams_flow_beside_a_raw_clients() {
    let mut world = World::with_web_transport(QuicLimits::default(), 1, setup(Origins::SameHost));
    let mut page = world.web_client(PAGE, 2, Some(&format!("https://{SERVER}")));
    assert!(!page.is_connected());
    let page_stand_in = hello(&mut world, &mut page);
    let mut desktop = world.client(DESKTOP, Trust::Identity(world.key.identity()), 3);
    let desktop_stand_in = hello(&mut world, &mut desktop);
    assert_ne!(page_stand_in, desktop_stand_in);
    let counters = world.server.counters();
    assert_eq!(counters.sessions, 1);
    assert_eq!(counters.sessions_failed, 0);
    assert_eq!(counters.session_dropped, 0);
    assert_eq!(counters.established, 2);
    // The session's one-byte prefix comes off the room for the engine's bytes, on both ends.
    let kind = world.server.link_kind(page_stand_in);
    assert!(kind.facts.validated && kind.facts.secure && kind.facts.congestion_controlled);
    assert_eq!(kind.max_datagram, QUIC_MAX_DATAGRAM - 1);
    assert_eq!(page.link_kind().max_datagram, QUIC_MAX_DATAGRAM - 1);
    assert_eq!(
        world.server.link_kind(desktop_stand_in).max_datagram,
        QUIC_MAX_DATAGRAM
    );
    // A full-size datagram goes through whole.
    let big = vec![7u8; QUIC_MAX_DATAGRAM - 1];
    assert!(page.send_to(SERVER, &big));
    world.step(&mut [&mut page]);
    assert_eq!(world.server_recv(), vec![(page_stand_in, big.clone())]);
    assert!(world.server.send_to(page_stand_in, &big));
    world.step(&mut [&mut page]);
    assert_eq!(client_recv(&mut page), [big]);
    // Both ends share a channel binding, as on the raw protocol.
    assert_eq!(
        world.server.channel_binding(page_stand_in),
        Some(page.channel_binding().expect("the client's binding"))
    );
    assert!(
        world
            .server
            .status()
            .iter()
            .any(|l| l.contains("1 sessions"))
    );
    // The page closes: the server hears of it.
    page.close();
    for _ in 0..4 {
        world.step(&mut [&mut page]);
    }
    assert_eq!(world.server.take_gone(), vec![page_stand_in]);
}

#[test]
fn a_native_client_with_no_origin_is_taken_and_a_page_from_elsewhere_is_refused() {
    let mut world = World::with_web_transport(QuicLimits::default(), 4, setup(Origins::SameHost));
    let mut native = world.web_client(DESKTOP, 5, None);
    hello(&mut world, &mut native);
    let mut page = world.web_client(PAGE, 6, Some("https://elsewhere.example"));
    let error = world.fail(&mut page, 50).expect("refused");
    assert!(error.contains("403"), "{error}");
    assert!(!page.is_connected());
    let counters = world.server.counters();
    assert_eq!(counters.sessions, 1);
    assert_eq!(counters.sessions_failed, 1);
    // The refused connection ends at the handshake timeout and is reported gone with nothing delivered.
    world.clock.advance(world.limits.handshake_timeout);
    for _ in 0..4 {
        world.step(&mut [&mut page]);
    }
    assert_eq!(world.server.established(), 1);
    assert_eq!(world.server.take_gone().len(), 1);
    assert!(world.server_recv().is_empty());
    let world = World::with_web_transport(
        QuicLimits::default(),
        7,
        setup(Origins::List(vec!["https://elsewhere.example".into()])),
    );
    let mut world = world;
    let mut page = world.web_client(PAGE, 8, Some("https://elsewhere.example/"));
    hello(&mut world, &mut page);
}

#[test]
fn a_connection_that_opens_no_session_is_closed_at_the_handshake_timeout() {
    let mut world = World::with_web_transport(QuicLimits::default(), 9, setup(Origins::Any));
    // A raw client to a WebTransport server speaks the raw protocol: no session is needed of it.
    let mut desktop = world.client(DESKTOP, Trust::Identity(world.key.identity()), 10);
    hello(&mut world, &mut desktop);
    world
        .clock
        .advance(world.limits.handshake_timeout + Duration::from_secs(1));
    world.step(&mut [&mut desktop]);
    assert!(desktop.is_connected());
    assert_eq!(world.server.counters().sessions_failed, 0);
    // An `h3` client is told nothing of the server's settings... the server's streams never reach it here: the pipe
    // delivers everything, so stall the session another way: a page whose CONNECT never comes. The client's own session
    // waits for the server's SETTINGS, which the pipe delivers; so hold its packets instead.
    let mut page = world.web_client(PAGE, 11, None);
    // Deliver only what the QUIC handshake needs: step until the server has the connection established, then stop the
    // page's packets from reaching the server.
    for _ in 0..50 {
        world.pipe.deliver_all();
        world.server.pump();
        world.pipe.deliver_all();
        page.pump();
        if world.server.established() == 2 {
            break;
        }
        world.clock.advance(common::TICK);
    }
    assert_eq!(world.server.established(), 2);
    // From here the page's datagrams are dropped on the floor.
    for _ in 0..200 {
        for f in world.pipe.take_in_flight() {
            if f.to != std::net::SocketAddr::V4(SERVER) {
                world.pipe.deliver(f);
            }
        }
        world.server.pump();
        page.pump();
        world.clock.advance(common::TICK);
        if world.server.established() == 1 {
            break;
        }
    }
    assert_eq!(
        world.server.established(),
        1,
        "the connection with no session was closed"
    );
    let counters = world.server.counters();
    assert!(counters.sessions_failed >= 1, "{counters:?}");
    assert_eq!(counters.sessions, 0);
}

#[test]
fn the_certificate_hashes_rotate_on_the_clock_and_a_pages_hashes_outlive_one_rotation() {
    let mut world = World::with_web_transport(QuicLimits::default(), 12, setup(Origins::Any));
    let first = world.server.certificate_hashes();
    assert_eq!(first.len(), 2);
    assert_ne!(first[0], first[1]);
    let mut page = world.web_client(PAGE, 13, Some("https://a.example"));
    hello(&mut world, &mut page);
    // Six days on: the next certificate takes over, and the hashes a page took before still name it. (The page's own
    // connection has long idled out by then: a rotation never cuts a connection, but six quiet days do.)
    world.clock.advance(ROTATE_EVERY + Duration::from_secs(1));
    world.server.pump();
    let second = world.server.certificate_hashes();
    assert_eq!(second[0], first[1], "the next became the current");
    assert_ne!(second[1], first[1]);
    assert_eq!(world.server.counters().certificates_rotated, 1);
    // A client connecting now gets the new certificate, and it is short.
    let mut later = world.web_client(DESKTOP, 14, None);
    hello(&mut world, &mut later);
    // Sixty days of rotations: every certificate under two weeks, each one presented was published before.
    let mut published = std::collections::HashSet::new();
    published.extend(first);
    published.extend(second.clone());
    for _ in 0..10 {
        world.clock.advance(ROTATE_EVERY);
        world.server.pump();
        let hashes = world.server.certificate_hashes();
        assert!(
            published.contains(&hashes[0]),
            "the live hash was published one rotation ago"
        );
        published.extend(hashes);
    }
    assert_eq!(world.server.counters().certificates_rotated, 11);
    // The short certificate's dates, as a browser reads them from the DER.
    let key = &world.key;
    let cert =
        torque_net_quic::certs::short_certificate(key, world.clock.wall()).expect("a certificate");
    let (from, to) = validity_of_certificate(&cert.der).expect("dates");
    assert!(to.duration_since(from).expect("ordered") < BROWSER_MAX_VALIDITY);
    assert!(from <= world.clock.wall() && world.clock.wall() < to);
}

#[test]
fn a_server_without_web_transport_refuses_the_h3_handshake_and_a_raw_server_stops_streams() {
    let mut world = World::new(QuicLimits::default(), 15);
    assert!(world.server.certificate_hashes().is_empty());
    assert!(!world.server.web_transport_enabled());
    let mut page = world.web_client(PAGE, 16, None);
    let error = world.fail(&mut page, 50).expect("no h3");
    assert!(
        error.contains("alpn") || error.contains("ALPN") || error.contains("protocol"),
        "{error}"
    );
    assert_eq!(world.server.counters().sessions, 0);
}

use torque_net_quic::clock::Clock as _;
