//! The web transport: browsers join a game over WebSocket, and the same port serves them the game's page over HTTP.
//!
//! [`WebTransport`] is a [`Transport`] over one non-blocking TCP listener. Every connection starts as HTTP: a `GET` of a file
//! is answered from [`WebFiles`] in memory (none with `--no-files`: the page then comes from another web server), and a
//! `GET /ws` that asks for the WebSocket upgrade becomes a game socket. A game socket stands for one client: the host sees
//! it at a stand-in address in a range no datagram comes from ([`stand_in`]: one IP per peer address, so the host's
//! per-address limits apply to it as to a UDP client, and a port per socket), and each binary message is one datagram,
//! both ways. [`MultiTransport`] polls a UDP socket and a web transport on the same
//! address and port: desktop and browser players in one game.
//!
//! **No async runtime.** Nothing blocks: a pass ([`WebTransport::pump`]) accepts what is waiting, then reads and writes
//! each connection as far as its socket allows. The host's receive pass runs one when its queue is empty, and a dedicated
//! server's wait between ticks runs them too ([`Transport::service`]), so that files go out between ticks.
//!
//! **TLS.** With a certificate and key ([`TlsFiles`]) every connection is TLS (`https` and `wss`, rustls). Without them
//! the bind policy allows only a private address (loopback and the LAN): plain `ws` and `http` never listen on a public one
//! ([`PolicyRefusal::PlainWeb`]).
//!
//! **Who may reach it** ([`Access`]). A request whose `Host` names neither an IP address, `localhost`, nor one of the
//! server's names is refused (a page whose name was pointed at this server's address cannot reach it), and a game socket
//! asked for by a page (its `Origin`) from elsewhere than the server's own page, or than the origins listed, is refused
//! with 403.
//!
//! **Hostile input.** The HTTP side's caps are [`http`]'s (request line, head, headers, `GET` only, no body, paths
//! normalized without traversal, files from memory). Each connection has its timeouts and caps ([`WebLimits`]): a head must
//! come within [`WebLimits::head_timeout`] of the connection's start or of the last response; a game socket that is quiet
//! for [`WebLimits::idle_timeout`] is closed; writes that make no progress for [`WebLimits::stall_timeout`], or a socket
//! whose unsent bytes pass [`WebLimits::write_buffer`] (a slow reader), close the connection; a message larger than a
//! datagram ([`NET_MAX_DATAGRAM`]) or more messages a second than UDP's per-connection cap
//! ([`NET_PACKETS_PER_CONN_PER_S`]) closes the socket. Connections are capped in all, per address, and in game sockets
//! per address, and new connections per address per second; one pass accepts at most [`WebLimits::accepts_per_pass`],
//! reads at most [`WebLimits::messages_per_pass`] messages of each socket and writes at most a few MiB to each connection,
//! so a pass's time is bounded whatever the peers send. IPv6 peers are refused, as by the bind policy.

pub mod http;
pub mod stand_in;
pub mod tls;

mod conn;

use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, TcpListener};

use torque_net::cap::{NET_LIMITER_ENTRIES, NET_MAX_DATAGRAM, NET_PACKETS_PER_CONN_PER_S};
use web_time::{Duration, Instant};

use super::transport::{BindError, BindPolicy, PolicyRefusal, Recv, Transport, UdpTransport};
use conn::{Conn, Pass};
pub use http::{Access, Origins, WEB_SOCKET_PATH, WebFiles, accept_key};
use stand_in::StandIns;
pub use stand_in::{STAND_IN_NET, is_stand_in};
pub use tls::{Tls, TlsFiles};

/// The caps and timeouts of a web transport's connections (module docs). The defaults match UDP's per-connection caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebLimits {
    /// The most connections open at once.
    pub max_connections: usize,
    /// The most connections open from one address.
    pub connections_per_ip: usize,
    /// The most game sockets open from one address.
    pub sockets_per_ip: usize,
    /// The most new connections one address may open in a second.
    pub connects_per_ip_per_s: u32,
    /// The most connections one pass accepts.
    pub accepts_per_pass: usize,
    /// How long a request head may take (and a TLS handshake before it).
    pub head_timeout: Duration,
    /// How long a game socket may stay quiet.
    pub idle_timeout: Duration,
    /// How long writes may make no progress.
    pub stall_timeout: Duration,
    /// The most messages (and control frames) a game socket may send in a second.
    pub messages_per_s: u32,
    /// The largest message a game socket may send.
    pub max_message: usize,
    /// The most unsent bytes a game socket may hold.
    pub write_buffer: usize,
    /// The most requests one connection may make.
    pub requests_per_connection: u32,
    /// The most messages one pass reads from one socket.
    pub messages_per_pass: usize,
    /// The most datagrams waiting for the host.
    pub queue: usize,
}

impl Default for WebLimits {
    fn default() -> WebLimits {
        WebLimits {
            max_connections: 128,
            connections_per_ip: 8,
            sockets_per_ip: 4,
            connects_per_ip_per_s: 16,
            accepts_per_pass: 16,
            head_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(20),
            stall_timeout: Duration::from_secs(10),
            messages_per_s: NET_PACKETS_PER_CONN_PER_S,
            max_message: NET_MAX_DATAGRAM,
            write_buffer: 256 << 10,
            requests_per_connection: 64,
            messages_per_pass: 64,
            queue: 4096,
        }
    }
}

/// How a web transport serves: its files (none: game sockets only), its TLS (none: plain, private addresses only), who may
/// reach it (the Host names, the pages' origins) and its limits.
#[derive(Debug, Clone, Default)]
pub struct WebSetup {
    pub files: Option<WebFiles>,
    pub tls: Option<Tls>,
    pub access: Access,
    pub limits: WebLimits,
}

/// What a web transport has done, for tests and the console.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebCounters {
    /// Connections taken.
    pub accepted: u64,
    /// Connections dropped as they came (past a cap or a rate, or from IPv6).
    pub refused: u64,
    /// Request heads read.
    pub requests: u64,
    /// Files served.
    pub files: u64,
    /// Requests refused (malformed, too large, not `GET`, not found, a bad upgrade).
    pub refusals: u64,
    /// Game sockets opened.
    pub upgrades: u64,
    /// Datagrams taken from game sockets.
    pub messages: u64,
    /// Datagrams dropped because the host's queue was full.
    pub queue_full: u64,
    /// Connections closed at the end of their exchange or by the other end.
    pub closed: u64,
    pub closed_timeout: u64,
    pub closed_stalled: u64,
    pub closed_oversize: u64,
    pub closed_rate: u64,
    pub closed_protocol: u64,
}

/// The web transport (module docs).
pub struct WebTransport {
    policy: BindPolicy,
    setup: WebSetup,
    listener: Option<TcpListener>,
    conns: Vec<Conn>,
    inbox: VecDeque<(SocketAddr, Vec<u8>)>,
    /// New connections per address in the present second.
    connects: HashMap<IpAddr, (Instant, u32)>,
    stand_ins: StandIns,
    counters: WebCounters,
    last_pump: Option<Instant>,
}

impl WebTransport {
    pub fn new(policy: BindPolicy, setup: WebSetup) -> WebTransport {
        WebTransport {
            policy,
            setup,
            listener: None,
            conns: Vec::new(),
            inbox: VecDeque::new(),
            connects: HashMap::new(),
            stand_ins: StandIns::default(),
            counters: WebCounters::default(),
            last_pump: None,
        }
    }

    pub fn counters(&self) -> WebCounters {
        self.counters
    }

    /// The connections open (HTTP and game sockets).
    pub fn connections(&self) -> usize {
        self.conns.len()
    }

    /// The game sockets open.
    pub fn sockets(&self) -> usize {
        self.conns.iter().filter(|c| c.is_socket()).count()
    }

    /// Whether a game socket stands for `addr` (a stand-in address, [`stand_in`]).
    pub fn holds(&self, addr: SocketAddr) -> bool {
        self.conns
            .iter()
            .any(|c| c.stand_in == Some(addr) && c.is_socket())
    }

    /// The TCP peer of the game socket that stands for `addr`.
    pub fn peer_of(&self, addr: SocketAddr) -> Option<SocketAddr> {
        self.conns
            .iter()
            .find(|c| c.stand_in == Some(addr) && c.is_socket())
            .map(|c| c.peer)
    }

    /// The game sockets open or reserved from the peer address `ip`.
    pub fn sockets_from(&self, ip: IpAddr) -> usize {
        self.stand_ins.count(ip)
    }

    /// Whether files are served over plain HTTP or under TLS.
    pub fn is_tls(&self) -> bool {
        self.setup.tls.is_some()
    }

    fn pass_parts(&mut self, now: Instant) -> (Pass<'_>, &mut Vec<Conn>) {
        (
            Pass {
                now,
                limits: &self.setup.limits,
                files: self.setup.files.as_ref(),
                inbox: &mut self.inbox,
                counters: &mut self.counters,
                stand_ins: &mut self.stand_ins,
                access: &self.setup.access,
                tls: self.setup.tls.is_some(),
            },
            &mut self.conns,
        )
    }

    /// One pass: what is waiting accepted, then every connection read and written as far as it goes without blocking.
    pub fn pump(&mut self) {
        let now = Instant::now();
        self.last_pump = Some(now);
        self.accept(now);
        let (mut pass, conns) = self.pass_parts(now);
        conns.retain_mut(|c| c.advance(&mut pass));
    }

    fn accept(&mut self, now: Instant) {
        let Some(listener) = self.listener.as_ref() else {
            return;
        };
        let limits = &self.setup.limits;
        for _ in 0..limits.accepts_per_pass {
            let (stream, peer) = match listener.accept() {
                Ok(got) => got,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => continue,
            };
            let ip = peer.ip();
            let from_ip = self.conns.iter().filter(|c| c.peer.ip() == ip).count();
            let rate_ok = {
                if self.connects.len() >= NET_LIMITER_ENTRIES && !self.connects.contains_key(&ip) {
                    self.connects.retain(|_, (start, _)| {
                        now.duration_since(*start) < Duration::from_secs(1)
                    });
                }
                if self.connects.len() >= NET_LIMITER_ENTRIES && !self.connects.contains_key(&ip) {
                    false
                } else {
                    let (start, count) = self.connects.entry(ip).or_insert((now, 0));
                    if now.duration_since(*start) >= Duration::from_secs(1) {
                        *start = now;
                        *count = 0;
                    }
                    *count += 1;
                    *count <= limits.connects_per_ip_per_s
                }
            };
            let refused = ip.is_ipv6()
                || self.conns.len() >= limits.max_connections
                || from_ip >= limits.connections_per_ip
                || !rate_ok
                || stream.set_nonblocking(true).is_err();
            if refused {
                self.counters.refused += 1;
                drop(stream);
                continue;
            }
            let _ = stream.set_nodelay(true);
            let pipe = match &self.setup.tls {
                Some(tls) => match tls.wrap(stream) {
                    Ok(w) => w,
                    Err(_) => {
                        self.counters.refused += 1;
                        continue;
                    }
                },
                None => tls::Pipe::Plain(stream),
            };
            self.counters.accepted += 1;
            self.conns.push(Conn::new(peer, pipe, now));
        }
    }

    fn shut_all(&mut self) {
        let (mut pass, conns) = self.pass_parts(Instant::now());
        for mut c in conns.drain(..) {
            c.shut(conn::Closed::Done, &mut pass);
        }
        self.stand_ins.clear();
        self.inbox.clear();
    }
}

impl Transport for WebTransport {
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        self.policy.check(ip).map_err(BindError::Policy)?;
        if self.setup.tls.is_none() && !self.policy.rules().is_private(ip) {
            return Err(BindError::Policy(PolicyRefusal::PlainWeb(ip)));
        }
        self.close();
        let listener =
            TcpListener::bind(SocketAddr::new(ip, port)).map_err(|e| BindError::Os(e.kind()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| BindError::Os(e.kind()))?;
        self.listener = Some(listener);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        if self.inbox.is_empty()
            && self
                .last_pump
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(1))
        {
            self.pump();
        }
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
        if data.len() > NET_MAX_DATAGRAM {
            return false;
        }
        let now = Instant::now();
        let (mut pass, conns) = self.pass_parts(now);
        let Some(conn) = conns
            .iter_mut()
            .find(|c| c.stand_in == Some(to) && c.is_socket())
        else {
            return false;
        };
        let sent = conn.send(data, &mut pass);
        if conn.is_gone() {
            conns.retain(|c| !c.is_gone());
        }
        sent
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.listener.as_ref().and_then(|l| l.local_addr().ok())
    }

    fn is_real(&self) -> bool {
        true
    }

    fn close(&mut self) {
        self.listener = None;
        self.shut_all();
    }

    fn service(&mut self) {
        if self.listener.is_some() {
            self.pump();
        }
    }

    fn wants_service(&self) -> bool {
        self.listener.is_some()
    }
}

/// UDP and the web on one address and port (module docs). Game sockets go by stand-in addresses no UDP peer has
/// ([`stand_in`]): a datagram goes to the web when its address is a stand-in and to UDP otherwise, and a UDP datagram that
/// claims a stand-in address is dropped, so a socket and a UDP peer never share an address.
pub struct MultiTransport {
    udp: UdpTransport,
    web: WebTransport,
    web_first: bool,
}

impl MultiTransport {
    pub fn new(udp: UdpTransport, web: WebTransport) -> MultiTransport {
        MultiTransport {
            udp,
            web,
            web_first: false,
        }
    }

    pub fn web(&self) -> &WebTransport {
        &self.web
    }

    pub fn udp(&self) -> &UdpTransport {
        &self.udp
    }

    fn recv_udp(&mut self, buf: &mut [u8]) -> Recv {
        match self.udp.recv(buf) {
            Recv::Datagram { from, .. } if is_stand_in(from) => Recv::Skipped,
            got => got,
        }
    }
}

impl Transport for MultiTransport {
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        // Port 0: the system's free UDP port, then the same TCP port (taken again on a clash).
        for _ in 0..8 {
            self.udp.bind(ip, port)?;
            let bound = self.udp.local_addr().map_or(port, |a| a.port());
            match self.web.bind(ip, bound) {
                Ok(()) => return Ok(()),
                Err(BindError::Os(ErrorKind::AddrInUse)) if port == 0 => self.udp.close(),
                Err(e) => {
                    self.udp.close();
                    return Err(e);
                }
            }
        }
        Err(BindError::Os(ErrorKind::AddrInUse))
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        let web_first = self.web_first;
        self.web_first = !web_first;
        let first = if web_first {
            self.web.recv(buf)
        } else {
            self.recv_udp(buf)
        };
        if first != Recv::Empty {
            return first;
        }
        if web_first {
            self.recv_udp(buf)
        } else {
            self.web.recv(buf)
        }
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        if is_stand_in(to) {
            self.web.send_to(to, data)
        } else {
            self.udp.send_to(to, data)
        }
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.udp.local_addr()
    }

    fn is_real(&self) -> bool {
        true
    }

    fn close(&mut self) {
        self.udp.close();
        self.web.close();
    }

    fn service(&mut self) {
        self.web.service();
    }

    fn wants_service(&self) -> bool {
        self.web.wants_service()
    }
}
