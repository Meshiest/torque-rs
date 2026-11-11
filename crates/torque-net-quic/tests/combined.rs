//! The QUIC link on the datagram side of the web transport's combined transport (`MultiTransport`), as a server with a page
//! runs them on one port number: a QUIC connection's datagrams come through under its stand-in (the upper half of the reserved
//! range), the host asks the combined transport for the peer's real address and its channel binding and gets the QUIC link's
//! answers (the per-address caps count the real address), a connection that ends is reported gone through it, and both sides
//! are serviced.

mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};

use common::{Rng, TICK};
use torque_client::net::Datagrams;
use torque_net_quic::{FakeClock, Pipe, QuicClient, QuicLimits, QuicServer, ServerKey, Trust};
use torque_server::net::transport::{BindPolicy, LanRules, Recv, Transport};
use torque_server::net::web::{MultiTransport, WebSetup, WebTransport};

const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 4_000);

#[test]
fn a_quic_peer_through_the_combined_transport_keeps_its_address_binding_and_goodbye() {
    let pipe = Pipe::new();
    let clock = FakeClock::new();
    let key = ServerKey::generate().expect("a key");
    let limits = QuicLimits::default();
    let quic = QuicServer::new(
        pipe.end(),
        clock.clone(),
        &key,
        limits.clone(),
        Some(Rng::new(1).seed32()),
    )
    .expect("a server endpoint");
    let policy = BindPolicy::new(IpAddr::V4(Ipv4Addr::LOCALHOST), false, &LanRules::PLAIN)
        .expect("a loopback policy");
    let web = WebTransport::new(policy, WebSetup::default());
    let mut server = MultiTransport::new(quic, web);
    // The pipe hands out a port number; the web side takes the same on TCP.
    server
        .bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
        .expect("both sides bound");
    let at = match server.local_addr().expect("bound") {
        SocketAddr::V4(a) => a,
        SocketAddr::V6(_) => panic!("an IPv4 address"),
    };
    assert_eq!(
        server.web().local_addr().map(|a| a.port()),
        Some(at.port()),
        "one port number"
    );
    assert!(server.wants_service(), "the QUIC side wants servicing");

    let mut client = QuicClient::connect(
        pipe.bound(SocketAddr::V4(CLIENT)),
        clock.clone(),
        at,
        Trust::Identity(key.identity()),
        &limits,
        Some(Rng::new(2).seed32()),
    )
    .expect("a client endpoint");
    let step = |server: &mut MultiTransport<_>, client: &mut QuicClient<_, _>| {
        pipe.deliver_all();
        server.service();
        pipe.deliver_all();
        client.pump();
        pipe.deliver_all();
        server.service();
        clock.advance(TICK);
    };
    for _ in 0..50 {
        if client.is_connected() {
            break;
        }
        step(&mut server, &mut client);
    }
    assert!(client.is_connected(), "{:?}", client.error());
    assert!(client.send_to(at, b"hello"));
    step(&mut server, &mut client);
    let mut buf = vec![0; 2048];
    let mut got = None;
    for _ in 0..8 {
        match server.recv(&mut buf) {
            Recv::Datagram { len, from } => {
                got = Some((from, buf[..len].to_vec()));
                break;
            }
            Recv::Skipped => {}
            Recv::Empty => step(&mut server, &mut client),
        }
    }
    let (stand_in, data) = got.expect("the datagram came through");
    assert_eq!(data, b"hello");
    assert!(
        torque_net_quic::stand_in::is_stand_in(stand_in),
        "{stand_in}"
    );
    assert!(
        !torque_server::net::web::is_stand_in(stand_in),
        "not a game socket's: {stand_in}"
    );
    // The host's per-address caps see the peer, not the stand-in; the ticket gets the QUIC link's binding.
    assert_eq!(server.peer_ip(stand_in), IpAddr::V4(*CLIENT.ip()));
    assert_eq!(
        server.channel_binding(stand_in),
        Some(client.channel_binding().expect("the client's binding"))
    );
    let kind = server.link_kind(stand_in);
    assert!(kind.facts.secure && kind.facts.validated);
    assert_eq!(kind.facts.server_identity, Some(key.identity()));
    assert!(server.send_to(stand_in, b"world"));
    step(&mut server, &mut client);
    assert_eq!(client.recv().map(|(_, d)| d), Some(b"world".to_vec()));
    assert!(server.take_gone().is_empty());
    // The peer hangs up: the combined transport reports its stand-in gone.
    client.close();
    for _ in 0..8 {
        step(&mut server, &mut client);
    }
    assert_eq!(server.take_gone(), vec![stand_in]);
    assert!(server.status().iter().any(|l| l.starts_with("quic:")));
    server.close();
}
