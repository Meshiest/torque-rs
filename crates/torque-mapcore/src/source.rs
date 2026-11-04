//! Where an importer's files come from.
//!
//! Legacy maps read through each host's own files (the server's file host, the client's assets), so they
//! need no source of their own.
//! A BSP importer's would be the installed base archives and the map's own pack lump; another format provides one of its own.

/// A read-only store of a format's files, searched in its order.
pub trait AssetSource {
    /// The file at `path` (the format's own spelling: the source folds case and separators as its format does), at most
    /// `max_bytes` long; None when it is not there or is longer.
    fn read(&self, path: &str, max_bytes: usize) -> Option<Vec<u8>>;
    /// Whether `path` is there.
    fn exists(&self, path: &str) -> bool;
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
