//! WebRTC data channels for the web transport's browsers (the `webrtc` feature): datagrams on an unreliable, unordered
//! channel (`ordered: false, maxRetransmits: 0`) instead of the game socket's TCP stream, so a lost packet delays nothing
//! sent after it.
//!
//! **How a browser gets one.** The game socket ([`super`]) stays the browser's link and its signaling path: when it opens,
//! the server says it takes data channels ([`signal::HELLO`]); the browser sends an SDP offer over it, the server answers
//! with its ICE credentials, its DTLS fingerprint and its candidates, and the browser's ICE checks and DTLS handshake come
//! to the data-channel socket. The peer goes by the game socket's stand-in address ([`super::stand_in`]): datagrams from
//! the channel and from the socket reach the host from the same address, a datagram the host sends goes on the channel
//! while it is open and on the socket otherwise, and the host's per-address limits and the scripts' bans keyed by that
//! address hold for both. When ICE fails or the channel closes, the peer is dropped and the browser goes on over the socket
//! ([`signal::CLOSED`]); when the socket closes, its peer goes with it.
//!
//! **The socket.** One UDP socket of its own, on the web transport's address and a port of its own ([`RtcSetup::port`]),
//! not the game's UDP socket: the game's datagrams begin with any byte (a data packet's first byte is any odd one, and the
//! handshake's types sit in DTLS's range, RFC 7983), so a split by first byte would need the game's receive path to know
//! every peer's address, and a server on the web alone has no game socket to share. Apart, each socket has its own budget
//! per pass, and STUN and DTLS never reach the game's packet layer. The server is ICE-lite (RFC 8445): it answers checks
//! and never sends one, so it sends nothing to an address that has not proved it holds the peer's ICE password. Its
//! candidates are the bind address (unless it is `0.0.0.0`) and [`RtcSetup::public`] (a host behind a 1:1 NAT names its
//! public address there). The DTLS certificate is made once per transport.
//!
//! **Hostile input.** Every cap counts the socket's datagrams and their bytes as they come off the wire ([`RtcLimits`]):
//! a pass reads at most [`RtcLimits::datagrams_per_pass`] datagrams, hands one peer at most
//! [`RtcLimits::peer_datagrams_per_pass`] datagrams and [`RtcLimits::checks_per_pass`] binding requests, and the peers
//! still connecting at most [`RtcLimits::connecting_per_pass`] in all, and drains at most [`RtcLimits::outputs_per_pass`]
//! outputs of each, so a pass's time is bounded whatever the peers send. A UDP source address can be forged, so what the
//! stack has not authenticated never closes anything and never spends a peer's caps a second: those
//! ([`RtcLimits::packets_per_s`], [`RtcLimits::bytes_per_s`]) are spent only by datagrams the stack authenticated (a check
//! it answered, or one that brought the DTLS session's events or messages), past them the peer's datagrams are dropped
//! for the rest of the second, and only authenticated datagrams count as the peer being heard (its idle deadline).
//! Anything else (checks that fail their integrity, records that do not decrypt, garbage) is held to the per-pass caps
//! alone, dropped by the stack, and counted. The DTLS handshake's work is bounded per pass by those caps and in all by a
//! budget per peer: until its channel opens, a peer's datagrams other than checks are handed to the stack only within
//! [`RtcLimits::connect_datagrams`] and [`RtcLimits::connect_bytes`] (counted as they are admitted); past them they are
//! dropped, and a peer that has not opened its channel by [`RtcLimits::connect_timeout`] is dropped (its browser stays on
//! its socket). Checks are left out of the budget (forged ones, which fail their integrity, could spend it), on their own
//! per-pass cap. The budget does spend on forged DTLS datagrams, though: the stack cannot tell a handshake record's
//! sender before the handshake completes, so a datagram that arrives from the exact address a peer's checks validated is
//! handed to that peer and counted, whoever sent it. Someone who spoofs a connecting browser's NAT-mapped address within
//! its connect window can use its budget up and fail its handshake; the worst case is that browser staying on its game
//! socket.
//!
//! **Routing.** A STUN binding request goes to the peer its USERNAME names (our ICE user fragment), and only when it comes
//! from the IP address of that peer's game socket; requests are also held to a rate per source address. Any other datagram
//! goes to the peer whose stack has validated its source address by an authenticated check (str0m's `accepts`): the
//! address is mapped to that peer then (at most [`MAX_REMOTES`] addresses a peer, its oldest given up first, and a cap in
//! all), and a mapping moves only when its peer's stack no longer accepts the address, so a second peer that passes
//! checks from another's address cannot take its traffic. A datagram no peer accepts is dropped
//! unread. Peers behind one IP address share a NAT (and the per-address caps); one that races another's first DTLS
//! datagram can at worst make that handshake fail, and the browser stays on its socket.
//!
//! **Closing.** What closes a peer is authenticated: a message larger than a datagram ([`RtcLimits::max_message`]), more
//! messages a second than [`RtcLimits::messages_per_s`], a second channel or one that is not the game's
//! (`ordered: false, maxRetransmits: 0`, label [`signal::CHANNEL_LABEL`]), text on the channel or media, all of which
//! come decrypted from the peer's DTLS session; or the deadlines (the channel not open in time, ICE gone quiet). Closing a
//! peer closes its data channel only: the browser is told ([`signal::CLOSED`]) and goes on over its game socket, which is
//! closed only for what it does itself. Peers are capped in all, per address (the game socket's TCP address) and in new
//! peers a second; offers are capped per game socket. The SCTP layer's own receive buffer bounds a peer's memory (1 MiB).
//! The server relays nothing: a peer's messages go to the host as that peer's datagrams, there is no TURN, and answers go
//! only to the address a check came from.
//!
//! **Voice later.** A peer is one `Rtc` that can take audio sections in a later offer (renegotiation is allowed, with the
//! browser's ICE credentials unchanged: an ICE restart drops the data channel, and the browser stays on its game socket,
//! since the credentials the stack would mint for it could not be routed); until voice is built, an offer with any
//! section but `m=application` is refused ([`signal::data_only`]).

pub mod signal;
mod stun;

use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;

use str0m::change::SdpOffer;
use str0m::channel::{ChannelId, Reliability};
use str0m::config::{CryptoProvider, DtlsCert};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, IceCreds, Input, Output, Rtc, RtcConfig};
use torque_net::cap::{NET_LIMITER_ENTRIES, NET_MAX_DATAGRAM, NET_PACKETS_PER_CONN_PER_S};
use web_time::{Duration, Instant};

use super::super::transport::{BindError, BindPolicy, PolicyRefusal};
use signal::Signal;

/// The receive buffer: a datagram this long or longer is dropped unread (a browser's are about 1,200 bytes).
const RECV_BYTES: usize = 2048;
/// The wire bytes a datagram may carry beyond a message's (DTLS, SCTP and UDP headers, with room to spare).
const WIRE_ROOM: usize = 128;
/// The most source addresses mapped to one peer (its oldest is given up for a new one).
pub const MAX_REMOTES: usize = 4;
/// The most addresses kept as verified for one peer (its checks came from them and held their integrity; a browser checks
/// from a socket on each of its network interfaces).
pub const VERIFIED_PER_PEER: usize = 16;
/// The most candidates a player's host takes from one joiner, in its offer and trickled (its checks go to each).
pub const MAX_REMOTE_CANDIDATES: u32 = 8;
/// The most distinct candidate IP addresses a player's host holds across every joiner, per peer it may have.
pub const CANDIDATE_IPS_PER_PEER: usize = 2;
/// The most outputs taken from a peer that is closing (its DTLS close notify).
const CLOSE_OUTPUTS: usize = 16;
/// The characters of an ICE user fragment and password (RFC 8445's ice-char, without `+` and `/`).
const ICE_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// The caps of a web transport's data channels (module docs). The rates match UDP's per-connection caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcLimits {
    /// The most peers at once.
    pub max_peers: usize,
    /// The most peers of one address (the game socket's TCP address).
    pub peers_per_ip: usize,
    /// The most new peers a second, from every address together.
    pub new_peers_per_s: u32,
    /// The most offers one game socket may send (the first and renegotiations).
    pub offers_per_socket: u32,
    /// The largest signaling message (a text message on the game socket).
    pub max_signal: usize,
    /// The most signaling messages one game socket may send.
    pub signals_per_socket: u32,
    /// How long a peer has from its offer to its channel's opening.
    pub connect_timeout: Duration,
    /// How long a peer may send nothing (the browser's ICE consent checks come every few seconds).
    pub idle_timeout: Duration,
    /// The most datagrams one pass reads off the socket.
    pub datagrams_per_pass: usize,
    /// The most datagrams one pass hands one peer (the rest of the pass's are dropped, and counted).
    pub peer_datagrams_per_pass: usize,
    /// The most STUN binding requests one pass hands one peer.
    pub checks_per_pass: usize,
    /// The most datagrams one pass hands the peers whose channel is not open yet (ICE checks and the DTLS handshake).
    pub connecting_per_pass: usize,
    /// The most datagrams a peer may send in a second.
    pub packets_per_s: u32,
    /// The most bytes a peer may send in a second, counted as they come off the wire.
    pub bytes_per_s: usize,
    /// The most datagrams other than checks the stack is handed for a peer before its channel opens (the DTLS handshake).
    pub connect_datagrams: u32,
    /// The most bytes of those.
    pub connect_bytes: usize,
    /// The most STUN binding requests a second from one source address no peer has answered yet.
    pub stun_per_ip_per_s: u32,
    /// The most messages a peer's channel may carry in a second.
    pub messages_per_s: u32,
    /// The most messages one pass takes from one peer (the rest are dropped, and counted).
    pub messages_per_pass: usize,
    /// The largest message.
    pub max_message: usize,
    /// The most outputs (datagrams to send, events) one pass takes from one peer.
    pub outputs_per_pass: usize,
    /// The most bytes waiting on a peer's channel to be sent (past it a send fails, as on a full socket).
    pub send_buffer: usize,
    /// The most checks a player's host sends one peer's unverified addresses in a second (an address is verified once
    /// an authenticated datagram came from it); past it a check is held back (the stack sends it again later).
    pub unverified_checks_per_s: u32,
    /// The same, for every peer together.
    pub unverified_checks_per_s_all: u32,
}

impl Default for RtcLimits {
    fn default() -> RtcLimits {
        RtcLimits {
            max_peers: 64,
            peers_per_ip: 4,
            new_peers_per_s: 16,
            offers_per_socket: 4,
            max_signal: 16 << 10,
            signals_per_socket: 64,
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(15),
            datagrams_per_pass: 256,
            peer_datagrams_per_pass: 64,
            checks_per_pass: 8,
            connect_datagrams: 256,
            connect_bytes: 256 << 10,
            connecting_per_pass: 32,
            packets_per_s: NET_PACKETS_PER_CONN_PER_S,
            bytes_per_s: NET_PACKETS_PER_CONN_PER_S as usize * (NET_MAX_DATAGRAM + WIRE_ROOM),
            stun_per_ip_per_s: 64,
            messages_per_s: NET_PACKETS_PER_CONN_PER_S,
            messages_per_pass: 64,
            max_message: NET_MAX_DATAGRAM,
            outputs_per_pass: 256,
            send_buffer: 64 << 10,
            unverified_checks_per_s: 20,
            unverified_checks_per_s_all: 200,
        }
    }
}

/// How a web transport takes data channels: their socket's port number, the addresses given to browsers besides the
/// bind address, the caps, and a share of datagrams to drop both ways (a test of the channel under loss; 0 normally).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RtcSetup {
    /// The UDP port (0: any free one).
    pub port: u16,
    /// Addresses given to browsers as candidates besides the bind address (a host behind a 1:1 NAT names its public one).
    pub public: Vec<Ipv4Addr>,
    pub limits: RtcLimits,
    /// The percentage of datagrams dropped as they are received and as they are sent (a test; 0 drops none).
    pub loss_percent: u8,
    /// A player's host (`net::p2p`) rather than a server browsers reach directly: full ICE (the host sends checks too, so
    /// a NAT in front of it opens), checks taken from any address (a joiner has no game socket whose address they must
    /// share; they are still held to the rate per source address), the joiner's trickled candidates taken (at most
    /// [`MAX_REMOTE_CANDIDATES`], IPv4 UDP, no `.local` names, only addresses [`candidate_allowed`] takes: the host
    /// sends its checks there), and a server-reflexive candidate once a STUN server answered.
    pub host: bool,
    /// A player's host also takes joiners' private (LAN) candidates: players on its own network (off by default, as a
    /// joiner could otherwise aim the host's checks at machines on the host's network).
    pub lan_candidates: bool,
}

/// What the data channels have done, for tests and the console.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RtcCounters {
    /// Datagrams read off the socket.
    pub datagrams: u64,
    /// Datagrams dropped unread: no peer accepts their address, and they are not a binding request naming a peer from
    /// its game socket's IP address.
    pub stray: u64,
    /// Datagrams dropped by a peer's caps a second (spent by its authenticated datagrams).
    pub rate_dropped: u64,
    /// Datagrams the stack refused (dropped).
    pub refused: u64,
    /// Datagrams handed to the stack.
    pub handled: u64,
    /// Datagrams dropped because their peer spent its budget before its channel opened.
    pub connect_spent: u64,
    /// Binding requests dropped by the per-address rate.
    pub stun_limited: u64,
    /// A joiner's candidates a player's host did not take (an address it may not check, past a cap, malformed).
    pub candidates_refused: u64,
    /// Checks to unverified addresses held back by their rate.
    pub checks_held: u64,
    /// Datagrams and messages dropped by a per-pass cap.
    pub pass_dropped: u64,
    /// Datagrams dropped by the loss test, both ways.
    pub loss_dropped: u64,
    /// Offers answered.
    pub offers: u64,
    /// Offers turned away by a cap (the browser stays on its game socket).
    pub busy: u64,
    /// Channels opened.
    pub channels: u64,
    /// Game messages taken from channels.
    pub messages: u64,
    /// Game messages sent on channels.
    pub sent: u64,
    /// Messages dropped because the host's queue was full, or not sent because a channel's buffer was.
    pub full: u64,
    /// Peers closed by ICE failing, the browser, or its game socket closing.
    pub closed: u64,
    pub closed_timeout: u64,
    pub closed_rate: u64,
    pub closed_oversize: u64,
    pub closed_protocol: u64,
}

/// A peer's traffic so far (for tests and the console).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerStats {
    /// Whether its channel is open.
    pub open: bool,
    /// Game messages taken from it.
    pub messages_in: u64,
    /// Game messages sent to it.
    pub messages_out: u64,
    /// The longest time between two of its game messages.
    pub max_gap: Duration,
}

/// Why a peer was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// ICE failed or went quiet, or the browser closed it.
    Peer,
    /// Its game socket closed, or the transport did.
    Gone,
    /// Its channel did not open in time, or it went quiet.
    Timeout,
    /// More datagrams, bytes or messages a second than the caps.
    Rate,
    /// A message larger than a datagram.
    Oversize,
    /// Anything a browser does not send on its channel: a second channel, text, media.
    Protocol,
}

impl Why {
    fn count(self, counters: &mut RtcCounters) {
        let slot = match self {
            Why::Peer | Why::Gone => &mut counters.closed,
            Why::Timeout => &mut counters.closed_timeout,
            Why::Rate => &mut counters.closed_rate,
            Why::Oversize => &mut counters.closed_oversize,
            Why::Protocol => &mut counters.closed_protocol,
        };
        *slot += 1;
    }
}

/// What a signaling message asks of the game socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reply {
    /// Nothing.
    None,
    /// A text message to send back.
    Text(String),
}

/// A peer's datagram and message counts in the present second and pass.
#[derive(Debug)]
struct Meter {
    window: Instant,
    /// Authenticated datagrams and their bytes in the present second.
    datagrams: u32,
    bytes: usize,
    messages: u32,
    pass_datagrams: usize,
    pass_checks: usize,
    pass_messages: usize,
    /// Datagrams other than checks, and their bytes, admitted before the channel opened.
    before_open: u32,
    before_open_bytes: usize,
}

/// What a meter says of one more datagram or message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Take {
    Ok,
    /// Past the pass's cap: dropped.
    Skip,
    /// Past a cap a second: a datagram is dropped, a message (which is authenticated) closes the peer.
    Over,
}

impl Meter {
    fn new(now: Instant) -> Meter {
        Meter {
            window: now,
            datagrams: 0,
            bytes: 0,
            messages: 0,
            pass_datagrams: 0,
            pass_checks: 0,
            pass_messages: 0,
            before_open: 0,
            before_open_bytes: 0,
        }
    }

    fn roll(&mut self, now: Instant) {
        if now.duration_since(self.window) >= Duration::from_secs(1) {
            self.window = now;
            self.datagrams = 0;
            self.bytes = 0;
            self.messages = 0;
        }
    }

    fn refill(&mut self) {
        self.pass_datagrams = 0;
        self.pass_checks = 0;
        self.pass_messages = 0;
    }

    /// Whether a datagram goes to the stack: within the pass's caps (a binding request's own), and while the peer's
    /// authenticated datagrams are within the caps a second.
    fn admit(&mut self, check: bool, now: Instant, limits: &RtcLimits) -> Take {
        self.roll(now);
        if self.datagrams >= limits.packets_per_s || self.bytes >= limits.bytes_per_s {
            return Take::Over;
        }
        let (taken, cap) = if check {
            (&mut self.pass_checks, limits.checks_per_pass)
        } else {
            (&mut self.pass_datagrams, limits.peer_datagrams_per_pass)
        };
        if *taken >= cap {
            return Take::Skip;
        }
        *taken += 1;
        Take::Ok
    }

    /// Whether the budget before the channel opens has room for one more datagram.
    fn connect_room(&self, limits: &RtcLimits) -> bool {
        self.before_open < limits.connect_datagrams && self.before_open_bytes < limits.connect_bytes
    }

    /// An admitted datagram before the channel opens spends the budget.
    fn spend_connect(&mut self, len: usize) {
        self.before_open += 1;
        self.before_open_bytes += len;
    }

    /// A datagram the stack authenticated spends the caps a second.
    fn authentic(&mut self, len: usize) {
        self.datagrams += 1;
        self.bytes += len;
    }

    fn message(&mut self, now: Instant, limits: &RtcLimits) -> Take {
        self.roll(now);
        self.messages += 1;
        if self.messages > limits.messages_per_s {
            return Take::Over;
        }
        if self.pass_messages >= limits.messages_per_pass {
            return Take::Skip;
        }
        self.pass_messages += 1;
        Take::Ok
    }
}

/// One browser's peer connection.
struct Peer {
    rtc: Rtc,
    /// The game socket's TCP address (the per-address cap's key).
    ip: IpAddr,
    ufrag: Vec<u8>,
    /// The ICE credential lines of the browser's first offer ([`signal::ice_credentials`]); a renegotiation keeps them.
    remote_ice: Vec<String>,
    /// The source addresses mapped to it, oldest first.
    remotes: VecDeque<SocketAddr>,
    channel: Option<ChannelId>,
    opened: Instant,
    /// When it was last heard: a check its stack answered, or its DTLS session's events or messages (nothing forged).
    heard: Instant,
    /// When its last game message came.
    game_heard: Option<Instant>,
    /// When the stack asks to be woken.
    next: Instant,
    closing: Option<Why>,
    meter: Meter,
    stats: PeerStats,
    /// Candidates offered (taken or not).
    candidates: u32,
    /// The IP addresses of the candidates taken (each counted once in the side's set).
    candidate_ips: Vec<IpAddr>,
    /// Checks sent to its unverified addresses in the present second.
    unverified: (Instant, u32),
    /// Addresses whose checks the stack answered (their integrity held): verified, the newest last.
    verified: VecDeque<SocketAddr>,
}

impl Peer {
    fn close(&mut self, why: Why) {
        self.closing.get_or_insert(why);
    }
}

/// A deterministic share of datagrams dropped (the loss test).
#[derive(Debug)]
struct Loss {
    percent: u8,
    state: u64,
}

impl Loss {
    fn drop_one(&mut self) -> bool {
        if self.percent == 0 {
            return false;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state % 100) < u64::from(self.percent)
    }
}

/// What draining a peer's outputs needs besides the peer.
struct Sink<'a> {
    socket: &'a UdpSocket,
    stand_in: SocketAddr,
    inbox: &'a mut VecDeque<(SocketAddr, Vec<u8>)>,
    queue: usize,
    counters: &'a mut RtcCounters,
    loss: &'a mut Loss,
    limits: &'a RtcLimits,
    now: Instant,
    /// Set when the outputs show the stack authenticated what it was given (a check answered, the session's events).
    authentic: bool,
    /// Checks sent to unverified addresses by every peer in the present second.
    unverified: &'a mut (Instant, u32),
}

/// The data channels of a web transport (module docs).
pub(crate) struct RtcSide {
    setup: RtcSetup,
    policy: BindPolicy,
    socket: Option<UdpSocket>,
    /// The candidates given to browsers (the first is the address datagrams are taken to have come to).
    advertised: Vec<SocketAddr>,
    crypto: Arc<CryptoProvider>,
    cert: Option<DtlsCert>,
    /// The peers by their game socket's stand-in address.
    peers: HashMap<SocketAddr, Peer>,
    /// The peers by their ICE user fragment, with their game socket's IP address.
    by_ufrag: HashMap<Vec<u8>, (SocketAddr, IpAddr)>,
    /// Source addresses mapped to the peer whose stack accepts them.
    by_remote: HashMap<SocketAddr, SocketAddr>,
    /// Binding requests per source address no peer has answered, in the present second.
    stun_sources: HashMap<IpAddr, (Instant, u32)>,
    /// New peers in the present second.
    new_peers: (Instant, u32),
    /// Offers per game socket.
    offers: HashMap<SocketAddr, u32>,
    counters: RtcCounters,
    /// Peers closed by this side since the web transport last asked, and why.
    notices: Vec<(SocketAddr, Why)>,
    loss: Loss,
    buf: Vec<u8>,
    /// A binding request out to a STUN server (its address and transaction), and the address it saw this socket at.
    reflexive_query: Option<(SocketAddr, [u8; 12])>,
    reflexive: Option<SocketAddr>,
    /// Joiners' candidate IP addresses taken, and how many peers hold each.
    candidate_ips: HashMap<IpAddr, u32>,
    /// Checks sent to unverified addresses by every peer in the present second.
    unverified: (Instant, u32),
}

impl RtcSide {
    pub(crate) fn new(policy: BindPolicy, setup: RtcSetup) -> RtcSide {
        let loss = Loss {
            percent: setup.loss_percent.min(100),
            state: 0x9e37_79b9_7f4a_7c15,
        };
        RtcSide {
            setup,
            policy,
            socket: None,
            advertised: Vec::new(),
            crypto: Arc::new(str0m::crypto::from_feature_flags()),
            cert: None,
            peers: HashMap::new(),
            by_ufrag: HashMap::new(),
            by_remote: HashMap::new(),
            stun_sources: HashMap::new(),
            new_peers: (Instant::now(), 0),
            offers: HashMap::new(),
            counters: RtcCounters::default(),
            notices: Vec::new(),
            loss,
            buf: vec![0; RECV_BYTES],
            reflexive_query: None,
            reflexive: None,
            candidate_ips: HashMap::new(),
            unverified: (Instant::now(), 0),
        }
    }

    pub(crate) fn limits(&self) -> &RtcLimits {
        &self.setup.limits
    }

    pub(crate) fn counters(&self) -> RtcCounters {
        self.counters
    }

    pub(crate) fn local_addr(&self) -> Option<SocketAddr> {
        self.socket.as_ref().and_then(|s| s.local_addr().ok())
    }

    /// The candidates given to browsers.
    pub(crate) fn advertised(&self) -> &[SocketAddr] {
        &self.advertised
    }

    /// Each peer's stand-in and traffic.
    pub(crate) fn stats(&self) -> Vec<(SocketAddr, PeerStats)> {
        self.peers.iter().map(|(&a, p)| (a, p.stats)).collect()
    }

    /// Each peer's stand-in and traffic, its longest gap since the last call (which starts the next window).
    pub(crate) fn take_stats(&mut self) -> Vec<(SocketAddr, PeerStats)> {
        self.peers
            .iter_mut()
            .map(|(&a, p)| {
                let stats = p.stats;
                p.stats.max_gap = Duration::ZERO;
                (a, stats)
            })
            .collect()
    }

    /// The source addresses mapped to peers.
    pub(crate) fn routes(&self) -> usize {
        self.by_remote.len()
    }

    /// Whether the peer of `stand_in` has its channel open.
    pub(crate) fn is_open(&self, stand_in: SocketAddr) -> bool {
        self.peers
            .get(&stand_in)
            .is_some_and(|p| p.channel.is_some() && p.closing.is_none())
    }

    /// Whether `stand_in` has a peer, connecting or open.
    pub(crate) fn has_peer(&self, stand_in: SocketAddr) -> bool {
        self.peers
            .get(&stand_in)
            .is_some_and(|p| p.closing.is_none())
    }

    /// When the peer of `stand_in` last carried a game message.
    pub(crate) fn last_heard(&self, stand_in: SocketAddr) -> Option<Instant> {
        self.peers.get(&stand_in).and_then(|p| p.game_heard)
    }

    /// Opens the socket on `ip` (the web transport's address) and the setup's port.
    pub(crate) fn bind(&mut self, ip: IpAddr) -> Result<(), BindError> {
        self.policy.check(ip).map_err(BindError::Policy)?;
        self.close();
        let socket = UdpSocket::bind(SocketAddr::new(ip, self.setup.port))
            .map_err(|e| BindError::Os(e.kind()))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| BindError::Os(e.kind()))?;
        let port = socket
            .local_addr()
            .map_err(|e| BindError::Os(e.kind()))?
            .port();
        let mut advertised = Vec::new();
        if !ip.is_unspecified() {
            advertised.push(SocketAddr::new(ip, port));
        }
        for &public in &self.setup.public {
            let addr = SocketAddr::new(IpAddr::V4(public), port);
            if !advertised.contains(&addr) {
                advertised.push(addr);
            }
        }
        if advertised.is_empty() {
            return Err(BindError::Policy(PolicyRefusal::NoRtcAddress));
        }
        if self.cert.is_none() {
            self.cert = self.crypto.dtls_provider.generate_certificate();
        }
        if self.cert.is_none() {
            return Err(BindError::Os(ErrorKind::Other));
        }
        self.advertised = advertised;
        self.socket = Some(socket);
        Ok(())
    }

    /// A joiner's candidate, for a player's host: taken when it is well formed and its address one the host may check
    /// ([`candidate_allowed`]), within [`MAX_REMOTE_CANDIDATES`] a joiner and [`CANDIDATE_IPS_PER_PEER`] distinct IP
    /// addresses a peer across all joiners; otherwise counted and dropped.
    fn take_candidate(&mut self, stand_in: SocketAddr, line: &str) {
        let mut own: Vec<IpAddr> = self.advertised.iter().map(|a| a.ip()).collect();
        own.extend(self.reflexive.map(|a| a.ip()));
        own.extend(
            self.socket
                .as_ref()
                .and_then(|s| s.local_addr().ok())
                .map(|a| a.ip()),
        );
        let ips_cap = self.setup.limits.max_peers * CANDIDATE_IPS_PER_PEER;
        let lan = self.setup.lan_candidates;
        let Some(peer) = self.peers.get_mut(&stand_in) else {
            return;
        };
        peer.candidates += 1;
        let mut taken = None;
        if peer.candidates <= MAX_REMOTE_CANDIDATES
            && let Some(candidate) = remote_candidate(line)
            && let IpAddr::V4(ip) = candidate.addr().ip()
            && candidate_allowed(ip, lan, &own)
            && (peer.candidate_ips.contains(&candidate.addr().ip())
                || self.candidate_ips.contains_key(&candidate.addr().ip())
                || self.candidate_ips.len() < ips_cap)
        {
            taken = Some(candidate);
        }
        let Some(candidate) = taken else {
            self.counters.candidates_refused += 1;
            return;
        };
        let ip = candidate.addr().ip();
        if !peer.candidate_ips.contains(&ip) {
            peer.candidate_ips.push(ip);
            *self.candidate_ips.entry(ip).or_insert(0) += 1;
        }
        peer.rtc.add_remote_candidate(candidate);
    }

    /// Asks the STUN server at `server` which address this socket is seen at (a player's host behind a NAT gives its
    /// joiners that address too): the answer is read by the next passes, and peers made after it get the candidate.
    pub(crate) fn query_reflexive(&mut self, server: SocketAddr) -> bool {
        use ring::rand::{SecureRandom, SystemRandom};
        let Some(socket) = self.socket.as_ref() else {
            return false;
        };
        let mut id = [0u8; 12];
        if SystemRandom::new().fill(&mut id).is_err() {
            return false;
        }
        let sent = socket.send_to(&stun::binding_request(&id), server).is_ok();
        if sent {
            self.reflexive_query = Some((server, id));
        }
        sent
    }

    /// The address a STUN server saw this socket at, once it answered.
    pub(crate) fn reflexive(&self) -> Option<SocketAddr> {
        self.reflexive
    }

    /// Closes the socket and every peer.
    pub(crate) fn close(&mut self) {
        let all: Vec<SocketAddr> = self.peers.keys().copied().collect();
        for stand_in in all {
            self.finish(stand_in, Why::Gone, false);
        }
        self.offers.clear();
        self.notices.clear();
        self.socket = None;
    }

    /// The peers this side closed since the last call, and why (the web transport tells their game sockets).
    pub(crate) fn take_notices(&mut self) -> Vec<(SocketAddr, Why)> {
        std::mem::take(&mut self.notices)
    }

    /// A game socket closed: its peer goes with it.
    pub(crate) fn remove(&mut self, stand_in: SocketAddr) {
        self.finish(stand_in, Why::Gone, false);
        self.offers.remove(&stand_in);
    }

    /// One signaling message from the game socket of `stand_in`, whose TCP address is `ip`. Err: the message is not one a
    /// browser sends, or breaks a cap (the socket is closed).
    pub(crate) fn signal(
        &mut self,
        stand_in: SocketAddr,
        ip: IpAddr,
        text: &str,
        now: Instant,
    ) -> Result<Reply, ()> {
        let signal = signal::parse(text).ok_or(())?;
        let sdp = match signal {
            Signal::Candidate(line) => {
                if self.setup.host {
                    self.take_candidate(stand_in, line);
                }
                return Ok(Reply::None);
            }
            Signal::Closed => {
                self.finish(stand_in, Why::Peer, false);
                return Ok(Reply::None);
            }
            Signal::Offer(sdp) => sdp,
        };
        let offers = self.offers.entry(stand_in).or_insert(0);
        *offers += 1;
        if *offers > self.setup.limits.offers_per_socket || !signal::data_only(sdp) {
            return Err(());
        }
        // A player's host takes no candidate in the offer as it is: each goes through the same checks as a trickled one.
        let (sdp, offered) = if self.setup.host {
            split_candidates(sdp)
        } else {
            (sdp.to_string(), Vec::new())
        };
        let sdp = sdp.as_str();
        let offer = SdpOffer::from_sdp_string(sdp).map_err(|_| ())?;
        if self.peers.contains_key(&stand_in) {
            for line in &offered {
                self.take_candidate(stand_in, line);
            }
        }
        if let Some(peer) = self.peers.get_mut(&stand_in) {
            // A renegotiation of the peer the socket has. An ICE restart (changed credentials) is refused: the stack would
            // mint new local credentials no check could be routed by, so the browser stays on its game socket instead.
            if signal::ice_credentials(sdp) != peer.remote_ice {
                self.finish(stand_in, Why::Protocol, true);
                return Ok(Reply::None);
            }
            let Ok(answer) = peer.rtc.sdp_api().accept_offer(offer) else {
                self.finish(stand_in, Why::Protocol, false);
                return Err(());
            };
            self.counters.offers += 1;
            return Ok(Reply::Text(signal::message(
                signal::ANSWER,
                &answer.to_sdp_string(),
            )));
        }
        let limits = &self.setup.limits;
        let from_ip = self.peers.values().filter(|p| p.ip == ip).count();
        if now.duration_since(self.new_peers.0) >= Duration::from_secs(1) {
            self.new_peers = (now, 0);
        }
        let room = self.socket.is_some()
            && self.peers.len() < limits.max_peers
            && from_ip < limits.peers_per_ip
            && self.new_peers.1 < limits.new_peers_per_s;
        let busy = || Ok(Reply::Text(signal::BUSY.to_string()));
        if !room {
            self.counters.busy += 1;
            return busy();
        }
        let remote_ice = signal::ice_credentials(sdp);
        let Some((creds, ufrag)) = self.fresh_creds() else {
            self.counters.busy += 1;
            return busy();
        };
        let Some(cert) = self.cert.clone() else {
            return busy();
        };
        let mut rtc = RtcConfig::new()
            .set_ice_lite(!self.setup.host)
            .set_crypto_provider(Arc::clone(&self.crypto))
            .set_dtls_cert(cert)
            .set_local_ice_credentials(creds)
            .set_sctp_max_buffered_amount(limits.send_buffer)
            .set_stats_interval(None)
            .clear_codecs()
            .build(now);
        for &addr in &self.advertised {
            if let Ok(candidate) = Candidate::host(addr, "udp") {
                rtc.add_local_candidate(candidate);
            }
        }
        if let (Some(mapped), Some(&base)) = (self.reflexive, self.advertised.first())
            && let Ok(candidate) = Candidate::server_reflexive(mapped, base, "udp")
        {
            rtc.add_local_candidate(candidate);
        }
        let Ok(answer) = rtc.sdp_api().accept_offer(offer) else {
            return Err(());
        };
        self.new_peers.1 += 1;
        self.counters.offers += 1;
        self.by_ufrag.insert(ufrag.clone(), (stand_in, ip));
        self.peers.insert(
            stand_in,
            Peer {
                rtc,
                ip,
                ufrag,
                remote_ice,
                remotes: VecDeque::new(),
                channel: None,
                opened: now,
                heard: now,
                game_heard: None,
                next: now,
                closing: None,
                meter: Meter::new(now),
                stats: PeerStats::default(),
                candidates: 0,
                candidate_ips: Vec::new(),
                unverified: (now, 0),
                verified: VecDeque::new(),
            },
        );
        for line in &offered {
            self.take_candidate(stand_in, line);
        }
        Ok(Reply::Text(signal::message(
            signal::ANSWER,
            &answer.to_sdp_string(),
        )))
    }

    /// ICE credentials from the system's random source, with a user fragment no peer has.
    fn fresh_creds(&self) -> Option<(IceCreds, Vec<u8>)> {
        use ring::rand::{SecureRandom, SystemRandom};
        let random = SystemRandom::new();
        for _ in 0..4 {
            let mut bytes = [0u8; 32];
            random.fill(&mut bytes).ok()?;
            let text: String = bytes
                .iter()
                .map(|b| char::from(ICE_CHARS[usize::from(*b) % ICE_CHARS.len()]))
                .collect();
            // 8 characters of user fragment (47 bits), 24 of password (143 bits).
            let (ufrag, pass) = text.split_at(8);
            if self.by_ufrag.contains_key(ufrag.as_bytes()) {
                continue;
            }
            return Some((
                IceCreds {
                    ufrag: ufrag.to_string(),
                    pass: pass[..24].to_string(),
                },
                ufrag.as_bytes().to_vec(),
            ));
        }
        None
    }

    /// One pass: the datagrams waiting read and handed to their peers, the peers' timers run, their outputs sent and their
    /// messages put in `inbox` (at most `queue` waiting), every read and step within the caps.
    pub(crate) fn pump(
        &mut self,
        now: Instant,
        inbox: &mut VecDeque<(SocketAddr, Vec<u8>)>,
        queue: usize,
    ) {
        let Some(socket) = self.socket.as_ref() else {
            return;
        };
        for peer in self.peers.values_mut() {
            peer.meter.refill();
        }
        let limits = &self.setup.limits;
        let mut connecting = limits.connecting_per_pass;
        for _ in 0..limits.datagrams_per_pass {
            let (len, from) = match socket.recv_from(&mut self.buf) {
                Ok(got) => got,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                // Windows reports a send to a closed port on the next receive.
                Err(_) => continue,
            };
            self.counters.datagrams += 1;
            if len >= RECV_BYTES || from.is_ipv6() {
                self.counters.stray += 1;
                continue;
            }
            if self.loss.drop_one() {
                self.counters.loss_dropped += 1;
                continue;
            }
            let Some(data) = self.buf.get(..len) else {
                continue;
            };
            let Some(&to) = self.advertised.first() else {
                continue;
            };
            if let Some((server, id)) = self.reflexive_query
                && from == server
            {
                if let Some(mapped) = stun::mapped_address(data, &id) {
                    self.reflexive = Some(mapped);
                    self.reflexive_query = None;
                }
                continue;
            }
            let stun_to = match route_check(
                &self.by_ufrag,
                &mut self.stun_sources,
                limits,
                from,
                data,
                now,
                self.setup.host,
            ) {
                Routed::Peer(stand_in) => Some(stand_in),
                Routed::Stray => {
                    self.counters.stray += 1;
                    continue;
                }
                Routed::Limited => {
                    self.counters.stun_limited += 1;
                    continue;
                }
                Routed::NotACheck => None,
            };
            let Ok(receive) = Receive::new(Protocol::Udp, from, to, data) else {
                self.counters.refused += 1;
                continue;
            };
            let input = Input::Receive(now, receive);
            let (stand_in, new_route) = match stun_to {
                Some(stand_in) => (stand_in, false),
                None => {
                    match accepting(&self.peers, &self.by_remote, from, &input, self.setup.host) {
                        Some(found) => found,
                        None => {
                            self.counters.stray += 1;
                            continue;
                        }
                    }
                }
            };
            let Some(peer) = self.peers.get_mut(&stand_in) else {
                continue;
            };
            if peer.closing.is_some() {
                continue;
            }
            if new_route {
                map_route(&mut self.by_remote, peer, stand_in, from, limits);
            }
            let open = peer.channel.is_some();
            let check = stun_to.is_some();
            // A peer whose handshake spent its budget gets no more of the stack's time; its deadline drops it.
            if !open && !check && !peer.meter.connect_room(limits) {
                self.counters.connect_spent += 1;
                continue;
            }
            match peer.meter.admit(check, now, limits) {
                Take::Ok if open || connecting > 0 => {
                    if !open {
                        connecting -= 1;
                        if !check {
                            peer.meter.spend_connect(len);
                        }
                    }
                }
                Take::Ok | Take::Skip => {
                    self.counters.pass_dropped += 1;
                    continue;
                }
                // Never a reason to close (only authenticated datagrams spend these caps).
                Take::Over => {
                    self.counters.rate_dropped += 1;
                    continue;
                }
            }
            self.counters.handled += 1;
            if peer.rtc.handle_input(input).is_err() {
                self.counters.refused += 1;
                continue;
            }
            let mut sink = Sink {
                socket,
                stand_in,
                inbox: &mut *inbox,
                queue,
                counters: &mut self.counters,
                loss: &mut self.loss,
                limits,
                now,
                authentic: false,
                unverified: &mut self.unverified,
            };
            drain(peer, &mut sink);
            if sink.authentic {
                peer.meter.authentic(len);
            }
        }
        for (&stand_in, peer) in &mut self.peers {
            if peer.closing.is_some() {
                continue;
            }
            if (peer.channel.is_none() && now.duration_since(peer.opened) > limits.connect_timeout)
                || now.duration_since(peer.heard) > limits.idle_timeout
            {
                peer.close(Why::Timeout);
                continue;
            }
            if peer.next > now {
                continue;
            }
            if peer.rtc.handle_input(Input::Timeout(now)).is_err() {
                peer.close(Why::Peer);
                continue;
            }
            drain(
                peer,
                &mut Sink {
                    socket,
                    stand_in,
                    inbox: &mut *inbox,
                    queue,
                    counters: &mut self.counters,
                    loss: &mut self.loss,
                    limits,
                    now,
                    authentic: false,
                    unverified: &mut self.unverified,
                },
            );
        }
        let closing: Vec<(SocketAddr, Why)> = self
            .peers
            .iter()
            .filter_map(|(&a, p)| p.closing.map(|why| (a, why)))
            .collect();
        for (stand_in, why) in closing {
            self.finish(stand_in, why, true);
        }
    }

    /// Sends a datagram on the channel of `to`'s peer. None: it has no open channel (the game socket carries it).
    pub(crate) fn send(
        &mut self,
        to: SocketAddr,
        data: &[u8],
        now: Instant,
        inbox: &mut VecDeque<(SocketAddr, Vec<u8>)>,
        queue: usize,
    ) -> Option<bool> {
        let socket = self.socket.as_ref()?;
        let peer = self.peers.get_mut(&to)?;
        let id = peer.channel?;
        if peer.closing.is_some() {
            return None;
        }
        let written = peer.rtc.channel(id)?.write(true, data);
        let sent = match written {
            Ok(sent) => sent,
            Err(_) => {
                peer.close(Why::Peer);
                return None;
            }
        };
        if sent {
            self.counters.sent += 1;
            peer.stats.messages_out += 1;
        } else {
            self.counters.full += 1;
        }
        drain(
            peer,
            &mut Sink {
                socket,
                stand_in: to,
                inbox,
                queue,
                counters: &mut self.counters,
                loss: &mut self.loss,
                limits: &self.setup.limits,
                now,
                authentic: false,
                unverified: &mut self.unverified,
            },
        );
        Some(sent)
    }

    /// Ends the peer of `stand_in` (its DTLS close notify sent), and tells its game socket when `notify` says so.
    fn finish(&mut self, stand_in: SocketAddr, why: Why, notify: bool) {
        let Some(mut peer) = self.peers.remove(&stand_in) else {
            return;
        };
        why.count(&mut self.counters);
        if let Some(socket) = self.socket.as_ref()
            && peer.rtc.close().is_ok()
        {
            for _ in 0..CLOSE_OUTPUTS {
                match peer.rtc.poll_output() {
                    Ok(Output::Transmit(t)) => {
                        let _ = socket.send_to(&t.contents, t.destination);
                    }
                    Ok(Output::Event(_)) => {}
                    Ok(Output::Timeout(_)) | Err(_) => break,
                }
            }
        }
        for ip in peer.candidate_ips.drain(..) {
            if let Some(n) = self.candidate_ips.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    self.candidate_ips.remove(&ip);
                }
            }
        }
        if self.by_ufrag.get(&peer.ufrag).map(|&(s, _)| s) == Some(stand_in) {
            self.by_ufrag.remove(&peer.ufrag);
        }
        self.by_remote.retain(|_, s| *s != stand_in);
        if notify && self.notices.len() < self.setup.limits.max_peers * 2 {
            self.notices.push((stand_in, why));
        }
    }
}

/// Whether one more check may go to an unverified address of `peer` (one no authenticated datagram or check came from:
/// only a candidate named it): within its rate a second and every peer's.
fn allow_unverified(peer: &mut Peer, sink: &mut Sink<'_>) -> bool {
    take_check(
        &mut peer.unverified,
        sink.unverified,
        sink.now,
        sink.limits.unverified_checks_per_s,
        sink.limits.unverified_checks_per_s_all,
    )
}

/// Counts one check in a peer's window and in every peer's (each a second long, from its first check): false when
/// either is at its cap.
fn take_check(
    peer: &mut (Instant, u32),
    all: &mut (Instant, u32),
    now: Instant,
    peer_cap: u32,
    all_cap: u32,
) -> bool {
    let second = Duration::from_secs(1);
    for window in [&mut *peer, &mut *all] {
        if now.duration_since(window.0) >= second {
            *window = (now, 0);
        }
    }
    if peer.1 >= peer_cap || all.1 >= all_cap {
        return false;
    }
    peer.1 += 1;
    all.1 += 1;
    true
}

/// Whether a player's host may send its checks to `ip`, a joiner's candidate: a public unicast address, or a private one
/// when `lan` allows players on its network, and never one of the host's `own`. Refused: loopback, unspecified,
/// broadcast, multicast, link-local, the documentation, benchmarking, shared (carrier NAT) and reserved ranges, and
/// `0.0.0.0/8`.
pub fn candidate_allowed(ip: Ipv4Addr, lan: bool, own: &[IpAddr]) -> bool {
    let [a, b, c, _] = ip.octets();
    let reserved = a == 0
        || a >= 224
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 192 && b == 0 && c == 0);
    if reserved || own.contains(&IpAddr::V4(ip)) {
        return false;
    }
    lan || !ip.is_private()
}

/// An offer without its candidate lines, and those lines (a player's host takes each as a trickled one).
fn split_candidates(sdp: &str) -> (String, Vec<String>) {
    let mut kept = String::with_capacity(sdp.len());
    let mut lines = Vec::new();
    for line in sdp.split_inclusive('\n') {
        if let Some(c) = line.trim_end().strip_prefix("a=")
            && c.starts_with("candidate:")
        {
            if lines.len() < MAX_REMOTE_CANDIDATES as usize * 4 {
                lines.push(c.to_string());
            }
            continue;
        }
        kept.push_str(line);
    }
    (kept, lines)
}

/// A joiner's candidate line a player's host may take: well formed, IPv4 UDP with a port (a `.local` name, which only
/// the joiner's own network resolves, an IPv6 address or a TCP candidate is left out).
fn remote_candidate(line: &str) -> Option<Candidate> {
    let line = line.trim();
    let line = line.strip_prefix("a=").unwrap_or(line);
    if line.is_empty() || line.len() > 512 || line.contains(".local") {
        return None;
    }
    let candidate = Candidate::from_sdp_string(line).ok()?;
    (candidate.addr().is_ipv4()
        && candidate.proto() == Protocol::Udp
        && candidate.addr().port() != 0)
        .then_some(candidate)
}

/// Where a binding request goes.
enum Routed {
    /// To the peer of this stand-in (its USERNAME names the peer, from its game socket's IP address).
    Peer(SocketAddr),
    /// Nowhere: it names no peer, or comes from another IP address than the peer's game socket.
    Stray,
    /// Nowhere: past its source address's rate.
    Limited,
    /// Not a binding request: it goes by its source address.
    NotACheck,
}

/// The peer a STUN binding request is for: the one its USERNAME names, when it comes from that peer's game socket's IP
/// address (from any address for a player's host, whose joiners have no game socket) and within its source address's
/// rate. Whatever address it comes from, it never maps that address.
fn route_check(
    by_ufrag: &HashMap<Vec<u8>, (SocketAddr, IpAddr)>,
    stun_sources: &mut HashMap<IpAddr, (Instant, u32)>,
    limits: &RtcLimits,
    from: SocketAddr,
    data: &[u8],
    now: Instant,
    any_address: bool,
) -> Routed {
    let Some(ufrag) = stun::binding_ufrag(data) else {
        return Routed::NotACheck;
    };
    let Some(&(stand_in, ip)) = by_ufrag.get(ufrag) else {
        return Routed::Stray;
    };
    if from.ip() != ip && !any_address {
        return Routed::Stray;
    }
    let ip = from.ip();
    if stun_sources.len() >= NET_LIMITER_ENTRIES && !stun_sources.contains_key(&ip) {
        stun_sources.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(1));
        if stun_sources.len() >= NET_LIMITER_ENTRIES {
            return Routed::Limited;
        }
    }
    let (start, count) = stun_sources.entry(ip).or_insert((now, 0));
    if now.duration_since(*start) >= Duration::from_secs(1) {
        *start = now;
        *count = 0;
    }
    *count += 1;
    if *count > limits.stun_per_ip_per_s {
        return Routed::Limited;
    }
    Routed::Peer(stand_in)
}

/// The peer a datagram that is not a binding request goes to, and whether its source address is new to it: the peer the
/// address is mapped to while that peer's stack still accepts the address (validated by an authenticated check), else the
/// first peer of the same IP address whose stack accepts it (of any address for a player's host: a relayed joiner comes
/// from its TURN server's). None: no peer accepts it.
fn accepting(
    peers: &HashMap<SocketAddr, Peer>,
    by_remote: &HashMap<SocketAddr, SocketAddr>,
    from: SocketAddr,
    input: &Input<'_>,
    any_address: bool,
) -> Option<(SocketAddr, bool)> {
    if let Some(stand_in) = by_remote.get(&from)
        && let Some(peer) = peers.get(stand_in)
        && peer.closing.is_none()
        && peer.rtc.accepts(input)
    {
        return Some((*stand_in, false));
    }
    let mut same_ip: Vec<(&SocketAddr, &Peer)> = peers
        .iter()
        .filter(|(_, p)| (any_address || p.ip == from.ip()) && p.closing.is_none())
        .collect();
    same_ip.sort_by_key(|(a, _)| **a);
    let (&stand_in, _) = same_ip.into_iter().find(|(_, p)| p.rtc.accepts(input))?;
    Some((stand_in, true))
}

/// Maps `from` to `peer` (of `stand_in`): at most [`MAX_REMOTES`] addresses a peer (its oldest given up first), and
/// while the map holds fewer than that for every peer there may be (else the datagram goes unmapped).
fn map_route(
    by_remote: &mut HashMap<SocketAddr, SocketAddr>,
    peer: &mut Peer,
    stand_in: SocketAddr,
    from: SocketAddr,
    limits: &RtcLimits,
) {
    if !by_remote.contains_key(&from) && by_remote.len() >= limits.max_peers * MAX_REMOTES {
        return;
    }
    by_remote.insert(from, stand_in);
    peer.remotes.retain(|&r| r != from);
    peer.remotes.push_back(from);
    while peer.remotes.len() > MAX_REMOTES {
        let Some(old) = peer.remotes.pop_front() else {
            break;
        };
        if by_remote.get(&old) == Some(&stand_in) {
            by_remote.remove(&old);
        }
    }
}

/// Takes a peer's outputs (at most the pass's cap): datagrams sent, events handled, the next wake-up kept.
fn drain(peer: &mut Peer, sink: &mut Sink<'_>) {
    for _ in 0..sink.limits.outputs_per_pass {
        let output = match peer.rtc.poll_output() {
            Ok(o) => o,
            Err(_) => {
                peer.close(Why::Peer);
                return;
            }
        };
        match output {
            Output::Timeout(at) => {
                peer.next = at;
                return;
            }
            Output::Transmit(t) => {
                // A binding success response: the stack answered a check whose integrity held.
                if t.contents.get(..2) == Some(&[0x01, 0x01][..]) {
                    peer.heard = sink.now;
                    sink.authentic = true;
                    // The check came from there and held its integrity: the address is the peer's.
                    if !peer.verified.contains(&t.destination) {
                        peer.verified.push_back(t.destination);
                        if peer.verified.len() > VERIFIED_PER_PEER {
                            peer.verified.pop_front();
                        }
                    }
                }
                // A check of our own to an address nothing authenticated came from yet: held to its rates.
                if t.contents.get(..2) == Some(&[0x00, 0x01][..])
                    && !peer.remotes.contains(&t.destination)
                    && !peer.verified.contains(&t.destination)
                    && !allow_unverified(peer, sink)
                {
                    sink.counters.checks_held += 1;
                    continue;
                }
                if sink.loss.drop_one() {
                    sink.counters.loss_dropped += 1;
                    continue;
                }
                let _ = sink.socket.send_to(&t.contents, t.destination);
            }
            Output::Event(event) => {
                event_of(peer, event, sink);
                if peer.closing.is_some() {
                    return;
                }
            }
        }
    }
    // More outputs than a pass takes: the rest on the next.
    peer.next = sink.now;
}

fn event_of(peer: &mut Peer, event: Event, sink: &mut Sink<'_>) {
    match event {
        Event::IceConnectionStateChange(IceConnectionState::Disconnected)
        | Event::ChannelClose(_)
        | Event::Closed => peer.close(Why::Peer),
        Event::Connected => {
            peer.heard = sink.now;
            sink.authentic = true;
        }
        Event::ChannelOpen(id, label) => {
            peer.heard = sink.now;
            sink.authentic = true;
            let config = peer.rtc.channel(id).and_then(|c| c.config().cloned());
            let game = config.is_some_and(|c| {
                !c.ordered && c.reliability == Reliability::MaxRetransmits { retransmits: 0 }
            });
            if peer.channel.is_some() || label != signal::CHANNEL_LABEL || !game {
                peer.close(Why::Protocol);
                return;
            }
            peer.channel = Some(id);
            peer.stats.open = true;
            sink.counters.channels += 1;
        }
        Event::ChannelData(data) => {
            peer.heard = sink.now;
            sink.authentic = true;
            if Some(data.id) != peer.channel || !data.binary {
                peer.close(Why::Protocol);
                return;
            }
            if data.data.len() > sink.limits.max_message {
                peer.close(Why::Oversize);
                return;
            }
            match peer.meter.message(sink.now, sink.limits) {
                Take::Ok => {}
                Take::Skip => {
                    sink.counters.pass_dropped += 1;
                    return;
                }
                Take::Over => {
                    peer.close(Why::Rate);
                    return;
                }
            }
            if let Some(last) = peer.game_heard {
                let gap = sink.now.duration_since(last);
                peer.stats.max_gap = peer.stats.max_gap.max(gap);
            }
            peer.game_heard = Some(sink.now);
            peer.stats.messages_in += 1;
            if sink.inbox.len() < sink.queue {
                sink.counters.messages += 1;
                sink.inbox.push_back((sink.stand_in, data.data));
            } else {
                sink.counters.full += 1;
            }
        }
        // No media until voice: an offer with audio or video is refused, so none of this comes from a browser.
        Event::MediaAdded(_) | Event::MediaData(_) | Event::RtpPacket(_) => {
            peer.close(Why::Protocol)
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_checks_only_addresses_it_may() {
        let own = [
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9)),
            IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
        ];
        let public = Ipv4Addr::new(93, 184, 216, 35);
        assert!(candidate_allowed(public, false, &own));
        for refused in [
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(127, 9, 9, 9),
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::new(0, 1, 2, 3),
            Ipv4Addr::BROADCAST,
            Ipv4Addr::new(224, 0, 0, 251),
            Ipv4Addr::new(239, 255, 255, 250),
            Ipv4Addr::new(169, 254, 169, 254),
            Ipv4Addr::new(192, 0, 2, 1),
            Ipv4Addr::new(198, 51, 100, 1),
            Ipv4Addr::new(203, 0, 113, 1),
            Ipv4Addr::new(198, 18, 0, 1),
            Ipv4Addr::new(100, 64, 0, 1),
            Ipv4Addr::new(192, 0, 0, 8),
            Ipv4Addr::new(240, 0, 0, 1),
            Ipv4Addr::new(93, 184, 216, 34),
        ] {
            assert!(!candidate_allowed(refused, true, &own), "{refused}");
        }
        for private in [
            Ipv4Addr::new(10, 0, 0, 2),
            Ipv4Addr::new(172, 16, 0, 2),
            Ipv4Addr::new(192, 168, 1, 2),
        ] {
            assert!(
                !candidate_allowed(private, false, &own),
                "{private} without LAN play"
            );
            assert!(
                candidate_allowed(private, true, &own),
                "{private} with LAN play"
            );
        }
    }

    #[test]
    fn offers_lose_their_candidates_to_the_checks() {
        let sdp = "v=0
m=application 9 UDP/DTLS/SCTP webrtc-datachannel
a=candidate:1 1 udp 1 127.0.0.1 9 typ host
a=mid:0
";
        let (kept, lines) = split_candidates(sdp);
        assert_eq!(
            kept,
            "v=0
m=application 9 UDP/DTLS/SCTP webrtc-datachannel
a=mid:0
"
        );
        assert_eq!(lines, ["candidate:1 1 udp 1 127.0.0.1 9 typ host"]);
        let many = "a=candidate:1 1 udp 1 1.2.3.4 9 typ host
"
        .repeat(100);
        assert_eq!(
            split_candidates(&many).1.len(),
            MAX_REMOTE_CANDIDATES as usize * 4
        );
        assert!(remote_candidate("candidate:1 1 udp 1 abcd.local 9 typ host").is_none());
        assert!(
            remote_candidate("candidate:1 1 tcp 1 1.2.3.4 9 typ host tcptype passive").is_none()
        );
        assert!(remote_candidate("candidate:1 1 udp 1 1.2.3.4 0 typ host").is_none());
        assert!(remote_candidate("garbage").is_none());
        assert!(remote_candidate("a=candidate:1 1 udp 1 1.2.3.4 9 typ host").is_some());
    }

    #[test]
    fn checks_to_unverified_addresses_are_rated() {
        let t0 = Instant::now();
        let (mut a, mut b, mut all) = ((t0, 0), (t0, 0), (t0, 0));
        let sent = (0..30)
            .filter(|_| take_check(&mut a, &mut all, t0, 20, 25))
            .count();
        assert_eq!(sent, 20, "a peer's cap");
        let sent = (0..30)
            .filter(|_| take_check(&mut b, &mut all, t0, 20, 25))
            .count();
        assert_eq!(sent, 5, "every peer's cap");
        let later = t0 + Duration::from_secs(1);
        assert!(take_check(&mut b, &mut all, later, 20, 25), "a new second");
    }
}
