//! Maps and sets keyed by the engine's own ids (object ids, convex ids), with rustc-hash's hash: one multiply a key, where
//! std's SipHash would be most of the cost of a lookup. Only for keys the engine numbers itself, never for text or numbers a
//! peer or a script chooses, and only where no order of the map is ever read.

/// A map keyed by ids.
pub type IdMap<K, V> = rustc_hash::FxHashMap<K, V>;

/// A set of ids.
pub type IdSet<K> = rustc_hash::FxHashSet<K>;
