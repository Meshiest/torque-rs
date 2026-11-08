//! The front door of a plain link's server (plain UDP): what it does with a datagram from an address that has no session yet.
//!
//! It keeps nothing for a `Hello`: the answer is a cookie, a keyed digest of the address, port, the client's nonce and the time
//! epoch, which the client sends back in every `Opening`. Only an `Opening` whose cookie verifies opens a session, and the
//! pending sessions are bounded in all and per source IP address. A `Hello` shorter than [`HELLO_BYTES`] is not answered, so
//! no answer (a cookie, or a reject naming the game or the version) is larger than the datagram it answers, and answers are
//! rate-limited per source and in all. Every other datagram from an address without a session is dropped unanswered.
//!
//! An `Opening` opens at most one session: the (address, nonce) pairs that opened one are kept for as long as their cookie
//! could verify, so a copy replayed after the session settled is dropped. The gate's clock only goes forward: a step back is
//! counted as no time, so cookies keep expiring when the host's clock jumps.
//!
//! The gate bounds sessions that are not yet established. How many established sessions one address or network may hold
//! is the host's to cap (it owns the session table); see the module docs of [`super`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};

use super::framing::{self, COOKIE_BYTES, Cookie, CookieValue, Datagram, Reject, RejectCode};
use super::session::{CONNECT_TIMEOUT_MS, Session, SessionConfig};
use super::{ENGINE_FRAMING, HELLO_BYTES};
use crate::handshake::{RateWindow, SourceLimiter, siphash24};

/// How long a cookie stays in one epoch: one verifies in its epoch and the next, so it lives between one and two periods.
pub const COOKIE_PERIOD_MS: u32 = 30_000;

/// What the cookie digests start with, so they never equal a digest of the same key made for something else.
const COOKIE_DOMAIN: &[u8] = b"engine-cookie";

/// The cookie of `from` and `nonce` in `epoch`: two keyed digests that differ in the byte after the domain.
#[must_use]
pub fn cookie(key: &[u8; 16], from: SocketAddr, nonce: u64, epoch: u64) -> CookieValue {
    let half = COOKIE_DOMAIN.len();
    let mut data = Vec::with_capacity(48);
    data.extend_from_slice(COOKIE_DOMAIN);
    data.push(0);
    match from.ip() {
        IpAddr::V4(ip) => {
            data.push(4);
            data.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            data.push(6);
            data.extend_from_slice(&ip.octets());
        }
    }
    data.extend_from_slice(&from.port().to_le_bytes());
    data.extend_from_slice(&nonce.to_le_bytes());
    data.extend_from_slice(&epoch.to_le_bytes());
    let a = siphash24(key, &data);
    data[half] = 1;
    let b = siphash24(key, &data);
    let mut out = [0; COOKIE_BYTES];
    out[..8].copy_from_slice(&a.to_le_bytes());
    out[8..].copy_from_slice(&b.to_le_bytes());
    CookieValue(out)
}

fn same(a: &CookieValue, b: &CookieValue) -> bool {
    a.0.iter().zip(b.0).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The gate's limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateConfig {
    /// What each session it opens is set up with (its game is the one a `Hello` must name).
    pub session: SessionConfig,
    /// Sessions opened and not yet settled, in all.
    pub max_pending: usize,
    /// Of those, from one source IP address.
    pub pending_per_ip: usize,
    /// Answers to `Hello`s one source IP address gets a second.
    pub answers_per_ip_per_s: u32,
    /// Answers to `Hello`s the gate sends a second in all.
    pub answers_per_s: u32,
    /// (address, nonce) pairs remembered as having opened a session; while it is full, no new session opens.
    pub max_opened: usize,
}

impl GateConfig {
    #[must_use]
    pub fn new(session: SessionConfig) -> GateConfig {
        GateConfig {
            session,
            max_pending: 64,
            pending_per_ip: 2,
            answers_per_ip_per_s: 4,
            answers_per_s: 2_000,
            max_opened: 4_096,
        }
    }
}

/// What the gate made of one datagram.
pub enum GateOutcome {
    Drop,
    /// Send this back to the source.
    Reply(Vec<u8>),
    /// A session for the source, which has already read the datagram. The host keeps it, sends it the source's datagrams
    /// from now on and calls [`Gate::settle`] once it is established or closed.
    Open(Box<Session>),
}

/// A plain link's server front door.
pub struct Gate {
    config: GateConfig,
    key: [u8; 16],
    /// Milliseconds on the gate's own clock, which only goes forward.
    clock: u64,
    last_now: Option<u32>,
    per_ip: SourceLimiter,
    answers: RateWindow,
    /// Sessions opened and not settled, with when they opened.
    pending: HashMap<SocketAddr, u64>,
    /// The pairs that opened a session, and the order they did (oldest first) to forget them.
    opened: HashSet<(SocketAddr, u64)>,
    opened_order: VecDeque<(u64, SocketAddr, u64)>,
}

impl Gate {
    /// A gate whose cookies use `key` (from the host's random source).
    #[must_use]
    pub fn new(config: GateConfig, key: [u8; 16]) -> Gate {
        Gate {
            config,
            key,
            clock: 0,
            last_now: None,
            per_ip: SourceLimiter::default(),
            answers: RateWindow::default(),
            pending: HashMap::new(),
            opened: HashSet::new(),
            opened_order: VecDeque::new(),
        }
    }

    /// Replaces the key: cookies handed out before no longer verify.
    pub fn rekey(&mut self, key: [u8; 16]) {
        self.key = key;
    }

    /// The session for `from` is established or closed: it no longer counts as pending.
    pub fn settle(&mut self, from: SocketAddr) {
        self.pending.remove(&from);
    }

    /// Sessions opened and not settled.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Moves the gate's clock to `now` and returns the epoch: the 32-bit clock unwrapped into one that only goes forward, so
    /// an epoch never comes round again. A step forward of under half a wrap is time passing; any other step (the host's
    /// clock went back) counts as none, and the next step is measured from `now`.
    fn tick(&mut self, now: u32) -> u64 {
        match self.last_now {
            Some(last) => {
                let step = now.wrapping_sub(last);
                if step < 1 << 31 {
                    self.clock += u64::from(step);
                }
            }
            None => self.clock = u64::from(now),
        }
        self.last_now = Some(now);
        self.clock / u64::from(COOKIE_PERIOD_MS)
    }

    /// Forgets pending sessions past the connect timeout and opened pairs whose cookie can no longer verify.
    fn expire(&mut self) {
        let clock = self.clock;
        self.pending
            .retain(|_, opened| clock - *opened < u64::from(CONNECT_TIMEOUT_MS));
        let life = 2 * u64::from(COOKIE_PERIOD_MS);
        while let Some(&(at, from, nonce)) = self.opened_order.front()
            && clock - at >= life
        {
            self.opened_order.pop_front();
            self.opened.remove(&(from, nonce));
        }
    }

    /// Takes one datagram from `from`, an address with no session.
    pub fn receive(&mut self, from: SocketAddr, data: &[u8], now: u32) -> GateOutcome {
        let epoch = self.tick(now);
        self.expire();
        match framing::parse(data) {
            Ok(Datagram::Hello(hello)) => self.hello(from, &hello, epoch, now),
            Ok(Datagram::Opening {
                nonce,
                cookie: Some(c),
                ..
            }) => self.opening(from, nonce, c, epoch, data, now),
            _ => GateOutcome::Drop,
        }
    }

    fn hello(
        &mut self,
        from: SocketAddr,
        hello: &framing::Hello,
        epoch: u64,
        now: u32,
    ) -> GateOutcome {
        if !self
            .per_ip
            .allow(from.ip(), now, self.config.answers_per_ip_per_s)
            || !self.answers.allow(now, self.config.answers_per_s)
        {
            return GateOutcome::Drop;
        }
        let refusal = if hello.game != self.config.session.game {
            Some((
                RejectCode::Game,
                format!("the server runs {}", self.config.session.game),
            ))
        } else if !hello.versions.contains(ENGINE_FRAMING) {
            Some((
                RejectCode::Version,
                format!(
                    "server speaks network version {ENGINE_FRAMING}, you have {}",
                    hello.versions.max
                ),
            ))
        } else {
            None
        };
        let reply = match refusal {
            Some((code, text)) => Reject::new(hello.nonce, code, &text).encode(),
            None => Ok(Cookie {
                version: ENGINE_FRAMING,
                nonce: hello.nonce,
                cookie: cookie(&self.key, from, hello.nonce, epoch),
            }
            .encode()),
        };
        match reply {
            Ok(bytes) if bytes.len() <= HELLO_BYTES => GateOutcome::Reply(bytes),
            _ => GateOutcome::Drop,
        }
    }

    fn opening(
        &mut self,
        from: SocketAddr,
        nonce: u64,
        c: CookieValue,
        epoch: u64,
        data: &[u8],
        now: u32,
    ) -> GateOutcome {
        let fresh = same(&c, &cookie(&self.key, from, nonce, epoch))
            || epoch > 0 && same(&c, &cookie(&self.key, from, nonce, epoch - 1));
        let from_ip = self.pending.keys().filter(|a| a.ip() == from.ip()).count();
        if !fresh
            || self.opened.contains(&(from, nonce))
            || self.opened.len() >= self.config.max_opened
            || self.pending.contains_key(&from)
            || self.pending.len() >= self.config.max_pending
            || from_ip >= self.config.pending_per_ip
        {
            return GateOutcome::Drop;
        }
        let mut session = Session::admitted(self.config.session.clone(), nonce, c, now);
        session.receive(data, now);
        self.pending.insert(from, self.clock);
        self.opened.insert((from, nonce));
        self.opened_order.push_back((self.clock, from, nonce));
        GateOutcome::Open(Box::new(session))
    }
}
