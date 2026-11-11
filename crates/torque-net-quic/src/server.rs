//! The server's end: a `quinn_proto::Endpoint` behind one socket, every connection named to the host by a stand-in address,
//! the engine's datagrams on QUIC DATAGRAM frames both ways. The host's receive pass drives it ([`Transport::recv`] pumps
//! when nothing waits), a dedicated server's waits too ([`Transport::service`]).
//!
//! **Handshakes.** Each Initial that may open a connection is decided as it arrives ([`Incoming`]). Only established
//! connections count against `max_connections` and the per-address cap, so a flood of Initials from spoofed addresses
//! can neither fill the server nor lock an address out. An unvalidated Initial (no Retry token) is answered with a Retry,
//! which costs the server no state, when handshakes come faster than the caps ([`RetryGate`]), when the handshakes in
//! progress have reached their cap, when its address already holds an established connection, or when the server is full;
//! a validated one is dropped when handshakes come faster than the caps, refused when the server is full or its address
//! or network holds its share of connections or of validated handshakes in progress, and when the handshakes in progress
//! are at their cap it takes the place of the oldest unvalidated one, whoever's (a forged address never finishes its
//! handshake), else of a validated one from its own address or network only, else is refused. Unvalidated handshakes may
//! hold at most half the places (past that every unvalidated Initial gets a Retry), so a forged flood neither fills the
//! cap nor keeps validated newcomers out. A handshake not finished within `handshake_timeout` is dropped. The
//! address check (a ban list) and IPv6 refuse either kind. quinn-proto's anti-amplification limit holds for every
//! connection (at most three times the bytes received before the address is validated), and connection ids are its own
//! random 8-byte ones: a peer's address is never a connection's identity.
//!
//! **Inbox.** Application datagrams wait for the host per connection, each connection's backlog capped on its own in
//! datagrams and in bytes (`inbox_per_connection`, `inbox_bytes_per_connection`): a peer that floods loses only its own
//! datagrams, never a quiet peer's. A datagram over [`QUIC_MAX_DATAGRAM`] (a DATAGRAM frame may be far larger) is dropped
//! on receipt, and nothing is queued from a connection that is not established or was told gone.
//!
//! **Migration.** A connection whose peer address moves keeps going. quinn-proto reports no "path validated" event, so the
//! per-address cap and the address check run again when it switches the path, which is on the first non-probing packet
//! from the new address, before the path challenge completes; a connection they refuse is closed. A peer that forges its
//! source address can so move a connection to an address it does not hold, and have it closed by the checks there, but
//! cannot read from it. The host counts the connection by its current address ([`Transport::peer_ip`]).

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use quinn_proto::{
    ConnectionHandle, DatagramEvent, Endpoint, EndpointEvent, Event, Incoming, SendDatagramError,
    VarInt,
};
use torque_net::engine::{ChannelBinding, LinkFacts, LinkKind, ServerIdentity};
use torque_server::net::transport::{BindError, Recv, Transport};

use crate::clock::{Clock, SystemClock};
use crate::identity::ServerKey;
use crate::io::{Io, UdpIo};
use crate::retry::{Load, RetryGate, network_of};
use crate::stand_in::{StandIns, is_stand_in};
use crate::tls::{QUIC_MAX_DATAGRAM, QuicLimits, TlsError};

/// The most datagrams one pass reads from the socket.
pub const READS_PER_PASS: usize = 256;

/// The receive buffer: the largest UDP payload the endpoint takes.
const RECV_BYTES: usize = 1 << 16;

/// The application error code a connection the address check refused is closed with.
const CLOSE_ADDRESS_REFUSED: u32 = 1;
/// The application error code a connection that finished its handshake into a full server (or a full address) is closed with.
const CLOSE_FULL: u32 = 2;

/// What the server has done so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Handshakes accepted (a connection made; it may still fail).
    pub accepted: u64,
    /// Connections whose handshake completed.
    pub established: u64,
    /// Handshakes answered with a Retry.
    pub retried: u64,
    /// Handshakes refused (full, over a per-address cap, the address check, IPv6).
    pub refused: u64,
    /// Handshakes in progress dropped: not finished in time, or evicted for a validated one.
    pub handshakes_dropped: u64,
    /// Handshakes dropped without an answer (over the caps with no Retry possible).
    pub ignored: u64,
    /// Datagrams from the socket the endpoint made nothing of.
    pub stray: u64,
    /// Application datagrams handed to the host.
    pub datagrams_in: u64,
    /// Application datagrams the host sent.
    pub datagrams_out: u64,
    /// Application datagrams dropped because their connection's backlog was full.
    pub inbox_dropped: u64,
    /// Connections whose peer address moved.
    pub migrations: u64,
    /// Connections closed because their new address failed the checks.
    pub closed_on_migration: u64,
    /// Connections that ended, whichever way.
    pub closed: u64,
}

struct Conn {
    conn: quinn_proto::Connection,
    stand_in: SocketAddr,
    /// The peer's address as last reported (migration moves it).
    remote: SocketAddr,
    /// The largest application datagram the connection takes now (none until the handshake is done).
    max_datagram: Option<usize>,
    binding: Option<ChannelBinding>,
    timeout: Option<Instant>,
    /// The handshake is done: the connection counts against the caps and its address holds it.
    established: bool,
    /// The peer proved its address with a token before the handshake began (an unvalidated handshake's address may be
    /// forged: it holds no share and is the first to make room).
    validated: bool,
    /// When the handshake began (dropped past `handshake_timeout` if it does not finish).
    opened: Instant,
    /// Its datagrams waiting for the host, and their bytes.
    queued: usize,
    queued_bytes: usize,
    /// The host was told the connection is gone.
    gone: bool,
}

/// A server endpoint over the socket `I` with the clock `C` (module docs). [`QuicServerTransport`] is the real one.
pub struct QuicServer<I: Io, C: Clock> {
    io: I,
    clock: C,
    endpoint: Endpoint,
    identity: ServerIdentity,
    limits: QuicLimits,
    conns: HashMap<ConnectionHandle, Conn>,
    by_stand_in: HashMap<SocketAddr, ConnectionHandle>,
    /// Established connections per address.
    per_ip: HashMap<IpAddr, usize>,
    established: usize,
    stand_ins: StandIns,
    retry: RetryGate,
    inbox: VecDeque<(SocketAddr, Bytes)>,
    gone: Vec<SocketAddr>,
    /// Stand-ins of connections that ended, given back only once the host has read `gone` (a new connection must not get a
    /// stand-in the host still has as a live peer).
    released: Vec<SocketAddr>,
    address_check: Option<Box<dyn Fn(IpAddr) -> bool>>,
    counters: Counters,
    buf: Vec<u8>,
    recv_buf: Vec<u8>,
}

/// The server endpoint over a real UDP socket and the system clock.
pub type QuicServerTransport = QuicServer<UdpIo, SystemClock>;

impl<I: Io, C: Clock> QuicServer<I, C> {
    /// An endpoint on `key` over `io` (not yet bound: [`Transport::bind`] opens it), its RNG seeded with `seed` (none: the
    /// system's; tests seed it).
    ///
    /// # Errors
    /// The TLS config could not be made from the key.
    pub fn new(
        io: I,
        clock: C,
        key: &ServerKey,
        limits: QuicLimits,
        seed: Option<[u8; 32]>,
    ) -> Result<QuicServer<I, C>, TlsError> {
        let config = crate::tls::server_config(key, &limits, &clock)?;
        let endpoint = Endpoint::new(
            Arc::new(crate::tls::endpoint_config(seed)),
            Some(Arc::new(config)),
            true,
            None,
        );
        let now = clock.now();
        Ok(QuicServer {
            io,
            clock,
            endpoint,
            identity: key.identity(),
            retry: RetryGate::new(
                now,
                limits.handshakes_per_second,
                limits.handshakes_per_network_per_second,
            ),
            limits,
            conns: HashMap::new(),
            by_stand_in: HashMap::new(),
            per_ip: HashMap::new(),
            established: 0,
            stand_ins: StandIns::default(),
            inbox: VecDeque::new(),
            gone: Vec::new(),
            released: Vec::new(),
            address_check: None,
            counters: Counters::default(),
            buf: Vec::with_capacity(RECV_BYTES),
            recv_buf: vec![0; RECV_BYTES],
        })
    }

    /// Refuses handshakes from, and closes connections that move to, an address `check` says no to (a ban list).
    pub fn set_address_check(&mut self, check: impl Fn(IpAddr) -> bool + 'static) {
        self.address_check = Some(Box::new(check));
    }

    /// This server's identity.
    #[must_use]
    pub fn identity(&self) -> ServerIdentity {
        self.identity
    }

    #[must_use]
    pub fn counters(&self) -> Counters {
        self.counters
    }

    /// Connections held (handshaking, established or closing).
    #[must_use]
    pub fn connections(&self) -> usize {
        self.conns.len()
    }

    /// Connections whose handshake is done and that have not ended.
    #[must_use]
    pub fn established(&self) -> usize {
        self.established
    }

    /// Application datagrams waiting for the host.
    #[must_use]
    pub fn inbox_len(&self) -> usize {
        self.inbox.len()
    }

    /// The caps.
    #[must_use]
    pub fn limits(&self) -> &QuicLimits {
        &self.limits
    }

    /// The socket.
    pub fn io_mut(&mut self) -> &mut I {
        &mut self.io
    }

    /// Reads what the socket has, moves every connection along and sends what they have to send.
    pub fn pump(&mut self) {
        if self.io.local_addr().is_none() {
            return;
        }
        let now = self.clock.now();
        for _ in 0..READS_PER_PASS {
            let Some((len, from)) = self.io.recv(&mut self.recv_buf) else {
                break;
            };
            if from.is_ipv6() || is_stand_in(from) {
                self.counters.stray += 1;
                continue;
            }
            let data = BytesMut::from(&self.recv_buf[..len]);
            self.buf.clear();
            match self
                .endpoint
                .handle(now, from, None, None, data, &mut self.buf)
            {
                Some(DatagramEvent::NewConnection(incoming)) => self.incoming(incoming, now),
                Some(DatagramEvent::ConnectionEvent(ch, event)) => {
                    if let Some(c) = self.conns.get_mut(&ch) {
                        c.conn.handle_event(event);
                        self.drive(ch, now);
                    }
                }
                Some(DatagramEvent::Response(transmit)) => {
                    self.io
                        .send(transmit.destination, &self.buf[..transmit.size]);
                }
                None => self.counters.stray += 1,
            }
        }
        let due: Vec<ConnectionHandle> = self
            .conns
            .iter()
            .filter(|(_, c)| c.timeout.is_some_and(|t| t <= now))
            .map(|(&ch, _)| ch)
            .collect();
        for ch in due {
            if let Some(c) = self.conns.get_mut(&ch) {
                c.conn.handle_timeout(now);
                self.drive(ch, now);
            }
        }
        let stale: Vec<ConnectionHandle> = self
            .conns
            .iter()
            .filter(|(_, c)| {
                !c.established
                    && now.saturating_duration_since(c.opened) >= self.limits.handshake_timeout
            })
            .map(|(&ch, _)| ch)
            .collect();
        for ch in stale {
            self.drop_handshake(ch);
        }
    }

    /// Whether `ip` may hold one more established connection: under its share, and not banned.
    fn ip_allowed(&self, ip: IpAddr) -> bool {
        ip.is_ipv4()
            && self.ip_count(ip) < self.limits.connections_per_ip
            && self.address_check.as_ref().is_none_or(|check| check(ip))
    }

    fn ip_count(&self, ip: IpAddr) -> usize {
        self.per_ip.get(&ip).copied().unwrap_or(0)
    }

    fn release_ip(&mut self, ip: IpAddr) {
        if let Some(n) = self.per_ip.get_mut(&ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.per_ip.remove(&ip);
            }
        }
    }

    /// Handshakes in progress.
    fn in_progress(&self) -> usize {
        self.conns.len() - self.established
    }

    /// Validated handshakes in progress from `ip`, and from its network.
    fn in_progress_from(&self, ip: IpAddr) -> (usize, usize) {
        let network = network_of(ip);
        self.conns
            .values()
            .filter(|c| !c.established && c.validated)
            .fold((0, 0), |(same, near), c| {
                let at = c.remote.ip();
                (
                    same + usize::from(at == ip),
                    near + usize::from(network_of(at) == network),
                )
            })
    }

    /// What a validated newcomer from `ip` may take the place of when the handshakes in progress are at their cap: the
    /// oldest unvalidated handshake, whoever's (its address may be forged, and a forged one never finishes), else the oldest
    /// validated one from `ip` itself, else from its network. Never another address's validated handshake.
    fn oldest_handshake_near(&self, ip: IpAddr) -> Option<ConnectionHandle> {
        let network = network_of(ip);
        let oldest = |keep: &dyn Fn(&Conn) -> bool| {
            self.conns
                .iter()
                .filter(|(_, c)| !c.established && keep(c))
                .min_by_key(|(_, c)| c.opened)
                .map(|(&ch, _)| ch)
        };
        oldest(&|c| !c.validated)
            .or_else(|| oldest(&|c| c.validated && c.remote.ip() == ip))
            .or_else(|| oldest(&|c| c.validated && network_of(c.remote.ip()) == network))
    }

    /// Drops a handshake that did not finish: the host never saw it, nothing waits for it to drain.
    fn drop_handshake(&mut self, ch: ConnectionHandle) {
        let now = self.clock.now();
        if let Some(c) = self.conns.get_mut(&ch) {
            c.conn.close(
                now,
                VarInt::from_u32(0),
                Bytes::from_static(b"handshake timed out"),
            );
            c.gone = true;
            self.gone.push(c.stand_in);
            self.counters.closed += 1;
            self.counters.handshakes_dropped += 1;
        }
        self.drive(ch, now);
        self.endpoint.handle_event(ch, EndpointEvent::drained());
        self.remove(ch);
    }

    fn refuse(&mut self, incoming: Incoming) {
        self.buf.clear();
        let transmit = self.endpoint.refuse(incoming, &mut self.buf);
        self.io
            .send(transmit.destination, &self.buf[..transmit.size]);
        self.counters.refused += 1;
    }

    /// Asks the sender to prove its address first (a Retry), or drops the Initial when a Retry is not allowed.
    fn retry(&mut self, incoming: Incoming) {
        if !incoming.may_retry() {
            self.endpoint.ignore(incoming);
            self.counters.ignored += 1;
            return;
        }
        self.buf.clear();
        match self.endpoint.retry(incoming, &mut self.buf) {
            Ok(transmit) => {
                self.io
                    .send(transmit.destination, &self.buf[..transmit.size]);
                self.counters.retried += 1;
            }
            Err(e) => {
                self.endpoint.ignore(e.into_incoming());
                self.counters.ignored += 1;
            }
        }
    }

    fn incoming(&mut self, incoming: Incoming, now: Instant) {
        let remote = incoming.remote_address();
        let banned = remote.is_ipv6()
            || self
                .address_check
                .as_ref()
                .is_some_and(|check| !check(remote.ip()));
        if banned {
            return self.refuse(incoming);
        }
        let full = self.established >= self.limits.max_connections;
        let share_taken = self.ip_count(remote.ip()) >= self.limits.connections_per_ip;
        let validated = incoming.remote_address_validated();
        let rate = self.retry.arrival(now, remote.ip(), validated);
        if !validated {
            // Nothing is held for an address that has not proved itself while the server is busy, full, or already
            // holds a connection from it; a Retry costs the server no state.
            let busy = rate == Load::Busy
                // Unvalidated handshakes may hold at most half the places; a forged flood never fills them.
                || self.in_progress() >= self.limits.handshakes_in_progress / 2
                || self.ip_count(remote.ip()) > 0
                || full;
            if busy {
                return self.retry(incoming);
            }
        } else {
            if rate == Load::Busy {
                // Proven address or not, far faster than the caps is dropped.
                self.endpoint.ignore(incoming);
                self.counters.ignored += 1;
                return;
            }
            let (same, near) = self.in_progress_from(remote.ip());
            if full
                || share_taken
                || same >= self.limits.handshakes_in_progress_per_ip
                || near >= self.limits.handshakes_in_progress_per_network
            {
                return self.refuse(incoming);
            }
            if self.in_progress() >= self.limits.handshakes_in_progress {
                // At the cap a newcomer may take the place of a handshake from its own address or network only.
                let Some(oldest) = self.oldest_handshake_near(remote.ip()) else {
                    return self.refuse(incoming);
                };
                self.drop_handshake(oldest);
            }
        }
        self.buf.clear();
        match self.endpoint.accept(incoming, now, &mut self.buf, None) {
            Ok((ch, conn)) => {
                let stand_in = self.stand_ins.take(remote.ip());
                self.by_stand_in.insert(stand_in, ch);
                self.conns.insert(
                    ch,
                    Conn {
                        conn,
                        stand_in,
                        remote,
                        max_datagram: None,
                        binding: None,
                        timeout: None,
                        established: false,
                        validated,
                        opened: now,
                        queued: 0,
                        queued_bytes: 0,
                        gone: false,
                    },
                );
                self.counters.accepted += 1;
                self.drive(ch, now);
            }
            Err(e) => {
                if let Some(transmit) = e.response {
                    self.io
                        .send(transmit.destination, &self.buf[..transmit.size]);
                }
                self.counters.refused += 1;
            }
        }
    }

    /// Moves connection `ch` along: its endpoint events, its own events, the checks of a moved address, what it has to send,
    /// its next timeout.
    fn drive(&mut self, ch: ConnectionHandle, now: Instant) {
        let mut drained = self.poll_events(ch);
        if self.check_moved(ch, now) {
            // The close is an event of its own.
            drained |= self.poll_events(ch);
        }
        let Some(c) = self.conns.get_mut(&ch) else {
            return;
        };
        loop {
            self.buf.clear();
            let Some(transmit) = c.conn.poll_transmit(now, 1, &mut self.buf) else {
                break;
            };
            self.io
                .send(transmit.destination, &self.buf[..transmit.size]);
        }
        c.max_datagram = c.conn.datagrams().max_size();
        c.timeout = c.conn.poll_timeout();
        if drained || c.conn.is_drained() {
            self.remove(ch);
        }
    }

    /// Connection `ch`'s endpoint events and its own; whether it is drained.
    fn poll_events(&mut self, ch: ConnectionHandle) -> bool {
        let Some(c) = self.conns.get_mut(&ch) else {
            return false;
        };
        let mut drained = false;
        while let Some(event) = c.conn.poll_endpoint_events() {
            drained |= event.is_drained();
            if let Some(back) = self.endpoint.handle_event(ch, event) {
                c.conn.handle_event(back);
            }
        }
        while let Some(event) = c.conn.poll() {
            match event {
                Event::Connected => {
                    let mut binding = [0; 32];
                    c.binding = c
                        .conn
                        .crypto_session()
                        .export_keying_material(&mut binding, ChannelBinding::LABEL, &[])
                        .ok()
                        .map(|()| ChannelBinding(binding));
                    // The address proved itself: the connection counts against the caps from here. Handshakes that began
                    // together are checked against them only now, so the caps hold whatever the order they finish in.
                    let over = self.established >= self.limits.max_connections
                        || self.per_ip.get(&c.remote.ip()).copied().unwrap_or(0)
                            >= self.limits.connections_per_ip;
                    if over {
                        c.conn.close(
                            self.clock.now(),
                            VarInt::from_u32(CLOSE_FULL),
                            Bytes::from_static(b"server full"),
                        );
                        // The host hears of every connection that ends, whether or not it saw its stand-in.
                        c.gone = true;
                        self.gone.push(c.stand_in);
                        self.counters.refused += 1;
                        self.counters.closed += 1;
                        continue;
                    }
                    c.established = true;
                    self.established += 1;
                    *self.per_ip.entry(c.remote.ip()).or_insert(0) += 1;
                    self.counters.established += 1;
                }
                Event::ConnectionLost { .. } => {
                    if !c.gone {
                        c.gone = true;
                        self.gone.push(c.stand_in);
                        self.counters.closed += 1;
                    }
                }
                Event::DatagramReceived => {
                    while let Some(data) = c.conn.datagrams().recv() {
                        if !c.established || c.gone {
                            // Nothing reaches the host from a connection it has not been given or has been told is gone.
                            self.counters.inbox_dropped += 1;
                            continue;
                        }
                        if data.len() > QUIC_MAX_DATAGRAM
                            || c.queued >= self.limits.inbox_per_connection
                            || c.queued_bytes + data.len() > self.limits.inbox_bytes_per_connection
                        {
                            // Too large for the engine, or this connection's own backlog is full: its datagram goes,
                            // nobody else's.
                            self.counters.inbox_dropped += 1;
                            continue;
                        }
                        c.queued += 1;
                        c.queued_bytes += data.len();
                        self.inbox.push_back((c.stand_in, data));
                        self.counters.datagrams_in += 1;
                    }
                }
                Event::HandshakeDataReady | Event::Stream(_) | Event::DatagramsUnblocked => {}
            }
        }
        drained
    }

    /// Runs the per-address checks again for connection `ch` when its peer's address moved; true when they refused it and
    /// the connection was closed.
    fn check_moved(&mut self, ch: ConnectionHandle, now: Instant) -> bool {
        let Some(c) = self.conns.get_mut(&ch) else {
            return false;
        };
        let remote = c.conn.remote_address();
        if remote != c.remote && !c.gone {
            self.counters.migrations += 1;
            let old = c.remote;
            c.remote = remote;
            let established = c.established;
            if established {
                self.release_ip(old.ip());
            }
            let allowed = self.ip_allowed(remote.ip());
            if established {
                *self.per_ip.entry(remote.ip()).or_insert(0) += 1;
            }
            let Some(c) = self.conns.get_mut(&ch) else {
                return false;
            };
            if !allowed {
                c.conn.close(
                    now,
                    VarInt::from_u32(CLOSE_ADDRESS_REFUSED),
                    Bytes::from_static(b"address refused"),
                );
                // A local close raises no event of its own: the host hears of it now, the connection drains on its timer.
                c.gone = true;
                self.gone.push(c.stand_in);
                self.counters.closed += 1;
                self.counters.closed_on_migration += 1;
                return true;
            }
        }
        false
    }

    fn remove(&mut self, ch: ConnectionHandle) {
        let Some(c) = self.conns.remove(&ch) else {
            return;
        };
        self.by_stand_in.remove(&c.stand_in);
        // Given back once the host has read `gone` (see `take_gone`).
        self.released.push(c.stand_in);
        self.inbox.retain(|(from, _)| *from != c.stand_in);
        if c.established {
            self.established -= 1;
            self.release_ip(c.remote.ip());
        }
        if !c.gone {
            self.gone.push(c.stand_in);
            self.counters.closed += 1;
        }
    }

    fn close_all(&mut self) {
        let now = self.clock.now();
        let handles: Vec<ConnectionHandle> = self.conns.keys().copied().collect();
        for ch in handles {
            if let Some(c) = self.conns.get_mut(&ch) {
                c.conn
                    .close(now, VarInt::from_u32(0), Bytes::from_static(b"closed"));
                self.drive(ch, now);
            }
            // The host is gone with the socket: nothing waits for the draining period.
            self.endpoint.handle_event(ch, EndpointEvent::drained());
            self.remove(ch);
        }
        self.inbox.clear();
    }
}

impl<I: Io, C: Clock> Transport for QuicServer<I, C> {
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        self.close_all();
        self.io.bind(ip, port)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        if self.inbox.is_empty() {
            self.pump();
        }
        let Some((from, data)) = self.inbox.pop_front() else {
            return Recv::Empty;
        };
        if let Some(c) = self
            .by_stand_in
            .get(&from)
            .and_then(|ch| self.conns.get_mut(ch))
        {
            c.queued = c.queued.saturating_sub(1);
            c.queued_bytes = c.queued_bytes.saturating_sub(data.len());
        }
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);
        Recv::Datagram { len, from }
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        let Some(&ch) = self.by_stand_in.get(&to) else {
            return false;
        };
        let Some(c) = self.conns.get_mut(&ch) else {
            return false;
        };
        if c.gone || c.conn.is_closed() {
            return false;
        }
        match c.conn.datagrams().send(Bytes::copy_from_slice(data), false) {
            Ok(()) => {
                self.counters.datagrams_out += 1;
                let now = self.clock.now();
                self.drive(ch, now);
                true
            }
            Err(
                SendDatagramError::Blocked(_)
                | SendDatagramError::TooLarge
                | SendDatagramError::Disabled
                | SendDatagramError::UnsupportedByPeer,
            ) => false,
        }
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.io.local_addr()
    }

    fn is_real(&self) -> bool {
        I::REAL
    }

    fn close(&mut self) {
        self.close_all();
        self.io.close();
    }

    fn service(&mut self) {
        self.pump();
    }

    fn wants_service(&self) -> bool {
        self.io.local_addr().is_some()
    }

    /// The host reads what ended; the stand-ins of everything that ended before this call may be used again after it.
    fn take_gone(&mut self) -> Vec<SocketAddr> {
        for stand_in in self.released.drain(..) {
            self.stand_ins.give_back(stand_in);
        }
        std::mem::take(&mut self.gone)
    }

    fn status(&mut self) -> Vec<String> {
        let c = self.counters;
        vec![format!(
            "quic: {} connections ({} established now, {} established in all, {} closed), handshakes {} accepted, {} retried, {} refused, {} ignored, {} dropped; {} datagrams in, {} out, {} dropped at the inbox, {} stray; {} migrations ({} refused)",
            self.conns.len(),
            self.established,
            c.established,
            c.closed,
            c.accepted,
            c.retried,
            c.refused,
            c.ignored,
            c.handshakes_dropped,
            c.datagrams_in,
            c.datagrams_out,
            c.inbox_dropped,
            c.stray,
            c.migrations,
            c.closed_on_migration
        )]
    }

    /// Every peer is a QUIC connection: its address validated, TLS 1.3 on the wire, QUIC's congestion control, this server's
    /// identity presented. Datagrams are the connection's current size, at most [`QUIC_MAX_DATAGRAM`].
    fn link_kind(&self, peer: SocketAddr) -> LinkKind {
        let max = self
            .by_stand_in
            .get(&peer)
            .and_then(|ch| self.conns.get(ch))
            .and_then(|c| c.max_datagram)
            .unwrap_or(QUIC_MAX_DATAGRAM);
        LinkKind {
            facts: LinkFacts {
                validated: true,
                secure: true,
                congestion_controlled: true,
                server_identity: Some(self.identity),
            },
            max_datagram: max.min(QUIC_MAX_DATAGRAM),
        }
    }

    /// The peer's current address behind its stand-in.
    fn peer_ip(&self, peer: SocketAddr) -> IpAddr {
        self.by_stand_in
            .get(&peer)
            .and_then(|ch| self.conns.get(ch))
            .map_or(peer.ip(), |c| c.remote.ip())
    }

    fn channel_binding(&self, peer: SocketAddr) -> Option<ChannelBinding> {
        self.by_stand_in
            .get(&peer)
            .and_then(|ch| self.conns.get(ch))
            .and_then(|c| c.binding)
    }
}
