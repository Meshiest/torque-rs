//! The addresses connections go by toward the host: one stand-in in `248.0.0.0/5` per connection (the upper half of the
//! reserved `240.0.0.0/4`; the lower half is the web transport's game sockets', so a server on both tells them apart),
//! never a peer's own address (a QUIC connection is its connection ids, and a peer's address may move). The stand-in IP is a keyed hash of the
//! peer's IP, fixed for the endpoint's life, so the host's per-address limits and the scripts' bans keyed by address hold
//! for a peer's connections as for a UDP client; each connection has its own port.

use std::collections::HashSet;
use std::hash::{BuildHasher, RandomState};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// The first address of the stand-in range (`248.0.0.0/5`).
pub const STAND_IN_NET: Ipv4Addr = Ipv4Addr::new(248, 0, 0, 0);
const STAND_IN_MASK: u32 = (1 << 27) - 1;

/// Whether `addr` is a connection's stand-in (`248.0.0.0/5`).
#[must_use]
pub fn is_stand_in(addr: SocketAddr) -> bool {
    matches!(addr.ip(), IpAddr::V4(v4) if v4.octets()[0] & 0xf8 == 248)
}

/// Whether `addr` is in the reserved range no peer sends from (`240.0.0.0/4`: this crate's stand-ins and the web
/// transport's).
#[must_use]
pub fn is_reserved(addr: SocketAddr) -> bool {
    matches!(addr.ip(), IpAddr::V4(v4) if v4.octets()[0] & 0xf0 == 240)
}

#[derive(Debug, Default)]
pub(crate) struct StandIns {
    key: RandomState,
    taken: HashSet<SocketAddr>,
    next_port: u16,
}

impl StandIns {
    /// The stand-in IP of `peer`: the same for the endpoint's life.
    pub(crate) fn ip_of(&self, peer: IpAddr) -> Ipv4Addr {
        let hash = self.key.hash_one(peer) as u32 & STAND_IN_MASK;
        Ipv4Addr::from(u32::from(STAND_IN_NET) | hash)
    }

    /// A stand-in for a new connection from `peer`.
    pub(crate) fn take(&mut self, peer: IpAddr) -> SocketAddr {
        let ip = IpAddr::V4(self.ip_of(peer));
        loop {
            self.next_port = self.next_port.checked_add(1).unwrap_or(1);
            let addr = SocketAddr::new(ip, self.next_port);
            if self.taken.insert(addr) {
                return addr;
            }
        }
    }

    pub(crate) fn give_back(&mut self, addr: SocketAddr) {
        self.taken.remove(&addr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_stand_in_ip_per_peer_and_a_port_per_connection() {
        let a: IpAddr = Ipv4Addr::new(127, 0, 0, 1).into();
        let b: IpAddr = Ipv4Addr::new(192, 168, 1, 9).into();
        let mut s = StandIns::default();
        let (a1, a2, b1) = (s.take(a), s.take(a), s.take(b));
        assert!([a1, a2, b1].iter().all(|&x| is_stand_in(x)));
        assert_eq!(a1.ip(), a2.ip());
        assert_ne!(a1.port(), a2.port());
        assert_ne!(a1.ip(), b1.ip());
        s.give_back(a1);
        assert_eq!(s.take(a).ip(), a1.ip());
        assert!(!is_stand_in("127.0.0.1:5".parse().expect("an address")));
        assert!(
            !is_stand_in("240.0.0.1:5".parse().expect("an address")),
            "the web's half"
        );
        assert!(is_reserved("240.0.0.1:5".parse().expect("an address")));
        assert!(is_reserved(a1));
    }
}
