//! The rooms: a state machine over the service's sockets, with no I/O. The service hands it each message with the socket's
//! address (a stand-in the web transport gave it) and the IP address it comes from, each socket that closed, and the time;
//! it answers with messages to send and sockets to close ([`Out`]).
//!
//! **What it keeps**: the live rooms (the host's socket, its joiners' sockets), each socket's part, and counts per address
//! over the present minute or second. Nothing outlives a room but those counts.
//!
//! **Hostile input.** A message that is not UTF-8, not one of the protocol's, too long, or not the sender's to send (a
//! joiner's `to`, a host's `signal`, a second `host` or `join`) closes the socket. Requests over a cap are refused with a
//! reason and the socket kept: rooms in all ([`RoomLimits::max_rooms`]), live rooms per source network and new ones per
//! network a minute (an IPv4 /24, an IPv6 /48: one party with many addresses of a network is one host), and joins. The join
//! rules, in order: junk (an IPv6 source, a code that cannot be one) is refused `bad` before anything is counted; joins per
//! address a minute (`rate`); every attempt, hit or miss, counts for its network a minute, and past it the reply is `busy`
//! (a code is 80 bits, so guessing one takes about 2^80 / live rooms joins, and this bounds the guesses however a network
//! spreads them); past its network's share of misses of a second, or with no room left to count them, every join from that
//! network is `busy` until the second passes, hits and misses alike, so no reply tells them apart (a joiner refused `busy`
//! tries again, backing off); every miss is charged to its network and to the total of a second, which only sheds load:
//! past it, a network that missed already in the second is held to its share at once, and a quiet network is never
//! refused for it. No stop refuses every network: a code is 80 bits ([`torque_server::net::room::CODE_BITS`]), so a
//! flood however spread cannot guess one. Joiners waiting in one room are capped too. A joiner may send, and be sent, at
//! most [`RoomLimits::signals_per_joiner`] signaling messages each way (an offer, an answer and a few candidates are a
//! dozen), and a body must be a signaling message ([`torque_server::net::room::is_signal`]), so the service cannot carry
//! a game. Of those, one SDP each way: the joiner's offer and the host's answer to it (a second offer closes the joiner,
//! as does a second answer to it; an answer from a joiner or an offer from a host closes its sender), so a join carries
//! at most two SDPs and the rest is candidate lines. Lifetimes: a room ([`RoomLimits::room_lifetime`], then its host is told `expired` and its socket closed), a
//! joiner's signaling ([`RoomLimits::joiner_lifetime`]), and a socket that has neither hosted nor joined
//! ([`RoomLimits::unplaced_lifetime`] from its first message). The counts per address are capped
//! ([`RoomLimits::tracked_addresses`]); when they are full a new address's requests are refused.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use torque_net::fair::NetworkKey;

use torque_server::net::room::{
    CODE_LEN, FromService, MAX_MESSAGE, ToService, code_from, read_code,
};

use crate::ice::Ice;

/// A time: the service's clock (the tests' own), in milliseconds since some start.
pub type Ms = u64;

/// The room service's caps (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomLimits {
    pub max_rooms: usize,
    pub rooms_per_network: usize,
    pub new_rooms_per_network_per_min: u32,
    pub joiners_per_room: usize,
    pub joins_per_ip_per_min: u32,
    pub joins_per_network_per_min: u32,
    pub failed_joins_per_s: u32,
    pub failed_joins_per_network_per_s: u32,
    pub signals_per_joiner: u32,
    pub room_lifetime: Duration,
    pub joiner_lifetime: Duration,
    pub unplaced_lifetime: Duration,
    pub tracked_addresses: usize,
}

impl Default for RoomLimits {
    fn default() -> RoomLimits {
        RoomLimits {
            max_rooms: 1024,
            rooms_per_network: 4,
            new_rooms_per_network_per_min: 8,
            joiners_per_room: 16,
            joins_per_ip_per_min: 20,
            joins_per_network_per_min: 60,
            failed_joins_per_s: 1_024,
            failed_joins_per_network_per_s: 8,
            signals_per_joiner: 64,
            room_lifetime: Duration::from_secs(6 * 3600),
            joiner_lifetime: Duration::from_secs(60),
            unplaced_lifetime: Duration::from_secs(15),
            tracked_addresses: 8192,
        }
    }
}

/// The longest body a message to the service may carry (room for the head the service puts on it).
pub const MAX_BODY: usize = MAX_MESSAGE - 32;

/// The signaling kinds that carry an SDP: one each per joiner, the offer from it and the answer to it.
const OFFER: &str = "offer";
const ANSWER: &str = "answer";

/// A signaling body's kind (its first line).
fn signal_kind(body: &str) -> &str {
    body.split_once('\n').map_or(body, |(kind, _)| kind)
}

/// What the service does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// Send this message on that socket.
    Send(SocketAddr, String),
    /// Close that socket (after what was sent on it).
    Close(SocketAddr),
}

/// What the rooms have done, for tests and the service's status line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoomCounters {
    pub rooms: u64,
    pub joins: u64,
    pub relayed: u64,
    /// Requests refused by a cap or a code that names no room.
    pub refused: u64,
    /// Sockets closed for a bad message.
    pub closed_bad: u64,
    /// Sockets closed by a lifetime.
    pub closed_late: u64,
    /// Joiners closed for too many signaling messages (a second offer or answer among them).
    pub closed_chatty: u64,
    pub expired: u64,
}

#[derive(Debug)]
enum Part {
    /// Neither host nor joiner yet (since its first message).
    Unplaced {
        since: Ms,
    },
    Host {
        code: String,
    },
    Joiner {
        code: String,
        peer: u32,
    },
}

#[derive(Debug)]
struct Socket {
    part: Part,
}

#[derive(Debug)]
struct Joiner {
    socket: SocketAddr,
    since: Ms,
    sent: u32,
    got: u32,
    /// Its offer went to the host.
    offered: bool,
    /// The host's answer went to it.
    answered: bool,
}

#[derive(Debug)]
struct Room {
    host: SocketAddr,
    host_ip: IpAddr,
    opened: Ms,
    next_peer: u32,
    joiners: HashMap<u32, Joiner>,
}

/// Counts per key (an address, a network) in a window of time.
#[derive(Debug)]
struct Window<K> {
    span: Ms,
    counts: HashMap<K, (Ms, u32)>,
}

impl<K: Copy + Eq + Hash> Window<K> {
    fn new(span: Duration) -> Window<K> {
        Window {
            span: span.as_millis() as Ms,
            counts: HashMap::new(),
        }
    }

    /// Counts one for `ip`; false when that passes `cap` (or no room is left to count a new key).
    fn take(&mut self, ip: K, cap: u32, now: Ms, tracked: usize) -> bool {
        if !self.counts.contains_key(&ip) && self.counts.len() >= tracked {
            let span = self.span;
            self.counts
                .retain(|_, (start, _)| now.saturating_sub(*start) < span);
            if self.counts.len() >= tracked {
                return false;
            }
        }
        let (start, count) = self.counts.entry(ip).or_insert((now, 0));
        if now.saturating_sub(*start) >= self.span {
            *start = now;
            *count = 0;
        }
        *count += 1;
        *count <= cap
    }

    /// Counts `key` up to `cap` at once (only a key counted already: a new one has no window yet).
    fn fill(&mut self, key: K, cap: u32, now: Ms) {
        let span = self.span;
        if let Some((start, count)) = self.counts.get_mut(&key) {
            if now.saturating_sub(*start) >= span {
                *start = now;
                *count = 0;
            }
            *count = (*count).max(cap);
        }
    }

    /// Whether a count for `key` could be kept at `now`: it is counted already, or there is room for it.
    fn can_take(&mut self, key: K, now: Ms, tracked: usize) -> bool {
        if self.counts.contains_key(&key) || self.counts.len() < tracked {
            return true;
        }
        let span = self.span;
        self.counts
            .retain(|_, (start, _)| now.saturating_sub(*start) < span);
        self.counts.len() < tracked
    }

    /// What `key` has counted in its window at `now`, counting nothing.
    fn used(&self, key: K, now: Ms) -> u32 {
        self.counts
            .get(&key)
            .filter(|(start, _)| now.saturating_sub(*start) < self.span)
            .map_or(0, |(_, count)| *count)
    }
}

/// The rooms (module docs).
pub struct Rooms {
    limits: RoomLimits,
    ice: Ice,
    rooms: HashMap<String, Room>,
    sockets: HashMap<SocketAddr, Socket>,
    rooms_of: HashMap<NetworkKey, usize>,
    new_rooms: Window<NetworkKey>,
    joins: Window<IpAddr>,
    network_joins: Window<NetworkKey>,
    failed: (Ms, u32),
    failed_of: Window<NetworkKey>,
    counters: RoomCounters,
    random: Random,
}

impl std::fmt::Debug for Rooms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rooms")
            .field("rooms", &self.rooms.len())
            .field("sockets", &self.sockets.len())
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

/// Where room codes' random bytes come from: true when `bytes` was filled.
pub type Random = Box<dyn FnMut(&mut [u8; CODE_LEN]) -> bool>;

/// Fills `bytes` from the system's random source.
pub fn system_random(bytes: &mut [u8; CODE_LEN]) -> bool {
    use ring::rand::{SecureRandom, SystemRandom};
    SystemRandom::new().fill(bytes).is_ok()
}

impl Rooms {
    /// Rooms under `limits`, giving players `ice`, with codes from the system's random source.
    pub fn new(limits: RoomLimits, ice: Ice) -> Rooms {
        Rooms::with_random(limits, ice, Box::new(system_random))
    }

    /// Rooms whose codes come from `random` (a test's).
    pub fn with_random(limits: RoomLimits, ice: Ice, random: Random) -> Rooms {
        Rooms {
            new_rooms: Window::new(Duration::from_secs(60)),
            joins: Window::new(Duration::from_secs(60)),
            network_joins: Window::new(Duration::from_secs(60)),
            limits,
            ice,
            rooms: HashMap::new(),
            sockets: HashMap::new(),
            rooms_of: HashMap::new(),
            failed: (0, 0),
            failed_of: Window::new(Duration::from_secs(1)),
            counters: RoomCounters::default(),
            random,
        }
    }

    pub fn counters(&self) -> RoomCounters {
        self.counters
    }

    /// The live rooms.
    pub fn rooms(&self) -> usize {
        self.rooms.len()
    }

    /// The sockets known (those that have sent a message and not closed).
    pub fn sockets(&self) -> usize {
        self.sockets.len()
    }

    /// The joiners waiting in the room of `code`.
    pub fn joiners(&self, code: &str) -> usize {
        self.rooms.get(code).map_or(0, |r| r.joiners.len())
    }

    /// The ICE servers for a player at `ip`: its TURN user name is the player's address (in hex) and nothing a player sent.
    fn ice_for(&self, ip: IpAddr, unix_now: u64) -> String {
        self.ice.servers(unix_now, &crate::ice::user_id(ip))
    }

    /// A message from `socket` (its TCP address's IP is `ip`) at `now`; `unix_now` dates TURN credentials.
    pub fn message(
        &mut self,
        socket: SocketAddr,
        ip: IpAddr,
        bytes: &[u8],
        now: Ms,
        unix_now: u64,
        out: &mut Vec<Out>,
    ) {
        let parsed = std::str::from_utf8(bytes)
            .ok()
            .filter(|t| t.len() <= MAX_BODY)
            .and_then(ToService::parse);
        let Some(message) = parsed else {
            self.counters.closed_bad += 1;
            self.close(socket, out);
            return;
        };
        let known = self.sockets.entry(socket).or_insert(Socket {
            part: Part::Unplaced { since: now },
        });
        let unplaced = matches!(known.part, Part::Unplaced { .. });
        match message {
            ToService::Ping => out.push(Out::Send(socket, FromService::Pong.write())),
            ToService::Host if unplaced => self.host(socket, ip, now, unix_now, out),
            ToService::Join(code) if unplaced => self.join(socket, ip, code, now, unix_now, out),
            ToService::To(peer, body) => self.to(socket, peer, body, out),
            ToService::Signal(body) => self.signal(socket, body, out),
            ToService::Drop(peer) => self.drop_peer(socket, peer, out),
            ToService::Host | ToService::Join(_) => {
                self.counters.closed_bad += 1;
                self.close(socket, out);
            }
        }
    }

    fn refuse(&mut self, socket: SocketAddr, why: &str, out: &mut Vec<Out>) {
        self.counters.refused += 1;
        out.push(Out::Send(socket, FromService::Refused(why).write()));
    }

    fn host(&mut self, socket: SocketAddr, ip: IpAddr, now: Ms, unix_now: u64, out: &mut Vec<Out>) {
        let limits = &self.limits;
        if self.rooms.len() >= limits.max_rooms {
            return self.refuse(socket, "busy", out);
        }
        let network = NetworkKey::sharing(ip);
        if self.rooms_of.get(&network).copied().unwrap_or(0) >= limits.rooms_per_network {
            return self.refuse(socket, "rate", out);
        }
        if !self.new_rooms.take(
            network,
            limits.new_rooms_per_network_per_min,
            now,
            limits.tracked_addresses,
        ) {
            return self.refuse(socket, "rate", out);
        }
        let mut code = None;
        for _ in 0..4 {
            let mut bytes = [0u8; CODE_LEN];
            if !(self.random)(&mut bytes) {
                break;
            }
            let made = code_from(&bytes);
            if !self.rooms.contains_key(&made) {
                code = Some(made);
                break;
            }
        }
        let Some(code) = code else {
            return self.refuse(socket, "busy", out);
        };
        let ice = self.ice_for(ip, unix_now);
        self.rooms.insert(
            code.clone(),
            Room {
                host: socket,
                host_ip: ip,
                opened: now,
                next_peer: 1,
                joiners: HashMap::new(),
            },
        );
        *self.rooms_of.entry(network).or_insert(0) += 1;
        if let Some(s) = self.sockets.get_mut(&socket) {
            s.part = Part::Host { code: code.clone() };
        }
        self.counters.rooms += 1;
        out.push(Out::Send(
            socket,
            FromService::Room {
                code: &code,
                seconds: self.limits.room_lifetime.as_secs(),
                ice: &ice,
            }
            .write(),
        ));
    }

    fn join(
        &mut self,
        socket: SocketAddr,
        ip: IpAddr,
        code: &str,
        now: Ms,
        unix_now: u64,
        out: &mut Vec<Out>,
    ) {
        // Junk (a source that can never join, a code that cannot be one) is refused before anything is counted.
        let IpAddr::V4(v4) = ip.to_canonical() else {
            // The web transport refuses IPv6 peers.
            return self.refuse(socket, "bad", out);
        };
        let Some(code) = read_code(code) else {
            return self.refuse(socket, "bad", out);
        };
        let limits = &self.limits;
        let tracked = limits.tracked_addresses;
        if !self
            .joins
            .take(ip, limits.joins_per_ip_per_min, now, tracked)
        {
            return self.refuse(socket, "rate", out);
        }
        // Every attempt, hit or miss, counts for its network a minute: guesses spread over a network's addresses stay
        // bounded.
        let network = NetworkKey::sharing(ip);
        if !self
            .network_joins
            .take(network, limits.joins_per_network_per_min, now, tracked)
        {
            return self.refuse(socket, "busy", out);
        }
        // Past its network's share of misses (or with no room left to count them) every join from that network is refused
        // until the second passes, hits and misses alike, so no reply tells them apart; no other network is touched. A joiner
        // refused so tries again after the window.
        if now.saturating_sub(self.failed.0) >= 1_000 {
            self.failed = (now, 0);
        }
        let per_network = limits.failed_joins_per_network_per_s;
        if self.failed_of.used(network, now) >= per_network
            || !self.failed_of.can_take(network, now, tracked)
        {
            return self.refuse(socket, "busy", out);
        }
        // Every miss is charged to its network and to the total. The total only sheds load: past it, a network that missed
        // already in this second is held to its share at once (its every join refused for the rest of it); a quiet network
        // is never refused for it.
        if !self.rooms.contains_key(&code) {
            self.failed.1 = self.failed.1.saturating_add(1);
            let missed_before = self.failed_of.used(network, now) > 0;
            if !self.failed_of.take(network, per_network, now, tracked) {
                return self.refuse(socket, "busy", out);
            }
            if missed_before && self.failed.1 > limits.failed_joins_per_s {
                self.failed_of.fill(network, per_network, now);
            }
            return self.refuse(socket, "no-room", out);
        }
        let Some(room) = self.rooms.get_mut(&code) else {
            return self.refuse(socket, "no-room", out);
        };
        if room.joiners.len() >= limits.joiners_per_room || room.next_peer == u32::MAX {
            return self.refuse(socket, "full", out);
        }
        let peer = room.next_peer;
        room.next_peer += 1;
        room.joiners.insert(
            peer,
            Joiner {
                socket,
                since: now,
                sent: 0,
                got: 0,
                offered: false,
                answered: false,
            },
        );
        let host = room.host;
        if let Some(s) = self.sockets.get_mut(&socket) {
            s.part = Part::Joiner { code, peer };
        }
        self.counters.joins += 1;
        let ice = self.ice_for(ip, unix_now);
        out.push(Out::Send(host, FromService::Peer { peer, ip: v4 }.write()));
        out.push(Out::Send(
            socket,
            FromService::Joined { peer, ice: &ice }.write(),
        ));
    }

    fn to(&mut self, socket: SocketAddr, peer: u32, body: &str, out: &mut Vec<Out>) {
        let Some(Part::Host { code }) = self.sockets.get(&socket).map(|s| &s.part) else {
            self.counters.closed_bad += 1;
            return self.close(socket, out);
        };
        let kind = signal_kind(body);
        if kind == OFFER {
            // A host answers; it never offers.
            self.counters.closed_bad += 1;
            return self.close(socket, out);
        }
        let cap = self.limits.signals_per_joiner;
        let Some(joiner) = self
            .rooms
            .get_mut(code)
            .and_then(|r| r.joiners.get_mut(&peer))
        else {
            // It left meanwhile.
            return;
        };
        joiner.got += 1;
        let second = kind == ANSWER && std::mem::replace(&mut joiner.answered, true);
        if joiner.got > cap || second {
            let to = joiner.socket;
            self.counters.closed_chatty += 1;
            return self.close(to, out);
        }
        self.counters.relayed += 1;
        out.push(Out::Send(joiner.socket, FromService::Signal(body).write()));
    }

    fn signal(&mut self, socket: SocketAddr, body: &str, out: &mut Vec<Out>) {
        let Some(Part::Joiner { code, peer }) = self.sockets.get(&socket).map(|s| &s.part) else {
            self.counters.closed_bad += 1;
            return self.close(socket, out);
        };
        let peer = *peer;
        let kind = signal_kind(body);
        if kind == ANSWER {
            // A joiner offers; it never answers.
            self.counters.closed_bad += 1;
            return self.close(socket, out);
        }
        let cap = self.limits.signals_per_joiner;
        let Some(room) = self.rooms.get_mut(code) else {
            return;
        };
        let host = room.host;
        let Some(joiner) = room.joiners.get_mut(&peer) else {
            return;
        };
        joiner.sent += 1;
        let second = kind == OFFER && std::mem::replace(&mut joiner.offered, true);
        if joiner.sent > cap || second {
            self.counters.closed_chatty += 1;
            return self.close(socket, out);
        }
        self.counters.relayed += 1;
        out.push(Out::Send(host, FromService::From { peer, body }.write()));
    }

    fn drop_peer(&mut self, socket: SocketAddr, peer: u32, out: &mut Vec<Out>) {
        let Some(Part::Host { code }) = self.sockets.get(&socket).map(|s| &s.part) else {
            self.counters.closed_bad += 1;
            return self.close(socket, out);
        };
        let Some(joiner) = self.rooms.get(code).and_then(|r| r.joiners.get(&peer)) else {
            return;
        };
        let to = joiner.socket;
        out.push(Out::Send(to, FromService::Closed("dropped").write()));
        self.close(to, out);
    }

    /// Closes `socket` and forgets it: a host's room ends (its joiners are told and closed), a joiner leaves its room (its
    /// host is told).
    fn close(&mut self, socket: SocketAddr, out: &mut Vec<Out>) {
        self.forget(socket, out);
        out.push(Out::Close(socket));
    }

    /// `socket` closed (or is being closed): what it was part of is undone.
    fn forget(&mut self, socket: SocketAddr, out: &mut Vec<Out>) {
        let Some(gone) = self.sockets.remove(&socket) else {
            return;
        };
        match gone.part {
            Part::Unplaced { .. } => {}
            Part::Host { code } => {
                let Some(room) = self.rooms.remove(&code) else {
                    return;
                };
                let network = NetworkKey::sharing(room.host_ip);
                if let Some(n) = self.rooms_of.get_mut(&network) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        self.rooms_of.remove(&network);
                    }
                }
                for joiner in room.joiners.into_values() {
                    self.sockets.remove(&joiner.socket);
                    out.push(Out::Send(
                        joiner.socket,
                        FromService::Closed("host-left").write(),
                    ));
                    out.push(Out::Close(joiner.socket));
                }
            }
            Part::Joiner { code, peer } => {
                if let Some(room) = self.rooms.get_mut(&code)
                    && room.joiners.remove(&peer).is_some()
                {
                    out.push(Out::Send(room.host, FromService::Left(peer).write()));
                }
            }
        }
    }

    /// `socket` closed.
    pub fn gone(&mut self, socket: SocketAddr, out: &mut Vec<Out>) {
        self.forget(socket, out);
    }

    /// The lifetimes at `now`: rooms past theirs expire, joiners and sockets that neither host nor join past theirs are
    /// closed.
    pub fn tick(&mut self, now: Ms, out: &mut Vec<Out>) {
        let ms = |d: Duration| d.as_millis() as Ms;
        let (room_ms, joiner_ms, unplaced_ms) = (
            ms(self.limits.room_lifetime),
            ms(self.limits.joiner_lifetime),
            ms(self.limits.unplaced_lifetime),
        );
        let mut late = Vec::new();
        for room in self.rooms.values() {
            if now.saturating_sub(room.opened) >= room_ms {
                late.push((room.host, true));
                continue;
            }
            for joiner in room.joiners.values() {
                if now.saturating_sub(joiner.since) >= joiner_ms {
                    late.push((joiner.socket, false));
                }
            }
        }
        for (&addr, socket) in &self.sockets {
            if let Part::Unplaced { since } = socket.part
                && now.saturating_sub(since) >= unplaced_ms
            {
                late.push((addr, false));
            }
        }
        for (socket, room) in late {
            if room {
                self.counters.expired += 1;
                out.push(Out::Send(socket, FromService::Expired.write()));
            } else {
                self.counters.closed_late += 1;
                if matches!(
                    self.sockets.get(&socket).map(|s| &s.part),
                    Some(Part::Joiner { .. })
                ) {
                    out.push(Out::Send(socket, FromService::Closed("timeout").write()));
                }
            }
            self.close(socket, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torque_server::net::room::show_code;

    fn addr(n: u16) -> SocketAddr {
        SocketAddr::from(([240, 0, 0, 1], n))
    }

    /// Player `n`'s address, each in a network of its own (`ip(2)` is 192.0.2.2).
    fn ip(n: u8) -> IpAddr {
        IpAddr::from([192, 0, n, 2])
    }

    /// Rooms whose codes count up (`0000000000000001`, ...).
    fn rooms(limits: RoomLimits) -> Rooms {
        let mut n = 0u8;
        Rooms::with_random(
            limits,
            Ice::default(),
            Box::new(move |b| {
                n += 1;
                *b = [0; CODE_LEN];
                b[CODE_LEN - 1] = n;
                true
            }),
        )
    }

    fn say(r: &mut Rooms, from: u16, from_ip: u8, text: &str, now: Ms) -> Vec<Out> {
        let mut out = Vec::new();
        r.message(
            addr(from),
            ip(from_ip),
            text.as_bytes(),
            now,
            1_000_000,
            &mut out,
        );
        out
    }

    fn sent(out: &[Out], to: u16) -> Vec<String> {
        out.iter()
            .filter_map(|o| match o {
                Out::Send(a, t) if *a == addr(to) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn closed(out: &[Out], to: u16) -> bool {
        out.contains(&Out::Close(addr(to)))
    }

    #[test]
    fn a_host_and_a_joiner_signal_through_a_room() {
        let mut r = rooms(RoomLimits::default());
        let out = say(&mut r, 1, 1, "host", 0);
        assert_eq!(sent(&out, 1), ["room 0000000000000001 21600\n[]"]);
        let out = say(
            &mut r,
            2,
            2,
            &format!("join {}", show_code("0000000000000001").to_lowercase()),
            10,
        );
        assert_eq!(sent(&out, 1), ["peer 1 192.0.2.2"]);
        assert_eq!(sent(&out, 2), ["joined 1\n[]"]);
        let offer = "offer\nv=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n";
        let out = say(&mut r, 2, 2, &format!("signal\n{offer}"), 20);
        assert_eq!(sent(&out, 1), [format!("from 1\n{offer}")]);
        let out = say(&mut r, 1, 1, "to 1\nanswer\nv=0", 30);
        assert_eq!(sent(&out, 2), ["signal\nanswer\nv=0"]);
        assert_eq!(sent(&say(&mut r, 2, 2, "ping", 40), 2), ["pong"]);
        // The joiner leaves (its channel carries on): the host is told.
        let mut out = Vec::new();
        r.gone(addr(2), &mut out);
        assert_eq!(sent(&out, 1), ["left 1"]);
        // A second joiner, then the host leaves: the joiner is told and closed, and the room is gone.
        say(&mut r, 3, 3, "join 0000000000000001", 50);
        let mut out = Vec::new();
        r.gone(addr(1), &mut out);
        assert_eq!(sent(&out, 3), ["closed host-left"]);
        assert!(closed(&out, 3));
        assert_eq!((r.rooms(), r.sockets()), (0, 0));
        assert_eq!(
            sent(&say(&mut r, 4, 4, "join 0000000000000001", 60), 4),
            ["refused no-room"]
        );
    }

    #[test]
    fn credentials_go_only_to_sockets_in_a_room() {
        let ice = Ice {
            stun: vec!["stun:192.0.2.9:3478".into()],
            turn: vec!["turn:192.0.2.9:3478".into()],
            secret: Some(b"s".to_vec()),
            ttl: std::time::Duration::from_secs(600),
        };
        let limits = RoomLimits {
            joiners_per_room: 1,
            ..RoomLimits::default()
        };
        let mut n = 0u8;
        let mut r = Rooms::with_random(
            limits,
            ice,
            Box::new(move |b| {
                n += 1;
                *b = [0; CODE_LEN];
                b[CODE_LEN - 1] = n;
                true
            }),
        );
        let given = |out: &[Out]| {
            out.iter()
                .filter(|o| matches!(o, Out::Send(_, t) if t.contains("credential")))
                .count()
        };
        // Before any room: a ping, and a join with no room.
        assert_eq!(given(&say(&mut r, 5, 5, "ping", 0)), 0);
        assert_eq!(given(&say(&mut r, 6, 6, "join 0000-0000-0000-000Z", 0)), 0);
        // The host on `room` and the joiner on `joined`, each its own credential, expiring the TTL after the clock (the
        // tests' clock is 1,000,000).
        let out = say(&mut r, 1, 1, "host", 0);
        let room = &sent(&out, 1)[0];
        assert!(room.contains("\"username\":\"1000600:c0000102\""), "{room}");
        let out = say(&mut r, 2, 2, "join 0000000000000001", 0);
        assert_eq!(
            sent(&out, 1),
            ["peer 1 192.0.2.2"],
            "the host is given none"
        );
        let joined = &sent(&out, 2)[0];
        assert!(
            joined.contains("\"username\":\"1000600:c0000202\""),
            "{joined}"
        );
        // A join past the room's joiners is refused with none.
        let out = say(&mut r, 3, 3, "join 0000000000000001", 0);
        assert_eq!(sent(&out, 3), ["refused full"]);
        assert_eq!(given(&out), 0);
    }

    #[test]
    fn messages_out_of_place_close_the_socket() {
        let mut r = rooms(RoomLimits::default());
        say(&mut r, 1, 1, "host", 0);
        say(&mut r, 2, 2, "join 0000000000000001", 0);
        for (who, text) in [
            (2, "to 1\nanswer\nv=0"),      // a joiner may not address anyone
            (1, "signal\noffer\nv=0"),     // a host may not send a joiner's message
            (1, "host"),                   // one room a socket
            (2, "join 0000000000000001"),  // one join a socket
            (5, "to 1\nanswer\nv=0"),      // nobody's message
            (6, "drop 1"),                 // nor this
            (7, "signal\nGET / HTTP/1.1"), // a body that is not signaling
            (8, "hello"),
        ] {
            let mut r2 = rooms(RoomLimits::default());
            say(&mut r2, 1, 1, "host", 0);
            say(&mut r2, 2, 2, "join 0000000000000001", 0);
            let out = say(&mut r2, who, who as u8, text, 1);
            assert!(closed(&out, who), "{text:?} from {who}");
        }
        let mut out = Vec::new();
        r.message(addr(9), ip(9), &[0xff, 0xfe], 0, 0, &mut out);
        assert!(closed(&out, 9), "not UTF-8");
        let long = format!("signal\noffer\n{}", "a".repeat(MAX_BODY));
        assert!(closed(&say(&mut r, 2, 2, &long, 1), 2), "too long");
        // The host is told its joiner left.
        assert_eq!(r.joiners("0000000000000001"), 0);
    }

    /// `text` from `socket` at `from` and `now`: what the socket was sent.
    fn said(r: &mut Rooms, socket: u16, from: IpAddr, text: &str, now: Ms) -> Vec<String> {
        let mut out = Vec::new();
        r.message(
            addr(socket),
            from,
            text.as_bytes(),
            now,
            1_000_000,
            &mut out,
        );
        sent(&out, socket)
    }

    #[test]
    fn one_network_hosts_at_most_its_rooms() {
        let mut r = rooms(RoomLimits::default());
        let net = |h: u8| IpAddr::from([198, 51, 100, h]);
        for host in 1..=4 {
            assert!(said(&mut r, u16::from(host), net(host), "host", 0)[0].starts_with("room "));
        }
        assert_eq!(
            said(&mut r, 5, net(5), "host", 0),
            ["refused rate"],
            "a fifth host of the /24"
        );
        assert!(
            said(&mut r, 6, ip(9), "host", 0)[0].starts_with("room "),
            "another network"
        );
    }

    #[test]
    fn every_attempt_of_a_network_counts_and_over_it_the_reply_is_busy() {
        let limits = RoomLimits::default();
        let mut r = rooms(limits.clone());
        assert!(said(&mut r, 1, ip(1), "host", 0)[0].starts_with("room "));
        // Every address of a /24 joins the live room once: past the network's attempts of a minute every reply is busy.
        let mut passed = 0;
        for host in 0..=255u8 {
            let reply = said(
                &mut r,
                1_000 + u16::from(host),
                IpAddr::from([203, 0, 113, host]),
                "join 0000000000000001",
                u64::from(host) * 100,
            );
            if reply != ["refused busy"] {
                passed += 1;
            }
        }
        assert_eq!(passed, limits.joins_per_network_per_min as usize);
    }

    #[test]
    fn misses_charge_their_network_and_past_its_share_no_reply_tells_a_hit_from_a_miss() {
        let limits = RoomLimits::default();
        let mut r = rooms(limits.clone());
        assert!(said(&mut r, 1, ip(1), "host", 0)[0].starts_with("room "));
        // One network's misses, over many addresses: past its share, a hit and a miss from there are both busy.
        for host in 0..limits.failed_joins_per_network_per_s as u8 {
            assert_eq!(
                said(
                    &mut r,
                    10 + u16::from(host),
                    IpAddr::from([10, 7, 7, host]),
                    "join ZZZZZZZZZZZZZZZZ",
                    100
                ),
                ["refused no-room"]
            );
        }
        let hit = said(
            &mut r,
            50,
            IpAddr::from([10, 7, 7, 200]),
            "join 0000000000000001",
            150,
        );
        let miss = said(
            &mut r,
            51,
            IpAddr::from([10, 7, 7, 201]),
            "join ZZZZZZZZZZZZZZZZ",
            150,
        );
        assert_eq!((hit.clone(), miss), (vec!["refused busy".to_string()], hit));
        assert!(
            said(&mut r, 52, ip(30), "join 0000000000000001", 150)[0].starts_with("joined "),
            "another network joins"
        );
    }

    #[test]
    fn a_flood_of_misses_from_many_networks_never_stops_a_joiner_from_a_quiet_network() {
        let limits = RoomLimits::default();
        let mut r = rooms(limits.clone());
        assert!(said(&mut r, 1, ip(1), "host", 0)[0].starts_with("room "));
        // Four times the total of misses in one second, from 4,096 networks, each missing once and then once more.
        let total = limits.failed_joins_per_s;
        for round in 0..2u32 {
            for n in 0..2 * total {
                let from = IpAddr::from([11, (n >> 8) as u8, n as u8, 1 + round as u8]);
                let _ = said(
                    &mut r,
                    (n % 60_000) as u16 + 100,
                    from,
                    "join ZZZZZZZZZZZZZZZZ",
                    100 + u64::from(round),
                );
            }
        }
        // A quiet network's joiner is let in at once.
        assert!(said(&mut r, 2, ip(40), "join 0000000000000001", 200)[0].starts_with("joined "));
        // A network that missed past the total is held to its share: a hit from it is refused as a miss is.
        let from = IpAddr::from([11, 0, 5, 9]);
        let hit = said(&mut r, 3, from, "join 0000000000000001", 300);
        let miss = said(
            &mut r,
            4,
            IpAddr::from([11, 0, 5, 10]),
            "join ZZZZZZZZZZZZZZZZ",
            300,
        );
        assert_eq!((hit.clone(), miss), (vec!["refused busy".to_string()], hit));
    }

    #[test]
    fn junk_is_refused_before_anything_is_counted() {
        let mut r = rooms(RoomLimits::default());
        assert!(said(&mut r, 1, ip(1), "host", 0)[0].starts_with("room "));
        for n in 0..500u16 {
            let v6 = IpAddr::from([0x2001, 0xdb8, n, 0, 0, 0, 0, 1]);
            assert_eq!(
                said(&mut r, 1_000 + n, v6, "join 0000000000000001", 100),
                ["refused bad"]
            );
            let v4 = IpAddr::from([10, 1, 1, n as u8]);
            assert_eq!(said(&mut r, 2_000 + n, v4, "join !!", 100), ["refused bad"]);
        }
        assert!(
            said(
                &mut r,
                3_000,
                IpAddr::from([10, 1, 1, 7]),
                "join 0000000000000001",
                100
            )[0]
            .starts_with("joined ")
        );
    }

    #[test]
    fn ipv6_networks_are_counted_by_their_48() {
        assert_eq!(
            NetworkKey::sharing("2001:db8:1:2::1".parse().expect("v6")),
            NetworkKey::sharing("2001:db8:1:ffff::9".parse().expect("v6"))
        );
        assert_ne!(
            NetworkKey::sharing("2001:db8:1::1".parse().expect("v6")),
            NetworkKey::sharing("2001:db8:2::1".parse().expect("v6"))
        );
    }

    #[test]
    fn caps_refuse_rooms_and_joins() {
        let limits = RoomLimits {
            max_rooms: 3,
            rooms_per_network: 2,
            new_rooms_per_network_per_min: 2,
            joiners_per_room: 2,
            joins_per_ip_per_min: 4,
            failed_joins_per_s: 5,
            ..RoomLimits::default()
        };
        let mut r = rooms(limits);
        assert!(sent(&say(&mut r, 1, 1, "host", 0), 1)[0].starts_with("room "));
        assert!(sent(&say(&mut r, 2, 1, "host", 0), 2)[0].starts_with("room "));
        assert_eq!(
            sent(&say(&mut r, 3, 1, "host", 0), 3),
            ["refused rate"],
            "rooms per address"
        );
        assert!(sent(&say(&mut r, 4, 2, "host", 0), 4)[0].starts_with("room "));
        assert_eq!(
            sent(&say(&mut r, 5, 3, "host", 0), 5),
            ["refused busy"],
            "rooms in all"
        );
        // Rooms a minute per address: closing and reopening does not get round it.
        let mut out = Vec::new();
        r.gone(addr(1), &mut out);
        assert_eq!(sent(&say(&mut r, 6, 1, "host", 1), 6), ["refused rate"]);
        assert!(sent(&say(&mut r, 6, 1, "host", 60_001), 6)[0].starts_with("room "));
        // Joiners waiting per room.
        say(&mut r, 10, 10, "join 0000000000000002", 70_000);
        say(&mut r, 11, 11, "join 0000000000000002", 70_000);
        assert_eq!(
            sent(&say(&mut r, 12, 12, "join 0000000000000002", 70_000), 12),
            ["refused full"]
        );
        // Joins per address a minute, whether they find a room or not.
        for n in 0..4 {
            assert_eq!(
                sent(
                    &say(&mut r, 20 + n, 20, "join ZZZZZZZZZZZZZZZZ", 70_000),
                    20 + n
                ),
                ["refused no-room"]
            );
        }
        assert_eq!(
            sent(&say(&mut r, 30, 20, "join 0000000000000002", 70_000), 30),
            ["refused rate"]
        );
        // Failed joins in a second from every address together (the code-guessing cap).
        let mut refusals = Vec::new();
        for n in 0..3u16 {
            refusals.extend(sent(
                &say(
                    &mut r,
                    40 + n,
                    40 + n as u8,
                    "join ZZZZZZZZZZZZZZZZ",
                    70_000,
                ),
                40 + n,
            ));
        }
        assert_eq!(
            refusals,
            ["refused no-room", "refused no-room", "refused no-room"],
            "past the total a network's first miss is still a miss"
        );
        // A network that misses again past the total is held to its share.
        assert_eq!(
            sent(&say(&mut r, 43, 42, "join ZZZZZZZZZZZZZZZZ", 70_100), 43),
            ["refused no-room"]
        );
        assert_eq!(
            sent(&say(&mut r, 44, 42, "join 0000000000000002", 70_200), 44),
            ["refused busy"]
        );
        assert_eq!(
            sent(&say(&mut r, 50, 50, "join ZZZZZZZZZZZZZZZZ", 71_000), 50),
            ["refused no-room"]
        );
    }

    #[test]
    fn signaling_is_capped_per_joiner_both_ways() {
        let limits = RoomLimits {
            signals_per_joiner: 3,
            ..RoomLimits::default()
        };
        let mut r = rooms(limits);
        say(&mut r, 1, 1, "host", 0);
        say(&mut r, 2, 2, "join 0000000000000001", 0);
        say(&mut r, 3, 3, "join 0000000000000001", 0);
        for _ in 0..3 {
            assert_eq!(
                sent(&say(&mut r, 2, 2, "signal\ncandidate\ncandidate:x", 1), 1).len(),
                1
            );
        }
        let out = say(&mut r, 2, 2, "signal\ncandidate\ncandidate:x", 1);
        assert!(closed(&out, 2));
        assert_eq!(sent(&out, 1), ["left 1"]);
        for _ in 0..3 {
            assert_eq!(
                sent(&say(&mut r, 1, 1, "to 2\ncandidate\ncandidate:x", 1), 3).len(),
                1
            );
        }
        let out = say(&mut r, 1, 1, "to 2\ncandidate\ncandidate:x", 1);
        assert!(
            closed(&out, 3) && !closed(&out, 1),
            "the joiner goes, not the host"
        );
        // A host's drop ends that joiner's signaling.
        say(&mut r, 4, 4, "join 0000000000000001", 2);
        let out = say(&mut r, 1, 1, "drop 3", 3);
        assert_eq!(sent(&out, 4), ["closed dropped"]);
        assert!(closed(&out, 4));
    }

    #[test]
    fn one_offer_and_one_answer_cross_per_joiner() {
        let offer = "signal\noffer\nv=0\r\na=x:aaaa";
        let answer = "to 1\nanswer\nv=0\r\na=x:bbbb";
        let not_from = |out: &[Out]| sent(out, 1).iter().all(|m| !m.starts_with("from"));
        let mut r = rooms(RoomLimits::default());
        say(&mut r, 1, 1, "host", 0);
        say(&mut r, 2, 2, "join 0000000000000001", 0);
        assert_eq!(sent(&say(&mut r, 2, 2, offer, 1), 1).len(), 1);
        assert_eq!(sent(&say(&mut r, 1, 1, answer, 1), 2).len(), 1);
        // Candidates still cross both ways.
        let out = say(&mut r, 2, 2, "signal\ncandidate\ncandidate:x", 1);
        assert_eq!(sent(&out, 1).len(), 1);
        let out = say(&mut r, 1, 1, "to 1\ncandidate\ncandidate:x", 1);
        assert_eq!(sent(&out, 2).len(), 1);
        // A second answer to the joiner is not passed on, and the joiner goes (not the host).
        let out = say(&mut r, 1, 1, answer, 2);
        assert!(sent(&out, 2).is_empty());
        assert!(closed(&out, 2) && !closed(&out, 1));
        assert_eq!(sent(&out, 1), ["left 1"]);
        assert_eq!(r.counters().closed_chatty, 1);
        // A second offer from a joiner: not passed on, and the joiner goes.
        say(&mut r, 3, 3, "join 0000000000000001", 3);
        assert_eq!(sent(&say(&mut r, 3, 3, offer, 3), 1).len(), 1);
        let out = say(&mut r, 3, 3, offer, 4);
        assert!(not_from(&out));
        assert!(closed(&out, 3) && !closed(&out, 1));
        assert_eq!(r.counters().closed_chatty, 2);
        // A joiner's answer and a host's offer are not theirs to send.
        say(&mut r, 4, 4, "join 0000000000000001", 5);
        let out = say(&mut r, 4, 4, "signal\nanswer\nv=0", 5);
        assert!(closed(&out, 4) && not_from(&out));
        say(&mut r, 5, 5, "join 0000000000000001", 6);
        let out = say(&mut r, 1, 1, "to 4\noffer\nv=0", 6);
        assert!(closed(&out, 1));
        assert!(sent(&out, 5).iter().all(|m| !m.starts_with("signal")));
        assert_eq!(r.counters().closed_bad, 2);
    }

    #[test]
    fn lifetimes_close_rooms_joiners_and_idle_sockets() {
        let limits = RoomLimits {
            room_lifetime: Duration::from_secs(100),
            joiner_lifetime: Duration::from_secs(10),
            unplaced_lifetime: Duration::from_secs(5),
            ..RoomLimits::default()
        };
        let mut r = rooms(limits);
        say(&mut r, 1, 1, "host", 0);
        say(&mut r, 2, 2, "join 0000000000000001", 1_000);
        say(&mut r, 3, 3, "ping", 2_000);
        let mut out = Vec::new();
        r.tick(7_000, &mut out);
        assert!(closed(&out, 3) && !closed(&out, 2) && !closed(&out, 1));
        let mut out = Vec::new();
        r.tick(11_000, &mut out);
        assert_eq!(sent(&out, 2), ["closed timeout"]);
        assert_eq!(sent(&out, 1), ["left 1"]);
        let mut out = Vec::new();
        r.tick(100_000, &mut out);
        assert_eq!(sent(&out, 1), ["expired"]);
        assert!(closed(&out, 1));
        assert_eq!((r.rooms(), r.sockets()), (0, 0));
        assert_eq!(r.counters().expired, 1);
    }

    #[test]
    fn the_counts_per_address_are_capped() {
        let limits = RoomLimits {
            tracked_addresses: 2,
            ..RoomLimits::default()
        };
        let mut r = rooms(limits);
        say(&mut r, 1, 1, "join ZZZZZZZZZZZZZZZZ", 0);
        say(&mut r, 2, 2, "join ZZZZZZZZZZZZZZZZ", 0);
        assert_eq!(
            sent(&say(&mut r, 3, 3, "join ZZZZZZZZZZZZZZZZ", 0), 3),
            ["refused rate"]
        );
        // Once a minute has passed, the old counts make room.
        assert_eq!(
            sent(&say(&mut r, 3, 3, "join ZZZZZZZZZZZZZZZZ", 60_000), 3),
            ["refused no-room"]
        );
    }
}
