//! The addresses game sockets go by: never a UDP peer's, and always the same IP for one peer.
//!
//! A game socket is named to the host by a stand-in address in `240.0.0.0/5` (the lower half of the reserved
//! `240.0.0.0/4`: no datagram comes from it, and a UDP datagram that claims any of the `/4` is dropped; the upper half,
//! `248.0.0.0/5`, is the QUIC link's connections', so a server on both tells them apart), so a socket and a UDP peer can
//! never share an address. A peer address's
//! stand-in IP is fixed for the transport's life: a keyed hash of the peer's IP (the key is drawn when the transport is
//! made), so every socket of one peer, now or after it reconnects, has the same stand-in IP, and the host's limits and
//! the scripts' bans keyed by address hold for it as for a UDP peer. Two peers whose IPs hash alike share one stand-in IP
//! (and so their limits): stricter, never looser, and with the key unknown no peer can aim at another's. Each socket has
//! its own port. The stand-in is taken when the upgrade's answer is queued, which also reserves the socket's place under
//! the per-address cap, and given back when the connection ends, whichever way.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// The first address of the stand-in range (`240.0.0.0/5`).
pub const STAND_IN_NET: Ipv4Addr = Ipv4Addr::new(240, 0, 0, 0);
/// The host bits of the stand-in range.
const STAND_IN_MASK: u32 = (1 << 27) - 1;

/// Whether `addr` is a game socket's stand-in (`240.0.0.0/5`).
pub fn is_stand_in(addr: SocketAddr) -> bool {
    matches!(addr.ip(), IpAddr::V4(v4) if v4.octets()[0] & 0xf8 == 240)
}

/// Whether `addr` is in the reserved range no peer sends from (`240.0.0.0/4`: these stand-ins and the QUIC link's).
pub fn is_reserved(addr: SocketAddr) -> bool {
    matches!(addr.ip(), IpAddr::V4(v4) if v4.octets()[0] & 0xf0 == 240)
}

/// The stand-ins in use.
#[derive(Debug, Default)]
pub(crate) struct StandIns {
    key: RandomState,
    /// The sockets each peer address holds (open, or reserved by a queued answer).
    by_peer: HashMap<IpAddr, usize>,
    taken: HashSet<SocketAddr>,
    next_port: u16,
}

impl StandIns {
    /// The sockets open (or reserved) from `peer`.
    pub(crate) fn count(&self, peer: IpAddr) -> usize {
        self.by_peer.get(&peer).copied().unwrap_or(0)
    }

    /// The stand-in IP of `peer`: the same for the transport's life.
    pub(crate) fn ip_of(&self, peer: IpAddr) -> Ipv4Addr {
        let hash = self.key.hash_one(peer) as u32 & STAND_IN_MASK;
        Ipv4Addr::from(u32::from(STAND_IN_NET) | hash)
    }

    /// A stand-in for a new socket from `peer`.
    pub(crate) fn take(&mut self, peer: IpAddr) -> SocketAddr {
        *self.by_peer.entry(peer).or_insert(0) += 1;
        let ip = IpAddr::V4(self.ip_of(peer));
        loop {
            self.next_port = self.next_port.checked_add(1).unwrap_or(1);
            let addr = SocketAddr::new(ip, self.next_port);
            if self.taken.insert(addr) {
                return addr;
            }
        }
    }

    /// Gives back the stand-in `addr` a socket from `peer` held.
    pub(crate) fn give_back(&mut self, peer: IpAddr, addr: SocketAddr) {
        if !self.taken.remove(&addr) {
            return;
        }
        if let Some(n) = self.by_peer.get_mut(&peer) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.by_peer.remove(&peer);
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.by_peer.clear();
        self.taken.clear();
    }

    /// The stand-ins in use.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.taken.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stand_ins_are_reserved_and_one_ip_per_peer_for_good() {
        let a: IpAddr = Ipv4Addr::new(127, 0, 0, 1).into();
        let b: IpAddr = Ipv4Addr::new(192, 168, 1, 9).into();
        let mut s = StandIns::default();
        let a1 = s.take(a);
        let a2 = s.take(a);
        let b1 = s.take(b);
        assert!(
            [a1, a2, b1]
                .iter()
                .all(|&x| is_stand_in(x) && is_reserved(x))
        );
        assert!(
            !is_stand_in("248.0.0.1:5".parse().expect("an address")),
            "the QUIC link's half"
        );
        assert!(is_reserved("248.0.0.1:5".parse().expect("an address")));
        assert_eq!(a1.ip(), a2.ip(), "one stand-in IP per peer");
        assert_ne!(a1.port(), a2.port());
        assert_ne!(a1.ip(), b1.ip());
        assert_eq!((s.count(a), s.count(b)), (2, 1));
        s.give_back(a, a1);
        s.give_back(a, a1);
        assert_eq!(
            s.count(a),
            1,
            "a second give-back of one stand-in counts once"
        );
        s.give_back(a, a2);
        s.give_back(b, b1);
        assert_eq!((s.count(a), s.len()), (0, 0));
        // A peer that comes back after all its sockets closed has the same stand-in IP.
        let again = s.take(a);
        assert_eq!(again.ip(), a1.ip());
        assert_eq!(IpAddr::V4(s.ip_of(b)), b1.ip());
        assert!(!is_stand_in("127.0.0.1:5".parse().unwrap()));
        assert!(is_stand_in("255.255.255.255:5".parse().unwrap()));
    }
}
