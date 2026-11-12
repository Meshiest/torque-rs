use super::*;
use std::net::{Ipv4Addr, Ipv6Addr};

fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

#[test]
fn a_network_is_a_v4_24_or_a_v6_64_and_a_mapped_address_is_its_v4() {
    assert_eq!(
        NetworkKey::of(v4(10, 1, 2, 3)),
        NetworkKey::of(v4(10, 1, 2, 200))
    );
    assert_ne!(
        NetworkKey::of(v4(10, 1, 2, 3)),
        NetworkKey::of(v4(10, 1, 3, 3))
    );
    let mapped = IpAddr::V6(Ipv4Addr::new(10, 1, 2, 9).to_ipv6_mapped());
    assert_eq!(NetworkKey::of(mapped), NetworkKey::of(v4(10, 1, 2, 3)));
    let a: IpAddr = "2001:db8:1:2::1".parse().expect("v6");
    let b: IpAddr = "2001:db8:1:2:ffff::9".parse().expect("v6");
    let c: IpAddr = "2001:db8:1:3::1".parse().expect("v6");
    assert_eq!(NetworkKey::of(a), NetworkKey::of(b));
    assert_ne!(NetworkKey::of(a), NetworkKey::of(c));
    assert_eq!(
        canonical(SocketAddr::new(mapped, 7)),
        SocketAddr::new(v4(10, 1, 2, 9), 7)
    );
    assert_ne!(
        NetworkKey::of(IpAddr::V6(Ipv6Addr::LOCALHOST)),
        NetworkKey::of(v4(0, 0, 0, 1))
    );
}

#[test]
fn a_steady_source_stays_limited_under_a_flood_of_rotating_sources() {
    // 2,000 other sources a second rotate through the table while one source sends 10 a second: it still gets 4.
    let mut limiter = SourceLimiter::default();
    let steady = v4(192, 0, 2, 1);
    let mut allowed = 0;
    let mut other = 0u32;
    for tick in 0..100u32 {
        let now = tick * 10;
        for _ in 0..20 {
            other += 1;
            let ip = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + other));
            let _ = limiter.allow(ip, now, 4);
        }
        if tick % 10 == 0 && limiter.allow(steady, now, 4) {
            allowed += 1;
        }
    }
    assert_eq!(allowed, 4);
    assert!(limiter.len() <= NET_LIMITER_ENTRIES);
}

#[test]
fn a_full_table_forgets_its_least_recently_used_key() {
    let mut windows = KeyedWindows::new(3);
    let ips = [
        v4(1, 0, 0, 1),
        v4(1, 0, 0, 2),
        v4(1, 0, 0, 3),
        v4(1, 0, 0, 4),
    ];
    for ip in &ips[..3] {
        assert!(windows.allow(*ip, 0, 1));
    }
    // The first is used again, so the second is the least recent and goes when the fourth comes.
    assert!(!windows.allow(ips[0], 0, 1));
    assert!(windows.allow(ips[3], 0, 1));
    assert_eq!(windows.len(), 3);
    assert!(!windows.allow(ips[0], 0, 1), "still counted");
    assert!(windows.allow(ips[1], 0, 1), "forgotten, so fresh");
    // Fresh keys only: each goes in the order it came, the first first.
    let mut windows = KeyedWindows::new(2);
    for ip in &ips[..3] {
        assert!(windows.allow(*ip, 0, 1));
    }
    assert_eq!(
        [ips[0], ips[1], ips[2]].map(|ip| windows.used(ip, 0)),
        [0, 1, 1],
        "the first is forgotten, the others held"
    );
}

#[test]
fn one_network_takes_at_most_its_share_of_a_total_and_others_still_get_through() {
    // 256 addresses of one /24 at 4 a second each against a total of 32: the network gets 4, every other network still finds
    // room in every second.
    let mut limiter = FairLimiter::new(FairLimits::shared(32, 4));
    for second in 0..5u32 {
        let base = second * 1_000;
        let mut hostile = 0;
        for round in 0..4u32 {
            for host in 0..=255u8 {
                hostile += u32::from(limiter.allow(v4(203, 0, 113, host), base + round).is_ok());
            }
        }
        assert_eq!(hostile, 4, "second {second}");
        assert!(limiter.allow(v4(198, 51, 100, 7), base + 500).is_ok());
    }
}

#[test]
fn a_refused_event_charges_nothing() {
    let mut limiter = FairLimiter::new(FairLimits {
        per_ip: 1,
        per_network: 10,
        total: Some(2),
    });
    let a = v4(10, 0, 0, 1);
    assert_eq!(limiter.allow(a, 0), Ok(()));
    for _ in 0..10 {
        assert_eq!(limiter.allow(a, 0), Err(Over::Ip));
    }
    assert_eq!(limiter.allow(v4(10, 0, 1, 1), 0), Ok(()));
    assert_eq!(limiter.allow(v4(10, 0, 2, 1), 0), Err(Over::Total));
    assert_eq!(limiter.allow(v4(10, 0, 2, 1), 1_000), Ok(()));
}

#[test]
fn units_are_charged_whole_or_not_at_all() {
    let mut limiter = FairLimiter::new(FairLimits::per_source(100, 150));
    let a = v4(10, 0, 0, 1);
    assert_eq!(limiter.allow_n(a, 80, 0), Ok(()));
    assert_eq!(limiter.allow_n(a, 30, 0), Err(Over::Ip));
    assert_eq!(limiter.allow_n(a, 20, 0), Ok(()));
    assert_eq!(limiter.allow_n(v4(10, 0, 0, 2), 60, 0), Err(Over::Network));
    assert_eq!(limiter.allow_n(v4(10, 0, 0, 2), 50, 0), Ok(()));
}

#[test]
fn the_eviction_rule_takes_from_a_network_over_its_share_or_an_old_slot_and_never_a_young_only_one()
{
    let slot = |key: u32, ip: IpAddr, age_ms: u64| PendingSlot { key, ip, age_ms };
    // One network holds three, another one: a newcomer from a third network evicts the first network's oldest.
    let held = [
        slot(1, v4(10, 0, 0, 1), 5),
        slot(2, v4(10, 0, 0, 2), 9),
        slot(3, v4(10, 0, 0, 3), 1),
        slot(4, v4(10, 1, 0, 1), 50),
    ];
    assert_eq!(
        choose_eviction(&held, v4(10, 2, 0, 1), 1_000),
        Eviction::Evict(2)
    );
    // The requester's network already holds its share: refused.
    assert_eq!(
        choose_eviction(&held, v4(10, 0, 0, 9), 1_000),
        Eviction::Refuse
    );
    // The share comes to 0 (more networks than slots): taken as 1, so a network's only slot is never over it.
    let spread = [slot(1, v4(10, 0, 0, 1), 10), slot(2, v4(10, 1, 0, 1), 20)];
    assert_eq!(
        choose_eviction(&spread, v4(10, 2, 0, 1), 1_000),
        Eviction::Refuse
    );
    // ... unless it is old: then the oldest goes.
    assert_eq!(
        choose_eviction(&spread, v4(10, 2, 0, 1), 15),
        Eviction::Evict(2)
    );
}

#[test]
fn a_network_shares_budgets_by_its_v6_48_and_holds_connections_by_its_v6_64() {
    let a: IpAddr = "2001:db8:1:2::1".parse().expect("v6");
    let b: IpAddr = "2001:db8:1:3::1".parse().expect("v6");
    assert_ne!(NetworkKey::of(a), NetworkKey::of(b));
    assert_eq!(NetworkKey::sharing(a), NetworkKey::sharing(b));
    // A /48's addresses share one network's part of a total.
    let mut limiter = FairLimiter::new(FairLimits::shared(16, 16));
    let got = (0..64u16)
        .filter(|n| {
            let ip = IpAddr::from([0x2001, 0xdb8, 1, *n, 0, 0, 0, 1]);
            limiter.allow(ip, 0).is_ok()
        })
        .count();
    assert_eq!(got, 2);
}

#[test]
fn the_caps_count_pending_and_all_connections_per_ip_and_per_network() {
    let caps = SourceCaps::default();
    let ip = v4(10, 0, 0, 1);
    let same_net = v4(10, 0, 0, 2);
    let held = [
        (ip, Stage::Pending),
        (same_net, Stage::Established),
        (v4(10, 9, 0, 1), Stage::Pending),
    ];
    let count = SourceCount::of(ip, held);
    assert_eq!(
        count,
        SourceCount {
            ip: 1,
            network: 2,
            pending_ip: 1,
            pending_network: 1,
        }
    );
    assert_eq!(caps.admits(&count), Ok(()));
    let pending = [(ip, Stage::Pending); NET_PENDING_PER_ADDR];
    assert_eq!(
        caps.admits(&SourceCount::of(ip, pending)),
        Err(CapHit::PendingPerIp)
    );
    let network: Vec<_> = (2..2 + NET_PENDING_PER_NETWORK as u8)
        .map(|h| (v4(10, 0, 0, h), Stage::Pending))
        .collect();
    assert_eq!(
        caps.admits(&SourceCount::of(ip, network)),
        Err(CapHit::PendingPerNetwork)
    );
    let established = [(ip, Stage::Established); NET_CONNS_PER_ADDR];
    assert_eq!(
        caps.admits(&SourceCount::of(ip, established)),
        Err(CapHit::PerIp)
    );
    let network: Vec<_> = (2..2 + NET_CONNS_PER_NETWORK as u8)
        .map(|h| (v4(10, 0, 0, h), Stage::Established))
        .collect();
    assert_eq!(
        caps.admits(&SourceCount::of(ip, network)),
        Err(CapHit::PerNetwork)
    );
    assert_eq!(
        SourceCaps::unlimited().admits(&SourceCount::of(ip, established)),
        Ok(())
    );
}

/// A held key over its limit, then `fresh` other keys in the same window, the held key asking again every `every` of them:
/// how many of the held key's asks were allowed.
fn held_allowed_under_a_flood<T: Clock>(
    windows: &mut KeyedWindows<IpAddr, T>,
    now: T,
    fresh: u32,
    every: u32,
) -> usize {
    let held = v4(192, 0, 2, 1);
    for _ in 0..4 {
        assert!(windows.allow(held, now, 4));
    }
    let mut allowed = 0;
    for i in 0..fresh {
        let _ = windows.allow(IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + i)), now, 4);
        if i % every == 0 && windows.allow(held, now, 4) {
            allowed += 1;
        }
    }
    allowed
}

#[test]
fn a_flood_of_fresh_keys_cannot_reset_a_held_key_when_the_table_evicts() {
    let mut windows = KeyedWindows::new(64);
    assert_eq!(
        held_allowed_under_a_flood(&mut windows, 0u32, 10_000, 32),
        0
    );
    assert!(windows.len() <= 64);
    let mut windows: KeyedWindows<IpAddr, u64> = KeyedWindows::bounded(64, 60_000, Overflow::Evict);
    assert_eq!(
        held_allowed_under_a_flood(&mut windows, 5_000u64, 10_000, 32),
        0
    );
    assert!(windows.len() <= 64);
}

#[test]
fn a_flood_of_fresh_keys_cannot_reset_a_held_key_when_the_table_refuses() {
    let mut windows: KeyedWindows<IpAddr, u64> =
        KeyedWindows::bounded(64, 60_000, Overflow::Refuse);
    // The held key asks only as the flood begins: a refusing table forgets no live count however long it goes unused.
    assert_eq!(
        held_allowed_under_a_flood(&mut windows, 0u64, 10_000, u32::MAX),
        0
    );
    assert_eq!(windows.len(), 64);
    let held = v4(192, 0, 2, 1);
    assert!(!windows.allow(held, 59_999, 4), "still counted");
    // A newcomer is refused while every count is live, and counted nowhere.
    let newcomer = v4(198, 51, 100, 1);
    assert!(!windows.can_hold(newcomer, 59_999));
    assert!(!windows.allow(newcomer, 59_999, 4));
    assert_eq!(windows.used(newcomer, 59_999), 0);
    // Once the windows have passed the table makes room, and the held key's window has rolled too.
    assert!(windows.allow(newcomer, 60_000, 4));
    assert!(windows.allow(held, 60_000, 4));
    let mut windows = KeyedWindows::bounded(64, 1_000, Overflow::Refuse);
    assert_eq!(
        held_allowed_under_a_flood(&mut windows, 7u32, 10_000, 32),
        0
    );
    // A refusing table of no keys refuses every one.
    let mut none: KeyedWindows<IpAddr> = KeyedWindows::bounded(0, 1_000, Overflow::Refuse);
    assert!(!none.allow(held, 0, 4));
    assert!(none.is_empty());
}

#[test]
fn a_window_of_its_own_span_counts_until_it_passes_and_fills_on_demand() {
    let mut w: RateWindow<u64> = RateWindow::span(60_000);
    assert!(w.allow(100, 2));
    assert!(w.allow(59_000, 2));
    assert!(!w.allow(60_099, 2));
    assert_eq!(w.used(60_099), 2);
    assert_eq!(w.used(60_100), 0);
    assert!(w.allow(60_100, 2));
    w.fill(60_100, 2);
    assert!(!w.allow(60_100, 2));
    // A growing clock: a time before the window's start is no time passed.
    assert!(!w.allow(10, 2));
    // A wrapping clock rolls across the wrap.
    let mut w: RateWindow = RateWindow::default();
    assert!(w.allow(u32::MAX - 100, 1));
    assert!(!w.allow(u32::MAX, 1));
    assert!(w.allow(900, 1));
}

#[test]
fn a_bucket_refills_with_its_clock_and_never_past_its_burst() {
    let mut b: TokenBucket<u64> = TokenBucket::starting_at(10, 3, 1_000);
    assert_eq!((0..10).filter(|_| b.take(1_000, 1)).count(), 3);
    // 0.25 s at 10 a second: two and a half tokens, the half kept.
    assert!(b.take(1_250, 2));
    assert!(!b.take(1_250, 1));
    assert!(b.take(1_300, 1));
    // A time before the last adds nothing and leaves the clock where it was.
    assert!(!b.take(0, 1));
    assert!(!b.take(1_399, 1));
    assert!(b.take(1_400, 1));
    b.refund(100);
    assert!(b.has(3) && !b.has(4));
    b.spend(10);
    assert!(!b.has(1));
    let mut b: TokenBucket<u32> = TokenBucket::new(1_000, 10);
    assert!(b.take(u32::MAX - 5, 10));
    assert!(b.take(4, 10), "10 ms across the wrap");
}

#[test]
fn an_address_text_is_its_canonical_ip_and_digests_take_the_address_as_given() {
    let ip = |t: &str| parse_address_text(t);
    assert_eq!(ip("IP:1.2.3.4:28000"), Some(v4(1, 2, 3, 4)));
    assert_eq!(ip("IP6:[::ffff:1.2.3.4]:28000"), Some(v4(1, 2, 3, 4)));
    assert_eq!(ip("IP6:[fe80::1%3]:9"), "fe80::1".parse().ok());
    assert_eq!(ip("local"), None);
    let mut a = Vec::new();
    put_addr(&mut a, SocketAddr::new(v4(1, 2, 3, 4), 0x1234));
    assert_eq!(a, [4, 1, 2, 3, 4, 0x34, 0x12]);
    let key = [7u8; 16];
    let mut data = vec![9, 0, 5];
    let [x, y] = keyed128(&key, &mut data, 1);
    assert_eq!(data, [9, 1, 5]);
    assert_eq!(x, siphash24(&key, &[9, 0, 5]));
    assert_eq!(y, siphash24(&key, &[9, 1, 5]));
}

#[test]
fn a_full_refusing_table_frees_a_slot_as_soon_as_a_window_passes() {
    let mut windows: KeyedWindows<u32, u64> = KeyedWindows::bounded(2, 1_000, Overflow::Refuse);
    assert!(windows.allow(1, 0, 4));
    assert!(windows.allow(2, 500, 4));
    // Many newcomers while both windows are live: all refused, none counted.
    for (n, now) in (100..1_000).step_by(3).enumerate() {
        assert!(!windows.allow(1_000 + n as u32, now, 4), "{now}");
    }
    assert_eq!(windows.len(), 2);
    // The first window passes at 1,000: the next newcomer takes its slot, and the table is full again.
    assert!(windows.allow(10, 1_000, 4));
    for now in 1_000..1_500 {
        assert!(!windows.allow(20, now, 4), "{now}");
    }
    // The second passes at 1,500.
    assert!(windows.allow(30, 1_500, 4));
    assert!(windows.used(2, 1_500) == 0 && windows.used(10, 1_500) == 1);
    // On a wrapping clock too, across the wrap.
    let mut windows: KeyedWindows<u32> = KeyedWindows::bounded(1, 1_000, Overflow::Refuse);
    assert!(windows.allow(1, u32::MAX - 200, 4));
    assert!(!windows.allow(2, 798, 4));
    assert!(windows.allow(2, 799, 4));
}
