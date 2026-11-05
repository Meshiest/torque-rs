//! The addresses game sockets go by: never a UDP peer's.
//!
//! A game socket is named to the host by a stand-in address in `240.0.0.0/4` (reserved: no datagram comes from it, and a
//! UDP datagram that claims it is dropped), so a socket and a UDP peer can never share an address. Every socket from one
//! peer address shares one stand-in IP (the host's per-address limits then hold for that peer as for a UDP one), and each
//! socket has its own port. The stand-in is taken when the upgrade's answer is queued, which also reserves the socket's
//! place under the per-address cap, and given back when the connection ends, whichever way.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// The first address of the stand-in range (`240.0.0.0/4`).
pub const STAND_IN_NET: Ipv4Addr = Ipv4Addr::new(240, 0, 0, 0);
/// The addresses in the stand-in range.
const STAND_IN_IPS: u32 = 1 << 28;

/// Whether `addr` is a game socket's stand-in (`240.0.0.0/4`).
pub fn is_stand_in(addr: SocketAddr) -> bool {
    matches!(addr.ip(), IpAddr::V4(v4) if v4.octets()[0] & 0xf0 == 240)
}

/// The stand-ins in use.
#[derive(Debug, Default)]
pub(crate) struct StandIns {
    /// Each peer address's stand-in IP and how many sockets hold it.
    by_peer: HashMap<IpAddr, (Ipv4Addr, usize)>,
    ips: HashSet<Ipv4Addr>,
    taken: HashSet<SocketAddr>,
    next_ip: u32,
    next_port: u16,
}

impl StandIns {
    /// The sockets open (or reserved) from `peer`.
    pub(crate) fn count(&self, peer: IpAddr) -> usize {
        self.by_peer.get(&peer).map_or(0, |(_, n)| *n)
    }

    /// A stand-in for a new socket from `peer`.
    pub(crate) fn take(&mut self, peer: IpAddr) -> SocketAddr {
        let ip = match self.by_peer.get_mut(&peer) {
            Some((ip, n)) => {
                *n += 1;
                *ip
            }
            None => {
                let ip = loop {
                    let ip = Ipv4Addr::from(u32::from(STAND_IN_NET) + self.next_ip % STAND_IN_IPS);
                    self.next_ip = self.next_ip.wrapping_add(1);
                    if self.ips.insert(ip) {
                        break ip;
                    }
                };
                self.by_peer.insert(peer, (ip, 1));
                ip
            }
        };
        loop {
            self.next_port = self.next_port.checked_add(1).unwrap_or(1);
            let addr = SocketAddr::new(IpAddr::V4(ip), self.next_port);
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
        if let Some((ip, n)) = self.by_peer.get_mut(&peer) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                let ip = *ip;
                self.ips.remove(&ip);
                self.by_peer.remove(&peer);
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.by_peer.clear();
        self.ips.clear();
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
    fn stand_ins_are_reserved_shared_per_peer_and_given_back() {
        let a: IpAddr = Ipv4Addr::new(127, 0, 0, 1).into();
        let b: IpAddr = Ipv4Addr::new(192, 168, 1, 9).into();
        let mut s = StandIns::default();
        let a1 = s.take(a);
        let a2 = s.take(a);
        let b1 = s.take(b);
        assert!([a1, a2, b1].iter().all(|&x| is_stand_in(x)));
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
        assert_eq!((s.count(a), s.len(), s.ips.len()), (0, 0, 0));
        assert!(!is_stand_in("127.0.0.1:5".parse().unwrap()));
        assert!(is_stand_in("255.255.255.255:5".parse().unwrap()));
    }
}
