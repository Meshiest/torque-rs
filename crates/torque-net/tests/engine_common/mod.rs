//! A client and a server session over two one-way test links (loss, a datagram size that may change every tick, a "no room"
//! switch), with the gate in front of the server on a plain link. Every datagram sent is logged with the size limit of its tick.

#![allow(dead_code, reason = "each test file uses a different part")]

use std::collections::VecDeque;
use std::net::SocketAddr;

use torque_net::engine::Kind;
use torque_net::engine::{
    Accept, Connect, Event, Gate, GateConfig, GateOutcome, Link, LinkFacts, NoRoom, NoStreams,
    Session, SessionConfig, State, StreamLane, StreamSource, TableDigests, Widths,
};

/// The seeded generator the engine tests draw from: `Rng::star`, whose bits `engine_golden_v2` is pinned to.
pub use torque_testkit::Rng;

pub const GAME: &str = "fixture";
pub const KEY: [u8; 16] = [7; 16];
pub const TICK_MS: u32 = 32;

pub fn client_addr() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 7], 4000))
}

/// One direction of a link.
pub struct Pipe {
    pub facts: LinkFacts,
    pub max: usize,
    pub full: bool,
    pub loss_percent: u64,
    pub rng: Rng,
    pub queue: VecDeque<Vec<u8>>,
    /// Every datagram sent: its bytes and the size limit when it was sent.
    pub log: Vec<(Vec<u8>, usize)>,
}

impl Pipe {
    pub fn new(facts: LinkFacts, seed: u64) -> Pipe {
        Pipe {
            facts,
            max: 1_200,
            full: false,
            loss_percent: 0,
            rng: Rng::star(seed),
            queue: VecDeque::new(),
            log: Vec::new(),
        }
    }

    /// Sends what a host sends by itself (a gate's answer), through the same loss.
    pub fn raw(&mut self, datagram: Vec<u8>) {
        self.log.push((datagram.clone(), self.max));
        if !self.rng.chance(self.loss_percent) {
            self.queue.push_back(datagram);
        }
    }
}

impl Link for Pipe {
    fn max_datagram(&self) -> usize {
        self.max
    }

    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        if self.full {
            return Err(NoRoom);
        }
        self.raw(datagram.to_vec());
        Ok(())
    }

    fn facts(&self) -> LinkFacts {
        self.facts
    }
}

/// A stream source that fills each lane it is set to fill with as much as the room allows, and remembers what it wrote.
#[derive(Default)]
pub struct Flood {
    pub lanes: Vec<StreamLane>,
    pub written: Vec<(u16, StreamLane, usize)>,
    pub unsent: Vec<u16>,
}

impl StreamSource for Flood {
    fn write(&mut self, lane: StreamLane, seq: u16, room: usize, out: &mut Vec<u8>) {
        if self.lanes.contains(&lane) {
            out.resize(out.len() + room, lane as u8 | 0x40);
            self.written.push((seq, lane, room));
        }
    }

    fn unsent(&mut self, seq: u16) {
        self.unsent.push(seq);
    }
}

pub fn accept(max_packet: u16) -> Accept {
    Accept {
        session: 0x5e55_1011_cafe_f00d,
        max_packet,
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

pub fn connect() -> Connect {
    Connect {
        game: GAME.to_owned(),
        manifest: vec![1, 2, 3, 4],
        ticket: vec![],
        mods: vec![9],
        password: "pw".to_owned(),
        args: vec!["Player".to_owned(), "0".to_owned()],
        ..Connect::default()
    }
}

/// A client, the server (made by the gate on a plain link), and the two directions between them.
pub struct Pair {
    pub client: Session,
    pub server: Option<Session>,
    pub gate: Option<Gate>,
    pub up: Pipe,
    pub down: Pipe,
    pub now: u32,
    pub client_events: Vec<Event>,
    pub server_events: Vec<Event>,
    pub accept: Accept,
    pub client_streams: Flood,
    pub server_streams: Flood,
}

impl Pair {
    /// A pair over a link with `facts`; nothing is sent until the first tick.
    pub fn new(facts: LinkFacts, seed: u64) -> Pair {
        Pair::with(facts, seed, SessionConfig::new(GAME), connect())
    }

    pub fn with(facts: LinkFacts, seed: u64, config: SessionConfig, connect: Connect) -> Pair {
        let now = 1_000;
        let client = Session::client(
            config.clone(),
            facts,
            connect,
            0x1111_2222_3333_4444 ^ seed,
            now,
        )
        .expect("a client");
        let (server, gate) = if facts.validated {
            (
                Some(Session::server(config, facts, now).expect("a server")),
                None,
            )
        } else {
            (None, Some(Gate::new(GateConfig::new(config), KEY)))
        };
        Pair {
            client,
            server,
            gate,
            up: Pipe::new(facts, seed.wrapping_add(1)),
            down: Pipe::new(facts, seed.wrapping_add(2)),
            now,
            client_events: Vec::new(),
            server_events: Vec::new(),
            accept: accept(1_200),
            client_streams: Flood::default(),
            server_streams: Flood::default(),
        }
    }

    pub fn lossy(mut self, percent: u64) -> Pair {
        self.up.loss_percent = percent;
        self.down.loss_percent = percent;
        self
    }

    pub fn server(&mut self) -> &mut Session {
        self.server.as_mut().expect("a server session")
    }

    /// Delivers what is on the wires, answers connect requests with the pair's accept, then polls both sides once.
    pub fn tick(&mut self) {
        while let Some(d) = self.up.queue.pop_front() {
            match (&mut self.server, &mut self.gate) {
                (Some(s), _) => s.receive(&d, self.now),
                (None, Some(g)) => match g.receive(client_addr(), &d, self.now) {
                    GateOutcome::Drop => {}
                    GateOutcome::Reply(r) => self.down.raw(r),
                    GateOutcome::Open { session, .. } => self.server = Some(*session),
                },
                (None, None) => unreachable!("a pair has a server or a gate"),
            }
        }
        while let Some(d) = self.down.queue.pop_front() {
            self.client.receive(&d, self.now);
        }
        while let Some(e) = self.client.next_event() {
            self.client_events.push(e);
        }
        if let Some(server) = &mut self.server {
            while let Some(e) = server.next_event() {
                if matches!(e, Event::ConnectRequest(_)) {
                    server.accept(self.accept).expect("an accept");
                }
                self.server_events.push(e);
            }
            if server.state() == State::Established
                && let Some(g) = &mut self.gate
            {
                g.settle(client_addr());
            }
        }
        self.client
            .poll(&mut self.up, &mut self.client_streams, self.now);
        if let Some(server) = &mut self.server {
            server.poll(&mut self.down, &mut self.server_streams, self.now);
        }
        self.now = self.now.wrapping_add(TICK_MS);
    }

    pub fn established(&self) -> bool {
        self.client.state() == State::Established
            && self
                .server
                .as_ref()
                .is_some_and(|s| s.state() == State::Established)
    }

    /// Ticks until both sides are established (at most `ticks`).
    pub fn connect(&mut self, ticks: u32) -> bool {
        for _ in 0..ticks {
            if self.established() {
                return true;
            }
            self.tick();
        }
        self.established()
    }

    /// Ticks a lossless established pair until both are established and nothing is on the wires.
    pub fn connected(facts: LinkFacts) -> Pair {
        let mut p = Pair::new(facts, 0);
        assert!(p.connect(200), "the pair connects");
        p.tick();
        p.tick();
        p
    }

    pub fn messages(events: &[Event]) -> Vec<&Vec<u8>> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Message { data, .. } => Some(data),
                _ => None,
            })
            .collect()
    }
}

/// The kind byte of a datagram.
pub fn kind(datagram: &[u8]) -> Option<Kind> {
    [
        Kind::Hello,
        Kind::Cookie,
        Kind::Opening,
        Kind::Data,
        Kind::Reject,
        Kind::Disconnect,
    ]
    .into_iter()
    .find(|k| datagram.get(1) == Some(&(*k as u8)))
}

pub fn no_streams() -> NoStreams {
    NoStreams
}
