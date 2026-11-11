//! When the server asks for address validation first: a count of new handshakes a second, over all addresses and per
//! network (an IPv4 /24, an IPv6 /48). Above either cap an unvalidated handshake is answered with a Retry (RFC 9000
//! section 8.1.2), which costs the server no state and the sender a round trip; below them it is accepted at once. Validated
//! handshakes (the Retry's return, or a token from an earlier connection) are counted in windows of their own and dropped
//! only past [`VALIDATED_FACTOR`] times the caps: the returns of the Retries just sent must get through, and a flood of
//! forged unvalidated Initials never decides a validated arrival's fate.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Instant;

/// The most networks whose counts are kept; past it the counts of past seconds are dropped first.
const MAX_NETWORKS: usize = 4_096;

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

#[derive(Debug, Clone, Copy, Default)]
struct Window {
    second: u64,
    count: u32,
}

impl Window {
    /// Counts one arrival in `second`; the count so far in that second.
    fn bump(&mut self, second: u64) -> u32 {
        if self.second != second {
            *self = Window { second, count: 0 };
        }
        self.count = self.count.saturating_add(1);
        self.count
    }
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
#[derive(Debug, Default)]
struct Windows {
    all: Window,
    networks: HashMap<u128, Window>,
}

impl Windows {
    /// Counts an arrival from `ip` in `second`; the counts so far in that second over all and for its network.
    fn bump(&mut self, second: u64, ip: IpAddr) -> (u32, u32) {
        let all = self.all.bump(second);
        if self.networks.len() >= MAX_NETWORKS {
            self.networks.retain(|_, w| w.second == second);
            if self.networks.len() >= MAX_NETWORKS {
                self.networks.clear();
            }
        }
        let network = self
            .networks
            .entry(network_of(ip))
            .or_default()
            .bump(second);
        (all, network)
    }
}

impl RetryGate {
    #[must_use]
    pub fn new(start: Instant, per_second: u32, per_network_per_second: u32) -> RetryGate {
        RetryGate {
            start,
            per_second,
            per_network_per_second,
            unvalidated: Windows::default(),
            validated: Windows::default(),
        }
    }

    /// Counts a new handshake from `ip` at `now` and says whether it is over the caps: for an unvalidated one, whether to
    /// validate its address first; for a `validated` one, whether to drop it ([`VALIDATED_FACTOR`] times the caps).
    pub fn arrival(&mut self, now: Instant, ip: IpAddr, validated: bool) -> Load {
        let second = now.saturating_duration_since(self.start).as_secs();
        let (all, network) = if validated {
            self.validated.bump(second, ip)
        } else {
            self.unvalidated.bump(second, ip)
        };
        let factor = if validated { VALIDATED_FACTOR } else { 1 };
        if all > self.per_second.saturating_mul(factor)
            || network > self.per_network_per_second.saturating_mul(factor)
        {
            Load::Busy
        } else {
            Load::Calm
        }
    }
}

/// The network an address counts under: an IPv4 /24, an IPv6 /48.
pub(crate) fn network_of(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(v4) => u128::from(u32::from(v4) >> 8),
        IpAddr::V6(v6) => (1 << 120) | (u128::from(v6) >> 80),
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
        for i in 0..(MAX_NETWORKS as u32 * 3) {
            let ip = IpAddr::V4(Ipv4Addr::from(i << 8));
            gate.arrival(start, ip, false);
        }
        assert!(gate.unvalidated.networks.len() <= MAX_NETWORKS);
    }
}
