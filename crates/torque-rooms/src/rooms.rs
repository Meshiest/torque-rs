//! The rooms: a state machine over the service's sockets, with no I/O. The service hands it each message with the socket's
//! address (a stand-in the web transport gave it) and the IP address it comes from, each socket that closed, and the time;
//! it answers with messages to send and sockets to close ([`Out`]).
//!
//! **What it keeps**: the live rooms (the host's socket, its joiners' sockets), each socket's part, and counts per address
//! over the present minute or second. Nothing outlives a room but those counts.
//!
//! **Hostile input.** A message that is not UTF-8, not one of the protocol's, too long, or not the sender's to send (a
//! joiner's `to`, a host's `signal`, a second `host` or `join`) closes the socket. Requests over a cap are refused with a
//! reason and the socket kept: rooms in all ([`RoomLimits::max_rooms`]), live rooms per address and new ones per address a
//! minute, joins per address a minute (a code is 40 bits, so guessing one takes about 2^40 / live rooms joins), joins that
//! name no room in a second from every address together, joiners waiting in one room. A joiner may send, and be sent, at
//! most [`RoomLimits::signals_per_joiner`] signaling messages each way (an offer, an answer and a few candidates are a
//! dozen), and a body must be a signaling message ([`torque_server::net::room::is_signal`]), so the service cannot carry
//! a game. Lifetimes: a room ([`RoomLimits::room_lifetime`], then its host is told `expired` and its socket closed), a
//! joiner's signaling ([`RoomLimits::joiner_lifetime`]), and a socket that has neither hosted nor joined
//! ([`RoomLimits::unplaced_lifetime`] from its first message). The counts per address are capped
//! ([`RoomLimits::tracked_addresses`]); when they are full a new address's requests are refused.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

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
    pub rooms_per_ip: usize,
    pub new_rooms_per_ip_per_min: u32,
    pub joiners_per_room: usize,
    pub joins_per_ip_per_min: u32,
    pub failed_joins_per_s: u32,
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
            rooms_per_ip: 4,
            new_rooms_per_ip_per_min: 8,
            joiners_per_room: 16,
            joins_per_ip_per_min: 20,
            failed_joins_per_s: 64,
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
    /// Joiners closed for too many signaling messages.
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
}

#[derive(Debug)]
struct Room {
    host: SocketAddr,
    host_ip: IpAddr,
    opened: Ms,
    next_peer: u32,
    joiners: HashMap<u32, Joiner>,
}

/// Counts per key in a window of time.
#[derive(Debug)]
struct Window {
    span: Ms,
    counts: HashMap<IpAddr, (Ms, u32)>,
}

impl Window {
    fn new(span: Duration) -> Window {
        Window {
            span: span.as_millis() as Ms,
            counts: HashMap::new(),
        }
    }

    /// Counts one for `ip`; false when that passes `cap` (or no room is left to count a new address).
    fn take(&mut self, ip: IpAddr, cap: u32, now: Ms, tracked: usize) -> bool {
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
}

/// The rooms (module docs).
pub struct Rooms {
    limits: RoomLimits,
    ice: Ice,
    rooms: HashMap<String, Room>,
    sockets: HashMap<SocketAddr, Socket>,
    rooms_of: HashMap<IpAddr, usize>,
    new_rooms: Window,
    joins: Window,
    failed: (Ms, u32),
    /// The ids TURN credentials are made for (one per room or joiner).
    next_id: u64,
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
            limits,
            ice,
            rooms: HashMap::new(),
            sockets: HashMap::new(),
            rooms_of: HashMap::new(),
            failed: (0, 0),
            next_id: 0,
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

    fn ice_for(&mut self, unix_now: u64) -> String {
        self.next_id += 1;
        let id = self.next_id.to_string();
        self.ice.servers(unix_now, &id)
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
        if self.rooms_of.get(&ip).copied().unwrap_or(0) >= limits.rooms_per_ip {
            return self.refuse(socket, "rate", out);
        }
        if !self.new_rooms.take(
            ip,
            limits.new_rooms_per_ip_per_min,
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
        let ice = self.ice_for(unix_now);
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
        *self.rooms_of.entry(ip).or_insert(0) += 1;
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
        let limits = &self.limits;
        if !self.joins.take(
            ip,
            limits.joins_per_ip_per_min,
            now,
            limits.tracked_addresses,
        ) {
            return self.refuse(socket, "rate", out);
        }
        if now.saturating_sub(self.failed.0) >= 1000 {
            self.failed = (now, 0);
        }
        if self.failed.1 >= limits.failed_joins_per_s {
            return self.refuse(socket, "busy", out);
        }
        let IpAddr::V4(v4) = ip else {
            // The web transport refuses IPv6 peers.
            return self.refuse(socket, "bad", out);
        };
        let room = read_code(code).and_then(|c| self.rooms.get_mut(&c).map(|r| (c, r)));
        let Some((code, room)) = room else {
            self.failed.1 += 1;
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
            },
        );
        let host = room.host;
        if let Some(s) = self.sockets.get_mut(&socket) {
            s.part = Part::Joiner { code, peer };
        }
        self.counters.joins += 1;
        let ice = self.ice_for(unix_now);
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
        if joiner.got > cap {
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
        let cap = self.limits.signals_per_joiner;
        let Some(room) = self.rooms.get_mut(code) else {
            return;
        };
        let host = room.host;
        let Some(joiner) = room.joiners.get_mut(&peer) else {
            return;
        };
        joiner.sent += 1;
        if joiner.sent > cap {
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
                if let Some(n) = self.rooms_of.get_mut(&room.host_ip) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        self.rooms_of.remove(&room.host_ip);
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

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([192, 0, 2, n])
    }

    /// Rooms whose codes count up (`00000001`, ...).
    fn rooms(limits: RoomLimits) -> Rooms {
        let mut n = 0u8;
        Rooms::with_random(
            limits,
            Ice::default(),
            Box::new(move |b| {
                n += 1;
                *b = [0, 0, 0, 0, 0, 0, 0, n];
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
        assert_eq!(sent(&out, 1), ["room 00000001 21600\n[]"]);
        let out = say(
            &mut r,
            2,
            2,
            &format!("join {}", show_code("00000001").to_lowercase()),
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
        say(&mut r, 3, 3, "join 00000001", 50);
        let mut out = Vec::new();
        r.gone(addr(1), &mut out);
        assert_eq!(sent(&out, 3), ["closed host-left"]);
        assert!(closed(&out, 3));
        assert_eq!((r.rooms(), r.sockets()), (0, 0));
        assert_eq!(
            sent(&say(&mut r, 4, 4, "join 00000001", 60), 4),
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
                *b = [0, 0, 0, 0, 0, 0, 0, n];
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
        assert_eq!(given(&say(&mut r, 6, 6, "join 0000-000Z", 0)), 0);
        // The host on `room` and the joiner on `joined`, each its own credential, expiring the TTL after the clock (the
        // tests' clock is 1,000,000).
        let out = say(&mut r, 1, 1, "host", 0);
        let room = &sent(&out, 1)[0];
        assert!(room.contains("\"username\":\"1000600:1\""), "{room}");
        let out = say(&mut r, 2, 2, "join 00000001", 0);
        assert_eq!(
            sent(&out, 1),
            ["peer 1 192.0.2.2"],
            "the host is given none"
        );
        let joined = &sent(&out, 2)[0];
        assert!(joined.contains("\"username\":\"1000600:2\""), "{joined}");
        // A join past the room's joiners is refused with none.
        let out = say(&mut r, 3, 3, "join 00000001", 0);
        assert_eq!(sent(&out, 3), ["refused full"]);
        assert_eq!(given(&out), 0);
    }

    #[test]
    fn messages_out_of_place_close_the_socket() {
        let mut r = rooms(RoomLimits::default());
        say(&mut r, 1, 1, "host", 0);
        say(&mut r, 2, 2, "join 00000001", 0);
        for (who, text) in [
            (2, "to 1\nanswer\nv=0"),      // a joiner may not address anyone
            (1, "signal\noffer\nv=0"),     // a host may not send a joiner's message
            (1, "host"),                   // one room a socket
            (2, "join 00000001"),          // one join a socket
            (5, "to 1\nanswer\nv=0"),      // nobody's message
            (6, "drop 1"),                 // nor this
            (7, "signal\nGET / HTTP/1.1"), // a body that is not signaling
            (8, "hello"),
        ] {
            let mut r2 = rooms(RoomLimits::default());
            say(&mut r2, 1, 1, "host", 0);
            say(&mut r2, 2, 2, "join 00000001", 0);
            let out = say(&mut r2, who, who as u8, text, 1);
            assert!(closed(&out, who), "{text:?} from {who}");
        }
        let mut out = Vec::new();
        r.message(addr(9), ip(9), &[0xff, 0xfe], 0, 0, &mut out);
        assert!(closed(&out, 9), "not UTF-8");
        let long = format!("signal\noffer\n{}", "a".repeat(MAX_BODY));
        assert!(closed(&say(&mut r, 2, 2, &long, 1), 2), "too long");
        // The host is told its joiner left.
        assert_eq!(r.joiners("00000001"), 0);
    }

    #[test]
    fn caps_refuse_rooms_and_joins() {
        let limits = RoomLimits {
            max_rooms: 3,
            rooms_per_ip: 2,
            new_rooms_per_ip_per_min: 2,
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
        say(&mut r, 10, 10, "join 00000002", 70_000);
        say(&mut r, 11, 11, "join 00000002", 70_000);
        assert_eq!(
            sent(&say(&mut r, 12, 12, "join 00000002", 70_000), 12),
            ["refused full"]
        );
        // Joins per address a minute, whether they find a room or not.
        for n in 0..4 {
            assert_eq!(
                sent(&say(&mut r, 20 + n, 20, "join ZZZZZZZZ", 70_000), 20 + n),
                ["refused no-room"]
            );
        }
        assert_eq!(
            sent(&say(&mut r, 30, 20, "join 00000002", 70_000), 30),
            ["refused rate"]
        );
        // Failed joins in a second from every address together (the code-guessing cap).
        let mut refusals = Vec::new();
        for n in 0..3u16 {
            refusals.extend(sent(
                &say(&mut r, 40 + n, 40 + n as u8, "join ZZZZZZZZ", 70_000),
                40 + n,
            ));
        }
        assert_eq!(
            refusals,
            ["refused no-room", "refused busy", "refused busy"]
        );
        assert_eq!(
            sent(&say(&mut r, 50, 50, "join ZZZZZZZZ", 71_000), 50),
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
        say(&mut r, 2, 2, "join 00000001", 0);
        say(&mut r, 3, 3, "join 00000001", 0);
        for _ in 0..3 {
            assert_eq!(
                sent(&say(&mut r, 2, 2, "signal\ncandidate\nx", 1), 1).len(),
                1
            );
        }
        let out = say(&mut r, 2, 2, "signal\ncandidate\nx", 1);
        assert!(closed(&out, 2));
        assert_eq!(sent(&out, 1), ["left 1"]);
        for _ in 0..3 {
            assert_eq!(
                sent(&say(&mut r, 1, 1, "to 2\ncandidate\nx", 1), 3).len(),
                1
            );
        }
        let out = say(&mut r, 1, 1, "to 2\ncandidate\nx", 1);
        assert!(
            closed(&out, 3) && !closed(&out, 1),
            "the joiner goes, not the host"
        );
        // A host's drop ends that joiner's signaling.
        say(&mut r, 4, 4, "join 00000001", 2);
        let out = say(&mut r, 1, 1, "drop 3", 3);
        assert_eq!(sent(&out, 4), ["closed dropped"]);
        assert!(closed(&out, 4));
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
        say(&mut r, 2, 2, "join 00000001", 1_000);
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
        say(&mut r, 1, 1, "join ZZZZZZZZ", 0);
        say(&mut r, 2, 2, "join ZZZZZZZZ", 0);
        assert_eq!(
            sent(&say(&mut r, 3, 3, "join ZZZZZZZZ", 0), 3),
            ["refused rate"]
        );
        // Once a minute has passed, the old counts make room.
        assert_eq!(
            sent(&say(&mut r, 3, 3, "join ZZZZZZZZ", 60_000), 3),
            ["refused no-room"]
        );
    }
}
