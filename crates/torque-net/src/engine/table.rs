//! A server's engine-mode sessions: which datagram goes to which session, which source may open one, and how many each source
//! holds. Sans-IO, like the sessions: the host hands it each datagram with the source's [`LinkKind`] and polls the sessions.
//!
//! **Routing.** A datagram from a source that has a session goes to that session. One from a source without goes, on a plain
//! link, to the [`Gate`] (a cookie round trip first); on a validated link only an `Opening` without a cookie opens a session,
//! and everything else is dropped unanswered.
//!
//! **Caps.** The table holds at most [`TableLimits::sessions`] sessions, [`TableLimits::per_ip`] from one IP address and
//! [`TableLimits::per_network`] from one network (an IPv4 /24, an IPv6 /64), established or not, counted by the peer's own
//! address (the host passes the real one a transport knows behind a stand-in address): a datagram that would
//! open one past a cap is dropped. Of those not yet established, a plain link's are bounded by the gate's caps, a validated
//! link's by [`TableLimits::pending_per_ip`], [`TableLimits::pending_per_network`] (refused past them) and
//! [`TableLimits::pending`] (the oldest is evicted). An evicted session is gone from the table; the host drops what it made
//! for it ([`SessionTable::take_evicted`]). Addresses are taken in their canonical form (an IPv4-mapped IPv6 address is the
//! IPv4 address).
//!
//! **Order.** The sessions are kept by address in a `BTreeMap`, so every walk the host makes over them is in the same order on
//! every run and target.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use super::framing::{self, Datagram};
pub use super::gate::canonical;
use super::gate::{Gate, GateConfig, GateOutcome, Network};
use super::link::LinkKind;
use super::order::PacketOrder;
use super::session::{Session, SessionConfig, State};
use crate::handshake::siphash24;

/// How many sessions a [`SessionTable`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableLimits {
    /// Sessions in all.
    pub sessions: usize,
    /// Sessions from one source IP address.
    pub per_ip: usize,
    /// Sessions from one source network.
    pub per_network: usize,
    /// Sessions on validated links not yet established, in all; past it the oldest is evicted.
    pub pending: usize,
    /// Of those, from one source IP address; past it a new one is refused.
    pub pending_per_ip: usize,
    /// Of those, from one source network; past it a new one is refused.
    pub pending_per_network: usize,
}

impl Default for TableLimits {
    fn default() -> TableLimits {
        TableLimits {
            sessions: 512,
            per_ip: 16,
            per_network: 64,
            pending: 64,
            pending_per_ip: 4,
            pending_per_network: 16,
        }
    }
}

/// Where a session is, as the caps count it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Opened, not yet established: counted against the pending caps.
    Pending,
    Established,
}

/// One source's session and what the host keeps beside it.
pub struct Peer {
    pub session: Session,
    pub kind: LinkKind,
    /// The packet numbers of what the host's stream lanes wrote ([`PacketOrder`]).
    pub order: PacketOrder,
    pub stage: Stage,
    /// The peer's own IP address, which the caps count by: the source's, or the one behind a transport's stand-in address.
    pub ip: IpAddr,
    /// When the session was opened, for the pending eviction order.
    opened: u64,
}

impl Peer {
    fn new(session: Session, kind: LinkKind, ip: IpAddr, opened: u64) -> Peer {
        Peer {
            session,
            kind,
            order: PacketOrder::default(),
            stage: Stage::Pending,
            ip,
            opened,
        }
    }
}

/// What the table did with one datagram.
#[derive(Debug, PartialEq, Eq)]
pub enum Arrival {
    /// Nothing: no session took it, or a cap refused one.
    Dropped,
    /// Send this back to the source (the gate's cookie or refusal).
    Reply(Vec<u8>),
    /// The source's session read it; `opened` when it opened just now.
    Session { opened: bool },
}

/// What [`SessionTable::count`] found for one source.
#[derive(Debug, Default)]
struct Count {
    all: usize,
    ip: usize,
    network: usize,
    pending: usize,
    pending_ip: usize,
    pending_network: usize,
    oldest_pending: Option<SocketAddr>,
}

/// A server's engine-mode sessions (module docs).
pub struct SessionTable {
    limits: TableLimits,
    config: SessionConfig,
    gate: Gate,
    peers: BTreeMap<SocketAddr, Peer>,
    evicted: Vec<SocketAddr>,
    key: [u8; 16],
    drawn: u64,
    opened: u64,
}

impl SessionTable {
    /// A table whose sessions are set up with `config` and whose cookies and session ids come from `key` (the host's random
    /// source). The gate's caps are [`GateConfig::new`]'s.
    #[must_use]
    pub fn new(config: SessionConfig, limits: TableLimits, key: [u8; 16]) -> SessionTable {
        SessionTable {
            gate: Gate::new(GateConfig::new(config.clone()), key),
            limits,
            config,
            peers: BTreeMap::new(),
            evicted: Vec::new(),
            key,
            drawn: 0,
            opened: 0,
        }
    }

    /// Replaces the key: cookies handed out before no longer verify, and session ids follow the new key.
    pub fn rekey(&mut self, key: [u8; 16]) {
        self.key = key;
        self.gate.rekey(key);
    }

    #[must_use]
    pub fn limits(&self) -> TableLimits {
        self.limits
    }

    pub fn set_limits(&mut self, limits: TableLimits) {
        self.limits = limits;
    }

    /// A fresh session id: a keyed digest of a counter, so ids never repeat within a key and cannot be guessed without it.
    pub fn session_id(&mut self) -> u64 {
        self.drawn += 1;
        let mut data = *b"engine-session\0\0\0\0\0\0\0\0";
        data[14..].copy_from_slice(&self.drawn.to_le_bytes());
        siphash24(&self.key, &data)
    }

    /// Takes one datagram from `from`, whose link is `kind`; `ip` is the peer's own address the caps count by (the source's
    /// IP, or the real one a transport knows behind a stand-in address).
    pub fn receive(
        &mut self,
        from: SocketAddr,
        ip: IpAddr,
        kind: LinkKind,
        data: &[u8],
        now: u32,
    ) -> Arrival {
        let key = canonical(from);
        let ip = ip.to_canonical();
        if let Some(peer) = self.peers.get_mut(&key) {
            peer.session.receive(data, now);
            self.note_state(key);
            return Arrival::Session { opened: false };
        }
        if kind.facts.validated {
            self.open_validated(key, ip, kind, data, now)
        } else {
            self.open_plain(from, key, ip, kind, data, now)
        }
    }

    fn open_validated(
        &mut self,
        key: SocketAddr,
        ip: IpAddr,
        kind: LinkKind,
        data: &[u8],
        now: u32,
    ) -> Arrival {
        if !matches!(
            framing::parse(data),
            Ok(Datagram::Opening { cookie: None, .. })
        ) {
            return Arrival::Dropped;
        }
        let count = self.count(ip);
        if count.pending_ip >= self.limits.pending_per_ip
            || count.pending_network >= self.limits.pending_per_network
            || !self.has_room(&count)
        {
            return Arrival::Dropped;
        }
        if count.pending >= self.limits.pending.max(1)
            && let Some(oldest) = count.oldest_pending
        {
            self.peers.remove(&oldest);
            // A plain link's pending session counts at the gate too.
            self.gate.settle(oldest);
            self.evicted.push(oldest);
        }
        let Ok(mut session) = Session::server(self.config.clone(), kind.facts, now) else {
            return Arrival::Dropped;
        };
        session.receive(data, now);
        self.insert(key, ip, session, kind);
        Arrival::Session { opened: true }
    }

    fn open_plain(
        &mut self,
        from: SocketAddr,
        key: SocketAddr,
        ip: IpAddr,
        kind: LinkKind,
        data: &[u8],
        now: u32,
    ) -> Arrival {
        match self.gate.receive(from, data, now) {
            GateOutcome::Drop => Arrival::Dropped,
            GateOutcome::Reply(reply) => Arrival::Reply(reply),
            GateOutcome::Open { session, evicted } => {
                if let Some(old) = evicted.map(canonical)
                    && self.peers.remove(&old).is_some()
                {
                    self.evicted.push(old);
                }
                if !self.has_room(&self.count(ip)) {
                    self.gate.settle(key);
                    return Arrival::Dropped;
                }
                self.insert(key, ip, *session, kind);
                Arrival::Session { opened: true }
            }
        }
    }

    /// What the table holds that the caps count for a new session from `ip`, in one walk.
    fn count(&self, ip: IpAddr) -> Count {
        let network = Network::of_ip(ip);
        let mut count = Count::default();
        let mut oldest: Option<(u64, SocketAddr)> = None;
        for (addr, peer) in &self.peers {
            let same_ip = peer.ip == ip;
            let same_network = Network::of_ip(peer.ip) == network;
            count.ip += usize::from(same_ip);
            count.network += usize::from(same_network);
            if peer.stage == Stage::Pending {
                count.pending += 1;
                count.pending_ip += usize::from(same_ip);
                count.pending_network += usize::from(same_network);
                if oldest.is_none_or(|(at, _)| peer.opened < at) {
                    oldest = Some((peer.opened, *addr));
                }
            }
        }
        count.all = self.peers.len();
        count.oldest_pending = oldest.map(|(_, addr)| addr);
        count
    }

    /// Whether one more session stays within the totals `count` found.
    fn has_room(&self, count: &Count) -> bool {
        count.all < self.limits.sessions
            && count.ip < self.limits.per_ip
            && count.network < self.limits.per_network
    }

    fn insert(&mut self, key: SocketAddr, ip: IpAddr, session: Session, kind: LinkKind) {
        self.opened += 1;
        self.peers
            .insert(key, Peer::new(session, kind, ip, self.opened));
        self.note_state(key);
    }

    /// Marks `key`'s session established once it is (it no longer counts as pending, here or at the gate).
    fn note_state(&mut self, key: SocketAddr) {
        let Some(peer) = self.peers.get_mut(&key) else {
            return;
        };
        if peer.stage == Stage::Pending && peer.session.state() == State::Established {
            peer.stage = Stage::Established;
            self.gate.settle(key);
        }
    }

    /// The plain links' sessions the gate counts as pending.
    #[must_use]
    pub fn gate_pending(&self) -> usize {
        self.gate.pending()
    }

    /// The sessions not yet established.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.peers
            .values()
            .filter(|p| p.stage == Stage::Pending)
            .count()
    }

    /// The sessions evicted since the last call: they are gone from the table, and the host drops what it made for them.
    pub fn take_evicted(&mut self) -> Vec<SocketAddr> {
        std::mem::take(&mut self.evicted)
    }

    pub fn peer(&self, addr: SocketAddr) -> Option<&Peer> {
        self.peers.get(&canonical(addr))
    }

    pub fn peer_mut(&mut self, addr: SocketAddr) -> Option<&mut Peer> {
        self.peers.get_mut(&canonical(addr))
    }

    /// Every session's address, in address order.
    #[must_use]
    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.peers.keys().copied().collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Takes `addr`'s session out of the table.
    pub fn remove(&mut self, addr: SocketAddr) -> Option<Peer> {
        let key = canonical(addr);
        self.gate.settle(key);
        self.peers.remove(&key)
    }
}

#[cfg(test)]
mod tests;
