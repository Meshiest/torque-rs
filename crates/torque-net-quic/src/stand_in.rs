//! The addresses connections go by toward the host: one stand-in in `248.0.0.0/5` per connection (the upper half of the
//! reserved `240.0.0.0/4`; the lower half is the web transport's game sockets', so a server on both tells them apart),
//! never a peer's own address (a QUIC connection is its connection ids, and a peer's address may move). The stand-in IP is a
//! keyed hash of the peer's IP, fixed for the endpoint's life, so the host's per-address limits and the scripts' bans keyed
//! by address hold for a peer's connections as for a UDP client; each connection has its own port. The pool is
//! [`torque_net::stand_in::StandIns`].

use std::net::{Ipv4Addr, SocketAddr};

pub use torque_net::stand_in::{StandIns, is_reserved};

/// The first address of the stand-in range (`248.0.0.0/5`).
pub const STAND_IN_NET: Ipv4Addr = torque_net::stand_in::QUIC_NET;

/// Whether `addr` is a connection's stand-in (`248.0.0.0/5`).
#[must_use]
pub fn is_stand_in(addr: SocketAddr) -> bool {
    torque_net::stand_in::is_stand_in(addr, STAND_IN_NET)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn one_stand_in_ip_per_peer_and_a_port_per_connection() {
        let a: IpAddr = Ipv4Addr::new(127, 0, 0, 1).into();
        let b: IpAddr = Ipv4Addr::new(192, 168, 1, 9).into();
        let mut s = StandIns::new(STAND_IN_NET);
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
