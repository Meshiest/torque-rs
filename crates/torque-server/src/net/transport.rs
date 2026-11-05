//! The datagram transport under the network host and the bind policy that gates the only real socket.
//!
//! [`Transport`] is the whole OS-facing surface: bind, a receive that never blocks, a send. [`UdpTransport`] is the one
//! implementation that touches the operating system (a non-blocking `std::net::UdpSocket`, opened only by [`Transport::bind`]);
//! [`MemoryTransport`] is the in-memory one the tests and the fuzzers drive, with [`MemoryEnd`] as the test's hand on its two
//! queues. Nothing else in the crate opens a socket.
//!
//! **Bind policy.** A server has no socket unless its host names a bind address. That address is the only one any bind may use;
//! an address outside the private set is refused unless the host allows public addresses. The plain private set
//! ([`LanRules::PLAIN`]) is 10/8, 172.16/12, 192.168/16, 169.254/16 and 127/8; a game may give its own ([`LanRules`]).
//! `0.0.0.0` counts as public (it listens on every interface). **IPv6 is refused outright** ([`PolicyRefusal::Ipv6`]; at the
//! argument, in [`BindPolicy::check`] and so in every transport's `bind`): the per-source limiters key on the full address,
//! which one /64 would multiply.
//!
//! **Broadcasts (the LAN list).** A client's LAN query broadcasts its ping to `255.255.255.255:port` from a socket bound to
//! every address, and a server socket bound to every address hears it. On Windows a socket bound to a LAN address hears the
//! broadcasts that arrive on that interface, and a socket bound to `127.0.0.1` hears only those sent from a `127.0.0.1` socket;
//! on Linux a socket bound to a unicast address hears none, and a socket bound to `255.255.255.255:port` beside it hears the
//! limited broadcasts (and only those). So on a system other than Windows, a bind to a non-loopback address also opens a
//! receive-only listener on `255.255.255.255:port` ([`UdpTransport::has_broadcast_listener`]) that passes on only the server
//! query's requests from a LAN address ([`LanRules::broadcast_ok`]); the answers leave from the main socket. Nothing listens on
//! any other address, so the bind policy's intent holds.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::rc::Rc;

use torque_net::cap::NET_MAX_DATAGRAM;
use torque_net::serverquery::{GAME_INFO_REQUEST, GAME_PING_REQUEST};

use crate::platform::{Network, inet_addr};

/// The most datagrams a [`MemoryTransport`] holds in either direction (a test that floods it stops at the cap, the way a full
/// socket buffer drops).
pub const MEMORY_QUEUE_MAX: usize = 65_536;

/// The size of the buffer `recv` is given: one byte more than the largest datagram the engine reads, so that a longer datagram
/// shows (its length is above [`NET_MAX_DATAGRAM`]) instead of being cut to a valid size.
pub const RECV_BUFFER_BYTES: usize = NET_MAX_DATAGRAM + 1;

/// What one `recv` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recv {
    /// Nothing is waiting.
    Empty,
    /// The operating system reported an error for a datagram (Windows reports a send to a closed port on the next receive): the
    /// caller counts it against its pass and asks again.
    Skipped,
    /// A datagram of `len` bytes is in the buffer.
    Datagram { len: usize, from: SocketAddr },
}

/// Why a bind failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindError {
    /// The bind policy refuses the address.
    Policy(PolicyRefusal),
    /// The operating system refused (the port is in use, ...).
    Os(ErrorKind),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::Policy(why) => write!(f, "{why}"),
            BindError::Os(kind) => write!(f, "the operating system refused the bind: {kind}"),
        }
    }
}

impl std::error::Error for BindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BindError::Policy(why) => Some(why),
            BindError::Os(_) => None,
        }
    }
}

/// The datagram socket of the network host.
pub trait Transport {
    /// Opens the socket on `ip:port`, closing the one it had. Port 0 asks for any free port (tests; `setNetPort` clamps to 1024
    /// first).
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError>;
    /// The next datagram into `buf` (at least [`RECV_BUFFER_BYTES`] long); never blocks.
    fn recv(&mut self, buf: &mut [u8]) -> Recv;
    /// Sends one datagram; false when it was not sent (no socket, the system's buffer is full).
    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool;
    /// The address the socket is bound to, if it is.
    fn local_addr(&self) -> Option<SocketAddr>;
    /// Whether this is a real socket. A real socket needs the system clock.
    fn is_real(&self) -> bool;
    /// Closes the socket now (it stays closed until the next `bind`), whoever else holds the transport. Not needed to free a
    /// port of a stopped server: a dropped server drops its transport, and the socket with it. An in-process test server can
    /// call it as well when it stops.
    fn close(&mut self) {}
    /// Work that is not a datagram, done when the host has time (a dedicated server between its ticks): a web transport
    /// accepts connections, serves files and moves the sockets' bytes. Nothing for a datagram socket.
    fn service(&mut self) {}
    /// Whether [`Transport::service`] has work to do (a host that waits may call it while it waits).
    fn wants_service(&self) -> bool {
        false
    }
}

/// A transport shared by the pump and the `Network` that `setNetPort` binds through.
pub type SharedTransport = Rc<RefCell<dyn Transport>>;

// ---- the policy
// --------------------------------------------------------------------------------------------------------------

/// What counts as private and as on the LAN, and which datagrams the broadcast listener passes on. A game may supply its own;
/// [`LanRules::PLAIN`] is the plain one.
#[derive(Debug)]
pub struct LanRules {
    /// The private set of IPv4 addresses: a bind outside it is refused unless public addresses are allowed.
    pub private_v4: fn(Ipv4Addr) -> bool,
    /// Whether a sender's IPv4 address counts as on the LAN.
    pub lan_v4: fn(Ipv4Addr) -> bool,
    /// The first bytes of the requests the broadcast listener passes on.
    pub broadcast_requests: &'static [u8],
}

/// The plain private set: 10/8, 172.16/12, 192.168/16, 169.254/16 and 127/8.
fn plain_private_v4(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local() || ip.is_loopback()
}

impl LanRules {
    /// The plain rules: the private set is the private, link-local and loopback ranges, the LAN is the private set, and the
    /// broadcast listener passes on the server query's ping and info requests.
    pub const PLAIN: LanRules = LanRules {
        private_v4: plain_private_v4,
        lan_v4: plain_private_v4,
        broadcast_requests: &[GAME_PING_REQUEST, GAME_INFO_REQUEST],
    };

    /// Whether `ip` is in the private set (plus `::1`). An IPv4-mapped IPv6 address is its IPv4 address.
    pub fn is_private(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => (self.private_v4)(v4),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => (self.private_v4)(v4),
                None => v6 == std::net::Ipv6Addr::LOCALHOST,
            },
        }
    }

    /// Whether a datagram the broadcast listener heard is passed on: one of the requests from an IPv4 address on the LAN.
    /// Everything else a broadcast carries is dropped there.
    pub fn broadcast_ok(&self, from: SocketAddr, data: &[u8]) -> bool {
        let request = data
            .first()
            .is_some_and(|b| self.broadcast_requests.contains(b));
        let lan = match from.ip() {
            IpAddr::V4(v4) => (self.lan_v4)(v4),
            IpAddr::V6(_) => false,
        };
        request && lan
    }
}

/// Why the policy refuses an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyRefusal {
    /// The address is outside the private set and public addresses are not allowed.
    Public(IpAddr),
    /// The address is not the host's bind address.
    NotTheBindAddress(IpAddr),
    /// The script's `$Pref::Net::BindAddress` is neither empty nor an address [`inet_addr`] takes.
    Unparsable,
    /// An IPv6 address: the per-source limiters key on the full address, which one /64 would multiply.
    Ipv6(IpAddr),
    /// Plain (unencrypted) HTTP and WebSocket on an address outside the private set: only TLS listens on a public one.
    PlainWeb(IpAddr),
}

impl fmt::Display for PolicyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyRefusal::Public(ip) => write!(
                f,
                "{ip} is not a private address (10/8, 172.16/12, 192.168/16, 169.254/16, 127.0.0.1, ::1); pass --allow-public to listen on it"
            ),
            PolicyRefusal::NotTheBindAddress(ip) => {
                write!(f, "{ip} is not the address --bind named")
            }
            PolicyRefusal::Unparsable => f.write_str("the bind address cannot be parsed"),
            PolicyRefusal::Ipv6(ip) => write!(
                f,
                "{ip} is an IPv6 address; the server is IPv4-only (the per-source limits key on the full address)"
            ),
            PolicyRefusal::PlainWeb(ip) => write!(
                f,
                "{ip} is not a private address; the web transport listens on one only with TLS (a certificate and its key)"
            ),
        }
    }
}

impl std::error::Error for PolicyRefusal {}

/// What the host allows: the one address a bind may use.
#[derive(Debug, Clone, Copy)]
pub struct BindPolicy {
    address: IpAddr,
    allow_public: bool,
    rules: &'static LanRules,
}

impl PartialEq for BindPolicy {
    fn eq(&self, other: &BindPolicy) -> bool {
        self.address == other.address
            && self.allow_public == other.allow_public
            && std::ptr::eq(self.rules, other.rules)
    }
}

impl Eq for BindPolicy {}

impl BindPolicy {
    /// The policy for the bind address `address` under `rules`, refused when the address is public and `allow_public` is false.
    pub fn new(
        address: IpAddr,
        allow_public: bool,
        rules: &'static LanRules,
    ) -> Result<BindPolicy, PolicyRefusal> {
        let policy = BindPolicy {
            address,
            allow_public,
            rules,
        };
        policy.check(address)?;
        Ok(policy)
    }

    /// The host's bind address.
    pub fn address(&self) -> IpAddr {
        self.address
    }

    /// The rules the policy applies.
    pub fn rules(&self) -> &'static LanRules {
        self.rules
    }

    /// Whether a socket may be bound to `ip`: it must be the host's bind address, and private unless public is allowed.
    pub fn check(&self, ip: IpAddr) -> Result<(), PolicyRefusal> {
        if ip.is_ipv6() {
            return Err(PolicyRefusal::Ipv6(ip));
        }
        if ip != self.address {
            return Err(PolicyRefusal::NotTheBindAddress(ip));
        }
        if !self.allow_public && !self.rules.is_private(ip) {
            return Err(PolicyRefusal::Public(ip));
        }
        Ok(())
    }

    /// The address a `setNetPort` binds, from `$Pref::Net::BindAddress` (empty: the host's bind address; else [`inet_addr`]'s
    /// IPv4 address, which must be the host's).
    pub fn resolve(&self, script_address: &[u8]) -> Result<IpAddr, PolicyRefusal> {
        let ip = if script_address.is_empty() {
            self.address
        } else {
            let host = inet_addr(script_address).ok_or(PolicyRefusal::Unparsable)?;
            IpAddr::V4(Ipv4Addr::from(host))
        };
        self.check(ip)?;
        Ok(ip)
    }
}

/// Parses a host's bind address: an IPv4 address in the standard text form (not `inet_addr`'s octal and hex spellings, which
/// would let `0177.1` pass for a text a person did not mean). An IPv6 address is refused ([`PolicyRefusal::Ipv6`]): the
/// per-source limiters key on the full address, so one /64 would be 2^64 sources.
pub fn parse_bind_argument(text: &str) -> Result<IpAddr, String> {
    let ip = text
        .parse::<IpAddr>()
        .map_err(|_| format!("--bind needs an IP address, not {text:?}"))?;
    if ip.is_ipv6() {
        return Err(format!("--bind {}", PolicyRefusal::Ipv6(ip)));
    }
    Ok(ip)
}

/// The flag a server sets when it keeps the computer's clock.
pub type ClockFlag = Rc<Cell<bool>>;

/// A [`Network`] over a shared transport: what `setNetPort` binds through when the server has a socket.
pub struct TransportNetwork {
    transport: SharedTransport,
    policy: BindPolicy,
    system_clock: Option<ClockFlag>,
    ports: PortChoice,
    /// The last band port an off-band bind stood in for, and the one the system gave it: the same band port asked again (a
    /// later `portInit`) binds that port again.
    stand_in: Cell<Option<(u16, u16)>>,
}

/// Which port a bind takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PortChoice {
    /// The script's port, as asked.
    #[default]
    Asked,
    /// A port in the host's band `first ..= last` is replaced by a free one the system picks, and `setNetPort`'s line names
    /// that one: a server a harness starts for its own clients (joined by address) leaves the band to other servers on the
    /// machine, so that two runs on one machine do not take each other's port.
    OffBand { first: u16, last: u16 },
}

impl PortChoice {
    /// The port to bind for `asked`.
    pub fn port_for(self, asked: u16) -> u16 {
        match self {
            PortChoice::OffBand { first, last } if (first..=last).contains(&asked) => 0,
            _ => asked,
        }
    }
}

impl TransportNetwork {
    pub fn new(transport: SharedTransport, policy: BindPolicy) -> TransportNetwork {
        TransportNetwork {
            transport,
            policy,
            system_clock: None,
            ports: PortChoice::Asked,
            stand_in: Cell::new(None),
        }
    }

    /// Binds as `ports` says ([`PortChoice`]).
    pub fn with_ports(mut self, ports: PortChoice) -> TransportNetwork {
        self.ports = ports;
        self
    }

    /// Refuses to bind a real socket while `flag` is false: a server with a socket keeps the system clock, and the check sits
    /// where the socket opens.
    pub fn with_clock_flag(mut self, flag: ClockFlag) -> TransportNetwork {
        self.system_clock = Some(flag);
        self
    }
}

impl Network for TransportNetwork {
    fn fixed_address(&self) -> Option<String> {
        Some(self.policy.address().to_string())
    }

    fn bind(&self, address: &[u8], port: u16) -> bool {
        let Ok(ip) = self.policy.resolve(address) else {
            return false;
        };
        if self.system_clock.as_ref().is_some_and(|flag| !flag.get())
            && self.transport.borrow().is_real()
        {
            return false;
        }
        let off_band = self.ports.port_for(port) != port;
        let want = match self.stand_in.get() {
            Some((asked, got)) if off_band && asked == port => got,
            _ => self.ports.port_for(port),
        };
        // A leaf: no script runs while the transport is borrowed.
        let mut transport = self.transport.borrow_mut();
        let bound = transport.bind(ip, want).is_ok();
        if bound
            && off_band
            && let Some(addr) = transport.local_addr()
        {
            self.stand_in.set(Some((port, addr.port())));
        }
        bound
    }

    fn shown_port(&self, asked: u16) -> u16 {
        if self.ports.port_for(asked) == asked {
            return asked;
        }
        self.transport
            .borrow()
            .local_addr()
            .map_or(asked, |a| a.port())
    }
}

// ---- the real socket
// ---------------------------------------------------------------------------------------------------------

/// Whether a bind to `ip` also opens the broadcast listener: a non-loopback IPv4 address on a system whose unicast-bound
/// sockets do not hear broadcasts (every one the engine runs on but Windows, where the socket on the LAN address hears them
/// itself).
pub fn wants_broadcast_listener(ip: IpAddr) -> bool {
    !cfg!(windows) && matches!(ip, IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified())
}

/// The one OS socket: non-blocking UDP, opened by `bind`, closed when dropped or rebound. It enforces the policy itself too
/// (the `Network` above is not the only way to call it). Beside it, on systems that need one, the receive-only broadcast
/// listener (see the module's text).
pub struct UdpTransport {
    socket: Option<UdpSocket>,
    broadcast: Option<UdpSocket>,
    policy: BindPolicy,
}

impl UdpTransport {
    pub fn new(policy: BindPolicy) -> UdpTransport {
        UdpTransport {
            socket: None,
            broadcast: None,
            policy,
        }
    }

    /// Whether the broadcast listener is open (it is opened by `bind` when [`wants_broadcast_listener`] says so and the system
    /// lets it).
    pub fn has_broadcast_listener(&self) -> bool {
        self.broadcast.is_some()
    }
}

impl Transport for UdpTransport {
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        self.policy.check(ip).map_err(BindError::Policy)?;
        // The old sockets are closed first: a rebind to the same port would otherwise fail against ourselves.
        self.socket = None;
        self.broadcast = None;
        let socket =
            UdpSocket::bind(SocketAddr::new(ip, port)).map_err(|e| BindError::Os(e.kind()))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| BindError::Os(e.kind()))?;
        if wants_broadcast_listener(ip)
            && let Ok(local) = socket.local_addr()
        {
            // Receive-only. A system that refuses the bind keeps the server without it (the LAN list will not show it there).
            self.broadcast = UdpSocket::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::BROADCAST),
                local.port(),
            ))
            .ok()
            .filter(|b| b.set_nonblocking(true).is_ok());
        }
        self.socket = Some(socket);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        let Some(socket) = self.socket.as_ref() else {
            return Recv::Empty;
        };
        match socket.recv_from(buf) {
            Ok((len, from)) => return Recv::Datagram { len, from },
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(_) => return Recv::Skipped,
        }
        let Some(listener) = self.broadcast.as_ref() else {
            return Recv::Empty;
        };
        match listener.recv_from(buf) {
            Ok((len, from))
                if self
                    .policy
                    .rules()
                    .broadcast_ok(from, buf.get(..len).unwrap_or_default()) =>
            {
                Recv::Datagram { len, from }
            }
            // Not a query request from a LAN address: dropped, and the pass counts it as a skipped datagram.
            Ok(_) => Recv::Skipped,
            Err(e) if e.kind() == ErrorKind::WouldBlock => Recv::Empty,
            Err(_) => Recv::Skipped,
        }
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        self.socket
            .as_ref()
            .is_some_and(|socket| socket.send_to(data, to).is_ok())
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.socket.as_ref().and_then(|s| s.local_addr().ok())
    }

    fn is_real(&self) -> bool {
        true
    }

    fn close(&mut self) {
        self.socket = None;
        self.broadcast = None;
    }
}

// ---- the in-memory one
// -------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Queues {
    inbox: VecDeque<(SocketAddr, Vec<u8>)>,
    outbox: VecDeque<(SocketAddr, Vec<u8>)>,
    bound: Option<SocketAddr>,
    /// Every address `bind` was asked for, in order (a test reads it).
    binds: Vec<SocketAddr>,
}

/// An in-memory transport: a queue in and a queue out.
pub struct MemoryTransport {
    queues: Rc<RefCell<Queues>>,
    policy: Option<BindPolicy>,
}

/// The test's end of a [`MemoryTransport`].
#[derive(Clone)]
pub struct MemoryEnd {
    queues: Rc<RefCell<Queues>>,
}

impl MemoryTransport {
    /// A transport that binds anywhere and the end that drives it.
    pub fn new() -> (MemoryTransport, MemoryEnd) {
        let queues = Rc::new(RefCell::new(Queues::default()));
        (
            MemoryTransport {
                queues: Rc::clone(&queues),
                policy: None,
            },
            MemoryEnd { queues },
        )
    }

    /// The same, applying `policy` to every bind as the real socket does.
    pub fn with_policy(policy: BindPolicy) -> (MemoryTransport, MemoryEnd) {
        let (mut transport, end) = MemoryTransport::new();
        transport.policy = Some(policy);
        (transport, end)
    }
}

impl Transport for MemoryTransport {
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        if let Some(policy) = &self.policy {
            policy.check(ip).map_err(BindError::Policy)?;
        }
        let addr = SocketAddr::new(ip, port);
        let mut queues = self.queues.borrow_mut();
        queues.binds.push(addr);
        queues.bound = Some(addr);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        let mut queues = self.queues.borrow_mut();
        if queues.bound.is_none() {
            return Recv::Empty;
        }
        let Some((from, data)) = queues.inbox.pop_front() else {
            return Recv::Empty;
        };
        // A datagram longer than the buffer is cut, as the system cuts it.
        let len = data.len().min(buf.len());
        if let (Some(dst), Some(src)) = (buf.get_mut(..len), data.get(..len)) {
            dst.copy_from_slice(src);
        }
        Recv::Datagram { len, from }
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        let mut queues = self.queues.borrow_mut();
        if queues.bound.is_none() || queues.outbox.len() >= MEMORY_QUEUE_MAX {
            return false;
        }
        queues.outbox.push_back((to, data.to_vec()));
        true
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.queues.borrow().bound
    }

    fn is_real(&self) -> bool {
        false
    }
}

impl MemoryEnd {
    /// A datagram arrives from `from`. False when the queue is full (dropped, like a full socket buffer).
    pub fn inject(&self, from: SocketAddr, data: &[u8]) -> bool {
        let mut queues = self.queues.borrow_mut();
        if queues.inbox.len() >= MEMORY_QUEUE_MAX {
            return false;
        }
        queues.inbox.push_back((from, data.to_vec()));
        true
    }

    /// Everything the server sent since the last call, oldest first.
    pub fn take_sent(&self) -> Vec<(SocketAddr, Vec<u8>)> {
        self.queues.borrow_mut().outbox.drain(..).collect()
    }

    /// The datagrams sent and not yet taken.
    pub fn sent_len(&self) -> usize {
        self.queues.borrow().outbox.len()
    }

    /// The datagrams injected and not yet read.
    pub fn waiting(&self) -> usize {
        self.queues.borrow().inbox.len()
    }

    /// Whether the server bound a port.
    pub fn is_bound(&self) -> bool {
        self.queues.borrow().bound.is_some()
    }

    /// Every address the server asked to bind, in order.
    pub fn binds(&self) -> Vec<SocketAddr> {
        self.queues.borrow().binds.clone()
    }
}
