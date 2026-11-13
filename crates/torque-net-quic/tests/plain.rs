//! Plain datagrams beside QUIC (`QuicServer::pass_plain`): what reaches the host, from where, and what it may answer.

use crate::common;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use common::{SERVER, World};
use torque_net::engine::LinkKind;
use torque_net_quic::{Io, PLAIN_WAITING, PipeIo, QuicLimits};
use torque_server::net::transport::{Recv, Transport};

const REQUEST: u8 = 14;

fn requests_only(data: &[u8]) -> bool {
    data.first() == Some(&REQUEST)
}

fn socket(world: &World, addr: SocketAddrV4) -> PipeIo {
    world.pipe.bound(SocketAddr::V4(addr))
}

fn send(world: &World, io: &mut PipeIo, data: &[u8]) {
    assert!(io.send(SocketAddr::V4(SERVER), data));
    world.pipe.deliver_all();
}

/// Every datagram the host reads now, with its sender.
fn read_all(world: &mut World) -> Vec<(SocketAddr, Vec<u8>)> {
    let mut buf = [0u8; 2048];
    let mut out = Vec::new();
    while let Recv::Datagram { len, from } = world.server.recv(&mut buf) {
        out.push((from, buf[..len].to_vec()));
    }
    out
}

#[test]
fn a_plain_request_from_a_private_address_reaches_the_host_and_its_answer_goes_back() {
    let mut world = World::new(QuicLimits::default(), 1);
    world.server.pass_plain(requests_only);
    let lan = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 9), 5_000);
    let mut io = socket(&world, lan);
    send(&world, &mut io, &[REQUEST, 1, 2, 3, 4, 5]);
    let got = read_all(&mut world);
    assert_eq!(
        got,
        vec![(SocketAddr::V4(lan), vec![REQUEST, 1, 2, 3, 4, 5])]
    );
    assert_eq!(
        world.server.link_kind(SocketAddr::V4(lan)),
        LinkKind::PLAIN_UDP
    );
    assert!(world.server.send_to(SocketAddr::V4(lan), b"\x10answer"));
    let flights = world.pipe.take_in_flight();
    assert_eq!(flights.len(), 1);
    assert_eq!(flights[0].to, SocketAddr::V4(lan));
    assert_eq!(flights[0].data, b"\x10answer");
    assert_eq!(world.server.counters().plain_in, 1);
    assert_eq!(world.server.counters().plain_out, 1);
}

#[test]
fn a_public_sender_another_type_or_a_quic_packet_never_reaches_the_host_as_plain() {
    let mut world = World::new(QuicLimits::default(), 2);
    world.server.pass_plain(requests_only);
    let public = SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 5_000);
    let mut far = socket(&world, public);
    send(&world, &mut far, &[REQUEST, 1, 2, 3, 4, 5]);
    let lan = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 9), 5_000);
    let mut near = socket(&world, lan);
    send(&world, &mut near, &[18, 1, 2, 3, 4, 5]);
    // The fixed bit set: QUIC's, never plain, whatever the filter says.
    send(&world, &mut near, &[REQUEST | 0x40, 1, 2, 3, 4, 5]);
    assert!(read_all(&mut world).is_empty());
    assert_eq!(world.server.counters().plain_in, 0);
    // Nothing goes to a public address outside a connection.
    assert!(!world.server.send_to(SocketAddr::V4(public), b"\x10answer"));
    assert_eq!(world.pipe.in_flight(), 0);
}

#[test]
fn without_the_pass_plain_datagrams_are_stray_and_nothing_is_sent_outside_a_connection() {
    let mut world = World::new(QuicLimits::default(), 3);
    let lan = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 9), 5_000);
    let mut io = socket(&world, lan);
    send(&world, &mut io, &[REQUEST, 1, 2, 3, 4, 5]);
    assert!(read_all(&mut world).is_empty());
    assert!(world.server.counters().stray >= 1);
    assert!(!world.server.send_to(SocketAddr::V4(lan), b"\x10answer"));
}

#[test]
fn at_most_the_cap_of_plain_datagrams_wait_for_the_host() {
    let mut world = World::new(QuicLimits::default(), 4);
    world.server.pass_plain(requests_only);
    let lan = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 9), 5_000);
    let mut io = socket(&world, lan);
    for _ in 0..PLAIN_WAITING + 10 {
        assert!(io.send(SocketAddr::V4(SERVER), &[REQUEST, 0, 0, 0, 0, 0]));
    }
    world.pipe.deliver_all();
    // One pass reads them all; the ones past the cap are stray.
    world.server.service();
    assert_eq!(world.server.counters().plain_in, PLAIN_WAITING as u64);
    assert_eq!(read_all(&mut world).len(), PLAIN_WAITING);
    // Read, they make room again.
    send(&world, &mut io, &[REQUEST, 0, 0, 0, 0, 0]);
    assert_eq!(read_all(&mut world).len(), 1);
}
