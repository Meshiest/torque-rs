//! When the server asks for address validation first: a count of new handshakes a second, over all addresses and per
//! network (an IPv4 /24, an IPv6 /48: [`NetworkKey::sharing`]). Above either cap an unvalidated handshake is answered with a
//! Retry (RFC 9000 section 8.1.2), which costs the server no state and the sender a round trip; below them it is accepted at
//! once. Validated handshakes (the Retry's return, or a token from an earlier connection) are counted in windows of their own
//! and dropped only past [`VALIDATED_FACTOR`] times the caps: the returns of the Retries just sent must get through, and a
//! flood of forged unvalidated Initials never decides a validated arrival's fate. The per-network counts live in
//! [`KeyedWindows`], which forgets the least recently used network first, so a flood of fresh networks cannot reset the count
//! of one that keeps sending.
//!
//! An arrival is counted only when every window has room for it, so what one network is refused costs the others nothing: a
//! network over its own cap never spends the shared all-address budget. The network table sees an arrival only while the
//! all-address window has room, and a network refused for its own cap is touched (kept recent) there without being charged.
//! So the networks newly entered in a second are at most those the all-address cap admits, at most twice that cap (a second's
//! window may straddle two all-address windows) of other networks come between two arrivals of one network, and the table is
//! sized above that (up to [`MAX_NETWORKS_CEILING`]): a flood of spoofed networks cannot push a held network's count out
//! however high the caps are set.

use std::net::IpAddr;
use std::time::Instant;

use torque_net::fair::{KeyedWindows, NetworkKey, RateWindow};

/// The fewest networks whose counts are kept per kind of arrival.
const MAX_NETWORKS: usize = 4_096;

/// The most networks whose counts are kept per kind of arrival, however high the caps are set.
const MAX_NETWORKS_CEILING: usize = 1 << 20;

/// How many times the caps a validated handshake may arrive above before it is dropped.
pub const VALIDATED_FACTOR: u32 = 4;

/// What the gate says of a handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Load {
    /// Under the caps: accept.
    Calm,
    /// Over a cap: validate the address first.
    Busy,
}

/// The handshake rate gate (module docs).
#[derive(Debug)]
pub struct RetryGate {
    start: Instant,
    per_second: u32,
    per_network_per_second: u32,
    unvalidated: Windows,
    validated: Windows,
}

/// One kind of arrival's counts: over all addresses, and per network.
#[derive(Debug)]
struct Windows {
    all: RateWindow,
    networks: KeyedWindows<NetworkKey>,
}

impl Windows {
    /// Windows for arrivals admitted up to `all_cap` a second over all addresses.
    fn new(all_cap: u32) -> Windows {
        let networks = (all_cap as usize)
            .saturating_mul(2)
            .saturating_add(2)
            .clamp(MAX_NETWORKS, MAX_NETWORKS_CEILING);
        Windows {
            all: RateWindow::default(),
            networks: KeyedWindows::new(networks),
        }
    }

    /// Counts an arrival from `ip` at `now` (ms) when it is within `all_cap` over all addresses and `network_cap` for its
    /// network, and says whether it was. A refused arrival counts nowhere.
    fn allow(&mut self, now: u32, ip: IpAddr, all_cap: u32, network_cap: u32) -> bool {
        if !self.all.has_room(now, 1, all_cap) {
            return false;
        }
        if !self
            .networks
            .allow(NetworkKey::sharing(ip), now, network_cap)
        {
            return false;
        }
        let _ = self.all.allow(now, all_cap);
        true
    }
}

impl RetryGate {
    #[must_use]
    pub fn new(start: Instant, per_second: u32, per_network_per_second: u32) -> RetryGate {
        RetryGate {
            start,
            per_second,
            per_network_per_second,
            unvalidated: Windows::new(per_second),
            validated: Windows::new(per_second.saturating_mul(VALIDATED_FACTOR)),
        }
    }

    /// Counts a new handshake from `ip` at `now` and says whether it is over the caps: for an unvalidated one, whether to
    /// validate its address first; for a `validated` one, whether to drop it ([`VALIDATED_FACTOR`] times the caps).
    pub fn arrival(&mut self, now: Instant, ip: IpAddr, validated: bool) -> Load {
        // Milliseconds since the gate began, wrapping (the windows compare by wrapping difference).
        let ms = now.saturating_duration_since(self.start).as_millis() as u32;
        let (windows, factor) = if validated {
            (&mut self.validated, VALIDATED_FACTOR)
        } else {
            (&mut self.unvalidated, 1)
        };
        let all_cap = self.per_second.saturating_mul(factor);
        let network_cap = self.per_network_per_second.saturating_mul(factor);
        if windows.allow(ms, ip, all_cap, network_cap) {
            Load::Calm
        } else {
            Load::Busy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    #[test]
    fn a_flood_from_one_network_is_busy_while_the_rest_stays_calm_and_the_second_clears_it() {
        let start = Instant::now();
        let mut gate = RetryGate::new(start, 100, 4);
        let flood = |i: u8| IpAddr::V4(Ipv4Addr::new(10, 0, 0, i));
        for i in 0..4 {
            assert_eq!(gate.arrival(start, flood(i), false), Load::Calm);
        }
        assert_eq!(gate.arrival(start, flood(9), false), Load::Busy);
        assert_eq!(
            gate.arrival(start, IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1)), false),
            Load::Calm,
            "another /24"
        );
        // Validated arrivals have windows of their own: a forged unvalidated flood never gates them, and they are let
        // through until four times the cap (16 of them).
        for _ in 0..100 {
            assert_eq!(gate.arrival(start, flood(9), false), Load::Busy);
        }
        for _ in 0..16 {
            assert_eq!(gate.arrival(start, flood(9), true), Load::Calm);
        }
        assert_eq!(gate.arrival(start, flood(9), true), Load::Busy);
        let later = start + Duration::from_secs(1);
        assert_eq!(gate.arrival(later, flood(9), false), Load::Calm);
        let mut gate = RetryGate::new(start, 3, 100);
        for _ in 0..3 {
            assert_eq!(gate.arrival(start, flood(1), false), Load::Calm);
        }
        assert_eq!(
            gate.arrival(start, IpAddr::V4(Ipv4Addr::new(10, 9, 9, 9)), false),
            Load::Busy
        );
    }

    #[test]
    fn the_network_table_is_bounded() {
        let start = Instant::now();
        let mut gate = RetryGate::new(start, 1 << 30, 1 << 30);
        for i in 0..(MAX_NETWORKS_CEILING as u32 + 1_000) {
            let ip = IpAddr::V4(Ipv4Addr::from(i << 8));
            gate.arrival(start, ip, false);
        }
        assert!(gate.unvalidated.networks.len() <= MAX_NETWORKS_CEILING);
    }

    #[test]
    fn an_ipv6_network_is_a_48_and_a_mapped_address_is_its_v4() {
        let start = Instant::now();
        let mut gate = RetryGate::new(start, 100, 2);
        let a = |s: &str| s.parse::<IpAddr>().expect("address");
        assert_eq!(gate.arrival(start, a("2001:db8:1:1::1"), false), Load::Calm);
        assert_eq!(
            gate.arrival(start, a("2001:db8:1:ffff::2"), false),
            Load::Calm
        );
        assert_eq!(gate.arrival(start, a("2001:db8:1:2::3"), false), Load::Busy);
        assert_eq!(gate.arrival(start, a("2001:db8:2::1"), false), Load::Calm);
        assert_eq!(gate.arrival(start, a("10.0.0.1"), false), Load::Calm);
        assert_eq!(gate.arrival(start, a("10.0.0.2"), false), Load::Calm);
        assert_eq!(gate.arrival(start, a("::ffff:10.0.0.3"), false), Load::Busy);
    }

    #[test]
    fn a_network_held_over_its_cap_stays_busy_under_a_flood_of_fresh_networks() {
        let start = Instant::now();
        let mut gate = RetryGate::new(start, 1 << 30, 4);
        let held = |i: u8| IpAddr::V4(Ipv4Addr::new(10, 0, 0, i));
        for i in 0..4 {
            assert_eq!(gate.arrival(start, held(i), false), Load::Calm);
        }
        // Twice the table's worth of fresh networks arrive in the same second while the held network keeps sending.
        for i in 0..(MAX_NETWORKS as u32 * 2) {
            let ip = IpAddr::V4(Ipv4Addr::from(0x0b00_0000 + (i << 8)));
            assert_eq!(gate.arrival(start, ip, false), Load::Calm);
            if i % 64 == 0 {
                assert_eq!(
                    gate.arrival(start, held(9), false),
                    Load::Busy,
                    "after {i} fresh networks"
                );
            }
        }
    }

    #[test]
    fn a_held_network_survives_a_flood_of_spoofed_networks_with_high_caps() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let all_cap = MAX_NETWORKS as u32 * 3 / 2;
        let mut gate = RetryGate::new(start, all_cap, 4);
        let held = |i: u8| IpAddr::V4(Ipv4Addr::new(10, 0, 0, i));
        let fresh = |i: u32| IpAddr::V4(Ipv4Addr::from(0x0b00_0000 + (i << 8)));
        // The held network's window opens at 400 ms, the all-address window at 0 ms.
        assert_eq!(gate.arrival(at(0), fresh(0), false), Load::Calm);
        for i in 0..4 {
            assert_eq!(gate.arrival(at(400), held(i), false), Load::Calm);
        }
        assert_eq!(gate.arrival(at(400), held(9), false), Load::Busy);
        // More fresh networks than the minimum table holds, up to the all-address cap.
        for i in 1..all_cap {
            gate.arrival(at(900), fresh(i), false);
        }
        assert!(gate.unvalidated.networks.len() >= MAX_NETWORKS);
        // The all-address window has rolled; the held network's has not.
        assert_eq!(gate.arrival(at(1_100), held(9), false), Load::Busy);
    }

    #[test]
    fn a_network_far_over_its_cap_spends_none_of_the_shared_budget() {
        let start = Instant::now();
        let mut gate = RetryGate::new(start, 20, 4);
        let flood = |i: u32| IpAddr::V4(Ipv4Addr::new(10, 0, 0, i as u8));
        let modest = |i: u32| IpAddr::V4(Ipv4Addr::new(10, 0, (i % 3) as u8 + 1, 1));
        let mut calm_from_flood = 0;
        for i in 0..1_000 {
            if gate.arrival(start, flood(i), false) == Load::Calm {
                calm_from_flood += 1;
            }
            if i % 100 == 0 {
                assert_eq!(gate.arrival(start, modest(i / 100), false), Load::Calm);
            }
        }
        assert_eq!(calm_from_flood, 4);
    }

    #[test]
    fn the_window_survives_the_millisecond_count_wrapping() {
        let start = Instant::now();
        let mut gate = RetryGate::new(start, 100, 1);
        let before = start + Duration::from_millis((1 << 32) - 500);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(gate.arrival(before, ip, false), Load::Calm);
        assert_eq!(gate.arrival(before, ip, false), Load::Busy);
        let after = start + Duration::from_millis((1 << 32) + 600);
        assert_eq!(gate.arrival(after, ip, false), Load::Calm);
    }
}
