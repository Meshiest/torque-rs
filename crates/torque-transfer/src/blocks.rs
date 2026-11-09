//! Blocks: the unit a transfer is requested, hashed and resumed in.
//!
//! A content of `size` bytes is cut into blocks of [`BLOCK_BYTES`] (the last one shorter). Each block has a hash
//! ([`block_hash`]: SHA-256 cut to [`BLOCK_HASH_LEN`] bytes) that the receiver checks before it keeps a byte, and the whole
//! content has a SHA-256 ([`content_hash`]) the receiver checks once every block is in. The mods download uses the same
//! blocks.

use sha2::{Digest, Sha256};

/// The size of a block.
pub const BLOCK_BYTES: usize = 64 << 10;

/// The bytes of a block's hash.
pub const BLOCK_HASH_LEN: usize = 16;

/// The most blocks one request asks for.
pub const MAX_RUN_BLOCKS: u32 = 4;

/// The most runs a receiver has requested and not yet been sent at once.
pub const MAX_OUTSTANDING_RUNS: usize = 2;

/// A block's hash.
pub type BlockHash = [u8; BLOCK_HASH_LEN];

/// The SHA-256 of a whole content.
pub type Hash = [u8; 32];

/// The hash of a block: SHA-256 cut short.
pub fn block_hash(data: &[u8]) -> BlockHash {
    let full: Hash = Sha256::digest(data).into();
    let mut out = [0u8; BLOCK_HASH_LEN];
    out.copy_from_slice(&full[..BLOCK_HASH_LEN]);
    out
}

/// The SHA-256 of a whole content.
pub fn content_hash(data: &[u8]) -> Hash {
    Sha256::digest(data).into()
}

/// The hash of each block of `data`.
pub fn block_hashes(data: &[u8]) -> Vec<BlockHash> {
    data.chunks(BLOCK_BYTES).map(block_hash).collect()
}

/// How many blocks a content of `size` bytes has.
pub fn block_count(size: u64) -> u64 {
    size.div_ceil(BLOCK_BYTES as u64)
}

/// The bytes of blocks `start..start + count` of a content of `size` bytes: from the first block's start to the run's end or the
/// content's, whichever is first. Saturates, so hostile numbers give an empty or short range, never an overflow.
pub fn run_bytes(size: u64, start: u64, count: u64) -> (u64, u64) {
    let from = start.saturating_mul(BLOCK_BYTES as u64).min(size);
    let to = start
        .saturating_add(count)
        .saturating_mul(BLOCK_BYTES as u64)
        .min(size);
    (from, to)
}

#[cfg(test)]
#[path = "blocks_tests.rs"]
mod tests;
