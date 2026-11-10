use std::net::SocketAddr;

use std::net::IpAddr;

use super::*;
use crate::engine::framing::{Accept, Connect, TableDigests, Versions, Widths};
use crate::engine::link::{Link, LinkFacts, NoRoom};
use crate::engine::session::{Event, NoStreams};
use crate::fair::SourceCaps;

const GAME: &str = "fixture";
const KEY: [u8; 16] = [3; 16];

/// What a client sent: every datagram, in order.
struct Out(Vec<Vec<u8>>, LinkFacts);

impl Link for Out {
    fn max_datagram(&self) -> usize {
        1_200
    }

    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        self.0.push(datagram.to_vec());
        Ok(())
    }

    fn facts(&self) -> LinkFacts {
        self.1
    }
}

fn connect() -> Connect {
    Connect {
        versions: Versions::CURRENT,
        game: GAME.into(),
        manifest: Vec::new(),
        ticket: Vec::new(),
        mods: Vec::new(),
        password: String::new(),
        args: Vec::new(),
    }
}

fn addr(ip: [u8; 4], port: u16) -> SocketAddr {
    SocketAddr::from((ip, port))
}

/// A client on `kind`'s link and its first datagram.
fn first_datagram(kind: LinkKind, nonce: u64) -> (Session, Vec<u8>) {
    let mut client = Session::client(SessionConfig::new(GAME), kind.facts, connect(), nonce, 0)
        .expect("a client");
    let mut out = Out(Vec::new(), kind.facts);
    client.poll(&mut out, &mut NoStreams, 0);
    let first = out.0.remove(0);
    (client, first)
}

/// A datagram whose source is the peer's own address.
trait Rx {
    fn rx(&mut self, from: SocketAddr, kind: LinkKind, data: &[u8], now: u32) -> Arrival;
}

impl Rx for SessionTable {
    fn rx(&mut self, from: SocketAddr, kind: LinkKind, data: &[u8], now: u32) -> Arrival {
        self.receive(from, from.ip(), kind, data, now)
    }
}

fn table(limits: TableLimits) -> SessionTable {
    SessionTable::new(SessionConfig::new(GAME), limits, KEY)
}

#[test]
fn a_validated_link_opens_a_session_only_for_an_opening_and_routes_the_rest_to_it() {
    let mut t = table(TableLimits::default());
    let from = addr([10, 0, 0, 1], 5000);
    assert_eq!(
        t.rx(from, LinkKind::MEMORY, b"\xe1junk", 0),
        Arrival::Dropped
    );
    let (_, opening) = first_datagram(LinkKind::MEMORY, 1);
    assert_eq!(
        t.rx(from, LinkKind::MEMORY, &opening, 0),
        Arrival::Session { opened: true }
    );
    assert_eq!(
        t.rx(from, LinkKind::MEMORY, &opening, 1),
        Arrival::Session { opened: false }
    );
    let peer = t.peer_mut(from).expect("a session");
    assert!(matches!(
        peer.session.next_event(),
        Some(Event::ConnectRequest(_))
    ));
    assert_eq!(t.len(), 1);
}

#[test]
fn a_plain_link_answers_a_hello_with_a_cookie_and_opens_on_the_opening() {
    let mut t = table(TableLimits::default());
    let from = addr([192, 168, 1, 9], 7000);
    let (mut client, hello) = first_datagram(LinkKind::PLAIN_UDP, 9);
    let Arrival::Reply(cookie) = t.rx(from, LinkKind::PLAIN_UDP, &hello, 0) else {
        panic!("a cookie");
    };
    assert!(t.is_empty(), "nothing is kept for a hello");
    client.receive(&cookie, 1);
    let mut out = Out(Vec::new(), LinkFacts::PLAIN);
    client.poll(&mut out, &mut NoStreams, 1);
    assert_eq!(
        t.rx(from, LinkKind::PLAIN_UDP, &out.0[0], 1),
        Arrival::Session { opened: true }
    );
}

#[test]
fn pending_sessions_from_one_address_are_capped_and_a_network_over_its_share_is_evicted() {
    let limits = TableLimits {
        pending: 3,
        caps: SourceCaps {
            pending_per_ip: 2,
            ..SourceCaps::default()
        },
        ..TableLimits::default()
    };
    let mut t = table(limits);
    let (_, opening) = first_datagram(LinkKind::MEMORY, 1);
    for port in 1..=2 {
        let a = addr([10, 0, 0, 1], port);
        assert_eq!(
            t.rx(a, LinkKind::MEMORY, &opening, 0),
            Arrival::Session { opened: true }
        );
    }
    assert_eq!(
        t.rx(addr([10, 0, 0, 1], 3), LinkKind::MEMORY, &opening, 0),
        Arrival::Dropped,
        "a third pending session from one address"
    );
    assert_eq!(
        t.rx(addr([10, 0, 1, 2], 1), LinkKind::MEMORY, &opening, 0),
        Arrival::Session { opened: true }
    );
    assert!(t.take_evicted().is_empty());
    // The table is full: the network holding two of three slots is over its share and pays, not the other network's one.
    assert_eq!(
        t.rx(addr([10, 0, 2, 3], 1), LinkKind::MEMORY, &opening, 0),
        Arrival::Session { opened: true }
    );
    assert_eq!(t.take_evicted(), [addr([10, 0, 0, 1], 1)]);
    assert_eq!(t.len(), 3);
}

/// Brings `from`'s session to established: the server accepts, the client reads the accept and answers.
fn establish(t: &mut SessionTable, from: SocketAddr, nonce: u64) {
    let (mut client, opening) = first_datagram(LinkKind::MEMORY, nonce);
    assert_eq!(
        t.rx(from, LinkKind::MEMORY, &opening, 0),
        Arrival::Session { opened: true }
    );
    let id = t.session_id();
    let peer = t.peer_mut(from).expect("a session");
    while peer.session.next_event().is_some() {}
    peer.session
        .accept(Accept {
            session: id,
            max_packet: 1_200,
            packet_interval_ms: 32,
            byte_rate: 1 << 18,
            widths: Widths::default(),
            digests: TableDigests::default(),
        })
        .expect("an accept");
    let mut down = Out(Vec::new(), LinkFacts::MEMORY);
    peer.session.poll(&mut down, &mut NoStreams, 1);
    for d in &down.0 {
        client.receive(d, 2);
    }
    let mut up = Out(Vec::new(), LinkFacts::MEMORY);
    client.poll(&mut up, &mut NoStreams, 2);
    for d in &up.0 {
        t.rx(from, LinkKind::MEMORY, d, 3);
    }
    assert!(t.peer(from).expect("a session").stage == Stage::Established);
    // The accept's packet carried no stream data, and its fate still counts.
    assert!(t.peer(from).expect("a session").session.fates().delivered >= 1);
}

#[test]
fn established_sessions_are_capped_per_address_and_per_network() {
    let limits = TableLimits {
        caps: SourceCaps {
            per_ip: 2,
            per_network: 3,
            ..SourceCaps::default()
        },
        ..TableLimits::default()
    };
    let mut t = table(limits);
    establish(&mut t, addr([10, 0, 0, 1], 1), 1);
    establish(&mut t, addr([10, 0, 0, 1], 2), 2);
    let (_, opening) = first_datagram(LinkKind::MEMORY, 3);
    assert_eq!(
        t.rx(addr([10, 0, 0, 1], 3), LinkKind::MEMORY, &opening, 4),
        Arrival::Dropped,
        "a third session from one address"
    );
    establish(&mut t, addr([10, 0, 0, 2], 1), 4);
    assert_eq!(
        t.rx(addr([10, 0, 0, 3], 1), LinkKind::MEMORY, &opening, 4),
        Arrival::Dropped,
        "a fourth session from one network"
    );
    assert_eq!(
        t.rx(addr([10, 0, 1, 3], 1), LinkKind::MEMORY, &opening, 4),
        Arrival::Session { opened: true }
    );
    t.remove(addr([10, 0, 0, 1], 1));
    assert_eq!(
        t.rx(addr([10, 0, 0, 1], 3), LinkKind::MEMORY, &opening, 5),
        Arrival::Session { opened: true },
        "a closed session makes room"
    );
}

#[test]
fn session_ids_follow_the_key_and_never_repeat() {
    let mut a = table(TableLimits::default());
    let mut b = table(TableLimits::default());
    let ids: Vec<u64> = (0..64).map(|_| a.session_id()).collect();
    let again: Vec<u64> = (0..64).map(|_| b.session_id()).collect();
    assert_eq!(ids, again);
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len());
    b.rekey([4; 16]);
    assert_ne!(b.session_id(), a.session_id());
}

#[test]
fn a_challenge_comes_back_answered_with_the_ticket_before_the_server_decides() {
    let mut t = table(TableLimits::default());
    let from = addr([10, 0, 0, 1], 5000);
    let (mut client, opening) = first_datagram(LinkKind::MEMORY, 7);
    assert!(
        client.answer(vec![1]).is_err(),
        "no answer without a challenge"
    );
    t.rx(from, LinkKind::MEMORY, &opening, 0);
    let peer = t.peer_mut(from).expect("a session");
    assert!(matches!(
        peer.session.next_event(),
        Some(Event::ConnectRequest(_))
    ));
    peer.session.challenge(0xfeed).expect("a challenge");
    assert_eq!(peer.session.state(), State::Challenging);
    assert!(
        peer.session.challenge(1).is_err(),
        "one challenge a connect"
    );
    let mut down = Out(Vec::new(), LinkFacts::MEMORY);
    peer.session.poll(&mut down, &mut NoStreams, 1);
    for d in &down.0 {
        client.receive(d, 2);
    }
    assert_eq!(client.next_event(), Some(Event::Challenge(0xfeed)));
    client.answer(b"ticket".to_vec()).expect("an answer");
    let mut up = Out(Vec::new(), LinkFacts::MEMORY);
    client.poll(&mut up, &mut NoStreams, 3);
    for d in &up.0 {
        t.rx(from, LinkKind::MEMORY, d, 4);
    }
    let peer = t.peer_mut(from).expect("a session");
    assert_eq!(
        peer.session.next_event(),
        Some(Event::Ticket(b"ticket".to_vec()))
    );
    assert_eq!(peer.session.state(), State::Deciding);
}

#[test]
fn the_network_caps_count_the_peers_own_address_behind_stand_ins() {
    let limits = TableLimits {
        caps: SourceCaps {
            per_network: 2,
            ..SourceCaps::default()
        },
        ..TableLimits::default()
    };
    let mut t = table(limits);
    let (_, opening) = first_datagram(LinkKind::MEMORY, 1);
    // Three stand-in addresses in three /24s, all of one peer network: the third is over the network's cap.
    let real = |host: u8| IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 7, host));
    for (k, standin) in [[240, 1, 0, 1], [241, 2, 0, 1]].into_iter().enumerate() {
        assert_eq!(
            t.receive(
                addr(standin, 9),
                real(k as u8 + 1),
                LinkKind::MEMORY,
                &opening,
                0
            ),
            Arrival::Session { opened: true }
        );
    }
    assert_eq!(
        t.receive(
            addr([242, 3, 0, 1], 9),
            real(3),
            LinkKind::MEMORY,
            &opening,
            0
        ),
        Arrival::Dropped
    );
    assert_eq!(t.pending(), 2);
}

#[test]
fn a_plain_pending_session_evicted_for_a_validated_one_is_settled_at_the_gate() {
    let limits = TableLimits {
        pending: 1,
        ..TableLimits::default()
    };
    let mut t = table(limits);
    let plain = addr([192, 168, 1, 9], 7000);
    let (mut client, hello) = first_datagram(LinkKind::PLAIN_UDP, 9);
    let Arrival::Reply(cookie) = t.rx(plain, LinkKind::PLAIN_UDP, &hello, 0) else {
        panic!("a cookie");
    };
    client.receive(&cookie, 1);
    let mut out = Out(Vec::new(), LinkFacts::PLAIN);
    client.poll(&mut out, &mut NoStreams, 1);
    t.rx(plain, LinkKind::PLAIN_UDP, &out.0[0], 1);
    assert_eq!(t.gate_pending(), 1);
    let (_, opening) = first_datagram(LinkKind::MEMORY, 1);
    // Its only slot is not evicted while young; past the evict age it is.
    assert_eq!(
        t.rx(addr([10, 0, 0, 1], 1), LinkKind::MEMORY, &opening, 2),
        Arrival::Dropped
    );
    let late = 2 + crate::cap::NET_PENDING_EVICT_AGE_MS;
    assert_eq!(
        t.rx(addr([10, 0, 0, 1], 1), LinkKind::MEMORY, &opening, late),
        Arrival::Session { opened: true }
    );
    assert_eq!(t.take_evicted(), [plain]);
    assert_eq!(t.gate_pending(), 0, "the gate no longer counts it");
}
