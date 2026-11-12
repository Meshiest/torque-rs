//! Fairness between sources: every budget a stranger can draw on is counted per source network as well as per address, so one
//! party holding many addresses of one network (a /24, a cloud VM's range) gets one network's share and no more.
//!
//! - [`NetworkKey`]: the network of an address (an IPv4-mapped IPv6 address counts as the IPv4 address it maps,
//!   [`canonical`]): for the connections one source may hold an IPv4 /24 or an IPv6 /64 ([`NetworkKey::of`]), for its share
//!   of a budget in all an IPv4 /24 or an IPv6 /48 ([`NetworkKey::sharing`]: one site's prefix, of which a party holds
//!   65,536 /64s).
//! - [`RateWindow`] and [`KeyedWindows`]: a count in a window of time (a second, or a span of its own), and such windows per
//!   key in a bounded table that, when full, forgets the least recently used key or refuses the new one ([`Overflow`]); under
//!   either a flood of other keys cannot reset a held key's count. Both count on a [`Clock`] clock (`u32` wrapping, `u64`
//!   growing).
//! - [`TokenBucket`]: tokens (messages, bytes) a second with a burst, in thousandths so no remainder is lost.
//! - [`put_addr`], [`keyed128`] and [`parse_address_text`]: an address into a digest, a 128-bit keyed digest, the IP of an
//!   address as the console writes it.
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
use crate::handshake::siphash24;

/// `addr` with an IPv4-mapped IPv6 address taken as the IPv4 address it maps.
#[must_use]
pub fn canonical(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}

/// Appends `addr` as a digest's input: 4 or 6, the IP's bytes, then its port little-endian. The address goes in as given: a caller
/// whose IPv4-mapped form must digest as the IPv4 one takes it [`canonical`] first.
pub fn put_addr(out: &mut Vec<u8>, addr: SocketAddr) {
    match addr.ip() {
        IpAddr::V4(ip) => {
            out.push(4);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(6);
            out.extend_from_slice(&ip.octets());
        }
    }
    out.extend_from_slice(&addr.port().to_le_bytes());
}

/// A 128-bit keyed digest of `data`: [`siphash24`] of it with `data[at]` set to 0, then with it set to 1 (as it is left).
///
/// # Panics
///
/// When `at` is not inside `data`.
pub fn keyed128(key: &[u8; 16], data: &mut [u8], at: usize) -> [u64; 2] {
    data[at] = 0;
    let a = siphash24(key, data);
    data[at] = 1;
    [a, siphash24(key, data)]
}

/// The IP of an address as text (`"IP:1.2.3.4:28000"`, `"IP6:[fe80::1%3]:28000"`, a bare IP with or without a port), with its
/// tag, brackets, port and zone taken off, [`canonical`]. `None` for text that is no address (`"local"`, junk).
#[must_use]
pub fn parse_address_text(text: &str) -> Option<IpAddr> {
    let text = text.trim();
    let text = text
        .strip_prefix("IP6:")
        .or_else(|| text.strip_prefix("IP:"))
        .unwrap_or(text);
    let host = match text.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        // A bare IPv6 address has colons of its own: only a single colon is a port.
        None if text.matches(':').count() == 1 => text.split(':').next().unwrap_or(text),
        None => text,
    };
    let host = host.split('%').next().unwrap_or(host);
    host.parse::<IpAddr>().ok().map(|ip| ip.to_canonical())
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

/// A time in milliseconds that [`RateWindow`], [`KeyedWindows`] and [`TokenBucket`] count by, one of two clocks:
/// - `u32`: a clock that wraps every 2^32 ms (49.7 days), compared by wrapping difference;
/// - `u64`: a clock that only grows: a time before the last one seen counts as no time passing.
pub trait Clock: Copy + Default + Eq + Hash + std::fmt::Debug + sealed::Sealed {
    /// The milliseconds from `earlier` to `self`.
    fn since(self, earlier: Self) -> u64;

    /// What a bucket keeps as its last time after one at `self` (`last` the one before).
    fn latest(self, last: Self) -> Self;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for u32 {}
    impl Sealed for u64 {}
}

impl Clock for u32 {
    fn since(self, earlier: u32) -> u64 {
        u64::from(self.wrapping_sub(earlier))
    }

    fn latest(self, _last: u32) -> u32 {
        self
    }
}

impl Clock for u64 {
    fn since(self, earlier: u64) -> u64 {
        self.saturating_sub(earlier)
    }

    fn latest(self, last: u64) -> u64 {
        self.max(last)
    }
}

/// A count of events (or of units, such as bytes) in a window of time: one second ([`RateWindow::default`]) or a span of
/// its own ([`RateWindow::span`]). The window opens at the first event counted and closes `span` later; a refused event is
/// counted nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateWindow<T: Clock = u32> {
    start: T,
    count: u32,
    span: u32,
}

impl<T: Clock> Default for RateWindow<T> {
    /// A one-second window.
    fn default() -> RateWindow<T> {
        RateWindow::span(1_000)
    }
}

impl<T: Clock> RateWindow<T> {
    /// A window of `ms` milliseconds (at least 1).
    #[must_use]
    pub fn span(ms: u32) -> RateWindow<T> {
        RateWindow {
            start: T::default(),
            count: 0,
            span: ms.max(1),
        }
    }

    /// Counts one event at `now`; false when `limit` were already counted in the current window.
    pub fn allow(&mut self, now: T, limit: u32) -> bool {
        self.allow_n(now, 1, limit)
    }

    /// Counts `n` units at `now`; false (and nothing counted) when they would take the window's count past `limit`.
    pub fn allow_n(&mut self, now: T, n: u32, limit: u32) -> bool {
        if !self.has_room(now, n, limit) {
            return false;
        }
        self.count += n;
        true
    }

    /// Whether `n` more units fit in the window at `now` (the window rolls first), counting nothing.
    pub fn has_room(&mut self, now: T, n: u32, limit: u32) -> bool {
        self.roll(now);
        self.count.checked_add(n).is_some_and(|c| c <= limit)
    }

    /// What the window has counted at `now` (0 once it has passed), counting nothing.
    #[must_use]
    pub fn used(&self, now: T) -> u32 {
        if self.is_live(now) { self.count } else { 0 }
    }

    /// Counts up to `limit` at once, so the rest of the window refuses every event (a count past it stays).
    pub fn fill(&mut self, now: T, limit: u32) {
        self.roll(now);
        self.count = self.count.max(limit);
    }

    /// Whether the window holds a count at `now`: something was counted and the span has not passed.
    #[must_use]
    pub fn is_live(&self, now: T) -> bool {
        self.count > 0 && now.since(self.start) < u64::from(self.span)
    }

    /// The milliseconds until the window passes (0 once it has).
    fn left(&self, now: T) -> u64 {
        if self.is_live(now) {
            u64::from(self.span) - now.since(self.start)
        } else {
            0
        }
    }

    fn roll(&mut self, now: T) {
        if !self.is_live(now) {
            self.start = now;
            self.count = 0;
        }
    }
}

/// What a full [`KeyedWindows`] does for a key it does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// It forgets its least recently used key. A key that keeps sending is always recent, so it is forgotten only after
    /// `capacity` other keys came between two of its events: the table must be sized above the keys that can arrive in that
    /// time.
    Evict,
    /// It forgets the keys whose windows have passed; if it is still full, the new key is refused (and counted nowhere). No
    /// live count is ever forgotten, so a table of live keys refuses newcomers until their windows pass.
    Refuse,
}

/// [`RateWindow`]s per key, in a table of at most `capacity` keys that does what its [`Overflow`] says when it is full. Under
/// either policy a flood of fresh keys cannot reset a held key's count: [`Overflow::Evict`] forgets only a key that has not
/// been seen while `capacity` others were, [`Overflow::Refuse`] forgets only counts whose windows have passed.
#[derive(Debug)]
pub struct KeyedWindows<K, T: Clock = u32> {
    /// Each key's window, and when it was last used (an evicting table's order).
    windows: HashMap<K, (RateWindow<T>, u64)>,
    capacity: usize,
    fresh: RateWindow<T>,
    full: Full<K, T>,
}

/// What a full table keeps to do its [`Overflow`].
#[derive(Debug)]
enum Full<K, T> {
    /// The keys by last use, and the next use's stamp.
    Evict { by_use: BTreeMap<u64, K>, next: u64 },
    /// When the passed windows were last forgotten, and how long after that no window can pass: a full table refusing
    /// newcomers looks for passed windows again only then, so a flood of them costs no scan each.
    Refuse { checked: T, wait: u64 },
}

impl<K: Copy + Eq + Hash> Default for KeyedWindows<K> {
    fn default() -> KeyedWindows<K> {
        KeyedWindows::new(NET_LIMITER_ENTRIES)
    }
}

impl<K: Copy + Eq + Hash> KeyedWindows<K> {
    /// One-second windows in a table of `capacity` keys (at least 1) that forgets the least recently used one
    /// ([`Overflow::Evict`]).
    #[must_use]
    pub fn new(capacity: usize) -> KeyedWindows<K> {
        KeyedWindows::bounded(capacity, 1_000, Overflow::Evict)
    }
}

impl<K: Copy + Eq + Hash, T: Clock> KeyedWindows<K, T> {
    /// A table of `capacity` keys, each counted in a window of `span` milliseconds ([`RateWindow::span`]), that does what
    /// `overflow` says when full. An evicting table holds at least one key; a refusing table of 0 refuses every key.
    #[must_use]
    pub fn bounded(capacity: usize, span: u32, overflow: Overflow) -> KeyedWindows<K, T> {
        let (capacity, full) = match overflow {
            Overflow::Evict => (
                capacity.max(1),
                Full::Evict {
                    by_use: BTreeMap::new(),
                    next: 0,
                },
            ),
            Overflow::Refuse => (
                capacity,
                Full::Refuse {
                    checked: T::default(),
                    wait: 0,
                },
            ),
        };
        KeyedWindows {
            windows: HashMap::new(),
            capacity,
            fresh: RateWindow::span(span),
            full,
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

    /// Whether `key` could be counted at `now`: it is held, or the table has (or, refusing, can make) room for it.
    pub fn can_hold(&mut self, key: K, now: T) -> bool {
        self.windows.contains_key(&key)
            || matches!(self.full, Full::Evict { .. })
            || self.make_room(now)
    }

    /// Whether a new key fits at `now`, after an evicting table forgets its least recently used keys or a refusing one its
    /// passed windows.
    fn make_room(&mut self, now: T) -> bool {
        if self.windows.len() < self.capacity {
            return true;
        }
        match &mut self.full {
            Full::Evict { by_use, .. } => {
                while self.windows.len() >= self.capacity
                    && let Some((_, oldest)) = by_use.pop_first()
                {
                    self.windows.remove(&oldest);
                }
                true
            }
            Full::Refuse { checked, wait } => {
                if now.since(*checked) >= *wait {
                    self.windows.retain(|_, (window, _)| window.is_live(now));
                    *checked = now;
                    *wait = self
                        .windows
                        .values()
                        .map(|(window, _)| window.left(now))
                        .min()
                        .unwrap_or(0);
                }
                self.windows.len() < self.capacity
            }
        }
    }

    /// `key`'s window, entered (or moved to most recent) now; `None` when a refusing table has no room for it.
    fn touch(&mut self, key: K, now: T) -> Option<&mut RateWindow<T>> {
        let held = self.windows.contains_key(&key);
        if !held {
            if !self.make_room(now) {
                return None;
            }
            self.windows.insert(key, (self.fresh, 0));
        }
        let (window, used) = self.windows.get_mut(&key)?;
        if let Full::Evict { by_use, next } = &mut self.full {
            if held {
                by_use.remove(used);
            }
            *used = *next;
            by_use.insert(*next, key);
            *next += 1;
        }
        Some(window)
    }

    /// Whether `key` may have one more event in its window, at most `limit` (counted when it may). False, counting nothing,
    /// when a refusing table has no room for a new key.
    pub fn allow(&mut self, key: K, now: T, limit: u32) -> bool {
        let refusing = matches!(self.full, Full::Refuse { .. });
        let Some(window) = self.touch(key, now) else {
            return false;
        };
        let allowed = window.allow(now, limit);
        // A window that counted nothing holds nothing: a refusing table, which forgets only passed windows on its own
        // schedule, lets it go now rather than keep its place.
        if refusing && !window.is_live(now) {
            self.windows.remove(&key);
        }
        allowed
    }

    /// What `key` has counted in its window at `now`, counting nothing (0 for a key not held).
    #[must_use]
    pub fn used(&self, key: K, now: T) -> u32 {
        self.windows.get(&key).map_or(0, |(w, _)| w.used(now))
    }

    /// Counts a held `key` up to `limit` at once ([`RateWindow::fill`]); a key not held has no window and is left so.
    pub fn fill(&mut self, key: K, now: T, limit: u32) {
        if let Some((window, _)) = self.windows.get_mut(&key) {
            window.fill(now, limit);
        }
    }

    /// Keeps only the keys `keep` says to (for keys that stand for something that can go away, such as an object).
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.windows.retain(|key, _| keep(key));
        if let Full::Evict { by_use, .. } = &mut self.full {
            let windows = &self.windows;
            by_use.retain(|_, key| windows.contains_key(key));
        }
    }

    fn has_room(&mut self, key: K, now: T, n: u32, limit: u32) -> bool {
        self.touch(key, now)
            .is_some_and(|w| w.has_room(now, n, limit))
    }

    fn charge(&mut self, key: K, now: T, n: u32, limit: u32) {
        if let Some(window) = self.touch(key, now) {
            let _ = window.allow_n(now, n, limit);
        }
    }
}

/// A token bucket: up to `burst` tokens, gaining `rate` a second, kept in thousandths of a token so a rate below one a
/// millisecond still refills and no remainder is lost. A bucket starts full. Taking is all or nothing ([`TokenBucket::take`]);
/// a caller that charges several buckets for one event checks them all ([`TokenBucket::has`]) before it spends from any
/// ([`TokenBucket::spend`]), so what one refuses costs the others nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBucket<T: Clock = u64> {
    milli: u64,
    last: Option<T>,
    rate: u64,
    burst: u64,
}

impl<T: Clock> TokenBucket<T> {
    /// A full bucket of at most `burst` tokens (at least 1) gaining `rate` a second, that starts counting time at its first
    /// refill.
    #[must_use]
    pub fn new(rate: u64, burst: u64) -> TokenBucket<T> {
        let burst = burst.max(1);
        TokenBucket {
            milli: burst.saturating_mul(1_000),
            last: None,
            rate,
            burst,
        }
    }

    /// [`TokenBucket::new`], counting time from `now`.
    #[must_use]
    pub fn starting_at(rate: u64, burst: u64, now: T) -> TokenBucket<T> {
        TokenBucket {
            last: Some(now),
            ..TokenBucket::new(rate, burst)
        }
    }

    /// Tokens a second.
    #[must_use]
    pub fn rate(&self) -> u64 {
        self.rate
    }

    /// The most tokens it holds.
    #[must_use]
    pub fn burst(&self) -> u64 {
        self.burst
    }

    /// The time it last counted to, if any.
    #[must_use]
    pub fn last(&self) -> Option<T> {
        self.last
    }

    /// Adds what the time since the last refill gained, up to the burst.
    pub fn refill(&mut self, now: T) {
        if let Some(last) = self.last {
            self.milli = self
                .milli
                .saturating_add(self.rate.saturating_mul(now.since(last)))
                .min(self.burst.saturating_mul(1_000));
        }
        self.last = Some(self.last.map_or(now, |last| now.latest(last)));
    }

    /// Whether it holds `n` tokens.
    #[must_use]
    pub fn has(&self, n: u64) -> bool {
        self.milli >= n.saturating_mul(1_000)
    }

    /// Spends `n` tokens, or all it holds when that is less.
    pub fn spend(&mut self, n: u64) {
        self.milli = self.milli.saturating_sub(n.saturating_mul(1_000));
    }

    /// Refills to `now` and spends `n` tokens: false, and nothing spent, when it does not hold them.
    pub fn take(&mut self, now: T, n: u64) -> bool {
        self.refill(now);
        if !self.has(n) {
            return false;
        }
        self.spend(n);
        true
    }

    /// Gives back `n` tokens (spent for something that never went out), up to the burst.
    pub fn refund(&mut self, n: u64) {
        self.milli = self
            .milli
            .saturating_add(n.saturating_mul(1_000))
            .min(self.burst.saturating_mul(1_000));
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
