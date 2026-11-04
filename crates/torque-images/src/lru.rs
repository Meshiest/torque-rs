//! A least-recently-used map for the client's caches of decoded files (GUI bitmaps, render textures, shapes). Each entry has a
//! cost (its bytes, or what the cache counts); the map keeps at most `max_entries` entries and `max_bytes` of cost, and an insert that
//! would pass either drops the least recently used entries first, never a full clear. A frame can pin what it used
//! ([`Lru::begin_frame`]): an entry touched since then is not dropped while the frame lasts, so a frame whose files do not all fit does
//! not decode its own files again and again.
//!
//! Lookups, inserts and drops are `O(log n)`.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

struct Slot<V> {
    value: V,
    stamp: u64,
    cost: usize,
}

/// See the module docs.
pub struct Lru<K, V> {
    map: HashMap<K, Slot<V>>,
    /// The keys by the stamp of their last use, oldest first.
    order: BTreeMap<u64, K>,
    clock: u64,
    /// The first stamp of the frame being drawn: entries stamped at or after it are pinned (None: nothing is).
    pin_from: Option<u64>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    /// How many entries were dropped to make room.
    pub evicted: u64,
}

impl<K: Hash + Eq + Clone, V> Lru<K, V> {
    pub fn new(max_entries: usize, max_bytes: usize) -> Lru<K, V> {
        Lru {
            map: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
            pin_from: None,
            bytes: 0,
            max_entries,
            max_bytes,
            evicted: 0,
        }
    }

    /// The entries held.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The cost held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The caps (entries, cost).
    pub fn caps(&self) -> (usize, usize) {
        (self.max_entries, self.max_bytes)
    }

    /// Changes the caps (a test lowers them); entries over them go at the next insert.
    pub fn set_caps(&mut self, max_entries: usize, max_bytes: usize) {
        self.max_entries = max_entries;
        self.max_bytes = max_bytes;
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Starts a frame: what is used from now on stays until the next frame starts.
    pub fn begin_frame(&mut self) {
        self.pin_from = Some(self.clock + 1);
    }

    /// The value of `key`, marked as used now.
    pub fn get<Q>(&mut self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let now = self.clock + 1;
        let slot = self.map.get_mut(key)?;
        self.clock = now;
        let old = std::mem::replace(&mut slot.stamp, now);
        if let Some(k) = self.order.remove(&old) {
            self.order.insert(now, k);
        }
        Some(&self.map.get(key)?.value)
    }

    /// The value of `key`, not marked as used.
    pub fn peek<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get(key).map(|s| &s.value)
    }

    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.contains_key(key)
    }

    /// Takes `key` out.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let slot = self.map.remove(key)?;
        self.order.remove(&slot.stamp);
        self.bytes -= slot.cost;
        Some(slot.value)
    }

    /// The cost of the entries the frame being drawn has pinned.
    fn pinned_bytes(&self) -> usize {
        match self.pin_from {
            None => 0,
            Some(from) => self
                .order
                .range(from..)
                .filter_map(|(_, k)| self.map.get(k))
                .map(|s| s.cost)
                .sum(),
        }
    }

    /// Whether an entry of `cost` could be kept once every entry the frame has not pinned is dropped.
    pub fn fits(&self, cost: usize) -> bool {
        cost <= self.max_bytes && self.pinned_bytes().saturating_add(cost) <= self.max_bytes
    }

    /// Drops the least recently used entry the frame has not pinned; false when there is none.
    fn drop_oldest(&mut self) -> bool {
        let Some((&stamp, _)) = self.order.iter().next() else {
            return false;
        };
        if self.pin_from.is_some_and(|from| stamp >= from) {
            return false;
        }
        let Some(key) = self.order.remove(&stamp) else {
            return false;
        };
        if let Some(slot) = self.map.remove(&key) {
            self.bytes -= slot.cost;
        }
        self.evicted += 1;
        true
    }

    /// Keeps `value` under `key` (replacing what `key` held), dropping the least recently used unpinned entries until both caps hold.
    /// False (and nothing kept, `key`'s old value gone too) when it cannot fit: its cost is above the byte cap, or the frame's pinned
    /// entries leave no room.
    pub fn insert(&mut self, key: K, value: V, cost: usize) -> bool {
        self.remove(&key);
        if cost > self.max_bytes || self.max_entries == 0 {
            return false;
        }
        while (self.map.len() >= self.max_entries
            || self.bytes.saturating_add(cost) > self.max_bytes)
            && self.drop_oldest()
        {}
        if self.map.len() >= self.max_entries || self.bytes.saturating_add(cost) > self.max_bytes {
            return false;
        }
        let now = self.tick();
        self.order.insert(now, key.clone());
        self.map.insert(
            key,
            Slot {
                value,
                stamp: now,
                cost,
            },
        );
        self.bytes += cost;
        true
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.bytes = 0;
        self.pin_from = None;
    }

    /// The values held, in no order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.map.values().map(|s| &s.value)
    }
}

#[cfg(test)]
#[path = "lru_tests.rs"]
mod tests;
