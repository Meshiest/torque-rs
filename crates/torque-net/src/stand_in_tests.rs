use super::*;

fn addr(s: &str) -> SocketAddr {
    s.parse().expect("an address")
}

fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

#[test]
fn the_two_ranges_are_the_halves_of_the_reserved_range() {
    for net in [WEB_NET, QUIC_NET] {
        assert!(is_stand_in(SocketAddr::new(net.into(), 5), net));
        assert!(is_reserved(SocketAddr::new(net.into(), 5)));
    }
    assert!(is_stand_in(
        addr("247.255.255.255:5"),
        Ipv4Addr::new(240, 0, 0, 0)
    ));
    assert!(!is_stand_in(addr("248.0.0.0:5"), WEB_NET));
    assert!(!is_stand_in(addr("247.255.255.255:5"), QUIC_NET));
    assert!(is_stand_in(addr("255.255.255.255:5"), QUIC_NET));
    assert!(!is_stand_in(addr("255.255.255.255:5"), WEB_NET));
    assert!(is_reserved(addr("255.255.255.255:5")));
    for outside in ["239.255.255.255:5", "127.0.0.1:5", "[::1]:5"] {
        assert!(!is_reserved(addr(outside)), "{outside}");
        assert!(!is_stand_in(addr(outside), WEB_NET) && !is_stand_in(addr(outside), QUIC_NET));
    }
}

#[test]
fn stand_ins_are_in_their_range_and_one_ip_per_peer_for_good() {
    for net in [WEB_NET, QUIC_NET] {
        let (a, b) = (v4(127, 0, 0, 1), v4(192, 168, 1, 9));
        let mut s = StandIns::new(net);
        let (a1, a2, b1) = (s.take(a), s.take(a), s.take(b));
        assert!(
            [a1, a2, b1]
                .iter()
                .all(|&x| is_stand_in(x, net) && is_reserved(x))
        );
        assert_eq!(a1.ip(), a2.ip(), "one stand-in IP per peer");
        assert_ne!(a1.port(), a2.port());
        assert_ne!(a1.ip(), b1.ip());
        assert_eq!((s.count(a), s.count(b), s.len()), (2, 1, 3));
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
        assert!(s.is_empty());
        // A peer that comes back after all its connections closed has the same stand-in IP.
        assert_eq!(s.take(a).ip(), a1.ip());
        assert_eq!(IpAddr::V4(s.ip_of(b)), b1.ip());
    }
}

#[test]
fn a_stand_in_in_use_is_not_taken_again_and_clear_forgets_them() {
    let peer = v4(10, 0, 0, 1);
    let mut s = StandIns::new(QUIC_NET);
    let first = s.take(peer);
    let rest: Vec<_> = (0..4).map(|_| s.take(peer)).collect();
    assert!(!rest.contains(&first));
    s.give_back(first);
    assert_eq!(s.take(peer).ip(), first.ip());
    s.clear();
    assert_eq!((s.count(peer), s.len()), (0, 0));
}

#[test]
fn the_port_wraps_past_the_top_and_skips_a_stand_in_still_in_use() {
    let peer = v4(10, 0, 0, 1);
    let mut s = StandIns::new(WEB_NET);
    let first = s.take(peer);
    assert_eq!(first.port(), 1);
    s.next_port = u16::MAX;
    let wrapped = s.take(peer);
    assert_eq!(wrapped.port(), 2, "port 1 is still in use");
    assert_eq!(s.take(peer).port(), 3);
}
