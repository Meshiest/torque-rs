//! Hostile datagrams at every engine-mode entry point: the gate (from several source addresses, across epochs and clock
//! jumps), server sessions it opened, a validated server, clients before and after the accept, established pairs on both
//! kinds of link (one with 64 KiB packets), and a session with a full window of packets in flight that gets hostile ack masks.
//! Random bytes up to 64 KiB, mutants of what honest sessions sent, and crafted packets (fragments with huge totals, offsets
//! near the end of a `u32`, ids at the window's edge, every lane, padding, right and wrong tokens). Some sessions are not
//! drained, so their events pile up. Beside them an honest lossy pair runs one tick a round, long enough at a million rounds
//! for its sequences and message ids to wrap many times.
//!
//! Nothing panics; no session holds more events or event bytes than its caps; a gate answer is never larger than its
//! request, and a server the gate opened never sends more, before it is established, than the datagrams it got since it
//! last sent.
//!
//! `TORQUE_FUZZ_ENGINE_HANDSHAKE=<n> cargo test -p torque-net --test engine_fuzz` runs `n` rounds (20,000 by default).

mod engine_common;

use std::collections::HashMap;
use std::net::SocketAddr;

use engine_common::{GAME, KEY, Pair, Pipe, Rng, TICK_MS, accept, connect};
use torque_net::engine::gate::COOKIE_PERIOD_MS;
use torque_net::engine::session::MAX_EVENTS;
use torque_net::engine::{
    Body, Control, CookieValue, Datagram, Delivery, Event, Fragment, Gate, GateConfig, GateOutcome,
    Header, Lane, LaneLimits, LinkFacts, NoStreams, Prefix, Session, SessionConfig, State,
    StreamLane, Unreliable, parse,
};

fn rounds() -> usize {
    std::env::var("TORQUE_FUZZ_ENGINE_HANDSHAKE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

/// What honest sessions send, both ways, on both kinds of link, as seeds for the mutants.
fn corpus() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for (facts, wide) in [
        (LinkFacts::MEMORY, false),
        (LinkFacts::PLAIN, false),
        (LinkFacts::MEMORY, true),
    ] {
        let mut p = Pair::new(facts, 5).lossy(5);
        if wide {
            p.accept.max_packet = u16::MAX;
        }
        p.client_streams.lanes = vec![StreamLane::Moves];
        p.server_streams.lanes = vec![StreamLane::Ghosts];
        assert!(p.connect(400));
        if wide {
            p.up.max = usize::from(u16::MAX);
            p.down.max = usize::from(u16::MAX);
        }
        p.client
            .send(vec![1; 5_000], Delivery::Reliable)
            .expect("queued");
        p.client
            .send(vec![2; 300], Delivery::Unreliable)
            .expect("queued");
        p.server()
            .send(vec![3; 2_000], Delivery::Reliable)
            .expect("queued");
        for _ in 0..40 {
            p.tick();
        }
        out.extend(p.up.log.drain(..).map(|(d, _)| d));
        out.extend(p.down.log.drain(..).map(|(d, _)| d));
    }
    out
}

/// The tokens a crafted packet may carry: right ones some of the time.
#[derive(Clone, Copy)]
struct Tokens {
    nonce: u64,
    session: u64,
    cookie: Option<CookieValue>,
}

fn write_fragment(s: &mut Vec<u8>, id: u16, total: u32, offset: u32, bytes: &[u8]) {
    let _ = Fragment {
        id,
        total,
        offset,
        bytes,
    }
    .write(s);
}

/// One reliable section: fragments with crafted fields, or a whole connect or accept.
fn reliable_section(rng: &mut Rng, s: &mut Vec<u8>, control: bool) {
    if control && rng.percent(30) {
        let message = if rng.percent(50) {
            Control::Connect(Box::new(connect())).encode()
        } else {
            Control::Accept(accept(1_200)).encode()
        }
        .expect("a control message");
        write_fragment(s, 0, message.len() as u32, 0, &message);
        return;
    }
    for _ in 0..1 + rng.below(3) {
        let total = match rng.below(4) {
            0 => u32::MAX,
            1 => 1 << 20,
            2 => rng.below(100) as u32,
            _ => rng.next_u64() as u32,
        };
        let offset = match rng.below(3) {
            0 => total.saturating_sub(2),
            1 => 0,
            _ => rng.next_u64() as u32,
        };
        let id = match rng.below(4) {
            0 => 63,
            1 => 64,
            2 => u16::MAX,
            _ => rng.next_u64() as u16,
        };
        let bytes = rng.some_bytes(40);
        write_fragment(s, id, total, offset, &bytes);
    }
}

/// A packet built by hand: any prefix, a random header and random, often broken, lane sections, some near 64 KiB.
fn crafted(rng: &mut Rng, t: Tokens) -> Vec<u8> {
    let mut d = Vec::new();
    let token = match rng.below(3) {
        0 => t.nonce,
        1 => t.session,
        _ => rng.next_u64(),
    };
    match rng.below(3) {
        0 => Prefix::Opening {
            nonce: t.nonce,
            cookie: t.cookie.filter(|_| rng.percent(80)),
        },
        1 => Prefix::Data,
        _ => Prefix::TokenData { token },
    }
    .write(&mut d);
    let mut sections: [Vec<u8>; 6] = Default::default();
    for lane in Lane::ALL {
        if !rng.percent(40) {
            continue;
        }
        let s = &mut sections[lane as usize - 1];
        match lane {
            Lane::Control | Lane::Rpc => reliable_section(rng, s, lane == Lane::Control),
            Lane::Unreliable => {
                let bytes = if rng.percent(5) {
                    rng.bytes(2_000)
                } else {
                    rng.some_bytes(60)
                };
                let _ = Unreliable {
                    seq: rng.next_u64() as u16,
                    bytes: &bytes,
                }
                .write(s);
            }
            _ => {
                let len = if rng.percent(3) {
                    60_000
                } else {
                    1 + rng.below(50) as usize
                };
                s.extend(rng.bytes(len));
            }
        }
        if rng.percent(10) {
            s.truncate(rng.below(s.len() as u64 + 1) as usize);
        }
    }
    let header = Header {
        seq: rng.next_u64() as u16,
        ack: rng.next_u64() as u16,
        mask: rng.next_u64() as u32,
    };
    let pad = if rng.percent(20) { 1_200 } else { 0 };
    let _ = Body {
        header,
        sections: sections.each_ref().map(Vec::as_slice),
    }
    .write(&mut d, pad);
    if rng.percent(5) {
        d.push(rng.next_u64() as u8);
    }
    d
}

/// An opening that carries a whole connect and nothing else, as an honest client's first packet does.
fn clean_connect(rng: &mut Rng, cookie: Option<CookieValue>, nonce: u64) -> Vec<u8> {
    let message = Control::Connect(Box::new(connect()))
        .encode()
        .expect("a connect");
    let mut control = Vec::new();
    write_fragment(&mut control, 0, message.len() as u32, 0, &message);
    let mut d = Vec::new();
    Prefix::Opening { nonce, cookie }.write(&mut d);
    let header = Header {
        seq: rng.next_u64() as u16,
        ack: 0,
        mask: 0,
    };
    let _ = Body {
        header,
        sections: [&control, &[], &[], &[], &[], &[]],
    }
    .write(&mut d, 1_200);
    d
}

fn mutant(rng: &mut Rng, corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut d = corpus[rng.below(corpus.len() as u64) as usize].clone();
    for _ in 0..1 + rng.below(4) {
        if d.is_empty() {
            break;
        }
        let at = rng.below(d.len() as u64) as usize;
        match rng.below(5) {
            0 => d[at] ^= 1 << rng.below(8),
            1 => d[at] = rng.next_u64() as u8,
            2 => d.truncate(at),
            3 => d.insert(at, rng.next_u64() as u8),
            _ => {
                // A length field set to an extreme.
                let v = if rng.percent(50) {
                    [0xff, 0xff]
                } else {
                    [0, 0]
                };
                d.splice(at..(at + 2).min(d.len()), v);
            }
        }
    }
    d
}

fn hostile(rng: &mut Rng, corpus: &[Vec<u8>], t: Tokens) -> Vec<u8> {
    match rng.below(12) {
        0 => rng.some_bytes(1_400),
        1 => {
            let n = rng.below(u64::from(u16::MAX) + 200) as usize;
            rng.bytes(n)
        }
        2..=5 => mutant(rng, corpus),
        6 => clean_connect(rng, t.cookie, t.nonce),
        _ => crafted(rng, t),
    }
}

/// Takes the session's events (or, on rounds the host "forgets", leaves them) and checks the caps.
fn drain(s: &mut Session, rng: &mut Rng) -> usize {
    assert!(
        s.held_event_bytes() <= LaneLimits::default().event_bytes,
        "{} event bytes",
        s.held_event_bytes()
    );
    if rng.percent(10) {
        return 0;
    }
    let mut n = 0;
    while let Some(e) = s.next_event() {
        if let Event::ConnectRequest(_) = e {
            let _ = s.accept(accept(1_200));
        }
        n += 1;
    }
    assert!(n <= MAX_EVENTS + 1, "{n} events");
    n
}

/// A server the gate opened, and the most it may answer before it is established.
struct Gated {
    session: Session,
    owed: usize,
}

/// Everything under attack, and the round's clock.
struct Fuzz {
    rng: Rng,
    corpus: Vec<Vec<u8>>,
    now: u32,
    gate: Gate,
    /// The cookie the gate gave each source address, with the nonce it is for.
    cookies: HashMap<SocketAddr, (u64, CookieValue)>,
    gated: HashMap<SocketAddr, Gated>,
    validated_server: Session,
    client_hello: Session,
    client_validated: Session,
    memory: Pair,
    plain: Pair,
    flooded: Pair,
    honest: Pair,
    honest_sent: u32,
    honest_got: u32,
    opened: usize,
    connects: usize,
    events: usize,
}

const ADDRS: [([u8; 4], u16); 4] = [
    ([10, 0, 0, 7], 4000),
    ([10, 0, 0, 7], 4001),
    ([10, 0, 0, 8], 4000),
    ([10, 0, 1, 9], 9),
];
const NONCE: u64 = 42;

fn addr(i: usize) -> SocketAddr {
    let (ip, port) = ADDRS[i % ADDRS.len()];
    SocketAddr::from((ip, port))
}

fn validated_server(now: u32) -> Session {
    Session::server(SessionConfig::new(GAME), LinkFacts::MEMORY, now).expect("a server")
}

fn client(facts: LinkFacts, now: u32) -> Session {
    Session::client(SessionConfig::new(GAME), facts, connect(), NONCE, now).expect("a client")
}

fn wide_pair() -> Pair {
    let mut p = Pair::new(LinkFacts::MEMORY, 3);
    p.accept.max_packet = u16::MAX;
    assert!(p.connect(200));
    p.up.max = usize::from(u16::MAX);
    p.down.max = usize::from(u16::MAX);
    p
}

/// A pair whose client has a full window of packets in flight: the server's packets are all lost for a while.
fn flooded_pair() -> Pair {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.client_streams.lanes = vec![StreamLane::Moves];
    p.down.loss_percent = 100;
    for i in 0..70u8 {
        let _ = p.client.send(vec![i; 700], Delivery::Reliable);
        p.tick();
    }
    p
}

impl Fuzz {
    fn new() -> Fuzz {
        let now = 10_000;
        let mut honest = Pair::new(LinkFacts::PLAIN, 99).lossy(2);
        assert!(honest.connect(400));
        Fuzz {
            rng: Rng::new(0xf022),
            corpus: corpus(),
            now,
            gate: Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY),
            cookies: HashMap::new(),
            gated: HashMap::new(),
            validated_server: validated_server(now),
            client_hello: client(LinkFacts::PLAIN, now),
            client_validated: client(LinkFacts::MEMORY, now),
            memory: wide_pair(),
            plain: Pair::connected(LinkFacts::PLAIN),
            flooded: flooded_pair(),
            honest,
            honest_sent: 0,
            honest_got: 0,
            opened: 0,
            connects: 0,
            events: 0,
        }
    }

    /// The clock moves a quarter tick a round, and now and then jumps: on by epochs, back, or to just before a wrap.
    fn clock(&mut self) {
        self.now = match self.rng.below(5_000) {
            0 => self.now.wrapping_add(2 * COOKIE_PERIOD_MS),
            1 => self.now.wrapping_sub(self.rng.below(1 << 30) as u32),
            2 => u32::MAX - 2_000,
            _ => self.now.wrapping_add(TICK_MS / 4),
        };
    }

    fn tokens(&mut self, at: SocketAddr) -> Tokens {
        Tokens {
            nonce: NONCE,
            session: accept(1_200).session,
            cookie: self.cookies.get(&at).map(|&(_, c)| c),
        }
    }

    /// The gate from one of several addresses: hellos to get cookies, then hostile datagrams carrying them.
    fn gate(&mut self, d: &[u8], at: SocketAddr) {
        if self.rng.percent(5) {
            let mut c = client(LinkFacts::PLAIN, self.now);
            let mut out = Pipe::new(LinkFacts::PLAIN, 1);
            c.poll(&mut out, &mut NoStreams, self.now);
            if let GateOutcome::Reply(r) = self.gate.receive(at, &out.log[0].0, self.now)
                && let Ok(Datagram::Cookie(cookie)) = parse(&r)
            {
                self.cookies.insert(at, (NONCE, cookie.cookie));
            }
        }
        match self.gate.receive(at, d, self.now) {
            GateOutcome::Drop => {}
            GateOutcome::Reply(r) => assert!(
                r.len() <= d.len(),
                "the gate answered {} with {}",
                d.len(),
                r.len()
            ),
            GateOutcome::Open { session, .. } => {
                self.opened += 1;
                self.gate.settle(at);
                self.gated.insert(
                    at,
                    Gated {
                        session: *session,
                        owed: d.len(),
                    },
                );
            }
        }
    }

    /// The servers the gate opened: before they are established, nothing larger or more than what they got since they last
    /// sent.
    fn gated(&mut self, d: &[u8], at: SocketAddr) {
        let Some(g) = self.gated.get_mut(&at) else {
            return;
        };
        let established = g.session.state() == State::Established;
        g.session.receive(d, self.now);
        g.owed = g.owed.max(d.len());
        self.events += drain(&mut g.session, &mut self.rng);
        let mut out = Pipe::new(LinkFacts::PLAIN, 1);
        g.session.poll(&mut out, &mut NoStreams, self.now);
        if !established && g.session.state() != State::Established {
            assert!(out.log.len() <= 1);
            for (r, _) in &out.log {
                assert!(
                    r.len() <= g.owed,
                    "a gated session answered {} with {}",
                    g.owed,
                    r.len()
                );
                g.owed = 0;
            }
        }
        if g.session.state() == State::Closed {
            self.gated.remove(&at);
        }
    }

    /// A validated server and clients before the accept, each recreated once closed.
    fn lone_sessions(&mut self, d: &[u8]) {
        let now = self.now;
        let mut sink = Pipe::new(LinkFacts::MEMORY, 9);
        for s in [
            &mut self.validated_server,
            &mut self.client_hello,
            &mut self.client_validated,
        ] {
            s.receive(d, now);
            if s.state() == State::Deciding {
                self.connects += 1;
            }
            self.events += drain(s, &mut self.rng);
            s.poll(&mut sink, &mut NoStreams, now);
        }
        if self.validated_server.state() == State::Closed {
            self.validated_server = validated_server(now);
        }
        if self.client_hello.state() == State::Closed {
            self.client_hello = client(LinkFacts::PLAIN, now);
        }
        if self.client_validated.state() == State::Closed {
            self.client_validated = client(LinkFacts::MEMORY, now);
        }
    }

    /// Established pairs on both kinds of link: the datagram goes to one side, then the pair runs a tick.
    fn pairs(&mut self, d: &[u8], round: usize) {
        for p in [&mut self.memory, &mut self.plain] {
            let at = p.now;
            if self.rng.percent(50) {
                p.client.receive(d, at);
            } else {
                p.server().receive(d, at);
            }
            p.tick();
            self.events += p.client_events.len() + p.server_events.len();
            p.client_events.clear();
            p.server_events.clear();
        }
        if round.is_multiple_of(256) || !self.memory.established() {
            self.memory = wide_pair();
        }
        if round.is_multiple_of(256) || !self.plain.established() {
            self.plain = Pair::connected(LinkFacts::PLAIN);
        }
    }

    /// Hostile acks at a client with a full window in flight: random sequences, acks and masks on well-formed packets.
    fn flooded(&mut self, round: usize) {
        let mut d = Vec::new();
        Prefix::Data.write(&mut d);
        let header = Header {
            seq: self.rng.next_u64() as u16,
            ack: self
                .flooded
                .client_seq_hint()
                .wrapping_sub(self.rng.below(80) as u16),
            mask: self.rng.next_u64() as u32,
        };
        let _ = Body {
            header,
            sections: [&[], &[], &[], &[], &[], &[]],
        }
        .write(&mut d, 0);
        let at = self.flooded.now;
        self.flooded.client.receive(&d, at);
        let _ = self.flooded.client.send(vec![9; 500], Delivery::Reliable);
        self.flooded.tick();
        self.flooded.client_events.clear();
        self.flooded.server_events.clear();
        let last = self.flooded.up.log.pop();
        self.flooded.up.log.clear();
        self.flooded.up.log.extend(last);
        self.flooded.down.log.clear();
        if round.is_multiple_of(128) || self.flooded.client.state() == State::Closed {
            self.flooded = flooded_pair();
        }
    }

    /// The honest pair: one small reliable message a round, all of them arriving in order.
    fn honest(&mut self) {
        if self.honest.client.unacked_reliable() < 32 << 10 {
            self.honest
                .client
                .send(self.honest_sent.to_le_bytes().to_vec(), Delivery::Reliable)
                .expect("queued");
            self.honest_sent += 1;
        }
        self.honest.tick();
        for e in self.honest.server_events.drain(..) {
            if let Event::Message { data, .. } = e {
                assert_eq!(
                    data,
                    self.honest_got.to_le_bytes(),
                    "the honest pair delivers in order"
                );
                self.honest_got += 1;
            }
        }
        self.honest.client_events.clear();
        self.honest.up.log.clear();
        self.honest.down.log.clear();
        assert!(self.honest.established(), "the honest pair stays up");
    }

    fn round(&mut self, round: usize) {
        self.clock();
        let at = addr(self.rng.below(ADDRS.len() as u64) as usize);
        let tokens = self.tokens(at);
        let d = hostile(&mut self.rng, &self.corpus, tokens);
        self.gate(&d, at);
        self.gated(&d, at);
        self.lone_sessions(&d);
        self.pairs(&d, round);
        self.flooded(round);
        self.honest();
    }
}

trait SeqHint {
    /// About where the client's sequence numbers are, from the last packet it sent.
    fn client_seq_hint(&self) -> u16;
}

impl SeqHint for Pair {
    fn client_seq_hint(&self) -> u16 {
        self.up
            .log
            .last()
            .and_then(|(d, _)| match parse(d) {
                Ok(Datagram::Data { body, .. }) => Body::read(body).ok().map(|b| b.header.seq),
                _ => None,
            })
            .unwrap_or(0)
    }
}

#[test]
fn hostile_datagrams_never_panic_and_never_amplify() {
    let mut fuzz = Fuzz::new();
    let rounds = rounds();
    for round in 0..rounds {
        fuzz.round(round);
    }
    // The hostile input reached past the cookie and the connect, not only the outer parse.
    let deep = rounds / 1_000;
    assert!(
        fuzz.opened > deep && fuzz.connects > deep && fuzz.events > deep,
        "{} opened, {} connects, {} events",
        fuzz.opened,
        fuzz.connects,
        fuzz.events
    );
    assert!(
        fuzz.honest_got as usize > rounds / 2,
        "{} honest messages",
        fuzz.honest_got
    );
}
