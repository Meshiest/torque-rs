//! Hostile datagrams at both ends of the QUIC link, over the in-process pipe under the fake clock: bit flips in both
//! directions, random bytes, truncations, replays, Initial-shaped junk, and floods of real handshakes from one network that
//! are never finished. Honest clients keep connecting, exchanging datagrams and reconnecting beside them.
//!
//! Nothing panics; no bit-flipped datagram reaches either host as an application datagram; before an address is validated
//! the server never sends it more than three times what it received from it plus one datagram (quinn-proto's bound); the
//! flood makes the server answer with Retry; the connections and the inbox stay under their caps.
//!
//! `TORQUE_FUZZ_QUIC=<n> cargo test -p torque-net-quic --test quic_fuzz` runs `n` rounds (20,000 by default).

mod common;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use common::{Client, Rng, SERVER, World, client_recv};
use torque_client::net::Datagrams;
use torque_net_quic::{Flight, Io, QuicLimits, Trust};
use torque_server::net::transport::Transport;

fn rounds() -> usize {
    std::env::var("TORQUE_FUZZ_QUIC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

const HONEST: usize = 3;
/// The most datagrams kept for replays.
const CORPUS: usize = 256;

struct Honest {
    addr: SocketAddrV4,
    client: Client,
    /// Payloads sent and not yet seen at the server.
    sent: HashSet<Vec<u8>>,
    /// Payloads the server sent it and it has not yet seen.
    expected: HashSet<Vec<u8>>,
    stand_in: Option<SocketAddr>,
}

struct Fuzz {
    world: World,
    rng: Rng,
    honest: Vec<Honest>,
    corpus: VecDeque<Flight>,
    /// Bytes the server sent to, and received from, each address that never finished a handshake.
    flood_sent: HashMap<SocketAddr, u64>,
    flood_received: HashMap<SocketAddr, u64>,
    flooders: Vec<Client>,
    flipped: u64,
    payload: u64,
    reconnects: u64,
}

impl Fuzz {
    fn new(seed: u64) -> Fuzz {
        let limits = QuicLimits {
            max_connections: 8,
            connections_per_ip: 4,
            handshakes_per_second: 6,
            handshakes_per_network_per_second: 3,
            handshakes_in_progress: 16,
            handshake_timeout: Duration::from_secs(3),
            inbox_per_connection: 16,
            datagram_receive_buffer: 8 << 10,
            datagram_send_buffer: 8 << 10,
            ..QuicLimits::default()
        };
        let world = World::new(limits, seed);
        let mut fuzz = Fuzz {
            world,
            rng: Rng::new(seed ^ 0x5151),
            honest: Vec::new(),
            corpus: VecDeque::new(),
            flood_sent: HashMap::new(),
            flood_received: HashMap::new(),
            flooders: Vec::new(),
            flipped: 0,
            payload: 0,
            reconnects: 0,
        };
        for i in 0..HONEST {
            let addr = SocketAddrV4::new(Ipv4Addr::new(10, 0, 1, 10 + i as u8), 4_000);
            let honest = fuzz.fresh(addr);
            fuzz.honest.push(honest);
        }
        fuzz
    }

    fn fresh(&mut self, addr: SocketAddrV4) -> Honest {
        let seed = self.rng.next_u64();
        let client = self
            .world
            .client(addr, Trust::Identity(self.world.key.identity()), seed);
        Honest {
            addr,
            client,
            sent: HashSet::new(),
            expected: HashSet::new(),
            stand_in: None,
        }
    }

    /// Counts a datagram's bytes between the server and an address that never finished a handshake.
    fn count(&mut self, f: &Flight) {
        if f.from == SocketAddr::V4(SERVER) && self.flood_received.contains_key(&f.to) {
            *self.flood_sent.entry(f.to).or_insert(0) += f.data.len() as u64;
        }
        if f.to == SocketAddr::V4(SERVER) && self.flood_received.contains_key(&f.from) {
            *self.flood_received.entry(f.from).or_insert(0) += f.data.len() as u64;
        }
    }

    /// Takes what is in flight, remembers some for replays and counts the server's bytes toward unfinished addresses.
    fn take(&mut self) -> Vec<Flight> {
        let flights = self.world.pipe.take_in_flight();
        for f in &flights {
            self.count(f);
            if self.corpus.len() >= CORPUS {
                self.corpus.pop_front();
            }
            self.corpus.push_back(f.clone());
        }
        flights
    }

    fn deliver(&mut self, flights: Vec<Flight>) {
        for f in flights {
            self.world.pipe.deliver(f);
        }
    }

    /// Delivers everything in flight as it is, then runs both ends.
    fn settle(&mut self) {
        for _ in 0..3 {
            let flights = self.take();
            self.deliver(flights);
            self.pump_all();
        }
    }

    fn pump_all(&mut self) {
        self.world.server.pump();
        for h in &mut self.honest {
            h.client.pump();
        }
        for f in &mut self.flooders {
            f.pump();
        }
    }

    /// Reads what reached each end and checks it against what was sent.
    fn collect(&mut self) {
        for (from, data) in self.world.server_recv() {
            let Some(h) = self.honest.iter_mut().find(|h| h.sent.contains(&data)) else {
                panic!("the server got a datagram nobody sent: {data:?} from {from}");
            };
            h.sent.remove(&data);
            h.stand_in = Some(from);
        }
        for h in &mut self.honest {
            for data in client_recv(&mut h.client) {
                assert!(
                    h.expected.remove(&data),
                    "the client got {data:?}, never sent"
                );
            }
        }
    }

    /// An honest round: each connected client sends a payload, the server answers each client it knows.
    fn honest_round(&mut self) {
        for i in 0..self.honest.len() {
            let h = &mut self.honest[i];
            if !h.client.is_connected() {
                if h.client.error().is_some() {
                    // Back on a new port, as a client that opens a new socket does.
                    self.reconnects += 1;
                    let addr =
                        SocketAddrV4::new(*h.addr.ip(), 5_000 + self.reconnects as u16 % 50_000);
                    let mut fresh = self.fresh(addr);
                    // What the old connection sent may still arrive.
                    fresh.sent = std::mem::take(&mut self.honest[i].sent);
                    self.honest[i] = fresh;
                }
                continue;
            }
            self.payload += 1;
            let mut data = self.payload.to_le_bytes().to_vec();
            let extra = self.rng.below(900) as usize;
            data.extend(self.rng.bytes(extra));
            if h.client.send_to(SERVER, &data) {
                h.sent.insert(data);
            }
            if let Some(stand_in) = h.stand_in {
                self.payload += 1;
                let mut back = self.payload.to_le_bytes().to_vec();
                let extra = self.rng.below(900) as usize;
                back.extend(self.rng.bytes(extra));
                if self.world.server.send_to(stand_in, &back) {
                    h.expected.insert(back);
                }
            }
        }
    }

    /// Every datagram in flight gets 1 to 3 bits flipped. A datagram that is one short-header packet (every one past the
    /// handshake) then carries nothing: nothing may arrive as an application datagram. A long-header datagram may coalesce
    /// several packets, and the ones the flips missed still count (RFC 9000 section 12.2), so a round with one of those in
    /// flight checks only that whatever arrives is what an honest end sent.
    fn flipped_round(&mut self) {
        // What already arrived is read first, then the honest ends send: only flipped datagrams are delivered below.
        self.collect();
        self.honest_round();
        let before_server = self.world.server.counters().datagrams_in;
        let mut flights = self.take();
        let mut any_long = false;
        for f in &mut flights {
            let Some(&first) = f.data.first() else {
                continue;
            };
            any_long |= first & 0x80 != 0;
            // One bit in each of 1 to 3 neighboring bytes: two flips can never undo each other.
            let len = f.data.len();
            let start = self.rng.below(len as u64) as usize;
            for i in 0..(1 + self.rng.below(3) as usize).min(len) {
                f.data[(start + i) % len] ^= 1 << self.rng.below(8);
            }
            if first & 0x80 == 0 {
                self.flipped += 1;
            }
        }
        self.deliver(flights);
        self.pump_all();
        if any_long {
            self.collect();
            return;
        }
        assert_eq!(
            self.world.server.counters().datagrams_in,
            before_server,
            "a flipped datagram reached the server as an application datagram"
        );
        for h in &mut self.honest {
            let got = client_recv(&mut h.client);
            assert!(
                got.is_empty(),
                "a flipped datagram reached a client: {got:?}"
            );
        }
    }

    fn truncated_round(&mut self) {
        let mut flights = self.take();
        for f in &mut flights {
            let keep = self.rng.below(f.data.len() as u64 + 1) as usize;
            f.data.truncate(keep);
        }
        self.deliver(flights);
        self.pump_all();
    }

    fn replay_round(&mut self) {
        let n = 1 + self.rng.below(16) as usize;
        for _ in 0..n {
            if self.corpus.is_empty() {
                break;
            }
            let at = self.rng.below(self.corpus.len() as u64) as usize;
            let f = self.corpus[at].clone();
            // A replayed Initial is bytes the server really received: its budget toward that address grows with it. A
            // replayed answer of the server is the test's doing, not the server's.
            if f.to == SocketAddr::V4(SERVER) {
                self.count(&f);
            }
            self.world.pipe.deliver(f);
        }
        self.pump_all();
    }

    fn random_address(&mut self) -> SocketAddr {
        if self.rng.percent(30) {
            let h = &self.honest[self.rng.below(self.honest.len() as u64) as usize];
            return SocketAddr::V4(h.addr);
        }
        SocketAddr::from((
            [10, 7, self.rng.below(256) as u8, self.rng.below(256) as u8],
            1 + self.rng.below(60_000) as u16,
        ))
    }

    /// Random bytes and Initial-shaped junk at the server from anywhere, and random bytes at the clients from the server's
    /// address.
    fn junk_round(&mut self) {
        let n = 1 + self.rng.below(8) as usize;
        for _ in 0..n {
            let from = self.random_address();
            let data = if self.rng.percent(50) {
                let mut d = vec![0xc0 | self.rng.below(16) as u8, 0, 0, 0, 1, 8];
                d.extend(self.rng.bytes(8));
                d.push(8);
                d.extend(self.rng.bytes(8));
                d.push(0);
                d.extend([0x44, 0x80]);
                d.extend(self.rng.bytes(1_180));
                d
            } else {
                let n = self.rng.below(1_500) as usize;
                self.rng.bytes(n)
            };
            self.world.pipe.deliver(Flight {
                from,
                to: SocketAddr::V4(SERVER),
                data,
            });
        }
        for i in 0..self.honest.len() {
            let n = self.rng.below(1_500) as usize;
            let data = self.rng.bytes(n);
            self.world.pipe.deliver(Flight {
                from: SocketAddr::V4(SERVER),
                to: SocketAddr::V4(self.honest[i].addr),
                data,
            });
        }
        self.pump_all();
    }

    /// Real handshakes from one /24, never finished: their first Initials cross, the rest is dropped.
    fn flood_round(&mut self) {
        let n = 4 + self.rng.below(12) as usize;
        for _ in 0..n {
            let addr = SocketAddrV4::new(
                Ipv4Addr::new(10, 9, 9, 1 + self.rng.below(250) as u8),
                1 + self.rng.below(60_000) as u16,
            );
            if self.world.pipe.in_flight() > 4_096 {
                break;
            }
            let seed = self.rng.next_u64();
            let mut io = self.world.pipe.end();
            if io.bind(IpAddr::V4(*addr.ip()), addr.port()).is_err() {
                continue;
            }
            let Ok(client) = torque_net_quic::QuicClient::connect(
                io,
                self.world.clock.clone(),
                SERVER,
                Trust::Identity(self.world.key.identity()),
                &self.world.limits,
                Some(Rng::new(seed).seed32()),
            ) else {
                continue;
            };
            self.flood_received.entry(SocketAddr::V4(addr)).or_insert(0);
            self.flooders.push(client);
        }
        let flights = self.take();
        self.deliver(flights);
        self.world.server.pump();
        // The flooders' sockets go away with them: what the server sends them lands nowhere, and the Retry answers are
        // never acted on.
        self.flooders.clear();
        let flights = self.take();
        self.deliver(flights);
    }

    fn check_bounds(&mut self) {
        let limits = self.world.server.limits().clone();
        assert!(self.world.server.established() <= limits.max_connections);
        assert!(
            self.world.server.connections()
                <= limits.max_connections + limits.handshakes_in_progress
        );
        assert!(
            self.world.server.inbox_len()
                <= limits.inbox_per_connection
                    * (limits.max_connections + limits.handshakes_in_progress)
        );
        // quinn-proto starts a datagram while any of the three-times budget is left, so one datagram may overshoot it
        // (its issue 1082); past that the limit holds.
        for (addr, &received) in &self.flood_received {
            let sent = self.flood_sent.get(addr).copied().unwrap_or(0);
            assert!(
                sent < 3 * received + 1_200,
                "the server sent {sent} bytes to the unvalidated {addr} that sent it {received}"
            );
        }
    }
}

#[test]
fn hostile_datagrams_never_panic_never_reach_the_hosts_and_the_server_stays_bounded() {
    let rounds = rounds();
    let mut fuzz = Fuzz::new(0xf00d);
    let mut last = (0, 0);
    for round in 0..rounds {
        match fuzz.rng.below(10) {
            0..=3 => fuzz.honest_round(),
            4 | 5 => fuzz.flipped_round(),
            6 => fuzz.truncated_round(),
            7 => fuzz.replay_round(),
            8 => fuzz.junk_round(),
            _ => fuzz.flood_round(),
        }
        if fuzz.rng.percent(60) {
            fuzz.settle();
            fuzz.collect();
        }
        let step = if fuzz.rng.percent(1) {
            Duration::from_secs(40)
        } else {
            Duration::from_millis(fuzz.rng.below(60))
        };
        fuzz.world.clock.advance(step);
        fuzz.check_bounds();
        if round % 2_000 == 1_999 {
            // The honest side is still alive beside the hostile one: handshakes keep finishing and datagrams keep arriving.
            let c = fuzz.world.server.counters();
            assert!(
                c.established > last.0 && c.datagrams_in > last.1,
                "round {round}: nothing honest happened since round {}: {c:?}",
                round - 1_999
            );
            last = (c.established, c.datagrams_in);
        }
    }
    fuzz.settle();
    fuzz.collect();
    let c = fuzz.world.server.counters();
    assert!(
        c.retried > 0,
        "the flood never made the server ask for validation: {c:?}"
    );
    assert!(c.established >= HONEST as u64, "{c:?}");
    assert!(c.datagrams_in > 0 && c.datagrams_out > 0, "{c:?}");
    if rounds >= 20_000 {
        assert!(
            fuzz.flipped >= 10_000,
            "only {} datagrams were flipped",
            fuzz.flipped
        );
    }
    eprintln!(
        "quic fuzz: {rounds} rounds, {} datagrams flipped, {} reconnects, {} flood addresses; {c:?}",
        fuzz.flipped,
        fuzz.reconnects,
        fuzz.flood_received.len()
    );
}
