//! Engine mode on the server: the network host's connections ride `torque_net::engine` sessions instead of TGE's handshake and
//! packet layer, and everything above the framing (the payload layer: events, net strings, ghosts, moves, channels) is the
//! same as in TGE mode.
//!
//! A server is in engine mode when its script engine's [`ProtocolMode`] says so; the host then keeps an [`EngineHost`]
//! ([`NetShared::engine_host`], made at the first pass). Its [`SessionTable`] routes each datagram by source, puts the gate in
//! front of plain links and caps the sessions one address and network may hold (the cap the framing leaves to the host),
//! counted by the peer's own address ([`Transport::peer_ip`](super::transport::Transport::peer_ip)): the same
//! [`torque_net::fair::SourceCaps`] as TGE mode's, read from the handshake's
//! [`ServerConfig::caps`](torque_net::handshake::ServerConfig::caps) when the host is made. A source's connects are counted
//! per address and per network as TGE mode's verified requests are ([`torque_net::handshake::verified_limits`]). A session's connect
//! becomes a [`PendingRequest`] judged as in TGE mode (the password, the player count, the connect gate with its manifest
//! and ticket, then `onConnectRequest`); an accepted one enters the handshake table as an engine connection
//! ([`Server::insert_engine`](torque_net::handshake::Server::insert_engine)) and the accept goes out on the control lane
//! with the packet size, the interval, the byte rate, the widths and the table digests. Every map the host keeps beside the
//! table is keyed by the canonical address ([`canonical`]).
//!
//! **Tickets.** A server with an auth provider ([`Auth::Challenge`]) challenges every connect with a nonce it draws, and
//! checks the ticket that answers it against that nonce, its own address and identity ([`ticket_context`]): a ticket in the
//! connect itself is ignored, so a ticket captured on one connection fails on any other. The connects waiting for their
//! answer are at most the table's pending cap. A ticket whose connection the scripts accepted is refused for
//! [`TICKET_REPLAY_MS`] after ([`TicketCache`], by the provider's canonical token), so a bearer token is good once in any
//! window; what the cache holds is shared out per source network and per identity, and its intake per second, so a flood
//! of valid tickets from one network locks out only that network. Tickets are checked on links that are not encrypted too (the memory link, plain UDP, `ws`),
//! which listen on private addresses only; the server says so once on its console.
//!
//! **Packets.** Once a tick (the send pass) each connection's session is polled: the payload layer writes into the ghost lane
//! ([`StreamLane::Ghosts`]), a client writes its moves and events into the move lane, and RPC batches ride the RPC lanes
//! ([`RpcLanes`]). Each packet the payload went into gets a wide number in send order ([`PacketOrder`]); its fate reaches the
//! payload layer in that order, as TGE's notifies do. A packet is at most the link's datagram size and the connection's byte
//! credit: [`DEFAULT_BYTE_RATE`] (or `$Pref::Server::MaxClientBytesPerSecond`, read each tick) a second as a ceiling on every
//! link, and on a plain link the engine's own loss backoff under it, over every packet (halved past 10% loss over a second,
//! else 10% more, never under [`MIN_BYTE_RATE`]).
//!
//! **Plain links are LAN only.** A plain link's datagrams are not served while the transport is bound to an address outside
//! the private set ([`PlainEngineRefusal`](super::transport::PlainEngineRefusal)); the host warns once.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::rc::Rc;
use std::sync::Arc;

use torque_engine::ProtocolKind;
use torque_net::NetError;
use torque_net::bits::BitWriter;
use torque_net::connect::{AuthContext, ConnectGate, Identity};
use torque_net::engine::bridge::{fields_of, ticket_context};
use torque_net::engine::{
    Accept, Arrival, CloseReason, Connect, DEFAULT_MAX_PACKET, Event, Link, LinkFacts, LinkKind,
    NoRoom, NoStreams, PacketFates, PacketOrder, Polled, RejectCode, SessionConfig, SessionTable,
    State, StreamLane, StreamSource, TableDigests, TableLimits, Widths, canonical,
};
use torque_net::fair::{FairLimiter, NetworkKey, SourceCaps};
use torque_net::handshake::{
    ConnId, ConnectFields, Decision, EngineAccepted, Finished, PendingRequest, RateWindow,
    siphash24, verified_limits,
};
use torque_net::huffman::Table;
use torque_net::profile::HandshakeProfile;
use torque_net::protocol::{Notify, Protocol, Received};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::conn::DeleteMode;
use super::payload::{ConnCtx, PayloadLayer};
use super::peer::ProtocolMode;
use super::pump::{self, cost};
use super::rpc::RpcLanes;
use super::transport::{SharedTransport, check_plain_engine_bind};
use super::{NetGame, NetShared, digest_key};

/// The bytes a second a client is sent at most unless `$Pref::Server::MaxClientBytesPerSecond` says otherwise.
pub const DEFAULT_BYTE_RATE: u32 = 256 << 10;
/// The least a plain link's loss backoff lowers a client's byte rate to.
pub const MIN_BYTE_RATE: u32 = 16 << 10;
/// The preference that sets the byte rate's ceiling.
pub const BYTE_RATE_PREF: &str = "$Pref::Server::MaxClientBytesPerSecond";
/// Milliseconds between two packets each way: one a tick.
pub const PACKET_INTERVAL_MS: u16 = 32;
/// How long a ticket the server accepted stays refused.
pub const TICKET_REPLAY_MS: u32 = 120_000;
/// The tickets the server accepts in all, a second: so many that the cache can never fill within a window
/// (32 x 120 s = 3,840 < [`TICKET_CACHE`]).
pub const TICKETS_PER_S: u32 = 32;
/// The tickets [`TicketCache`] holds at most; [`TICKETS_PER_S`] keeps it from filling within [`TICKET_REPLAY_MS`], and if it
/// is full anyway (a clock that jumped) new tickets are refused until it empties.
pub const TICKET_CACHE: usize = 4_096;
/// The tickets accepted from one source network (an IPv4 /24, an IPv6 /64) that the cache holds in a window; past it that
/// network's new tickets are refused, and no other network's.
pub const TICKETS_PER_NETWORK: usize = 64;
/// The tickets accepted for one identity that the cache holds in a window.
pub const TICKETS_PER_IDENTITY: usize = 16;
/// The work units a connect costs beyond its bytes (it runs script), as in TGE mode.
const REQUEST_WORK: u32 = 8;
/// The reject text of a ticket presented again.
const TICKET_USED: &[u8] = b"this ticket was used already";
/// The reject text of a ticket over a network's, an identity's or the whole server's share.
const TICKETS_FULL: &[u8] = b"too many sign-ins from here just now; try again later";

type Shared<G> = std::rc::Rc<std::cell::RefCell<NetShared<G>>>;

/// What [`TicketCache::admit`] or [`TicketCache::record`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    Fresh,
    /// Accepted within the window.
    Replayed,
    /// The ticket's network, its identity or the whole server has had its share: refused, for them only.
    Full,
}

/// One accepted ticket the cache holds.
#[derive(Debug, Clone, Copy)]
struct Held {
    digest: u64,
    at: u32,
    network: NetworkKey,
    identity: u64,
}

/// The tickets a server accepted lately, by a digest of their canonical tokens under the cache's own key: one presented again
/// within [`TICKET_REPLAY_MS`] is refused. What it holds is shared out so that no one can crowd the others out: at most
/// [`TICKETS_PER_NETWORK`] from one source network and [`TICKETS_PER_IDENTITY`] for one identity in a window, and at most
/// [`TICKETS_PER_S`] accepted a second in all. A ticket is recorded only once the scripts accepted its connection
/// ([`TicketCache::record`]).
#[derive(Debug)]
pub struct TicketCache {
    key: [u8; 16],
    seen: HashSet<u64>,
    order: VecDeque<Held>,
    per_network: HashMap<NetworkKey, usize>,
    per_identity: HashMap<u64, usize>,
    rate: RateWindow,
}

impl TicketCache {
    #[must_use]
    pub fn new(key: [u8; 16]) -> TicketCache {
        TicketCache {
            key,
            seen: HashSet::new(),
            order: VecDeque::new(),
            per_network: HashMap::new(),
            per_identity: HashMap::new(),
            rate: RateWindow::default(),
        }
    }

    fn digest(&self, bytes: &[u8]) -> u64 {
        siphash24(&self.key, bytes)
    }

    fn expire(&mut self, now: u32) {
        while let Some(&held) = self.order.front()
            && now.wrapping_sub(held.at) >= TICKET_REPLAY_MS
        {
            self.order.pop_front();
            self.seen.remove(&held.digest);
            for count in [
                self.per_network.get_mut(&held.network),
                self.per_identity.get_mut(&held.identity),
            ]
            .into_iter()
            .flatten()
            {
                *count -= 1;
            }
            self.per_network.retain(|_, n| *n > 0);
            self.per_identity.retain(|_, n| *n > 0);
        }
    }

    /// Whether the ticket whose canonical token is `token`, from a source at `ip`, may be checked at `now`.
    pub fn admit(&mut self, token: &[u8], ip: IpAddr, now: u32) -> Admit {
        self.expire(now);
        let network = self
            .per_network
            .get(&NetworkKey::of(ip))
            .copied()
            .unwrap_or(0);
        if self.seen.contains(&self.digest(token)) {
            Admit::Replayed
        } else if network >= TICKETS_PER_NETWORK || self.order.len() >= TICKET_CACHE {
            Admit::Full
        } else {
            Admit::Fresh
        }
    }

    /// Records the ticket `token` from a source at `ip` for `identity`, whose connection the scripts accepted at `now`.
    /// `Full` (the identity's share, or the server's for this second) and `Replayed` record nothing, and the connection is
    /// refused.
    pub fn record(&mut self, token: &[u8], ip: IpAddr, identity: &[u8], now: u32) -> Admit {
        let admit = self.admit(token, ip, now);
        if admit != Admit::Fresh {
            return admit;
        }
        let identity = self.digest(identity);
        if self.per_identity.get(&identity).copied().unwrap_or(0) >= TICKETS_PER_IDENTITY
            || !self.rate.allow(now, TICKETS_PER_S)
        {
            return Admit::Full;
        }
        let held = Held {
            digest: self.digest(token),
            at: now,
            network: NetworkKey::of(ip),
            identity,
        };
        self.seen.insert(held.digest);
        *self.per_network.entry(held.network).or_default() += 1;
        *self.per_identity.entry(identity).or_default() += 1;
        self.order.push_back(held);
        Admit::Fresh
    }

    /// The tickets held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// A ticket that passed its checks, recorded once the scripts accept its connection ([`finish`]).
struct Checked {
    token: Vec<u8>,
    ip: IpAddr,
}

/// Whether the server checks tickets, and so challenges every connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Auth {
    /// No auth provider: connects are judged without tickets.
    #[default]
    None,
    /// An auth provider: every connect is challenged, and its ticket checked against the challenge.
    Challenge,
}

/// What the scripts made of the request being decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// No accept or reject was queued (the scripts dropped the request).
    None,
    /// [`finish`] queued an accept or a reject on the session.
    Given,
}

/// What the host says once on its console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Once {
    PublicPlainBind,
    UnencryptedTickets,
}

/// A connect waiting for the answer to its challenge.
struct Stash {
    fields: Box<ConnectFields>,
    challenge: u64,
}

/// One connection's byte credit and, on a plain link, its loss backoff.
#[derive(Debug, Clone)]
struct Pace {
    cap: u32,
    rate: u32,
    credit: u64,
    last: Option<u32>,
    window: Option<u32>,
    /// The session's packet fates when the window began.
    base: PacketFates,
}

impl Pace {
    fn new(cap: u32) -> Pace {
        Pace {
            cap,
            rate: cap,
            credit: u64::from(DEFAULT_MAX_PACKET),
            last: None,
            window: None,
            base: PacketFates::default(),
        }
    }

    /// Follows the preference's ceiling (read each tick): the rate never goes above it.
    fn set_cap(&mut self, cap: u32) {
        self.cap = cap;
        self.rate = self.rate.min(cap);
    }

    /// The bytes the connection's next packet may take at `now`: the credit earned since the last packet at the current rate,
    /// saved up to an eighth of a second's worth.
    fn room(&mut self, now: u32, kind: LinkKind) -> usize {
        if let Some(last) = self.last {
            let dt = u64::from(now.wrapping_sub(last).min(1_000));
            self.credit += u64::from(self.rate) * dt / 1_000;
        }
        self.last = Some(now);
        let most = (u64::from(self.rate) / 8).max(u64::from(DEFAULT_MAX_PACKET));
        self.credit = self.credit.min(most);
        usize::try_from(self.credit)
            .unwrap_or(usize::MAX)
            .min(kind.max_datagram)
    }

    fn spend(&mut self, bytes: usize) {
        self.credit = self.credit.saturating_sub(bytes as u64);
    }

    /// The session's packet fates so far (every packet). On a plain link, each second the rate is halved when more than a
    /// tenth of the second's packets were lost, else raised by a tenth, within [`MIN_BYTE_RATE`] and the cap.
    fn observe(&mut self, fates: PacketFates, now: u32, plain: bool) {
        let start = *self.window.get_or_insert_with(|| {
            self.base = fates;
            now
        });
        if now.wrapping_sub(start) < 1_000 {
            return;
        }
        let lost = fates.lost - self.base.lost;
        let all = lost + fates.delivered - self.base.delivered;
        if plain && all > 0 {
            self.rate = if lost * 10 > all {
                (self.rate / 2).max(MIN_BYTE_RATE)
            } else {
                self.rate.saturating_add(self.rate / 10).min(self.cap)
            };
        }
        self.window = Some(now);
        self.base = fates;
    }
}

/// A server's engine-mode state (module docs).
pub struct EngineHost {
    pub table: SessionTable,
    /// Whether every connect is challenged for a ticket (the host that knows the engine's auth provider sets it).
    pub auth: Auth,
    pub tickets: TicketCache,
    /// The digests of the channel, RPC and upload tables the accept carries (the host that knows the engine's manifest sets
    /// them).
    pub digests: TableDigests,
    stashed: BTreeMap<SocketAddr, Stash>,
    pace: BTreeMap<SocketAddr, Pace>,
    requests: FairLimiter,
    /// The byte rate the next accept names.
    byte_rate: u32,
    verdict: Verdict,
    /// The ticket of the request being decided, recorded if the scripts accept it.
    checked: Option<Checked>,
    said: BTreeSet<Once>,
}

impl EngineHost {
    /// A host whose sessions speak for `game`, whose cookies and session ids come from `key` and whose ticket cache digests
    /// under `ticket_key` (both from the host's random source), and which lets one source hold what `caps` allow.
    #[must_use]
    pub fn new(game: &str, key: [u8; 16], ticket_key: [u8; 16], caps: SourceCaps) -> EngineHost {
        let limits = TableLimits {
            caps,
            ..TableLimits::default()
        };
        EngineHost {
            table: SessionTable::new(SessionConfig::new(game), limits, key),
            auth: Auth::None,
            tickets: TicketCache::new(ticket_key),
            digests: TableDigests::default(),
            stashed: BTreeMap::new(),
            pace: BTreeMap::new(),
            requests: FairLimiter::new(verified_limits(0)),
            byte_rate: DEFAULT_BYTE_RATE,
            verdict: Verdict::None,
            checked: None,
            said: BTreeSet::new(),
        }
    }

    /// The connects waiting for the answer to their challenge (at most the table's pending cap).
    #[must_use]
    pub fn challenged(&self) -> usize {
        self.stashed.len()
    }

    /// Says `line` on `vm`'s console the first time `what` comes up.
    fn say_once(&mut self, vm: &mut Vm, what: Once, line: impl FnOnce() -> String) {
        if self.said.insert(what) {
            vm.warn(line());
        }
    }

    /// Takes `key`'s session and everything kept beside it out (the session may be gone from the table already).
    fn drop_peer(&mut self, key: SocketAddr) {
        self.table.remove(key);
        self.pace.remove(&key);
        self.stashed.remove(&key);
    }
}

/// The server's engine host, made now if the server speaks the engine's protocol and has none yet; `None` in TGE mode.
pub fn ensure_host<'a, G: NetGame>(
    vm: &Vm,
    sh: &'a mut NetShared<G>,
) -> Option<&'a mut EngineHost> {
    if sh.engine_host.is_none() && ProtocolMode::of(vm) == ProtocolKind::Engine {
        let config = sh.hs.config();
        let game = String::from_utf8_lossy(&config.profile.game).into_owned();
        let caps = config.caps;
        sh.engine_host = Some(Box::new(EngineHost::new(
            &game,
            digest_key(),
            digest_key(),
            caps,
        )));
    }
    sh.engine_host.as_deref_mut()
}

/// The link to one peer, through the shared transport.
struct HostLink<'a> {
    transport: &'a SharedTransport,
    to: SocketAddr,
    kind: LinkKind,
    room: usize,
}

impl Link for HostLink<'_> {
    fn max_datagram(&self) -> usize {
        self.kind.max_datagram.min(self.room)
    }

    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        if self.transport.borrow_mut().send_to(self.to, datagram) {
            Ok(())
        } else {
            Err(NoRoom)
        }
    }

    fn facts(&self) -> LinkFacts {
        self.kind.facts
    }
}

/// The payload layer as a session's stream source: it writes the ghost lane of each packet.
struct Writer<'a> {
    payload: &'a mut dyn PayloadLayer,
    protocol: &'a mut Protocol,
    order: &'a mut PacketOrder,
    vm: &'a mut Vm,
    conn: ConnId,
    obj: Option<ObjId>,
    ghost_bits: u8,
    table: Rc<Table>,
    now: u32,
    failed: Option<NetError>,
    withheld: u64,
}

impl StreamSource for Writer<'_> {
    fn write(&mut self, lane: StreamLane, seq: u16, room: usize, out: &mut Vec<u8>) {
        if lane != StreamLane::Ghosts || room == 0 || self.failed.is_some() {
            return;
        }
        let wide = self.order.sent(seq);
        self.protocol.set_engine_packet(wide, room);
        let mut w = BitWriter::with_limit(room * 8);
        let mut free = |_: u64| true;
        let mut ctx = ConnCtx {
            conn: self.conn,
            obj: self.obj,
            protocol: &mut *self.protocol,
            now: self.now,
            vm: &mut *self.vm,
            ghost_bits: self.ghost_bits,
            table: Rc::clone(&self.table),
            charge: &mut free,
        };
        let bytes = match self.payload.write(&mut ctx, &mut w) {
            Err(e) => {
                self.failed = Some(e);
                None
            }
            Ok(()) if w.is_overflowed() => {
                self.withheld += 1;
                None
            }
            Ok(()) => Some(w.into_bytes()),
        };
        match bytes {
            Some(bytes) if !bytes.is_empty() && bytes.len() <= room => {
                out.extend_from_slice(&bytes);
            }
            // What was written is counted lost at once, so the layer sends it again.
            _ => self.order.unsent(seq),
        }
    }

    fn unsent(&mut self, seq: u16) {
        self.order.unsent(seq);
    }
}

/// The engine connection of the session at `addr`, if the host accepted it.
fn conn_of<G: NetGame>(sh: &NetShared<G>, addr: SocketAddr) -> Option<ConnId> {
    sh.hs
        .lookup(addr)
        .filter(|id| sh.hs.connection(*id).is_some_and(|c| c.engine))
}

/// The byte rate's ceiling from the preference, never under [`MIN_BYTE_RATE`].
pub(super) fn byte_rate_pref(vm: &Vm) -> u32 {
    pump::global_text(vm, BYTE_RATE_PREF)
        .and_then(|v| String::from_utf8_lossy(&v).trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_BYTE_RATE)
        .max(MIN_BYTE_RATE)
}

/// Whether a plain link's datagram must be refused: the transport is a real socket bound outside the private set.
fn plain_refused<G: NetGame>(
    vm: &mut Vm,
    sh: &mut NetShared<G>,
    transport: &SharedTransport,
) -> bool {
    let (bound, real) = {
        let t = transport.borrow();
        (t.local_addr(), t.is_real())
    };
    let Some(bound) = bound.filter(|_| real) else {
        return false;
    };
    let Err(refusal) = check_plain_engine_bind(sh.codecs.lan_rules(), bound.ip()) else {
        return false;
    };
    if let Some(host) = sh.engine_host.as_mut() {
        host.say_once(vm, Once::PublicPlainBind, || refusal.to_string());
    }
    true
}

/// One datagram in engine mode (the receive pass's [`pump::receive`] hands each here). Returns extra work units.
pub(super) fn receive<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    from: SocketAddr,
    data: &[u8],
    now: u32,
) -> u32 {
    let transport = Rc::clone(&shared.borrow().transport);
    let (kind, ip) = {
        let t = transport.borrow();
        (t.link_kind(from), t.peer_ip(from))
    };
    let arrival = {
        let mut sh = shared.borrow_mut();
        if !kind.facts.validated && plain_refused(vm, &mut sh, &transport) {
            Arrival::Dropped
        } else {
            match ensure_host(vm, &mut sh) {
                Some(host) => host.table.receive(from, ip, kind, data, now),
                None => Arrival::Dropped,
            }
        }
    };
    match arrival {
        Arrival::Dropped => shared.borrow_mut().counters.ignored += 1,
        Arrival::Reply(reply) => {
            pump::send(shared, from, &reply);
        }
        Arrival::Session { .. } => {}
    }
    drop_evicted(vm, shared);
    events(vm, shared, from, now)
}

/// Drops what the host made for the sessions the table evicted: their connections and what is kept beside them.
fn drop_evicted<G: NetGame>(vm: &mut Vm, shared: &Shared<G>) {
    let evicted = shared
        .borrow_mut()
        .engine_host
        .as_mut()
        .map(|h| h.table.take_evicted())
        .unwrap_or_default();
    for addr in evicted {
        let conn = conn_of(&shared.borrow(), addr);
        forget(vm, shared, addr);
        if let Some(conn) = conn {
            pump::drop_conn(vm, shared, conn, None, DeleteMode::Immediate);
        }
    }
}

/// Works through the events of `addr`'s session. Returns extra work units.
fn events<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, addr: SocketAddr, now: u32) -> u32 {
    let mut work = 0u32;
    loop {
        let event = {
            let mut sh = shared.borrow_mut();
            let Some(peer) = sh.engine_host.as_mut().and_then(|h| h.table.peer_mut(addr)) else {
                break;
            };
            peer.session.next_event()
        };
        let Some(event) = event else { break };
        match event {
            Event::ConnectRequest(connect) => {
                request(vm, shared, addr, &connect, now);
                work = work.saturating_add(REQUEST_WORK);
            }
            Event::Ticket(ticket) => {
                answered(vm, shared, addr, &ticket, now);
                work = work.saturating_add(REQUEST_WORK);
            }
            Event::Accepted(_) | Event::Challenge(_) => {}
            Event::Message { data, .. } => message(vm, shared, addr, data),
            Event::Stream {
                lane: StreamLane::Moves,
                seq,
                data,
            } => work = work.saturating_add(stream(vm, shared, addr, seq, &data, now)),
            Event::Stream { .. } => fail(vm, shared, addr, "a stream lane a server does not read"),
            Event::Notify { seq, delivered } => {
                if let Some(peer) = shared
                    .borrow_mut()
                    .engine_host
                    .as_mut()
                    .and_then(|h| h.table.peer_mut(addr))
                {
                    peer.order.resolve(seq, delivered);
                }
                fates(vm, shared, addr, now);
            }
            Event::Closed(reason) => {
                closed(vm, shared, addr, reason);
                break;
            }
        }
    }
    work
}

/// Hands the payload layer the fates of `addr`'s packets that are known, in send order.
fn fates<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, addr: SocketAddr, now: u32) {
    let mut sh = shared.borrow_mut();
    let sh = &mut *sh;
    let Some(conn) = conn_of(sh, addr) else {
        return;
    };
    let obj = sh.by_conn.get(&conn).copied();
    let table = sh.game.string_table();
    let NetShared {
        hs,
        engine_host,
        payload,
        ..
    } = sh;
    let (Some(host), Some(c)) = (engine_host.as_mut(), hs.connection_mut(conn)) else {
        return;
    };
    let Some(peer) = host.table.peer_mut(addr) else {
        return;
    };
    let ghost_bits = c.ghost_bits;
    let mut free = |_: u64| true;
    let mut ctx = ConnCtx {
        conn,
        obj,
        protocol: &mut c.protocol,
        now,
        vm,
        ghost_bits,
        table,
        charge: &mut free,
    };
    while let Some((send_seq, delivered)) = peer.order.pop() {
        payload.on_notify(
            &mut ctx,
            Notify {
                send_seq,
                delivered,
            },
        );
    }
}

/// A client's packet: its move lane is the payload a TGE packet carries after its header.
fn stream<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    addr: SocketAddr,
    seq: u16,
    data: &[u8],
    now: u32,
) -> u32 {
    let Some(conn) = conn_of(&shared.borrow(), addr) else {
        return 0;
    };
    pump::handle_game(
        vm,
        shared,
        conn,
        &Received::payload(u32::from(seq)),
        0,
        data,
        now,
    )
}

/// An RPC batch from `addr`'s client, for the RPC pump.
fn message<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, addr: SocketAddr, data: Vec<u8>) {
    let Some(conn) = conn_of(&shared.borrow(), addr) else {
        return;
    };
    match vm.extension::<RpcLanes>() {
        Some(lanes) if lanes.received(Some(conn), data) => {}
        Some(_) => pump::packet_error(vm, shared, conn, "too many RPC bytes held"),
        None => pump::packet_error(vm, shared, conn, "an RPC message to a server without RPCs"),
    }
}

/// `addr`'s peer sent what no client of this engine sends: its connection ends with a packet error (a session the host has
/// not accepted just closes).
fn fail<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, addr: SocketAddr, why: &str) {
    let conn = conn_of(&shared.borrow(), addr);
    match conn {
        Some(conn) => pump::packet_error(vm, shared, conn, why),
        None => forget(vm, shared, addr),
    }
}

/// `addr`'s session closed: it leaves the table, and its connection is dropped for the session's reason.
fn closed<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, addr: SocketAddr, reason: CloseReason) {
    let conn = conn_of(&shared.borrow(), addr);
    forget(vm, shared, addr);
    let Some(conn) = conn else {
        return;
    };
    match reason {
        CloseReason::Disconnected(text) => {
            pump::peer_disconnected(vm, shared, conn, text.as_bytes());
        }
        CloseReason::TimedOut => {
            let obj = shared.borrow().by_conn.get(&conn).copied();
            if let Some(obj) = obj {
                vm.print(format!("Client {obj} timed out.").as_bytes());
            }
            pump::drop_conn(vm, shared, conn, Some(b"TimedOut"), DeleteMode::Immediate);
        }
        CloseReason::Protocol(error) => {
            let why = shared.borrow().game.error_text(&error);
            pump::packet_error(vm, shared, conn, &why);
        }
        CloseReason::Rejected { .. } | CloseReason::Local => {
            pump::drop_conn(vm, shared, conn, None, DeleteMode::Immediate);
        }
    }
}

/// Takes `addr`'s session and what the host keeps beside it out, with nothing sent (a link that closed, a session that ended
/// or was evicted).
pub(super) fn forget<G: NetGame>(vm: &Vm, shared: &Shared<G>, addr: SocketAddr) {
    let conn = {
        let mut sh = shared.borrow_mut();
        let conn = conn_of(&sh, addr);
        if let Some(host) = sh.engine_host.as_mut() {
            host.drop_peer(canonical(addr));
        }
        conn
    };
    if let (Some(conn), Some(lanes)) = (conn, vm.extension::<RpcLanes>()) {
        lanes.close(Some(conn));
    }
}

/// Says goodbye to `addr`'s client with `reason` (when `send`) and takes its session out of the table: the drop of an engine
/// connection, in place of TGE's Disconnect datagram.
pub(super) fn goodbye<G: NetGame>(shared: &Shared<G>, addr: SocketAddr, reason: &[u8], send: bool) {
    let transport = Rc::clone(&shared.borrow().transport);
    let now = shared.borrow().now();
    let mut sh = shared.borrow_mut();
    let Some(host) = sh.engine_host.as_mut() else {
        return;
    };
    let key = canonical(addr);
    let peer = host.table.remove(key);
    host.drop_peer(key);
    let Some(mut peer) = peer.filter(|_| send) else {
        return;
    };
    peer.session.disconnect(&String::from_utf8_lossy(reason));
    let mut link = HostLink {
        transport: &transport,
        to: addr,
        kind: peer.kind,
        room: usize::MAX,
    };
    peer.session.poll(&mut link, &mut NoStreams, now);
}

/// Refuses `addr`'s connect with `code` and `text`: the reject goes out at the session's next poll, which closes it.
fn reject<G: NetGame>(shared: &Shared<G>, addr: SocketAddr, code: RejectCode, text: &[u8]) {
    let mut sh = shared.borrow_mut();
    sh.counters.rejected += 1;
    if let Some(peer) = sh.engine_host.as_mut().and_then(|h| h.table.peer_mut(addr)) {
        peer.session.reject(code, &String::from_utf8_lossy(text));
    }
}

/// What the checks before the scripts made of a connect.
enum Judged {
    /// Dropped without an answer (connections are off, the source's rate is spent, too many wait for a challenge).
    Silent,
    Refused(RejectCode, Vec<u8>),
    /// Challenged: the decision waits for the ticket ([`answered`]).
    Challenged,
    Request(Box<PendingRequest>),
}

/// What the server's config says of a connect's checks, read once.
struct Checks {
    profile: HandshakeProfile,
    password: Vec<u8>,
    per_source: u32,
    gate: Option<Arc<dyn ConnectGate>>,
    /// The address clients reach the server by: what a ticket is bound to.
    server: SocketAddr,
    full: bool,
    allow: bool,
}

impl Checks {
    fn of<G: NetGame>(sh: &NetShared<G>) -> Checks {
        let config = sh.hs.config();
        Checks {
            profile: config.profile.clone(),
            password: config.password.clone(),
            per_source: config.connect_requests_per_src_per_s,
            gate: config.gate.clone(),
            server: config
                .address
                .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0))),
            full: sh.hs.len() >= config.max_connections(),
            allow: config.allow_connections,
        }
    }

    /// The ticket context of the session challenged with `challenge`. The server's identity is none until a link verifies
    /// one.
    // TODO: bind to the server identity key's hash once the QUIC link verifies it (the client's link reports it in
    // `LinkFacts::server_identity`).
    fn context(&self, challenge: u64) -> AuthContext {
        ticket_context(self.server, challenge, None)
    }

    /// The connect gate's judgment of `fields` (with the ticket, if any) for the session challenged with `challenge`.
    fn gated(&self, addr: SocketAddr, ip: IpAddr, fields: ConnectFields, challenge: u64) -> Judged {
        let identity = match self
            .gate
            .as_ref()
            .map(|g| g.check(addr, &self.context(challenge), &fields))
        {
            None => None,
            Some(Ok(identity)) => identity,
            Some(Err(reason)) => return Judged::Refused(RejectCode::Manifest, reason),
        };
        Judged::Request(Box::new(PendingRequest {
            addr,
            ip,
            seq: 0,
            fields,
            identity,
            protocol_version: self.profile.protocol_current,
            replaced: None,
            extension: true,
        }))
    }
}

/// The checks a connect passes before the scripts see it, in TGE mode's order after the game and the version (which the
/// session checked): the allow flag, the source's rate, the password, the player count, then the challenge or the gate.
fn judge<G: NetGame>(
    vm: &Vm,
    sh: &mut NetShared<G>,
    addr: SocketAddr,
    connect: &Connect,
    now: u32,
) -> Judged {
    let checks = Checks::of(sh);
    let Some(host) = sh.engine_host.as_mut() else {
        return Judged::Silent;
    };
    host.byte_rate = byte_rate_pref(vm);

    let ip = host.table.peer(addr).map_or(addr.ip(), |p| p.ip);
    let limits = verified_limits(checks.per_source);
    if !checks.allow || host.requests.allow_within(limits, ip, now).is_err() {
        return Judged::Silent;
    }
    let mut fields = match fields_of(connect, &checks.profile) {
        Ok(fields) => fields,
        Err(text) => return Judged::Refused(RejectCode::Manifest, text.into_bytes()),
    };
    if !checks.password.is_empty() && fields.password != checks.password {
        return Judged::Refused(
            RejectCode::Password,
            checks.profile.rejects.password.clone(),
        );
    }
    if checks.full {
        return Judged::Refused(RejectCode::Full, checks.profile.rejects.server_full.clone());
    }
    // A ticket in the connect was made before the server chose anything: it counts for nothing.
    if let Some(block) = fields.block.as_mut() {
        block.ticket = None;
    }
    match host.auth {
        Auth::Challenge => challenge(host, addr, fields),
        Auth::None => checks.gated(addr, ip, fields, 0),
    }
}

/// Challenges `addr`'s connect and keeps its fields until the answer, within the table's pending cap.
fn challenge(host: &mut EngineHost, addr: SocketAddr, fields: ConnectFields) -> Judged {
    if host.stashed.len() >= host.table.limits().pending {
        return Judged::Silent;
    }
    let challenge = host.table.session_id();
    let Some(peer) = host.table.peer_mut(addr) else {
        return Judged::Silent;
    };
    if peer.session.challenge(challenge).is_err() {
        return Judged::Silent;
    }
    host.stashed.insert(
        canonical(addr),
        Stash {
            fields: Box::new(fields),
            challenge,
        },
    );
    Judged::Challenged
}

/// The client's answer to its challenge: a ticket accepted within the window (or one the full cache cannot remember) is
/// refused, then the gate checks the ticket against the challenge, and an accepted ticket is remembered.
fn judge_ticket<G: NetGame>(
    vm: &mut Vm,
    sh: &mut NetShared<G>,
    addr: SocketAddr,
    ticket: &[u8],
    now: u32,
) -> Judged {
    let checks = Checks::of(sh);
    let Some(host) = sh.engine_host.as_mut() else {
        return Judged::Silent;
    };
    let Some(Stash {
        mut fields,
        challenge,
    }) = host.stashed.remove(&canonical(addr))
    else {
        return Judged::Silent;
    };
    let token = checks
        .gate
        .as_ref()
        .map_or_else(|| ticket.to_vec(), |g| g.ticket_id(ticket));
    let ip = host.table.peer(addr).map_or(addr.ip(), |p| p.ip);
    if !ticket.is_empty() {
        if let Some(refused) = refusal(host.tickets.admit(&token, ip, now)) {
            return refused;
        }
        if !host.table.peer(addr).is_some_and(|p| p.kind.facts.secure) {
            host.say_once(vm, Once::UnencryptedTickets, || {
                "engine protocol: auth tickets are checked over an unencrypted link (private addresses and tests only)"
                    .to_string()
            });
        }
    }
    if let Some(block) = fields.block.as_mut() {
        block.ticket = (!ticket.is_empty()).then(|| ticket.to_vec());
    }
    let judged = checks.gated(addr, ip, *fields, challenge);
    host.checked = (matches!(judged, Judged::Request(_)) && !ticket.is_empty())
        .then_some(Checked { token, ip });
    judged
}

/// The refusal of a ticket the cache does not admit.
fn refusal(admit: Admit) -> Option<Judged> {
    match admit {
        Admit::Fresh => None,
        Admit::Replayed => Some(Judged::Refused(RejectCode::Ticket, TICKET_USED.to_vec())),
        Admit::Full => Some(Judged::Refused(RejectCode::Ticket, TICKETS_FULL.to_vec())),
    }
}

/// What [`judge`] or [`judge_ticket`] decided, acted on: a refusal is sent, a request goes to the scripts. A session the
/// scripts neither accepted nor refused (they dropped the request at a limit) is let go without a word.
fn decide<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, addr: SocketAddr, judged: Judged, now: u32) {
    match judged {
        Judged::Silent => forget(vm, shared, addr),
        Judged::Refused(code, text) => reject(shared, addr, code, &text),
        Judged::Challenged => {}
        Judged::Request(request) => {
            // What `finish` records is this request's ticket only.
            let checked = shared
                .borrow_mut()
                .engine_host
                .as_mut()
                .and_then(|h| h.checked.take());
            if let Some(host) = shared.borrow_mut().engine_host.as_mut() {
                host.verdict = Verdict::None;
                host.checked = checked;
            }
            pump::handle_request(vm, shared, *request, now);
            if let Some(host) = shared.borrow_mut().engine_host.as_mut() {
                host.checked = None;
            }
            let verdict = shared
                .borrow()
                .engine_host
                .as_ref()
                .map_or(Verdict::None, |h| h.verdict);
            if verdict == Verdict::None {
                forget(vm, shared, addr);
            }
        }
    }
}

/// The answer to `addr`'s challenge.
fn answered<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    addr: SocketAddr,
    ticket: &[u8],
    now: u32,
) {
    let judged = judge_ticket(vm, &mut shared.borrow_mut(), addr, ticket, now);
    decide(vm, shared, addr, judged, now);
}

/// A session's connect: judged, then handed to the scripts as in TGE mode.
fn request<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    addr: SocketAddr,
    connect: &Connect,
    now: u32,
) {
    let judged = judge(vm, &mut shared.borrow_mut(), addr, connect, now);
    decide(vm, shared, addr, judged, now);
}

/// The host's decision on an engine request ([`pump::handle_request`] calls it in place of the handshake's
/// `finish_request`): an accept enters the connection and queues the session's accept; a reject queues the reject. The
/// replies are the session's, so the returned ones are empty.
pub(super) fn finish<G: NetGame>(
    sh: &mut NetShared<G>,
    request: &PendingRequest,
    decision: Decision,
    now: u32,
) -> Finished {
    let addr = request.addr;
    if let Some(host) = sh.engine_host.as_mut() {
        host.verdict = Verdict::Given;
    }
    let refuse = |sh: &mut NetShared<G>, code, text: &[u8]| {
        if let Some(peer) = sh.engine_host.as_mut().and_then(|h| h.table.peer_mut(addr)) {
            peer.session.reject(code, &String::from_utf8_lossy(text));
        }
        Finished::Rejected { reply: Vec::new() }
    };
    if let Decision::Reject(reason) = decision {
        return refuse(sh, RejectCode::Script, &reason);
    }
    if sh.hs.len() >= sh.hs.config().max_connections() {
        let full = sh.hs.config().profile.rejects.server_full.clone();
        return refuse(sh, RejectCode::Full, &full);
    }
    let ghost_bits = sh.hs.network_ghost_bits(true);
    let mods = sh
        .hs
        .config()
        .gate
        .as_ref()
        .filter(|_| sh.hs.config().channels)
        .and_then(|g| g.mods_support(&request.fields));
    let widths = Widths {
        ghost_id: ghost_bits,
        ghost_class: sh.codecs.ghost_class_bits(),
        datablock_class: sh.codecs.datablock_id_bits(),
        event_class: 0,
    };
    let Some(host) = sh.engine_host.as_mut() else {
        return Finished::Refused;
    };
    // The ticket is spent now that the scripts accepted: past its identity's share or the server's for this second, the
    // connection is refused instead.
    if let Some(checked) = host.checked.take() {
        let identity = request.identity.as_ref().map_or(&[][..], Identity::id);
        if let Some(Judged::Refused(code, text)) =
            refusal(
                host.tickets
                    .record(&checked.token, checked.ip, identity, now),
            )
        {
            return refuse(sh, code, &text);
        }
    }
    let Some(host) = sh.engine_host.as_mut() else {
        return Finished::Refused;
    };
    let session_id = host.table.session_id();
    let (byte_rate, digests) = (host.byte_rate, host.digests);
    let Some(peer) = host.table.peer_mut(addr) else {
        return Finished::Refused;
    };
    let accept = Accept {
        session: session_id,
        // The link's: the accept's limit is the server's own sends too, so it is not cut to the 1,500 bytes this host reads. A client
        // of this engine keeps its sends to NET_MAX_DATAGRAM (`WireLink`), and a datagram above it is dropped as it is read.
        max_packet: u16::try_from(peer.kind.max_datagram).unwrap_or(u16::MAX),
        packet_interval_ms: PACKET_INTERVAL_MS,
        byte_rate,
        widths,
        digests,
    };
    if peer.session.accept(accept).is_err() {
        return Finished::Refused;
    }
    let accepted = EngineAccepted {
        addr,
        ip: peer.ip,
        ghost_bits,
        identity: request.identity.clone(),
        mods,
    };
    let conn = sh.hs.insert_engine(accepted, now);
    Finished::Accepted {
        conn,
        reply: Vec::new(),
        replaced: None,
        evicted: None,
    }
}

/// What one poll of a connection's session came to.
struct Sent {
    addr: SocketAddr,
    work: u32,
}

/// The send pass for engine connection `id` (`cap`: the byte rate's ceiling, [`byte_rate_pref`], read once a pass): one poll of its session, whose ghost lane the payload layer writes, then its
/// events. Returns the work of the bytes sent; a payload error is pushed to `failed`.
pub(super) fn send_conn<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    id: ConnId,
    now: u32,
    cap: u32,
    failed: &mut Vec<(ConnId, NetError)>,
) -> u32 {
    let Some(sent) = poll_conn(vm, shared, id, now, cap, failed) else {
        return 0;
    };
    events(vm, shared, sent.addr, now);
    sent.work
}

/// Polls `id`'s session once: its RPC batches handed over, its pace kept, the payload layer as its ghost lane's writer.
fn poll_conn<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    id: ConnId,
    now: u32,
    cap: u32,
    failed: &mut Vec<(ConnId, NetError)>,
) -> Option<Sent> {
    let transport = Rc::clone(&shared.borrow().transport);
    let lanes = vm.extension::<RpcLanes>();
    let mut sh = shared.borrow_mut();
    let sh = &mut *sh;
    let table = sh.game.string_table();
    let obj = sh.by_conn.get(&id).copied();
    let NetShared {
        hs,
        engine_host,
        payload,
        counters,
        ..
    } = sh;
    let (host, c) = (engine_host.as_mut()?, hs.connection_mut(id)?);
    let addr = c.addr;
    let key = canonical(addr);
    let peer = host.table.peer_mut(key)?;
    if let Some(lanes) = &lanes {
        lanes.feed(Some(id), &mut peer.session);
    }
    c.protocol
        .set_round_trip(peer.session.round_trip_ms() as f32);
    let pace = host.pace.entry(key).or_insert_with(|| Pace::new(cap));
    pace.set_cap(cap);
    pace.observe(peer.session.fates(), now, !peer.kind.facts.validated);
    let mut link = HostLink {
        transport: &transport,
        to: addr,
        kind: peer.kind,
        room: pace.room(now, peer.kind),
    };
    let mut writer = Writer {
        payload: &mut **payload,
        protocol: &mut c.protocol,
        order: &mut peer.order,
        vm,
        conn: id,
        obj,
        ghost_bits: c.ghost_bits,
        table,
        now,
        failed: None,
        withheld: 0,
    };
    let polled = peer.session.poll(&mut link, &mut writer, now);
    counters.packets_withheld += writer.withheld;
    if let Some(e) = writer.failed {
        failed.push((id, e));
    }
    let work = match polled {
        Polled::Sent(n) => {
            pace.spend(n);
            counters.packets_sent += 1;
            cost(n)
        }
        Polled::Idle | Polled::NoRoom | Polled::Closed => 0,
    };
    Some(Sent { addr, work })
}

/// The send pass for the sessions the host has not accepted: their cookies, accepts not yet acknowledged, challenges,
/// rejects and goodbyes go out, and the ones that closed leave the table.
pub(super) fn poll_pending<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, now: u32) {
    let transport = Rc::clone(&shared.borrow().transport);
    let addrs: Vec<SocketAddr> = {
        let sh = shared.borrow();
        let Some(host) = sh.engine_host.as_ref() else {
            return;
        };
        host.table
            .addrs()
            .into_iter()
            .filter(|a| conn_of(&sh, *a).is_none())
            .collect()
    };
    for addr in addrs {
        {
            let mut sh = shared.borrow_mut();
            let Some(peer) = sh.engine_host.as_mut().and_then(|h| h.table.peer_mut(addr)) else {
                continue;
            };
            let mut link = HostLink {
                transport: &transport,
                to: addr,
                kind: peer.kind,
                room: usize::MAX,
            };
            if let Polled::Sent(_) = peer.session.poll(&mut link, &mut NoStreams, now) {
                sh.counters.packets_sent += 1;
            }
        }
        events(vm, shared, addr, now);
        let closed = shared
            .borrow()
            .engine_host
            .as_ref()
            .and_then(|h| h.table.peer(addr))
            .is_some_and(|p| p.session.state() == State::Closed);
        if closed {
            forget(vm, shared, addr);
        }
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
