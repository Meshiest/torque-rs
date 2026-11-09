//! Fairness between sources: every budget a stranger can draw on is counted per source network as well as per address, so one
//! party holding many addresses of one network (a /24, a cloud VM's range) gets one network's share and no more.
//!
//! - [`NetworkKey`]: the network of an address (an IPv4-mapped IPv6 address counts as the IPv4 address it maps,
//!   [`canonical`]): for the connections one source may hold an IPv4 /24 or an IPv6 /64 ([`NetworkKey::of`]), for its share
//!   of a budget in all an IPv4 /24 or an IPv6 /48 ([`NetworkKey::sharing`]: one site's prefix, of which a party holds
//!   65,536 /64s).
//! - [`RateWindow`] and [`KeyedWindows`]: a count in a one-second window, and such windows per key in a bounded table that
//!   forgets the least recently used key first (a key that keeps sending stays known, so a flood of other keys cannot reset
//!   its count).
//! - [`FairLimiter`]: a per-second budget charged per IP, per network and in all; with a total, one network takes at most
//!   [`NET_NETWORK_SHARE`]'s fraction of it ([`FairLimits::shared`]). A refused event charges nothing.
//! - [`SourceCaps`] and [`SourceCount`]: how many connections one IP and one network may hold, pending and in all, and the
//!   count a table takes of what it holds before it admits one more.
//! - [`choose_eviction`]: what a full pending table does for a newcomer, the one rule the TGE handshake, the engine gate and
//!   the session table share.
//!
//! The TGE handshake ([`crate::handshake`]), the engine gate and session table ([`crate::engine`]) and the hosts above them
//! all count through these, so the rule is the same everywhere.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::net::{IpAddr, SocketAddr};

use crate::cap::{
    NET_CONNS_PER_ADDR, NET_CONNS_PER_NETWORK, NET_LIMITER_ENTRIES, NET_NETWORK_SHARE,
    NET_PENDING_PER_ADDR, NET_PENDING_PER_NETWORK,
};

/// `addr` with an IPv4-mapped IPv6 address taken as the IPv4 address it maps.
#[must_use]
pub fn canonical(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}

/// A source network, the unit one party is assumed to hold many addresses of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NetworkKey {
    /// The first three bytes (a /24).
    V4([u8; 3]),
    /// The first eight bytes (a /64).
    V6([u8; 8]),
}

impl NetworkKey {
    /// The network of `ip`, taken in its canonical form.
    #[must_use]
    pub fn of(ip: IpAddr) -> NetworkKey {
        match ip.to_canonical() {
            IpAddr::V4(ip) => {
                let [a, b, c, _] = ip.octets();
                NetworkKey::V4([a, b, c])
            }
            IpAddr::V6(ip) => {
                let o = ip.octets();
                NetworkKey::V6([o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7]])
            }
        }
    }

    /// The network of `addr`'s IP address.
    #[must_use]
    pub fn of_addr(addr: SocketAddr) -> NetworkKey {
        NetworkKey::of(addr.ip())
    }

    /// The network `ip`'s share of a budget in all is counted by: an IPv4 /24, an IPv6 /48 (its last two bytes zero).
    #[must_use]
    pub fn sharing(ip: IpAddr) -> NetworkKey {
        match NetworkKey::of(ip) {
            NetworkKey::V6([a, b, c, d, e, f, _, _]) => NetworkKey::V6([a, b, c, d, e, f, 0, 0]),
            v4 => v4,
        }
    }
}

/// A count of events (or of units, such as bytes) in a one-second window.
#[derive(Debug, Clone, Copy, Default)]
pub struct RateWindow {
    start: u32,
    count: u32,
}

impl RateWindow {
    /// Counts one event at `now`; false when `limit` were already counted in the current second.
    pub fn allow(&mut self, now: u32, limit: u32) -> bool {
        self.allow_n(now, 1, limit)
    }

    /// Counts `n` units at `now`; false (and nothing counted) when they would take the second's count past `limit`.
    pub fn allow_n(&mut self, now: u32, n: u32, limit: u32) -> bool {
        if !self.has_room(now, n, limit) {
            return false;
        }
        self.count += n;
        true
    }

    /// Whether `n` more units fit in the second at `now` (the window rolls first).
    fn has_room(&mut self, now: u32, n: u32, limit: u32) -> bool {
        self.roll(now);
        self.count.checked_add(n).is_some_and(|c| c <= limit)
    }

    fn roll(&mut self, now: u32) {
        if self.count == 0 || now.wrapping_sub(self.start) >= 1_000 {
            self.start = now;
            self.count = 0;
        }
    }
}

/// [`RateWindow`]s per key, in a table of at most `capacity` keys. A new key in a full table makes the table forget its least
/// recently used key: a key that keeps sending is always recent, so it is forgotten only after `capacity` other keys came
/// between two of its events, and a flood of fresh keys cannot reset its count.
#[derive(Debug)]
pub struct KeyedWindows<K> {
    windows: HashMap<K, (RateWindow, u64)>,
    by_use: BTreeMap<u64, K>,
    next: u64,
    capacity: usize,
}

impl<K: Copy + Eq + Hash> Default for KeyedWindows<K> {
    fn default() -> KeyedWindows<K> {
        KeyedWindows::new(NET_LIMITER_ENTRIES)
    }
}

impl<K: Copy + Eq + Hash> KeyedWindows<K> {
    #[must_use]
    pub fn new(capacity: usize) -> KeyedWindows<K> {
        KeyedWindows {
            windows: HashMap::new(),
            by_use: BTreeMap::new(),
            next: 0,
            capacity: capacity.max(1),
        }
    }

    /// The keys held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// `key`'s window, entered (or moved to most recent) now.
    fn touch(&mut self, key: K) -> &mut RateWindow {
        let stamp = self.next;
        self.next += 1;
        if let Some((_, used)) = self.windows.get_mut(&key) {
            self.by_use.remove(used);
            *used = stamp;
        } else {
            while self.windows.len() >= self.capacity
                && let Some((_, oldest)) = self.by_use.pop_first()
            {
                self.windows.remove(&oldest);
            }
            self.windows.insert(key, (RateWindow::default(), stamp));
        }
        self.by_use.insert(stamp, key);
        &mut self.windows.get_mut(&key).expect("entered just now").0
    }

    /// Whether `key` may have one more event this second, at most `limit` (counted when it may).
    pub fn allow(&mut self, key: K, now: u32, limit: u32) -> bool {
        self.touch(key).allow(now, limit)
    }

    fn has_room(&mut self, key: K, now: u32, n: u32, limit: u32) -> bool {
        self.touch(key).has_room(now, n, limit)
    }

    fn charge(&mut self, key: K, now: u32, n: u32, limit: u32) {
        let _ = self.touch(key).allow_n(now, n, limit);
    }
}

/// Per-IP counters in a bounded table ([`KeyedWindows`]).
pub type SourceLimiter = KeyedWindows<IpAddr>;

/// The limits of a [`FairLimiter`], each a count (or a sum of units) per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FairLimits {
    /// What one IP address may take.
    pub per_ip: u32,
    /// What one network may take.
    pub per_network: u32,
    /// What all sources together may take; `None` for no total.
    pub total: Option<u32>,
}

impl FairLimits {
    /// A total of `total` of which one network takes at most a [`NET_NETWORK_SHARE`]th (at least 1) and one IP `per_ip`.
    #[must_use]
    pub const fn shared(total: u32, per_ip: u32) -> FairLimits {
        let share = total / NET_NETWORK_SHARE;
        FairLimits {
            per_ip,
            per_network: if share == 0 { 1 } else { share },
            total: Some(total),
        }
    }

    /// Limits per IP and per network with no total.
    #[must_use]
    pub const fn per_source(per_ip: u32, per_network: u32) -> FairLimits {
        FairLimits {
            per_ip,
            per_network,
            total: None,
        }
    }
}

/// Which limit refused an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Over {
    Ip,
    Network,
    Total,
}

/// A per-second budget charged per IP, per network ([`NetworkKey::sharing`]) and in all ([`FairLimits`]). An event is counted only when every limit
/// has room for it, so what one source is refused costs the others nothing.
#[derive(Debug)]
pub struct FairLimiter {
    limits: FairLimits,
    ips: KeyedWindows<IpAddr>,
    networks: KeyedWindows<NetworkKey>,
    total: RateWindow,
}

impl FairLimiter {
    #[must_use]
    pub fn new(limits: FairLimits) -> FairLimiter {
        FairLimiter {
            limits,
            ips: KeyedWindows::default(),
            networks: KeyedWindows::default(),
            total: RateWindow::default(),
        }
    }

    /// Counts one event from `ip` at `now`, or says which limit refuses it.
    pub fn allow(&mut self, ip: IpAddr, now: u32) -> Result<(), Over> {
        self.allow_n(ip, 1, now)
    }

    /// [`FairLimiter::allow`] under `limits`, which replace the stored ones: for limits read from a config at each event, so
    /// no copy of them goes stale when the config changes.
    pub fn allow_within(&mut self, limits: FairLimits, ip: IpAddr, now: u32) -> Result<(), Over> {
        self.limits = limits;
        self.allow_n(ip, 1, now)
    }

    /// Counts `n` units from `ip` at `now`, or says which limit refuses them (nothing is counted then).
    pub fn allow_n(&mut self, ip: IpAddr, n: u32, now: u32) -> Result<(), Over> {
        self.check(ip, n, now)?;
        let ip = ip.to_canonical();
        let limits = self.limits;
        self.ips.charge(ip, now, n, limits.per_ip);
        self.networks
            .charge(NetworkKey::sharing(ip), now, n, limits.per_network);
        if let Some(total) = limits.total {
            let _ = self.total.allow_n(now, n, total);
        }
        Ok(())
    }

    /// Whether `n` units from `ip` would be counted at `now`, counting nothing.
    pub fn check(&mut self, ip: IpAddr, n: u32, now: u32) -> Result<(), Over> {
        let ip = ip.to_canonical();
        let limits = self.limits;
        if !self.ips.has_room(ip, now, n, limits.per_ip) {
            return Err(Over::Ip);
        }
        if !self
            .networks
            .has_room(NetworkKey::sharing(ip), now, n, limits.per_network)
        {
            return Err(Over::Network);
        }
        if let Some(total) = limits.total
            && !self.total.has_room(now, n, total)
        {
            return Err(Over::Total);
        }
        Ok(())
    }
}

/// Where a connection is, as the caps count it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Accepted (or opened), not yet heard from as a game connection: counted against the pending caps too.
    Pending,
    Established,
}

/// How many connections one source may hold: pending ones (a new one past either cap is refused) and all of them, pending or
/// established (a new one past either is refused with a reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceCaps {
    pub pending_per_ip: usize,
    pub pending_per_network: usize,
    pub per_ip: usize,
    pub per_network: usize,
}

impl Default for SourceCaps {
    /// [`NET_PENDING_PER_ADDR`], [`NET_PENDING_PER_NETWORK`], [`NET_CONNS_PER_ADDR`], [`NET_CONNS_PER_NETWORK`].
    fn default() -> SourceCaps {
        SourceCaps {
            pending_per_ip: NET_PENDING_PER_ADDR,
            pending_per_network: NET_PENDING_PER_NETWORK,
            per_ip: NET_CONNS_PER_ADDR,
            per_network: NET_CONNS_PER_NETWORK,
        }
    }
}

impl SourceCaps {
    /// Caps no table reaches: for a harness that runs many clients from one address.
    #[must_use]
    pub const fn unlimited() -> SourceCaps {
        SourceCaps {
            pending_per_ip: usize::MAX,
            pending_per_network: usize::MAX,
            per_ip: usize::MAX,
            per_network: usize::MAX,
        }
    }

    /// Whether one more pending connection from the source `count` was taken for stays within the pending caps, or the cap it
    /// would pass.
    pub fn admits_pending(&self, count: &SourceCount) -> Result<(), CapHit> {
        if count.pending_ip >= self.pending_per_ip {
            Err(CapHit::PendingPerIp)
        } else if count.pending_network >= self.pending_per_network {
            Err(CapHit::PendingPerNetwork)
        } else {
            Ok(())
        }
    }

    /// Whether one more connection from the source `count` was taken for stays within the caps, or the cap it would pass.
    pub fn admits(&self, count: &SourceCount) -> Result<(), CapHit> {
        self.admits_pending(count)?;
        if count.ip >= self.per_ip {
            Err(CapHit::PerIp)
        } else if count.network >= self.per_network {
            Err(CapHit::PerNetwork)
        } else {
            Ok(())
        }
    }
}

/// The cap a new connection would pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapHit {
    PendingPerIp,
    PendingPerNetwork,
    PerIp,
    PerNetwork,
}

impl CapHit {
    /// Whether it is one of the pending caps (refused without a word: the source may simply wait).
    #[must_use]
    pub fn is_pending(self) -> bool {
        matches!(self, CapHit::PendingPerIp | CapHit::PendingPerNetwork)
    }
}

/// What a table holds that [`SourceCaps`] count for a new connection from one IP.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceCount {
    pub ip: usize,
    pub network: usize,
    pub pending_ip: usize,
    pub pending_network: usize,
}

impl SourceCount {
    /// The count for a new connection from `ip` over what a table holds: each held connection's own IP and stage.
    pub fn of(ip: IpAddr, held: impl IntoIterator<Item = (IpAddr, Stage)>) -> SourceCount {
        let ip = ip.to_canonical();
        let network = NetworkKey::of(ip);
        let mut count = SourceCount::default();
        for (other, stage) in held {
            let other = other.to_canonical();
            let same_ip = usize::from(other == ip);
            let same_network = usize::from(NetworkKey::of(other) == network);
            count.ip += same_ip;
            count.network += same_network;
            if stage == Stage::Pending {
                count.pending_ip += same_ip;
                count.pending_network += same_network;
            }
        }
        count
    }
}

/// One pending connection a full table could evict for a newcomer: its key in the table, its peer's own IP and its age.
#[derive(Debug, Clone, Copy)]
pub struct PendingSlot<K> {
    pub key: K,
    pub ip: IpAddr,
    pub age_ms: u64,
}

/// What a full pending table does for a newcomer ([`choose_eviction`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eviction<K> {
    /// This pending connection leaves to make room.
    Evict(K),
    /// The newcomer is refused.
    Refuse,
}

/// The one rule a full pending table follows for a newcomer from `requester`, over the evictable pending connections it
/// holds. The fair share is what they come to over the networks holding them (`requester`'s counted), and at least one:
/// 1. `requester`'s network already holds its share: refused;
/// 2. else the oldest of a network over its share leaves;
/// 3. else the oldest at least `min_age_ms` old leaves (an honest client has spoken long before);
/// 4. else refused: the table holds only fair, young connections, which is real capacity.
///
/// So no one holding many addresses can evict an honest client's only slot, nor keep a new network out of a table it
/// holds more than its share of.
pub fn choose_eviction<K: Copy>(
    pending: &[PendingSlot<K>],
    requester: IpAddr,
    min_age_ms: u64,
) -> Eviction<K> {
    let mut held: HashMap<NetworkKey, usize> = HashMap::new();
    for slot in pending {
        *held.entry(NetworkKey::of(slot.ip)).or_default() += 1;
    }
    let own = NetworkKey::of(requester);
    let networks = held.len() + usize::from(!held.contains_key(&own));
    let share = (pending.len() / networks.max(1)).max(1);
    if held.get(&own).copied().unwrap_or(0) >= share {
        return Eviction::Refuse;
    }
    let oldest = |filter: &dyn Fn(&PendingSlot<K>) -> bool| {
        pending
            .iter()
            .enumerate()
            .filter(|(_, s)| filter(s))
            .min_by_key(|(i, s)| (std::cmp::Reverse(s.age_ms), *i))
            .map(|(_, s)| s.key)
    };
    let over = |s: &PendingSlot<K>| held.get(&NetworkKey::of(s.ip)).is_some_and(|&n| n > share);
    oldest(&over)
        .or_else(|| oldest(&|s| s.age_ms >= min_age_ms))
        .map_or(Eviction::Refuse, Eviction::Evict)
}

#[cfg(test)]
#[path = "fair_tests.rs"]
mod tests;
