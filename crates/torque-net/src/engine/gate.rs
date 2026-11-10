//! The front door of a plain link's server (plain UDP): what it does with a datagram from an address that has no session yet.
//!
//! It keeps nothing for a `Hello`: the answer is a cookie, a keyed digest of the address, port, the client's nonce and the time
//! epoch, which the client sends back in every `Opening`. Only an `Opening` whose cookie verifies opens a session. Pending
//! sessions (opened, not yet established) are bounded per source IP address and per source network (an IPv4 /24, an IPv6
//! /64: [`GateConfig::caps`]), where a new one past the cap is refused, and in all, where a new one evicts a pending session
//! by the shared rule ([`crate::fair::choose_eviction`]: the oldest of a network over its fair share, or one old enough) instead
//! ([`GateOutcome::Open`] names it, and the host must drop it), so no one holding many addresses keeps others from
//! connecting: an honest handshake finishes in a few round trips, long before it is the oldest. A `Hello` shorter than
//! [`HELLO_BYTES`] is not answered, so no answer (a cookie, or a reject naming the game or the version) is larger than the
//! datagram it answers, and answers are rate-limited per source address and per source network ([`GateConfig::answers`],
//! with no total: no answer amplifies, and a total would let a spoofed flood take every honest client's cookie). Every other
//! datagram from an address without a session is dropped unanswered.
//!
//! Addresses are taken in their canonical form: an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`, as a dual-stack socket
//! reports IPv4 peers) counts as the IPv4 address it maps, for the cookie, the per-address and per-network caps and the
//! replay table alike. The address a host passes in is still the one it sends replies to.
//!
//! An `Opening` opens at most one session: the (address, nonce) pairs that opened one are kept for as long as their cookie
//! could verify ([`COOKIE_LIFE_MS`]), so a copy replayed after the session settled is dropped. The table never refuses a new
//! session: a source network (an IPv4 /24, an IPv6 /64) holds at most [`GateConfig::opened_per_network`] entries and
//! evicts its own oldest past that, and a full table evicts the oldest entry of all, so no one holding many addresses can
//! lock others out. An entry evicted before its cookie expired leaves that one opening replayable until then: at most
//! [`COOKIE_LIFE_MS`] after its `Hello`, from the same address and port with the same nonce, so only by a sender who can
//! already spoof that address and saw the opening, and it opens one pending session, as a fresh `Hello` from there would.
//! That happens only when a network opens more than its share, or all networks together more than the table, within one
//! cookie life. The gate's clock only goes forward: a step back is counted as no time, so cookies keep expiring when the
//! host's clock jumps.
//!
//! The gate bounds sessions that are not yet established. How many established sessions one address or network may hold
//! is the host's to cap (it owns the session table); see the module docs of [`super`].

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};

use super::framing::{self, COOKIE_BYTES, Cookie, CookieValue, Datagram, Reject, RejectCode};
use super::session::{CONNECT_TIMEOUT_MS, Session, SessionConfig};
use super::{ENGINE_FRAMING, HELLO_BYTES};
use crate::cap::{
    NET_CHALLENGES_PER_ADDR_PER_S, NET_CHALLENGES_PER_NETWORK_PER_S, NET_PENDING_EVICT_AGE_MS,
};
pub use crate::fair::canonical;
use crate::fair::{
    Eviction, FairLimiter, FairLimits, NetworkKey, PendingSlot, SourceCaps, SourceCount, Stage,
    choose_eviction,
};
use crate::handshake::siphash24;

/// How long a cookie stays in one epoch: one verifies in its epoch and the next, so it lives between one and two periods.
pub const COOKIE_PERIOD_MS: u32 = 30_000;

/// The most a cookie can live: it verifies in its epoch and the next.
pub const COOKIE_LIFE_MS: u32 = 2 * COOKIE_PERIOD_MS;

/// What the cookie digests start with, so they never equal a digest of the same key made for something else.
const COOKIE_DOMAIN: &[u8] = b"engine-cookie";

/// The cookie of `from` and `nonce` in `epoch`: two keyed digests that differ in the byte after the domain.
#[must_use]
pub fn cookie(key: &[u8; 16], from: SocketAddr, nonce: u64, epoch: u64) -> CookieValue {
    let from = canonical(from);
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
    /// Sessions opened and not yet settled, in all; past it the oldest is evicted.
    pub max_pending: usize,
    /// Of those, what one source IP address and one source network may hold ([`SourceCaps::pending_per_ip`],
    /// [`SourceCaps::pending_per_network`]); past them a new one is refused.
    pub caps: SourceCaps,
    /// Answers to `Hello`s a second, per source address, per source network and in all.
    pub answers: FairLimits,
    /// (address, nonce) pairs remembered as having opened a session; past it the oldest is forgotten.
    pub max_opened: usize,
    /// Of those, from one source network (an IPv4 /24 or an IPv6 /64); past it that network's oldest is forgotten.
    pub opened_per_network: usize,
}

impl GateConfig {
    #[must_use]
    pub fn new(session: SessionConfig) -> GateConfig {
        GateConfig {
            session,
            max_pending: 64,
            caps: SourceCaps::default(),
            answers: FairLimits::per_source(
                NET_CHALLENGES_PER_ADDR_PER_S,
                NET_CHALLENGES_PER_NETWORK_PER_S,
            ),
            max_opened: 4_096,
            opened_per_network: 256,
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
    Open {
        session: Box<Session>,
        /// The source (as the host passed it in) whose pending session made room for this one. The host must drop that
        /// session: the gate has already stopped counting it, so a host that kept it would hold more pending sessions than
        /// the gate's caps allow. It was never established.
        evicted: Option<SocketAddr>,
    },
}

/// A plain link's server front door.
pub struct Gate {
    config: GateConfig,
    key: [u8; 16],
    /// Milliseconds on the gate's own clock, which only goes forward.
    clock: u64,
    last_now: Option<u32>,
    answers: FairLimiter,
    /// Sessions opened and not settled, by canonical source.
    pending: HashMap<SocketAddr, Pending>,
    /// Orders pending sessions opened in the same millisecond.
    pending_next: u64,
    opened: Opened,
}

impl Gate {
    /// A gate whose cookies use `key` (from the host's random source).
    #[must_use]
    pub fn new(config: GateConfig, key: [u8; 16]) -> Gate {
        Gate {
            answers: FairLimiter::new(config.answers),
            config,
            key,
            clock: 0,
            last_now: None,
            pending: HashMap::new(),
            pending_next: 0,
            opened: Opened::default(),
        }
    }

    /// Replaces the key: cookies handed out before no longer verify.
    pub fn rekey(&mut self, key: [u8; 16]) {
        self.key = key;
    }

    /// Sets what one source may hold pending ([`GateConfig::caps`]).
    pub fn set_caps(&mut self, caps: SourceCaps) {
        self.config.caps = caps;
    }

    /// The session for `from` is established or closed: it no longer counts as pending.
    pub fn settle(&mut self, from: SocketAddr) {
        self.pending.remove(&canonical(from));
    }

    /// Sessions opened and not settled.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// (address, nonce) pairs remembered as having opened a session.
    #[must_use]
    pub fn remembered(&self) -> usize {
        self.opened.by_age.len()
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
            .retain(|_, p| clock - p.stamp.0 < u64::from(CONNECT_TIMEOUT_MS));
        self.opened
            .forget_before(clock.saturating_sub(u64::from(COOKIE_LIFE_MS)));
    }

    /// Takes one datagram from `source`, an address with no session.
    pub fn receive(&mut self, source: SocketAddr, data: &[u8], now: u32) -> GateOutcome {
        let epoch = self.tick(now);
        self.expire();
        let from = canonical(source);
        match framing::parse(data) {
            Ok(Datagram::Hello(hello)) => self.hello(from, &hello, epoch, now),
            Ok(Datagram::Opening {
                nonce,
                cookie: Some(c),
                ..
            }) => self.opening(Source { from, source }, nonce, c, epoch, data, now),
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
        if self.answers.allow(from.ip(), now).is_err() {
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
        Source { from, source }: Source,
        nonce: u64,
        c: CookieValue,
        epoch: u64,
        data: &[u8],
        now: u32,
    ) -> GateOutcome {
        let fresh = same(&c, &cookie(&self.key, from, nonce, epoch))
            || epoch > 0 && same(&c, &cookie(&self.key, from, nonce, epoch - 1));
        let count = SourceCount::of(
            from.ip(),
            self.pending.keys().map(|a| (a.ip(), Stage::Pending)),
        );
        if !fresh
            || self.opened.by_pair.contains_key(&(from, nonce))
            || self.pending.contains_key(&from)
            || self.config.caps.admits_pending(&count).is_err()
        {
            return GateOutcome::Drop;
        }
        // A full table follows the shared rule: a network over its fair share pays, never an honest client's only slot.
        let evicted = if self.pending.len() >= self.config.max_pending.max(1) {
            let mut slots: Vec<PendingSlot<SocketAddr>> = self
                .pending
                .iter()
                .map(|(a, p)| PendingSlot {
                    key: *a,
                    ip: a.ip(),
                    age_ms: self.clock - p.stamp.0,
                })
                .collect();
            slots.sort_by_key(|s| self.pending.get(&s.key).map(|p| p.stamp));
            match choose_eviction(&slots, from.ip(), u64::from(NET_PENDING_EVICT_AGE_MS)) {
                Eviction::Evict(a) => self.pending.remove(&a).map(|p| p.source),
                Eviction::Refuse => return GateOutcome::Drop,
            }
        } else {
            None
        };
        let mut session = Session::admitted(self.config.session.clone(), nonce, c, now);
        session.receive(data, now);
        let stamp = (self.clock, self.pending_next);
        self.pending.insert(from, Pending { stamp, source });
        self.pending_next += 1;
        self.opened.remember(
            (from, nonce),
            self.clock,
            self.config.opened_per_network.max(1),
            self.config.max_opened.max(1),
        );
        GateOutcome::Open {
            session: Box::new(session),
            evicted,
        }
    }
}

/// A datagram's source: canonical for every check, and as the host passed it in.
#[derive(Debug, Clone, Copy)]
struct Source {
    from: SocketAddr,
    source: SocketAddr,
}

/// A session opened and not yet settled.
#[derive(Debug, Clone, Copy)]
struct Pending {
    stamp: Stamp,
    /// The address as the host passed it in, to name the session if it is evicted.
    source: SocketAddr,
}

/// When an entry was made: the gate's clock, then a counter so two entries in one millisecond keep their order.
type Stamp = (u64, u64);

/// The pairs that opened a session, by pair, by age, and per network (each network's queue oldest first, so the oldest
/// entry of all is always the front of its network's queue).
#[derive(Default)]
struct Opened {
    by_pair: HashMap<(SocketAddr, u64), Stamp>,
    by_age: BTreeMap<Stamp, (SocketAddr, u64)>,
    per_network: HashMap<NetworkKey, VecDeque<Stamp>>,
    next: u64,
}

impl Opened {
    /// Remembers `pair` at `clock`, first forgetting its network's oldest past `per_network` and the oldest of all past
    /// `total`.
    fn remember(&mut self, pair: (SocketAddr, u64), clock: u64, per_network: usize, total: usize) {
        let network = NetworkKey::of_addr(pair.0);
        while let Some(queue) = self.per_network.get(&network)
            && queue.len() >= per_network
            && let Some(&oldest) = queue.front()
        {
            self.forget(oldest);
        }
        while self.by_age.len() >= total
            && let Some((&oldest, _)) = self.by_age.first_key_value()
        {
            self.forget(oldest);
        }
        let stamp = (clock, self.next);
        self.next += 1;
        self.by_pair.insert(pair, stamp);
        self.by_age.insert(stamp, pair);
        self.per_network
            .entry(network)
            .or_default()
            .push_back(stamp);
    }

    /// Forgets every entry made before `clock`.
    fn forget_before(&mut self, clock: u64) {
        while let Some((&oldest, _)) = self.by_age.first_key_value()
            && oldest.0 < clock
        {
            self.forget(oldest);
        }
    }

    /// Forgets one entry, which is always the oldest of its network.
    fn forget(&mut self, stamp: Stamp) {
        let Some(pair) = self.by_age.remove(&stamp) else {
            return;
        };
        self.by_pair.remove(&pair);
        let network = NetworkKey::of_addr(pair.0);
        if let Some(queue) = self.per_network.get_mut(&network) {
            queue.pop_front();
            if queue.is_empty() {
                self.per_network.remove(&network);
            }
        }
    }
}
