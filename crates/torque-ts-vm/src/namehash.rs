//! The hasher of the maps keyed by names (variables, namespaces and their functions, object names, classes, statics):
//! foldhash's keyed hash (a folded 64 x 64 -> 128-bit multiply over the seeds, per-process and per-map random seeds)
//! instead of the standard SipHash, whose cost showed in every variable and function look-up (a large load spent about
//! a tenth of its time hashing names). Names reach these maps from peers (a server's datablock and object names, tagged
//! strings, commands, downloaded scripts), so the hash is keyed: which names share a bucket is not known to whoever
//! chose them. Nothing reads a map's order.

/// A map's hasher builder: foldhash's, seeded at random for each map.
pub type NameBuild = foldhash::quality::RandomState;

/// A map keyed by names.
pub type NameMap<K, V> = std::collections::HashMap<K, V, NameBuild>;

#[cfg(test)]
#[path = "namehash_tests.rs"]
mod tests;
