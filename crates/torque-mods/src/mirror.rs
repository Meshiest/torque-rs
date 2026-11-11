//! A cache store kept in memory and mirrored to a database that answers later than a call is made, which is what a browser's
//! storage (IndexedDB) is: every [`CacheStore`] call is answered from memory at once, and each change is handed to a
//! [`Backing`] to keep. Whatever the backing does with it (it may refuse, or be gone) the cache works from memory for the session.
//!
//! What a backing holds is read back whole when a page starts ([`MirroredStore::load`]) and trusted no further than a folder of
//! files is: a name that is not an entry's, an entry over the cap and an entry beyond what the cap leaves room for are dropped from
//! the backing too, and an entry whose bytes were changed is found out by the cache's own hash check on its read
//! ([`crate::ContentCache::load`]), which deletes it here and in the backing.

use crate::cache::{CacheStore, Listed, MAX_ENTRIES, MemoryStore, entry_name, index_of};

/// Where a [`MirroredStore`]'s changes are kept. Every call is a request that is answered later or never; none can fail the cache.
pub trait Backing {
    /// Keeps an entry's bytes under `name` (replacing an entry of that name).
    fn put(&self, name: &str, bytes: &[u8]);
    /// Forgets an entry and its stamp.
    fn remove(&self, name: &str);
    /// Keeps when an entry was last used.
    fn stamp(&self, name: &str, stamp: u64);
}

/// An entry as a backing held it when read back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Record {
    /// The entry's name.
    pub name: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
    /// When it was last used (`0` when the backing has no stamp for it).
    pub stamp: u64,
}

/// A [`MemoryStore`] whose changes also go to a [`Backing`].
pub struct MirroredStore<B: Backing> {
    memory: MemoryStore,
    backing: B,
    limit: u64,
}

impl<B: Backing> MirroredStore<B> {
    /// An empty store over `backing` that holds at most `limit` bytes.
    pub fn new(backing: B, limit: u64) -> MirroredStore<B> {
        MirroredStore {
            memory: MemoryStore::default(),
            backing,
            limit,
        }
    }

    /// A store over `backing` holding `records`, the entries it had, within `limit` bytes. Records that are not entries (by
    /// name), are larger than `limit`, or come after the most recently used ones that fill `limit` are removed from the backing.
    pub fn load(backing: B, records: Vec<Record>, limit: u64) -> MirroredStore<B> {
        let mut store = MirroredStore::new(backing, limit);
        let listing: Vec<Listed> = records
            .iter()
            .take(MAX_ENTRIES.saturating_mul(2))
            .map(|r| Listed {
                name: r.name.clone(),
                size: r.bytes.len() as u64,
                stamp: r.stamp,
            })
            .collect();
        let mut wanted = index_of(&listing);
        wanted.sort_by_key(|e| std::cmp::Reverse(e.stamp));
        let mut room = limit;
        let mut kept = std::collections::HashSet::new();
        for entry in wanted {
            if entry.size > room {
                continue;
            }
            room -= entry.size;
            kept.insert(entry_name(&entry.hash, entry.owner));
        }
        for record in records {
            if !kept.contains(&record.name) {
                store.backing.remove(&record.name);
                continue;
            }
            store.memory.write(&record.name, &record.bytes);
            store.memory.touch(&record.name, record.stamp);
        }
        store
    }
}

impl<B: Backing> CacheStore for MirroredStore<B> {
    fn list(&self) -> Vec<Listed> {
        self.memory.list()
    }

    fn read(&self, name: &str, max: u64) -> Option<Vec<u8>> {
        self.memory.read(name, max)
    }

    fn read_head(&self, name: &str, n: usize) -> Option<Vec<u8>> {
        self.memory.read_head(name, n)
    }

    fn write(&mut self, name: &str, bytes: &[u8]) -> bool {
        self.memory.write(name, bytes);
        self.backing.put(name, bytes);
        true
    }

    fn remove(&mut self, name: &str) {
        self.memory.remove(name);
        self.backing.remove(name);
    }

    fn touch(&mut self, name: &str, stamp: u64) {
        self.memory.touch(name, stamp);
        self.backing.stamp(name, stamp);
    }

    fn limit(&self) -> u64 {
        self.limit
    }
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
