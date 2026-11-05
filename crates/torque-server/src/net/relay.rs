//! The transport of a game server whose links are held by someone else (the `p2p` feature): a browser's host, whose
//! server runs in a Web Worker while the page's main thread holds the room service's socket and the joiners' data
//! channels (browsers make no `RTCPeerConnection` in a worker). The page tells the transport what happens to each link
//! ([`RelayTransport::open`], [`RelayTransport::datagram`], [`RelayTransport::close`]) and sends what it asks
//! ([`RelayTransport::take_out`]); every cap is here, in the server's own code.
//!
//! **Links.** Each link has a number the page chose; link [`LOCAL_LINK`] is the host player's own client (the page's
//! main thread). A joiner's link goes by a stand-in address in `240.0.0.0/4` of the IP address the room service saw it
//! come from (a keyed hash: the same IP for every link of one address while the transport lives), so the host's
//! per-address limits and the scripts' bans hold for it; each link has its own port.
//!
//! **Hostile input.** A joiner's data channel is DTLS: what comes on it is from that joiner. Every message is counted
//! with the bytes it took on the wire ([`RelayLimits::wire_room`] beyond its own: DTLS, SCTP and UDP headers): a message
//! larger than a datagram, or more messages or bytes a second than the caps, closes the link (the page closes the
//! channel). Links are capped in all, per address and in new links a second; past them a link is refused and the page
//! closes it. At most [`RelayLimits::queue`] datagrams wait for the host and [`RelayLimits::out_queue`] for the page, the
//! rest dropped as a full socket drops them. A link that closes is reported gone ([`Transport::take_gone`]).

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, RandomState};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use torque_net::cap::{NET_MAX_DATAGRAM, NET_PACKETS_PER_CONN_PER_S};
use web_time::{Duration, Instant};

use super::transport::{BindError, Recv, Transport};

/// The host player's own client's link.
pub const LOCAL_LINK: u32 = 0;

/// A relay transport's caps (module docs). The rates match UDP's per-connection caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayLimits {
    pub max_links: usize,
    pub links_per_ip: usize,
    pub new_links_per_s: u32,
    pub messages_per_s: u32,
    /// Counted as they took the wire: each message's bytes and [`RelayLimits::wire_room`].
    pub bytes_per_s: usize,
    pub wire_room: usize,
    pub max_message: usize,
    pub queue: usize,
    pub out_queue: usize,
}

impl Default for RelayLimits {
    fn default() -> RelayLimits {
        RelayLimits {
            max_links: 64,
            links_per_ip: 4,
            new_links_per_s: 16,
            messages_per_s: NET_PACKETS_PER_CONN_PER_S,
            bytes_per_s: NET_PACKETS_PER_CONN_PER_S as usize * (NET_MAX_DATAGRAM + 128),
            wire_room: 128,
            max_message: NET_MAX_DATAGRAM,
            queue: 4096,
            out_queue: 4096,
        }
    }
}

/// Why a link was closed by the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    Rate,
    Oversize,
}

/// What a relay transport has done, for tests and the page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayCounters {
    pub opened: u64,
    pub refused: u64,
    pub messages: u64,
    pub sent: u64,
    pub dropped: u64,
    pub closed_rate: u64,
    pub closed_oversize: u64,
}

#[derive(Debug)]
struct Link {
    ip: IpAddr,
    stand_in: SocketAddr,
    window: Instant,
    messages: u32,
    bytes: usize,
}

/// A relay transport (module docs).
#[derive(Debug)]
pub struct RelayTransport {
    limits: RelayLimits,
    key: RandomState,
    links: HashMap<u32, Link>,
    by_stand_in: HashMap<SocketAddr, u32>,
    next_port: u16,
    new_links: Option<(Instant, u32)>,
    inbox: VecDeque<(SocketAddr, Vec<u8>)>,
    out: VecDeque<(u32, Vec<u8>)>,
    closed: Vec<(u32, Closed)>,
    gone: Vec<SocketAddr>,
    bound: bool,
    counters: RelayCounters,
}

impl RelayTransport {
    pub fn new(limits: RelayLimits) -> RelayTransport {
        RelayTransport {
            limits,
            key: RandomState::new(),
            links: HashMap::new(),
            by_stand_in: HashMap::new(),
            next_port: 0,
            new_links: None,
            inbox: VecDeque::new(),
            out: VecDeque::new(),
            closed: Vec::new(),
            gone: Vec::new(),
            bound: false,
            counters: RelayCounters::default(),
        }
    }

    pub fn counters(&self) -> RelayCounters {
        self.counters
    }

    /// The links open.
    pub fn links(&self) -> usize {
        self.links.len()
    }

    /// The stand-in address of link `id`.
    pub fn stand_in(&self, id: u32) -> Option<SocketAddr> {
        self.links.get(&id).map(|l| l.stand_in)
    }

    fn ip_of(&self, ip: IpAddr) -> Ipv4Addr {
        let hash = self.key.hash_one(ip) as u32 & ((1 << 28) - 1);
        Ipv4Addr::from(u32::from(Ipv4Addr::new(240, 0, 0, 0)) | hash)
    }

    /// Link `id` opened, from `ip` (the address the room service saw; the local link's is the loopback address). False:
    /// refused (a cap, or the number is taken), and the page closes it.
    pub fn open(&mut self, id: u32, ip: IpAddr, now: Instant) -> bool {
        let limits = &self.limits;
        let window = match self.new_links {
            Some((start, n)) if now.duration_since(start) < Duration::from_secs(1) => (start, n),
            _ => (now, 0),
        };
        self.new_links = Some(window);
        let from_ip = self.links.values().filter(|l| l.ip == ip).count();
        let room = !self.links.contains_key(&id)
            && self.links.len() < limits.max_links
            && (id == LOCAL_LINK
                || (from_ip < limits.links_per_ip && window.1 < limits.new_links_per_s));
        if !room {
            self.counters.refused += 1;
            return false;
        }
        if id != LOCAL_LINK {
            self.new_links = Some((window.0, window.1 + 1));
        }
        let stand_in_ip = IpAddr::V4(self.ip_of(ip));
        let stand_in = loop {
            self.next_port = self.next_port.checked_add(1).unwrap_or(1);
            let addr = SocketAddr::new(stand_in_ip, self.next_port);
            if !self.by_stand_in.contains_key(&addr) {
                break addr;
            }
        };
        self.links.insert(
            id,
            Link {
                ip,
                stand_in,
                window: now,
                messages: 0,
                bytes: 0,
            },
        );
        self.by_stand_in.insert(stand_in, id);
        self.counters.opened += 1;
        true
    }

    /// Link `id` closed (its channel did): the host is told.
    pub fn close(&mut self, id: u32) {
        let Some(link) = self.links.remove(&id) else {
            return;
        };
        self.by_stand_in.remove(&link.stand_in);
        self.out.retain(|(to, _)| *to != id);
        if self.gone.len() < 4096 {
            self.gone.push(link.stand_in);
        }
    }

    /// A message on link `id`. False: the link broke a cap and is closed (the page closes its channel).
    pub fn datagram(&mut self, id: u32, data: &[u8], now: Instant) -> bool {
        let limits = &self.limits;
        let Some(link) = self.links.get_mut(&id) else {
            return false;
        };
        if now.duration_since(link.window) >= Duration::from_secs(1) {
            link.window = now;
            link.messages = 0;
            link.bytes = 0;
        }
        link.messages += 1;
        link.bytes += data.len() + limits.wire_room;
        let why = if data.len() > limits.max_message {
            Some(Closed::Oversize)
        } else if link.messages > limits.messages_per_s || link.bytes > limits.bytes_per_s {
            Some(Closed::Rate)
        } else {
            None
        };
        if let Some(why) = why {
            match why {
                Closed::Rate => self.counters.closed_rate += 1,
                Closed::Oversize => self.counters.closed_oversize += 1,
            }
            if self.closed.len() < 4096 {
                self.closed.push((id, why));
            }
            self.close(id);
            return false;
        }
        let from = link.stand_in;
        if self.inbox.len() < limits.queue {
            self.counters.messages += 1;
            self.inbox.push_back((from, data.to_vec()));
        } else {
            self.counters.dropped += 1;
        }
        true
    }

    /// The datagrams to send, each with its link.
    pub fn take_out(&mut self) -> Vec<(u32, Vec<u8>)> {
        self.out.drain(..).collect()
    }

    /// The links this transport closed since the last call (the page closes their channels).
    pub fn take_closed(&mut self) -> Vec<(u32, Closed)> {
        std::mem::take(&mut self.closed)
    }
}

impl Transport for RelayTransport {
    fn bind(&mut self, _ip: IpAddr, _port: u16) -> Result<(), BindError> {
        self.bound = true;
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        let Some((from, data)) = self.inbox.pop_front() else {
            return Recv::Empty;
        };
        let len = data.len().min(buf.len());
        if let (Some(dst), Some(src)) = (buf.get_mut(..len), data.get(..len)) {
            dst.copy_from_slice(src);
        }
        Recv::Datagram { len, from }
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        let Some(&id) = self.by_stand_in.get(&to) else {
            return false;
        };
        if data.len() > NET_MAX_DATAGRAM || self.out.len() >= self.limits.out_queue {
            return false;
        }
        self.counters.sent += 1;
        self.out.push_back((id, data.to_vec()));
        true
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.bound
            .then(|| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
    }

    fn is_real(&self) -> bool {
        false
    }

    fn close(&mut self) {
        let all: Vec<u32> = self.links.keys().copied().collect();
        for id in all {
            self.close(id);
        }
        self.bound = false;
    }

    fn take_gone(&mut self) -> Vec<SocketAddr> {
        std::mem::take(&mut self.gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    const B: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));

    fn recv(t: &mut RelayTransport) -> Option<(SocketAddr, Vec<u8>)> {
        let mut buf = vec![0u8; NET_MAX_DATAGRAM + 1];
        match t.recv(&mut buf) {
            Recv::Datagram { len, from } => Some((from, buf[..len].to_vec())),
            _ => None,
        }
    }

    #[test]
    fn links_carry_datagrams_both_ways_under_stand_ins() {
        let now = Instant::now();
        let mut t = RelayTransport::new(RelayLimits::default());
        assert!(t.open(LOCAL_LINK, IpAddr::V4(Ipv4Addr::LOCALHOST), now));
        assert!(t.open(7, A, now));
        assert!(t.open(8, A, now));
        assert!(t.datagram(7, b"hi", now));
        let (from, data) = recv(&mut t).unwrap();
        assert_eq!(data, b"hi");
        assert_eq!(from, t.stand_in(7).unwrap());
        assert!(matches!(from.ip(), IpAddr::V4(v) if v.octets()[0] & 0xf0 == 240));
        assert_eq!(
            t.stand_in(7).unwrap().ip(),
            t.stand_in(8).unwrap().ip(),
            "one IP per address"
        );
        assert_ne!(t.stand_in(7), t.stand_in(8));
        assert!(t.send_to(from, b"back"));
        assert!(!t.send_to(SocketAddr::from(([240, 0, 0, 9], 1)), b"nobody"));
        assert_eq!(t.take_out(), [(7, b"back".to_vec())]);
        t.close(7);
        assert_eq!(t.take_gone(), [from]);
        assert!(!t.datagram(7, b"late", now));
        assert!(!t.send_to(from, b"late"));
    }

    #[test]
    fn caps_refuse_links_and_close_floods() {
        let now = Instant::now();
        let limits = RelayLimits {
            max_links: 4,
            links_per_ip: 2,
            new_links_per_s: 3,
            messages_per_s: 10,
            bytes_per_s: 10 * 228,
            wire_room: 128,
            ..RelayLimits::default()
        };
        let mut t = RelayTransport::new(limits);
        assert!(t.open(1, A, now));
        assert!(t.open(2, A, now));
        assert!(!t.open(3, A, now), "links per address");
        assert!(!t.open(1, B, now), "a number taken");
        assert!(t.open(4, B, now));
        assert!(!t.open(5, B, now), "new links a second");
        assert!(t.open(5, B, now + Duration::from_secs(1)));
        assert!(
            !t.open(
                6,
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 3)),
                now + Duration::from_secs(2)
            ),
            "links in all"
        );
        // Messages a second: the eleventh closes the link.
        for _ in 0..10 {
            assert!(t.datagram(1, &[0; 10], now));
        }
        assert!(!t.datagram(1, &[0; 10], now));
        // Bytes a second, counted with the wire's room: 100 bytes take 228.
        for _ in 0..10 {
            assert!(t.datagram(2, &[0; 100], now));
        }
        assert!(!t.datagram(2, &[0; 1], now));
        // A message larger than a datagram.
        assert!(!t.datagram(4, &[0; NET_MAX_DATAGRAM + 1], now));
        assert_eq!(
            t.take_closed(),
            [(1, Closed::Rate), (2, Closed::Rate), (4, Closed::Oversize)]
        );
        assert_eq!(t.links(), 1);
        assert_eq!(t.take_gone().len(), 3);
        // A full inbox drops, as a full socket does.
        let small = RelayLimits {
            queue: 2,
            ..RelayLimits::default()
        };
        let mut t = RelayTransport::new(small);
        t.open(1, A, now);
        for _ in 0..3 {
            assert!(t.datagram(1, b"x", now));
        }
        assert_eq!(t.counters().dropped, 1);
    }
}
