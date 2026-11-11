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

use std::net::{Ipv4Addr, SocketAddr};

pub(crate) use torque_net::stand_in::StandIns;
pub use torque_net::stand_in::is_reserved;

/// The first address of the stand-in range (`240.0.0.0/5`).
pub const STAND_IN_NET: Ipv4Addr = torque_net::stand_in::WEB_NET;

/// Whether `addr` is a game socket's stand-in (`240.0.0.0/5`).
#[must_use]
pub fn is_stand_in(addr: SocketAddr) -> bool {
    torque_net::stand_in::is_stand_in(addr, STAND_IN_NET)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn stand_ins_are_reserved_and_one_ip_per_peer_for_good() {
        let a: IpAddr = Ipv4Addr::new(127, 0, 0, 1).into();
        let b: IpAddr = Ipv4Addr::new(192, 168, 1, 9).into();
        let mut s = StandIns::new(STAND_IN_NET);
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
        s.give_back(a1);
        s.give_back(a1);
        assert_eq!(
            s.count(a),
            1,
            "a second give-back of one stand-in counts once"
        );
        s.give_back(a2);
        s.give_back(b1);
        assert_eq!((s.count(a), s.len()), (0, 0));
        // A peer that comes back after all its sockets closed has the same stand-in IP.
        let again = s.take(a);
        assert_eq!(again.ip(), a1.ip());
        assert_eq!(IpAddr::V4(s.ip_of(b)), b1.ip());
        assert!(!is_stand_in("127.0.0.1:5".parse().unwrap()));
        let top = "255.255.255.255:5".parse().unwrap();
        assert!(
            !is_stand_in(top) && is_reserved(top),
            "the top of the /4 is the QUIC link's half"
        );
    }
}
