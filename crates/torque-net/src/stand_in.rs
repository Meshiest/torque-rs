//! The addresses a transport names its connections by toward the host: never a UDP peer's own.
//!
//! A stand-in lies in a `/5` of the reserved `240.0.0.0/4` ([`is_reserved`]: no datagram comes from it, and a UDP datagram
//! that claims any of the `/4` is dropped), so a connection and a UDP peer can never share an address. The web transport's
//! game sockets use the lower half ([`WEB_NET`], `240.0.0.0/5`) and the QUIC link's connections the upper half
//! ([`QUIC_NET`], `248.0.0.0/5`), so a server on both tells them apart.
//!
//! A peer's stand-in IP is a keyed hash of the peer's IP, fixed for the pool's life (the key is drawn when it is made), so
//! every connection of one peer, now or after it reconnects, has the same stand-in IP, and the host's per-address limits
//! and the scripts' bans keyed by address hold for it as for a UDP peer. Two peers whose IPs hash alike share one stand-in
//! IP (and so their limits): stricter, never looser, and with the key unknown no peer can aim at another's. Each connection
//! has its own port.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::{BuildHasher, RandomState};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// The first address of the web transport's stand-in range (`240.0.0.0/5`).
pub const WEB_NET: Ipv4Addr = Ipv4Addr::new(240, 0, 0, 0);
/// The first address of the QUIC link's stand-in range (`248.0.0.0/5`).
pub const QUIC_NET: Ipv4Addr = Ipv4Addr::new(248, 0, 0, 0);

/// The host bits of a `/5` range.
const HOST_MASK: u32 = (1 << 27) - 1;

/// Whether `addr` is in the `/5` range that starts at `net`.
#[must_use]
pub fn is_stand_in(addr: SocketAddr, net: Ipv4Addr) -> bool {
    matches!(addr.ip(), IpAddr::V4(v4) if u32::from(v4) >> 27 == u32::from(net) >> 27)
}

/// Whether `addr` is in the reserved range no peer sends from (`240.0.0.0/4`: both transports' stand-ins).
#[must_use]
pub fn is_reserved(addr: SocketAddr) -> bool {
    matches!(addr.ip(), IpAddr::V4(v4) if v4.octets()[0] & 0xf0 == 240)
}

/// The stand-ins in use in one `/5` range.
#[derive(Debug)]
pub struct StandIns {
    net: Ipv4Addr,
    key: RandomState,
    /// The peer each stand-in in use was taken for.
    taken: HashMap<SocketAddr, IpAddr>,
    /// The stand-ins each peer address holds (open, or reserved by a queued answer).
    by_peer: HashMap<IpAddr, usize>,
    next_port: u16,
}

impl StandIns {
    /// An empty pool in the `/5` range that starts at `net` ([`WEB_NET`] or [`QUIC_NET`]), with a fresh key.
    #[must_use]
    pub fn new(net: Ipv4Addr) -> Self {
        Self {
            net,
            key: RandomState::new(),
            taken: HashMap::new(),
            by_peer: HashMap::new(),
            next_port: 0,
        }
    }

    /// The stand-in IP of `peer`: the same for the pool's life.
    #[must_use]
    pub fn ip_of(&self, peer: IpAddr) -> Ipv4Addr {
        let hash = self.key.hash_one(peer) as u32 & HOST_MASK;
        Ipv4Addr::from(u32::from(self.net) | hash)
    }

    /// A stand-in for a new connection from `peer`.
    pub fn take(&mut self, peer: IpAddr) -> SocketAddr {
        *self.by_peer.entry(peer).or_insert(0) += 1;
        let ip = IpAddr::V4(self.ip_of(peer));
        loop {
            self.next_port = self.next_port.checked_add(1).unwrap_or(1);
            let addr = SocketAddr::new(ip, self.next_port);
            if let Entry::Vacant(slot) = self.taken.entry(addr) {
                slot.insert(peer);
                return addr;
            }
        }
    }

    /// Gives back the stand-in `addr`; one that is not in use is ignored.
    pub fn give_back(&mut self, addr: SocketAddr) {
        let Some(peer) = self.taken.remove(&addr) else {
            return;
        };
        if let Some(n) = self.by_peer.get_mut(&peer) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.by_peer.remove(&peer);
            }
        }
    }

    /// The stand-ins in use (open, or reserved by a queued answer) from `peer`.
    #[must_use]
    pub fn count(&self, peer: IpAddr) -> usize {
        self.by_peer.get(&peer).copied().unwrap_or(0)
    }

    /// The stand-ins in use.
    #[must_use]
    pub fn len(&self) -> usize {
        self.taken.len()
    }

    /// Whether no stand-in is in use.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.taken.is_empty()
    }

    /// Forgets every stand-in in use.
    pub fn clear(&mut self) {
        self.by_peer.clear();
        self.taken.clear();
    }
}

#[cfg(test)]
#[path = "stand_in_tests.rs"]
mod tests;
