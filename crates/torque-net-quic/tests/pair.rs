//! A client and a server over the in-process pipe under the fake clock: the handshake, the datagrams both ways, the
//! shared channel binding, the identity checks (given, trusted on first use, changed), migration, and an engine session
//! pair that delivers the same events over the QUIC link as over the memory link.

mod common;

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use common::{Client, Rng, SERVER, TICK, World, client_recv};
use torque_client::net::Datagrams;
use torque_net::engine::{
    Accept, Connect, Delivery, Event, Link, LinkFacts, NoRoom, NoStreams, Session, SessionConfig,
    State, TableDigests, Widths,
};
use torque_net_quic::{QuicLimits, ServerIdentity, ServerKey, Trust};
use torque_server::net::transport::Transport;

const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 4_000);
const OTHER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 8), 4_000);

fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "torque-net-quic-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ))
}

/// Connects `client`, sends one datagram through and answers it: the server's stand-in for the client.
fn hello(world: &mut World, client: &mut Client) -> SocketAddr {
    assert!(world.connect(client, 50), "{:?}", client.error());
    assert!(client.send_to(SERVER, b"hello"));
    world.step(&mut [client]);
    let got = world.server_recv();
    assert_eq!(got.len(), 1, "{got:?}");
    let (stand_in, data) = got.into_iter().next().expect("one datagram");
    assert_eq!(data, b"hello");
    assert!(torque_net_quic::stand_in::is_stand_in(stand_in));
    assert!(world.server.send_to(stand_in, b"world"));
    world.step(&mut [client]);
    assert_eq!(client_recv(client), [b"world".to_vec()]);
    stand_in
}

#[test]
fn a_client_and_a_server_handshake_share_a_binding_and_report_the_identity() {
    let mut world = World::new(QuicLimits::default(), 1);
    let identity = world.key.identity();
    let mut client = world.client(CLIENT, Trust::Identity(identity), 2);
    assert!(!client.is_connected());
    let stand_in = hello(&mut world, &mut client);
    let binding = client.channel_binding().expect("the client's binding");
    assert_eq!(world.server.channel_binding(stand_in), Some(binding));
    let kind = world.server.link_kind(stand_in);
    assert!(kind.facts.validated && kind.facts.secure && kind.facts.congestion_controlled);
    assert_eq!(kind.facts.server_identity, Some(identity));
    assert!(
        kind.max_datagram <= 1_200 && kind.max_datagram > 1_000,
        "{kind:?}"
    );
    assert_eq!(client.link_kind(), kind);
    assert_eq!(world.server.peer_ip(stand_in), IpAddr::V4(*CLIENT.ip()));
    // A second connection has its own binding and its own stand-in under the same stand-in IP.
    let mut second = world.client(
        SocketAddrV4::new(*CLIENT.ip(), 4_001),
        Trust::Identity(identity),
        3,
    );
    let other = hello(&mut world, &mut second);
    assert_ne!(other, stand_in);
    assert_eq!(other.ip(), stand_in.ip());
    assert_ne!(second.channel_binding(), Some(binding));
    assert_eq!(world.server.connections(), 2);
    // A datagram to an address that is no connection goes nowhere; one to a client that hung up is refused once it is gone.
    assert!(
        !world
            .server
            .send_to(SocketAddr::from(([240, 0, 0, 1], 9)), b"x")
    );
    second.close();
    world.step(&mut [&mut second]);
    assert!(world.server.take_gone().contains(&other));
    assert!(!world.server.send_to(other, b"x"));
    assert!(client.send_to(SERVER, b"still here"));
    world.step(&mut [&mut client]);
    assert_eq!(world.server_recv(), [(stand_in, b"still here".to_vec())]);
}

#[test]
fn a_server_with_another_identity_is_refused_with_both_hashes() {
    let mut world = World::new(QuicLimits::default(), 4);
    let wanted = ServerIdentity([0xab; 32]);
    let mut client = world.client(CLIENT, Trust::Identity(wanted), 5);
    let error = world.fail(&mut client, 50).expect("refused");
    assert!(error.contains(&world.key.identity().to_string()), "{error}");
    assert!(error.contains(&wanted.to_string()), "{error}");
    assert!(error.contains("not the identity given"), "{error}");
    assert!(!client.is_connected());
    assert!(!client.send_to(SERVER, b"nothing"));
    // The server's side of the failed handshake drains, then goes.
    world.clock.advance(Duration::from_secs(10));
    world.step(&mut [&mut client]);
    assert_eq!(
        world.server.connections(),
        0,
        "the failed handshake is gone"
    );
}

#[test]
fn a_server_with_no_identity_given_is_trusted_once_and_a_changed_key_is_refused() {
    let dir = scratch("known");
    let file = dir.join(torque_net_quic::KNOWN_SERVERS_FILE);
    let first_key;
    {
        let mut world = World::new(QuicLimits::default(), 6);
        first_key = world.key.identity();
        let mut client = world.client(CLIENT, Trust::Known(file.clone()), 7);
        assert_eq!(client.identity(), None, "nothing known yet");
        hello(&mut world, &mut client);
        assert_eq!(client.identity(), Some(first_key));
        assert_eq!(client.watch().state().note, None);
        let text = std::fs::read_to_string(&file).expect("the file was written");
        assert_eq!(text.trim(), format!("{SERVER} {first_key}"));
        // The same server again: the remembered identity is expected from the start.
        let mut again = world.client(OTHER, Trust::Known(file.clone()), 8);
        assert_eq!(again.identity(), Some(first_key));
        hello(&mut world, &mut again);
    }
    let mut world = World::new(QuicLimits::default(), 9);
    assert_ne!(world.key.identity(), first_key);
    let mut client = world.client(CLIENT, Trust::Known(file.clone()), 10);
    let error = world.fail(&mut client, 50).expect("refused");
    assert!(error.contains("not the identity remembered"), "{error}");
    assert!(error.contains(&first_key.to_string()), "{error}");
    assert!(error.contains(&world.key.identity().to_string()), "{error}");
    std::fs::remove_dir_all(&dir).expect("removed");
}

#[test]
fn a_client_whose_address_moves_keeps_its_connection_and_the_checks_run_again() {
    let mut world = World::new(QuicLimits::default(), 11);
    world
        .server
        .set_address_check(|ip| ip != IpAddr::V4(Ipv4Addr::new(10, 0, 0, 66)));
    let identity = world.key.identity();
    let mut client = world.client(CLIENT, Trust::Identity(identity), 12);
    let stand_in = hello(&mut world, &mut client);
    let binding = client.channel_binding();
    let moved = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 9), 5_000);
    client.rebind(world.pipe.bound(SocketAddr::V4(moved)));
    assert!(client.send_to(SERVER, b"from elsewhere"));
    for _ in 0..4 {
        world.step(&mut [&mut client]);
    }
    assert_eq!(
        world.server_recv(),
        [(stand_in, b"from elsewhere".to_vec())]
    );
    assert_eq!(world.server.peer_ip(stand_in), IpAddr::V4(*moved.ip()));
    assert_eq!(world.server.counters().migrations, 1);
    assert_eq!(
        world.server.channel_binding(stand_in),
        binding,
        "the same connection"
    );
    assert!(world.server.send_to(stand_in, b"back"));
    world.step(&mut [&mut client]);
    assert_eq!(client_recv(&mut client), [b"back".to_vec()]);
    // A move to a refused address closes the connection.
    let banned = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 66), 5_000);
    client.rebind(world.pipe.bound(SocketAddr::V4(banned)));
    assert!(client.send_to(SERVER, b"from the banned address"));
    for _ in 0..6 {
        world.step(&mut [&mut client]);
    }
    assert_eq!(world.server.counters().closed_on_migration, 1);
    assert!(world.server.take_gone().contains(&stand_in));
    assert!(client.error().is_some(), "the client learns of the close");
}

#[test]
fn the_server_refuses_when_full_and_over_the_per_address_cap() {
    let limits = QuicLimits {
        max_connections: 3,
        connections_per_ip: 2,
        ..QuicLimits::default()
    };
    let mut world = World::new(limits, 13);
    let identity = world.key.identity();
    let mut a = world.client(
        SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 1),
        Trust::Identity(identity),
        14,
    );
    let mut b = world.client(
        SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 2),
        Trust::Identity(identity),
        15,
    );
    let mut c = world.client(
        SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 3),
        Trust::Identity(identity),
        16,
    );
    assert!(world.connect(&mut a, 50));
    assert!(world.connect(&mut b, 50));
    assert!(
        world.fail(&mut c, 50).is_some(),
        "a third from one address: {:?} {:?} {}",
        world.server.counters(),
        c.watch().state(),
        world.server.connections()
    );
    let mut d = world.client(
        SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 8), 1),
        Trust::Identity(identity),
        17,
    );
    assert!(world.connect(&mut d, 50));
    let mut e = world.client(
        SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 9), 1),
        Trust::Identity(identity),
        18,
    );
    assert!(world.fail(&mut e, 50).is_some(), "the server is full");
    assert_eq!(world.server.counters().refused, 2);
    assert_eq!(world.server.established(), 3);
    // The refused handshakes drain, then go.
    world.clock.advance(Duration::from_secs(10));
    world.step(&mut [&mut a, &mut b, &mut d]);
    assert_eq!(world.server.connections(), 3);
}

// ---- the engine session pair
// ----------------------------------------------------------------------------------------------------

const GAME: &str = "fixture";

/// What carries the sessions' datagrams: the memory link, or the QUIC link.
trait Carrier {
    fn max_datagram(&self) -> usize;
    fn to_server(&mut self, datagram: &[u8]) -> Result<(), NoRoom>;
    fn to_client(&mut self, datagram: &[u8]) -> Result<(), NoRoom>;
    /// What the server received since the last call.
    fn at_server(&mut self) -> Vec<Vec<u8>>;
    /// What the client received since the last call.
    fn at_client(&mut self) -> Vec<Vec<u8>>;
    fn tick(&mut self);
}

struct Memory {
    max: usize,
    up: VecDeque<Vec<u8>>,
    down: VecDeque<Vec<u8>>,
}

impl Carrier for Memory {
    fn max_datagram(&self) -> usize {
        self.max
    }

    fn to_server(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        self.up.push_back(datagram.to_vec());
        Ok(())
    }

    fn to_client(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        self.down.push_back(datagram.to_vec());
        Ok(())
    }

    fn at_server(&mut self) -> Vec<Vec<u8>> {
        self.up.drain(..).collect()
    }

    fn at_client(&mut self) -> Vec<Vec<u8>> {
        self.down.drain(..).collect()
    }

    fn tick(&mut self) {}
}

struct Quic {
    world: World,
    client: Client,
    stand_in: SocketAddr,
}

impl Carrier for Quic {
    fn max_datagram(&self) -> usize {
        self.client.link_kind().max_datagram
    }

    fn to_server(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        if self.client.send_to(SERVER, datagram) {
            Ok(())
        } else {
            Err(NoRoom)
        }
    }

    fn to_client(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        if self.world.server.send_to(self.stand_in, datagram) {
            Ok(())
        } else {
            Err(NoRoom)
        }
    }

    fn at_server(&mut self) -> Vec<Vec<u8>> {
        self.world.pipe.deliver_all();
        self.world.server.pump();
        self.world
            .server_recv()
            .into_iter()
            .map(|(_, d)| d)
            .collect()
    }

    fn at_client(&mut self) -> Vec<Vec<u8>> {
        self.world.pipe.deliver_all();
        self.client.pump();
        client_recv(&mut self.client)
    }

    fn tick(&mut self) {
        self.world.clock.advance(TICK);
    }
}

/// One side's end of a carrier as an engine link.
struct Side<'a, C: Carrier> {
    carrier: &'a mut C,
    up: bool,
}

impl<C: Carrier> Link for Side<'_, C> {
    fn max_datagram(&self) -> usize {
        self.carrier.max_datagram()
    }

    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        if self.up {
            self.carrier.to_server(datagram)
        } else {
            self.carrier.to_client(datagram)
        }
    }

    fn facts(&self) -> LinkFacts {
        LinkFacts {
            validated: true,
            secure: true,
            congestion_controlled: true,
            server_identity: None,
        }
    }
}

fn accept() -> Accept {
    Accept {
        session: 0x5e55_1011_cafe_f00d,
        max_packet: 1_200,
        packet_interval_ms: 32,
        byte_rate: 256 << 10,
        widths: Widths {
            ghost_id: 12,
            ghost_class: 6,
            datablock_class: 7,
            event_class: 5,
        },
        digests: TableDigests {
            channels: 0x0101,
            rpcs: 0x0202,
            uploads: 0x0303,
        },
    }
}

/// Runs a client and a server session over `carrier` for `ticks`: the client sends reliable messages of many sizes and
/// unreliable ones once established, the server answers; both sides' events, in order.
fn run_sessions<C: Carrier>(carrier: &mut C, ticks: u32) -> (Vec<Event>, Vec<Event>) {
    let facts = Side { carrier, up: true }.facts();
    let connect = Connect {
        game: GAME.to_owned(),
        manifest: vec![1, 2, 3, 4],
        password: "pw".to_owned(),
        args: vec!["Player".to_owned()],
        ..Connect::default()
    };
    let config = SessionConfig::new(GAME);
    let mut now = 1_000;
    let mut client =
        Session::client(config.clone(), facts, connect, 0x1234_5678, now).expect("a client");
    let mut server = Session::server(config, facts, now).expect("a server");
    let (mut client_events, mut server_events) = (Vec::new(), Vec::new());
    let mut rng = Rng::new(99);
    let mut sent = 0;
    for tick in 0..ticks {
        client.poll(
            &mut Side {
                carrier: &mut *carrier,
                up: true,
            },
            &mut NoStreams,
            now,
        );
        for d in carrier.at_server() {
            server.receive(&d, now);
        }
        while let Some(e) = server.next_event() {
            if matches!(e, Event::ConnectRequest(_)) {
                server.accept(accept()).expect("an accept");
            }
            server_events.push(e);
        }
        if server.state() == State::Established && tick % 3 == 0 {
            server
                .send(
                    vec![0xee; 100 + (tick as usize % 7) * 300],
                    Delivery::Reliable,
                )
                .expect("queued");
        }
        server.poll(
            &mut Side {
                carrier: &mut *carrier,
                up: false,
            },
            &mut NoStreams,
            now,
        );
        for d in carrier.at_client() {
            client.receive(&d, now);
        }
        while let Some(e) = client.next_event() {
            client_events.push(e);
        }
        if client.state() == State::Established && sent < 30 {
            let size = 1 + rng.below(4_000) as usize;
            client
                .send(rng.bytes(size), Delivery::Reliable)
                .expect("queued");
            client
                .send(rng.bytes(50), Delivery::Unreliable)
                .expect("queued");
            sent += 1;
        }
        carrier.tick();
        now += 32;
    }
    (client_events, server_events)
}

#[test]
fn an_engine_session_pair_over_quic_delivers_the_same_events_as_over_the_memory_link() {
    let mut world = World::new(QuicLimits::default(), 20);
    let identity = world.key.identity();
    let mut client = world.client(CLIENT, Trust::Identity(identity), 21);
    let stand_in = hello(&mut world, &mut client);
    let mut quic = Quic {
        world,
        client,
        stand_in,
    };
    let max = quic.max_datagram();
    let mut memory = Memory {
        max,
        up: VecDeque::new(),
        down: VecDeque::new(),
    };
    let over_quic = run_sessions(&mut quic, 150);
    let over_memory = run_sessions(&mut memory, 150);
    let messages = |events: &[Event]| {
        events
            .iter()
            .filter(|e| matches!(e, Event::Message { .. }))
            .count()
    };
    assert!(
        over_memory
            .0
            .iter()
            .any(|e| matches!(e, Event::Accepted(_))),
        "{:?}",
        over_memory.0
    );
    assert!(
        messages(&over_memory.1) >= 50,
        "{}",
        messages(&over_memory.1)
    );
    assert!(
        messages(&over_memory.0) >= 20,
        "{}",
        messages(&over_memory.0)
    );
    assert_eq!(over_quic.0, over_memory.0, "the client's events");
    assert_eq!(over_quic.1, over_memory.1, "the server's events");
    assert!(quic.world.server.counters().datagrams_in > 100);
}

#[test]
fn a_key_file_and_its_identity_text_round_trip_through_a_pinned_connection() {
    let dir = scratch("key-file");
    let path = dir.join(torque_net_quic::IDENTITY_FILE);
    let key = ServerKey::load_or_make(&path).expect("a key");
    let text = key.identity().to_string();
    let mut world = World::with_key(key, QuicLimits::default(), 30);
    let pinned: ServerIdentity = text.parse().expect("the text parses");
    let mut client = world.client(CLIENT, Trust::Identity(pinned), 31);
    hello(&mut world, &mut client);
    std::fs::remove_dir_all(&dir).expect("removed");
}

/// Initials from spoofed addresses that are never answered: the server holds no established connection for them, asks the
/// rest to prove their address, drops them on the handshake timeout, and an honest client (from the very address they
/// spoofed) still connects.
#[test]
fn a_flood_of_unfinished_handshakes_never_fills_the_server_or_locks_an_address_out() {
    let limits = QuicLimits {
        max_connections: 3,
        connections_per_ip: 2,
        handshakes_in_progress: 8,
        handshake_timeout: Duration::from_secs(3),
        handshakes_per_second: 10_000,
        handshakes_per_network_per_second: 10_000,
        ..QuicLimits::default()
    };
    let mut world = World::new(limits, 40);
    let identity = world.key.identity();
    for i in 0..24u16 {
        // Half from the victim's own address, half from a range; none is ever pumped again.
        let ip = if i % 2 == 0 {
            *CLIENT.ip()
        } else {
            Ipv4Addr::new(10, 0, 9, 1 + i as u8)
        };
        let flooder = world.client(
            SocketAddrV4::new(ip, 6_000 + i),
            Trust::Identity(identity),
            41 + u64::from(i),
        );
        world.pipe.deliver_all();
        world.server.pump();
        drop(flooder);
    }
    let c = world.server.counters();
    assert_eq!(world.server.established(), 0);
    assert!(
        world.server.connections() <= 8,
        "{}",
        world.server.connections()
    );
    assert!(c.retried > 0, "{c:?}");
    assert_eq!(
        c.refused, 0,
        "nothing honest-looking is refused for a flood: {c:?}"
    );
    // The victim connects from the spoofed address, right away.
    let mut honest = world.client(CLIENT, Trust::Identity(identity), 90);
    hello(&mut world, &mut honest);
    assert_eq!(world.server.established(), 1);
    // The handshakes that never finish go on their timer.
    world.clock.advance(Duration::from_secs(4));
    world.step(&mut [&mut honest]);
    assert_eq!(
        world.server.connections(),
        1,
        "{:?}",
        world.server.counters()
    );
    assert!(world.server.counters().handshakes_dropped > 0);
    assert!(honest.is_connected());
}

#[test]
fn a_flooding_peer_never_costs_a_quiet_peer_a_datagram() {
    let limits = QuicLimits {
        inbox_per_connection: 4,
        ..QuicLimits::default()
    };
    let mut world = World::new(limits, 50);
    let identity = world.key.identity();
    let mut loud = world.client(CLIENT, Trust::Identity(identity), 51);
    let mut quiet = world.client(OTHER, Trust::Identity(identity), 52);
    let loud_at = hello(&mut world, &mut loud);
    let quiet_at = hello(&mut world, &mut quiet);
    for i in 0..60u32 {
        assert!(loud.send_to(SERVER, &i.to_le_bytes()));
        world.pipe.deliver_all();
        world.server.pump();
    }
    assert!(quiet.send_to(SERVER, b"one word"));
    world.pipe.deliver_all();
    world.server.pump();
    let got = world.server_recv();
    let from_loud = got.iter().filter(|(from, _)| *from == loud_at).count();
    let from_quiet: Vec<&Vec<u8>> = got
        .iter()
        .filter(|(from, _)| *from == quiet_at)
        .map(|(_, d)| d)
        .collect();
    assert_eq!(from_quiet, [b"one word".as_slice()]);
    assert!(from_loud <= 4, "{from_loud}");
    assert!(world.server.counters().inbox_dropped > 0);
    // The loud peer's share frees as the host reads.
    assert!(loud.send_to(SERVER, b"again"));
    world.pipe.deliver_all();
    world.server.pump();
    assert_eq!(world.server_recv(), [(loud_at, b"again".to_vec())]);
}

#[test]
fn a_stand_in_is_not_reused_until_the_host_has_read_that_it_is_gone() {
    let mut world = World::new(QuicLimits::default(), 60);
    let identity = world.key.identity();
    let mut first = world.client(CLIENT, Trust::Identity(identity), 61);
    let gone = hello(&mut world, &mut first);
    first.close();
    world.step(&mut [&mut first]);
    world.clock.advance(Duration::from_secs(10));
    world.step(&mut [&mut first]);
    assert_eq!(world.server.connections(), 0);
    // The host has not read `gone` yet: a new connection from the same address gets another stand-in.
    let mut second = world.client(
        SocketAddrV4::new(*CLIENT.ip(), 4_001),
        Trust::Identity(identity),
        62,
    );
    let next = hello(&mut world, &mut second);
    assert_ne!(next, gone);
    assert_eq!(next.ip(), gone.ip());
    let read = world.server.take_gone();
    assert!(read.contains(&gone), "{read:?}");
    assert!(!read.contains(&next));
}

#[test]
fn a_full_known_servers_file_refuses_a_new_server_naming_the_file() {
    let dir = scratch("known-full");
    let file = dir.join(torque_net_quic::KNOWN_SERVERS_FILE);
    std::fs::create_dir_all(&dir).expect("the folder");
    let mut text = String::new();
    for i in 0..torque_net_quic::known::MAX_KNOWN_SERVERS {
        text.push_str(&format!(
            "10.{}.{}.2:28000 {}\n",
            i >> 8,
            i & 0xff,
            ServerIdentity([i as u8; 32])
        ));
    }
    std::fs::write(&file, &text).expect("written");
    let known = torque_net_quic::KnownServers::load(&file).expect("read");
    assert_eq!(
        (known.len(), known.skipped()),
        (torque_net_quic::known::MAX_KNOWN_SERVERS, 0)
    );
    assert!(!known.has_room());
    let world = World::new(QuicLimits::default(), 70);
    let error = torque_net_quic::QuicClient::connect(
        world.pipe.bound(SocketAddr::V4(CLIENT)),
        world.clock.clone(),
        SERVER,
        Trust::Known(file.clone()),
        &world.limits,
        None,
    )
    .err()
    .expect("refused");
    assert!(
        error.contains(torque_net_quic::KNOWN_SERVERS_FILE),
        "{error}"
    );
    assert!(error.contains("already holds"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&file).expect("the file"),
        text,
        "nothing forgotten"
    );
    std::fs::remove_dir_all(&dir).expect("removed");
}

/// Validated handshakes from one address or network fill their own shares and, at the cap, take only their own kind's
/// place: a handshake in progress from elsewhere is never evicted.
#[test]
fn a_validated_flood_from_one_network_never_evicts_another_addresss_handshake() {
    let limits = QuicLimits {
        handshakes_in_progress: 4,
        handshakes_in_progress_per_ip: 2,
        handshakes_in_progress_per_network: 3,
        handshakes_per_second: 10_000,
        handshakes_per_network_per_second: 10_000,
        ..QuicLimits::default()
    };
    let mut world = World::new(limits, 80);
    let identity = world.key.identity();
    let flood_ip = Ipv4Addr::new(10, 0, 9, 1);
    // Unvalidated junk from the flood's network fills the half of the places such handshakes may hold: every Initial after
    // it is answered with Retry first, so what the victim and the flooders send back is validated.
    let mut junk = Vec::new();
    for i in 0..2u16 {
        let j = world.client(
            SocketAddrV4::new(Ipv4Addr::new(10, 0, 9, 20 + i as u8), 1),
            Trust::Identity(identity),
            70 + u64::from(i),
        );
        world.pipe.deliver_all();
        world.server.pump();
        junk.push(j);
    }
    assert_eq!(world.server.connections(), 2);
    // The victim's handshake: its Initial, the Retry back, its Initial with the token, accepted; then it waits (its socket
    // is not pumped again until the flood is over).
    let mut victim = world.client(CLIENT, Trust::Identity(identity), 82);
    world.pipe.deliver_all();
    world.server.pump();
    world.pipe.deliver_all();
    victim.pump();
    world.pipe.deliver_all();
    world.server.pump();
    assert_eq!(world.server.connections(), 3);
    let before = world.server.counters();
    let mut flooders = Vec::new();
    for i in 0..12u16 {
        let ip = if i % 2 == 0 {
            flood_ip
        } else {
            Ipv4Addr::new(10, 0, 9, 2 + i as u8)
        };
        let mut flooder = world.client(
            SocketAddrV4::new(ip, 100 + i),
            Trust::Identity(identity),
            83 + u64::from(i),
        );
        // Its Initial, the Retry back, its Initial with the token, accepted: a validated handshake in progress that is
        // never finished (the flooder's socket is not pumped again).
        world.pipe.deliver_all();
        world.server.pump();
        world.pipe.deliver_all();
        flooder.pump();
        world.pipe.deliver_all();
        world.server.pump();
        flooders.push(flooder);
    }
    let c = world.server.counters();
    assert!(c.retried > before.retried, "{c:?}");
    assert!(c.refused > before.refused, "the shares hold: {c:?}");
    assert!(
        c.handshakes_dropped > before.handshakes_dropped,
        "the flood took its own kind's places: {c:?}"
    );
    assert_eq!(world.server.established(), 0);
    assert!(
        world.server.connections() <= 4,
        "{}",
        world.server.connections()
    );
    // The victim's handshake was never taken: it finishes.
    drop(flooders);
    drop(junk);
    assert!(world.connect(&mut victim, 50), "{:?}", victim.error());
}

#[test]
fn a_connection_closed_as_full_is_reported_gone_and_delivers_nothing() {
    let limits = QuicLimits {
        max_connections: 1,
        ..QuicLimits::default()
    };
    let mut world = World::new(limits, 85);
    let identity = world.key.identity();
    // Two handshakes begin together; only one may finish into the one place.
    let mut a = world.client(CLIENT, Trust::Identity(identity), 86);
    let mut b = world.client(OTHER, Trust::Identity(identity), 87);
    for _ in 0..30 {
        world.step(&mut [&mut a, &mut b]);
        // Whoever believes it is connected sends at once.
        for client in [&mut a, &mut b] {
            if client.is_connected() {
                client.send_to(SERVER, b"am I in?");
            }
        }
    }
    assert_eq!(world.server.established(), 1);
    assert_eq!(world.server.counters().refused, 1);
    let got = world.server_recv();
    let gone = world.server.take_gone();
    assert_eq!(gone.len(), 1, "the refused one is reported gone: {gone:?}");
    assert!(
        got.iter().all(|(from, _)| !gone.contains(from)),
        "nothing from the refused connection reaches the host: {got:?}"
    );
    assert!(!got.is_empty(), "the one that got in is heard");
    assert_eq!(
        [a.error().is_some(), b.error().is_some()]
            .iter()
            .filter(|e| **e)
            .count(),
        1
    );
}

#[test]
fn a_backlog_is_capped_in_bytes_too() {
    let limits = QuicLimits {
        inbox_per_connection: 256,
        inbox_bytes_per_connection: 2_500,
        ..QuicLimits::default()
    };
    let mut world = World::new(limits, 95);
    let identity = world.key.identity();
    let mut client = world.client(CLIENT, Trust::Identity(identity), 96);
    let stand_in = hello(&mut world, &mut client);
    for _ in 0..20 {
        assert!(client.send_to(SERVER, &[7; 1_000]));
        world.step(&mut [&mut client]);
    }
    let got = world.server_recv();
    let bytes: usize = got.iter().map(|(_, d)| d.len()).sum();
    assert!(got.iter().all(|(from, _)| *from == stand_in));
    assert!(
        bytes <= 2_500 && got.len() == 2,
        "{} datagrams, {bytes} bytes",
        got.len()
    );
    assert!(world.server.counters().inbox_dropped >= 18);
    // The room comes back as the host reads.
    assert!(client.send_to(SERVER, &[8; 1_000]));
    world.pipe.deliver_all();
    world.server.pump();
    assert_eq!(world.server_recv().len(), 1);
}

/// Forged half-open handshakes from many addresses, as many as the cap and more, then a newcomer from an unrelated
/// network: it proves its address on the Retry and gets in at once, no timeout waited for.
#[test]
fn a_forged_flood_never_keeps_a_validated_newcomer_out() {
    let limits = QuicLimits {
        handshakes_in_progress: 16,
        handshakes_per_second: 10_000,
        handshakes_per_network_per_second: 10_000,
        ..QuicLimits::default()
    };
    let mut world = World::new(limits, 100);
    let identity = world.key.identity();
    let mut forged = Vec::new();
    for i in 0..64u16 {
        let ip = Ipv4Addr::new(10, 20 + (i / 8) as u8, i as u8, 1);
        let f = world.client(
            SocketAddrV4::new(ip, 7_000 + i),
            Trust::Identity(identity),
            101 + u64::from(i),
        );
        world.pipe.deliver_all();
        world.server.pump();
        forged.push(f);
    }
    let c = world.server.counters();
    assert_eq!(
        world.server.connections(),
        8,
        "unvalidated handshakes hold at most half the places: {c:?}"
    );
    assert!(c.retried >= 56, "{c:?}");
    assert_eq!(c.refused, 0, "{c:?}");
    // From a network none of the forged addresses are in.
    let mut honest = world.client(CLIENT, Trust::Identity(identity), 200);
    assert!(
        world.connect(&mut honest, 6),
        "{:?} {:?}",
        honest.error(),
        world.server.counters()
    );
    assert_eq!(world.server.established(), 1);
    assert!(
        world.clock.elapsed() < Duration::from_secs(1),
        "no timeout was waited for"
    );
    hello(&mut world, &mut honest);
    // A second one, with the places full of validated handshakes too: the oldest forged one makes room.
    let mut more = Vec::new();
    for i in 0..8u16 {
        let mut m = world.client(
            SocketAddrV4::new(Ipv4Addr::new(10, 1, 1, 10 + i as u8), 1),
            Trust::Identity(identity),
            300 + u64::from(i),
        );
        world.pipe.deliver_all();
        world.server.pump();
        world.pipe.deliver_all();
        m.pump();
        world.pipe.deliver_all();
        world.server.pump();
        more.push(m);
    }
    assert_eq!(world.server.connections(), 1 + 16);
    let mut late = world.client(OTHER, Trust::Identity(identity), 400);
    assert!(
        world.connect(&mut late, 6),
        "{:?} {:?}",
        late.error(),
        world.server.counters()
    );
    assert!(forged.iter().all(|f| !f.is_connected()));
    drop(forged);
    drop(more);
}

/// A forged unvalidated flood far over the handshake rate, then a newcomer: its Initial gets a Retry like every other, and
/// what it sends back, validated, is admitted; the flood's counts never decide its fate.
#[test]
fn a_forged_flood_over_the_rate_never_drops_a_validated_newcomer() {
    let limits = QuicLimits {
        handshakes_per_second: 2,
        handshakes_per_network_per_second: 1,
        ..QuicLimits::default()
    };
    let mut world = World::new(limits, 110);
    let identity = world.key.identity();
    let mut forged = Vec::new();
    for i in 0..40u16 {
        let ip = Ipv4Addr::new(10, 0, 0, 100 + (i % 50) as u8);
        let f = world.client(
            SocketAddrV4::new(ip, 8_000 + i),
            Trust::Identity(identity),
            111 + u64::from(i),
        );
        world.pipe.deliver_all();
        world.server.pump();
        forged.push(f);
    }
    let c = world.server.counters();
    assert!(c.retried >= 36, "{c:?}");
    assert_eq!(c.ignored, 0, "{c:?}");
    // The newcomer shares the flood's network, in the same second.
    let mut honest = world.client(CLIENT, Trust::Identity(identity), 150);
    assert!(
        world.connect(&mut honest, 6),
        "{:?} {:?}",
        honest.error(),
        world.server.counters()
    );
    assert_eq!(
        world.server.counters().ignored,
        0,
        "no validated arrival was dropped"
    );
    hello(&mut world, &mut honest);
    drop(forged);
}
