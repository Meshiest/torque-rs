//! The client's network: TGE's one UDP socket (`setNetPort`), which the server query's client half ([`query`]) and the
//! connection to a server ([`conn`]) share, LAN only by default.
//!
//! **The socket.** TGE's client binds one UDP socket with `setNetPort(port)` on `$Pref::Net::BindAddress` (empty: every address),
//! and both the server query and the game connection use it. The host's bind (its platform's `Network`, torque-server's) opens it
//! under a [`NetPolicy`]:
//!
//! - [`NetPolicy::Off`] (the default: headless runs, the menus, the tests): nothing is bound, `setNetPort` says yes as a server
//!   host without a network does, and every send is dropped and counted (a query finds nothing, a connect is never answered).
//! - [`NetPolicy::Loopback`] (every headless harness): the socket is bound to `127.0.0.1` whatever the address asked (engine-only),
//!   so nothing of the run leaves the machine.
//! - [`NetPolicy::Internet`]: [`NetPolicy::Lan`] plus the internet's few doors ([`Internet`]): the rows of a server list the
//!   game read may be pinged (and answer), and a public server the user chose to join (a permit armed by an input event,
//!   [`Internet::arm`]) may be connected to.
//! - [`NetPolicy::Lan`]: the socket is bound as TGE binds it. **Engine-only**: a socket on every address also opens a twin on
//!   `127.0.0.1` (an ephemeral port) that sends the query's broadcasts too and is read like the main one: Windows does not deliver
//!   a broadcast sent from an any-address socket to a server bound to `127.0.0.1`, and a server bound there should still be listed.
//!
//! **LAN only, but for the internet's doors.** A datagram goes only to a LAN address ([`is_lan_target`]: [`is_lan`]'s set), to the
//! limited broadcast `255.255.255.255` or, under [`NetPolicy::Internet`] only, to a public address [`Internet`] holds; anything
//! else is not sent and is counted ([`NetStats::refused`]). A public source is read only when [`Internet`] holds it. Received
//! datagrams are hostile: each is at most [`RECV_BYTES`] and a tick reads at most [`MAX_DATAGRAMS_PER_TICK`]; what is not a
//! server-query answer goes to the connection whose server sent it, or is dropped.
//!
//! Credit: the one shared socket follows TGE's `NetInterface` as Torque3D carries it (MIT: `sim/netInterface.cpp`, Copyright (c)
//! 2012 GarageGames, LLC).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::rc::Rc;

pub use crate::{conn, ghost_objects, query};
pub mod held;
pub mod moves;
#[cfg(feature = "p2p")]
pub mod room_join;
#[cfg(feature = "webrtc")]
pub mod web_rtc;
#[cfg(feature = "web-socket")]
pub mod web_socket;

/// The most bytes of a datagram kept (the game's packets are at most 1,500; a longer one is cut and then fails to decode).
pub const RECV_BYTES: usize = 2048;
/// The most datagrams one tick reads from the socket (the rest wait for the next tick).
pub const MAX_DATAGRAMS_PER_TICK: usize = 1024;
/// The most datagrams held for the connection between two of its pumps.
pub const MAX_INBOX: usize = 4096;
/// The receive buffer: larger than any UDP datagram (65,507 bytes), so no receive fails for its size (`WSAEMSGSIZE`); what is
/// kept of a datagram is its first [`RECV_BYTES`].
pub const RECV_BUFFER: usize = 65_536;
/// The most failed receives (an error for one datagram) one [`Datagrams::recv`] skips before it gives up for this call.
pub const MAX_SKIPS_PER_RECV: usize = 4096;

/// Windows' codes a receive on a UDP socket may give for one datagram that is then gone (or comes again): `ERROR_IO_PENDING` (997),
/// `WSAEMSGSIZE` (10040: a datagram longer than the buffer, cut), `WSAECONNRESET` (10054: an ICMP port-unreachable of an earlier
/// send). `WSAEWOULDBLOCK` (10035) is `ErrorKind::WouldBlock`: nothing waiting.
pub const WIN_IO_PENDING: i32 = 997;
pub const WSA_WOULD_BLOCK: i32 = 10035;
pub const WSA_MSG_SIZE: i32 = 10040;
pub const WSA_CONN_RESET: i32 = 10054;

/// What a failed receive means for the loop reading the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    /// Nothing more is waiting now.
    Empty,
    /// One datagram is lost (or will come again): count it and read on.
    Skip,
    /// The socket is broken: stop reading it this tick.
    Fatal,
}

/// How [`UdpDatagrams`] takes a receive error (a sender cannot stop the client's reading with a datagram that fails).
pub fn recv_error(e: &std::io::Error) -> RecvError {
    match e.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => return RecvError::Empty,
        ErrorKind::ConnectionReset | ErrorKind::Interrupted => return RecvError::Skip,
        _ => {}
    }
    match e.raw_os_error() {
        Some(WSA_WOULD_BLOCK) if cfg!(windows) => RecvError::Empty,
        Some(WIN_IO_PENDING | WSA_MSG_SIZE | WSA_CONN_RESET) if cfg!(windows) => RecvError::Skip,
        // EMSGSIZE (Linux 90, macOS 40): the same lost datagram.
        Some(90) if cfg!(target_os = "linux") => RecvError::Skip,
        Some(40) if cfg!(target_os = "macos") => RecvError::Skip,
        _ => RecvError::Fatal,
    }
}

/// Whether the client takes a datagram from `from` under `policy`: a LAN source only, and under [`NetPolicy::Loopback`] the
/// loopback only (everything else is dropped before it is parsed).
pub fn source_ok(policy: NetPolicy, from: SocketAddrV4) -> bool {
    match policy {
        NetPolicy::Off => false,
        NetPolicy::Loopback => from.ip().is_loopback(),
        NetPolicy::Lan | NetPolicy::Internet => is_lan(IpAddr::V4(*from.ip())),
    }
}

/// Where the client's socket may be bound and what it may reach (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetPolicy {
    #[default]
    Off,
    Loopback,
    Lan,
    /// [`NetPolicy::Lan`] and the internet's doors: a server list's rows and their pings, a public server the user chose.
    Internet,
}

/// Whether `ip` is a public unicast address: none of the loopback, private, link-local, CGNAT (100.64/10), documentation, benchmark
/// (198.18/15), IETF (192.0.0/24), "this network" (0/8), multicast, reserved (240/4) or broadcast ranges. The internet's doors open
/// to these only (a list's row or a DNS answer naming a LAN or loopback address is refused: no way into the LAN from outside).
pub fn is_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1]))
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (18..20).contains(&o[1]))
        || o[0] >= 240)
}

/// How long a permit to join a public server lasts after the input event that armed it (Sim ms): a game's own join steps and the
/// connect's challenge tries fit well inside.
pub const PERMIT_MS: u32 = 120_000;
/// The most permits held: one, the user's last choice (a new one replaces it).
pub const MAX_PERMITS: usize = 1;
/// The most addresses of a server list held.
pub const MAX_LIST_PEERS: usize = 2_048;

/// A public server the user chose to join (a game's native armed it while an input event was handled): connectable until it expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permit {
    pub addr: SocketAddrV4,
    pub armed_at: u32,
}

/// The public addresses the client may reach under [`NetPolicy::Internet`] (nothing reads it under the other policies).
#[derive(Debug, Clone, Default)]
pub struct Internet {
    /// The rows of the last server list (`IP:PORT`, public ones only): pings go to these and their answers are read.
    pub list: std::collections::HashSet<SocketAddrV4>,
    /// The public server the connection was opened to (its packets are read, its address sent to).
    pub server: Option<SocketAddrV4>,
    pub permits: VecDeque<Permit>,
    /// Rows past [`MAX_LIST_PEERS`] (not added).
    pub list_dropped: u64,
    /// Public connects refused for want of a permit.
    pub unpermitted: u64,
}

impl Internet {
    /// A permit for `addr` from `now` (a public address only; the oldest goes past [`MAX_PERMITS`]).
    pub fn arm(&mut self, addr: SocketAddrV4, now: u32) {
        if !is_public(*addr.ip()) || addr.port() == 0 {
            return;
        }
        self.permits.retain(|p| p.addr != addr);
        if self.permits.len() >= MAX_PERMITS {
            self.permits.pop_front();
        }
        self.permits.push_back(Permit {
            addr,
            armed_at: now,
        });
    }

    /// Whether a permit for exactly `addr` holds at `now`.
    pub fn permitted(&self, addr: SocketAddrV4, now: u32) -> bool {
        self.permits
            .iter()
            .any(|p| p.addr == addr && now.wrapping_sub(p.armed_at) <= PERMIT_MS)
    }

    /// The permit for exactly `addr`, used up by the one connect it allows (a later connect, a reconnect after a drop included, needs
    /// the user to choose the server again). Expired permits go too.
    pub fn take_permit(&mut self, addr: SocketAddrV4, now: u32) -> bool {
        let ok = self.permitted(addr, now);
        self.permits
            .retain(|p| p.addr != addr && now.wrapping_sub(p.armed_at) <= PERMIT_MS);
        ok
    }

    /// A new list: the old rows go.
    pub fn clear_list(&mut self) {
        self.list.clear();
    }

    /// One row of a server list (a public address only; past [`MAX_LIST_PEERS`] it is counted, not added).
    pub fn add_row(&mut self, addr: SocketAddrV4) {
        if !is_public(*addr.ip()) || addr.port() == 0 {
            return;
        }
        if self.list.len() >= MAX_LIST_PEERS && !self.list.contains(&addr) {
            self.list_dropped += 1;
            return;
        }
        self.list.insert(addr);
    }

    /// Whether `addr` is one of the public doors (a list row or the server joined).
    pub fn holds(&self, addr: SocketAddrV4) -> bool {
        self.server == Some(addr) || self.list.contains(&addr)
    }
}

/// The limited broadcast address the server query pings (`IP:BROADCAST:<port>`).
pub const BROADCAST: Ipv4Addr = Ipv4Addr::BROADCAST;

/// Whether the client may send to `to`: a LAN address or the limited broadcast.
pub fn is_lan_target(to: SocketAddrV4) -> bool {
    *to.ip() == BROADCAST || is_lan(IpAddr::V4(*to.ip()))
}

/// Whether `ip` is a LAN address (the private set: 10/8, 172.16/12, 192.168/16, 169.254/16, and the loopback
/// 127/8). The client never connects anywhere else.
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 10
                || o[0] == 127
                || (o[0] == 172 && (16..32).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
        }
        IpAddr::V6(_) => false,
    }
}

/// What the socket did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetStats {
    /// Datagrams sent (to LAN addresses only).
    pub sent: u64,
    /// Sends refused: the address is not a LAN one.
    pub refused: u64,
    /// Sends with no socket ([`NetPolicy::Off`], or before `setNetPort`) or that the system refused.
    pub dropped: u64,
    pub received: u64,
    /// Received datagrams nothing took (no connection to that address, an answer to no query).
    pub stray: u64,
    /// Received datagrams from a source the policy does not accept (not a LAN address; not the loopback under
    /// [`NetPolicy::Loopback`]): dropped unread.
    pub foreign: u64,
}

/// The datagram socket under the client (the tests give an in-memory one).
pub trait Datagrams {
    fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool;
    fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)>;
    /// The socket's UDP port number (0 when it has none).
    fn port(&self) -> u16;
    /// Datagrams the receives lost (an error for one datagram), so far.
    fn lost(&self) -> u64 {
        0
    }
}

/// The real sockets: the main one and, in [`NetPolicy::Lan`] on every address, the loopback twin.
pub struct UdpDatagrams {
    main: UdpSocket,
    twin: Option<UdpSocket>,
    buf: Vec<u8>,
    lost: u64,
}

impl UdpDatagrams {
    pub fn open(ip: Ipv4Addr, port: u16, twin: bool) -> std::io::Result<UdpDatagrams> {
        let main = UdpSocket::bind(SocketAddrV4::new(ip, port))?;
        main.set_nonblocking(true)?;
        main.set_broadcast(true)?;
        let twin = if twin {
            UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
                .ok()
                .filter(|s| s.set_nonblocking(true).is_ok() && s.set_broadcast(true).is_ok())
        } else {
            None
        };
        Ok(UdpDatagrams {
            main,
            twin,
            buf: vec![0; RECV_BUFFER],
            lost: 0,
        })
    }
}

impl Datagrams for UdpDatagrams {
    fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
        let main_ok = self.main.send_to(bytes, to).is_ok();
        // The twin carries the broadcasts (the loopback servers hear them only from a loopback socket) and the datagrams to loopback
        // servers that answered it.
        if let Some(t) = &self.twin
            && (*to.ip() == BROADCAST || to.ip().is_loopback())
        {
            let _ = t.send_to(bytes, to);
        }
        main_ok
    }

    fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
        let sockets = [Some(&self.main), self.twin.as_ref()];
        for s in sockets.into_iter().flatten() {
            let mut skips = 0usize;
            while skips < MAX_SKIPS_PER_RECV {
                match s.recv_from(&mut self.buf) {
                    Ok((n, SocketAddr::V4(from))) => {
                        return Some((from, self.buf[..n.min(RECV_BYTES)].to_vec()));
                    }
                    // IPv6 never: the sockets are IPv4.
                    Ok(_) => skips += 1,
                    Err(e) => match recv_error(&e) {
                        RecvError::Empty | RecvError::Fatal => break,
                        RecvError::Skip => {
                            self.lost += 1;
                            skips += 1;
                        }
                    },
                }
            }
        }
        None
    }

    fn lost(&self) -> u64 {
        self.lost
    }

    fn port(&self) -> u16 {
        self.main.local_addr().map_or(0, |a| a.port())
    }
}

/// Datagrams with their addresses, shared (the in-memory socket's two directions).
pub type DatagramQueue = Rc<RefCell<VecDeque<(SocketAddrV4, Vec<u8>)>>>;

/// An in-memory socket: what it sends is kept, what is put in `incoming` is received. For tests and in-process servers.
#[derive(Default)]
pub struct MemoryDatagrams {
    pub sent: DatagramQueue,
    pub incoming: DatagramQueue,
    pub local_port: u16,
}

impl Datagrams for MemoryDatagrams {
    fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
        let mut q = self.sent.borrow_mut();
        if q.len() >= MAX_INBOX {
            return false;
        }
        q.push_back((to, bytes.to_vec()));
        true
    }

    fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
        self.incoming.borrow_mut().pop_front()
    }

    fn port(&self) -> u16 {
        self.local_port
    }
}

/// The client's socket and its counts.
#[derive(Default)]
pub struct ClientNet {
    pub policy: NetPolicy,
    pub socket: Option<Box<dyn Datagrams>>,
    pub stats: NetStats,
    /// What the server half of the VM receives next (the host's transport reads it): the socket's datagrams the client's query and
    /// connection did not take (TGE's one `NetInterface` hands every datagram to both), and the local connection's packets from the
    /// client half (from [`crate::conn::LOCAL_ADDR`]). At most [`MAX_INBOX`].
    pub server_inbox: VecDeque<(SocketAddr, Vec<u8>)>,
    /// The local connection's inbox on the client half, while there is one (the server half's packets to it go there).
    pub local_inbox: Option<Rc<RefCell<VecDeque<Vec<u8>>>>>,
    /// Datagrams for the server half dropped because its inbox was full.
    pub server_dropped: u64,
    /// The internet's doors ([`NetPolicy::Internet`] only).
    pub internet: Internet,
}

impl ClientNet {
    /// A datagram for the server half (dropped and counted past [`MAX_INBOX`]).
    pub fn to_server(&mut self, from: SocketAddr, data: Vec<u8>) {
        if self.server_inbox.len() < MAX_INBOX {
            self.server_inbox.push_back((from, data));
        } else {
            self.server_dropped += 1;
        }
    }
}

impl ClientNet {
    /// Whether the client may send to `to`: a LAN address or the broadcast, or under [`NetPolicy::Internet`] a door [`Internet`] holds.
    pub fn may_send(&self, to: SocketAddrV4) -> bool {
        is_lan_target(to) || (self.policy == NetPolicy::Internet && self.internet.holds(to))
    }

    /// Whether a datagram from `from` is read ([`source_ok`], or under [`NetPolicy::Internet`] a door [`Internet`] holds).
    pub fn may_receive(&self, from: SocketAddrV4) -> bool {
        source_ok(self.policy, from)
            || (self.policy == NetPolicy::Internet && self.internet.holds(from))
    }

    /// Sends `bytes` to `to` under the LAN rule and the internet's doors (see the module docs).
    pub fn send(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
        if !self.may_send(to) {
            self.stats.refused += 1;
            return false;
        }
        let Some(s) = self.socket.as_mut() else {
            self.stats.dropped += 1;
            return false;
        };
        if s.send_to(to, bytes) {
            self.stats.sent += 1;
            true
        } else {
            self.stats.dropped += 1;
            false
        }
    }

    /// The next datagram the socket holds from a source the policy accepts ([`source_ok`]; the others are dropped unread and
    /// counted in [`NetStats::foreign`]).
    pub fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
        for _ in 0..MAX_DATAGRAMS_PER_TICK {
            let (from, data) = self.socket.as_mut()?.recv()?;
            if self.may_receive(from) {
                self.stats.received += 1;
                return Some((from, data));
            }
            self.stats.foreign += 1;
        }
        None
    }

    /// The socket's port (0 without one).
    pub fn port(&self) -> u16 {
        self.socket.as_ref().map_or(0, |s| s.port())
    }
}

/// Keeps a callback's error (bounded).
pub fn keep_error<G: crate::conn::ConnGame>(
    client: &Rc<RefCell<crate::state::ClientState<G>>>,
    e: ts_vm::vm::VmError,
) {
    let mut s = client.borrow_mut();
    if s.errors.len() < crate::state::MAX_QUEUED {
        s.errors.push(e);
    }
}
