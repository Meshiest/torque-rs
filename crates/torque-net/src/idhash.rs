//! a hasher for maps keyed by object ids (`u32`): a multiply and a fold instead of the standard SipHash, which costs more per
//! lookup than these maps need (a server walks every scoped object through such maps on each full scope query). The ids come
//! from the engine's own counter, not from a peer, so no flood can be aimed at one bucket beyond what sequential ids give; nothing reads a map's order.

use std::hash::{BuildHasherDefault, Hasher};

/// The hasher (see the module docs).
#[derive(Default, Clone, Copy)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(u64::from(*b));
        }
    }

    fn write_u32(&mut self, v: u32) {
        self.write_u64(u64::from(v));
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }

    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }

    fn finish(&self) -> u64 {
        self.0 ^ (self.0 >> 29)
    }
}

/// A map's hasher builder of [`IdHasher`]s.
pub type IdBuild = BuildHasherDefault<IdHasher>;

/// A map keyed by object ids.
pub type IdMap<K, V> = std::collections::HashMap<K, V, IdBuild>;

#[cfg(test)]
#[path = "idhash_tests.rs"]
mod tests;
