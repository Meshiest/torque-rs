//! The byte buckets that pace requests.

use super::Millis;
use crate::blocks::{BLOCK_BYTES, MAX_OUTSTANDING_RUNS, MAX_RUN_BLOCKS};

/// The bytes of the largest run.
pub(super) const RUN_BYTES: u64 = MAX_RUN_BLOCKS as u64 * BLOCK_BYTES as u64;

/// A byte bucket in milli-bytes, so a slow rate does not lose its remainders.
pub(super) struct Bucket {
    rate: u64,
    cap: u64,
    tokens: u64,
    at: Millis,
}

impl Bucket {
    /// A full bucket of `rate` bytes a second that holds at least the runs that may be out at once.
    pub(super) fn new(rate: u64, now: Millis) -> Bucket {
        let cap = rate
            .max(RUN_BYTES * MAX_OUTSTANDING_RUNS as u64)
            .saturating_mul(1_000);
        Bucket {
            rate,
            cap,
            tokens: cap,
            at: now,
        }
    }

    pub(super) fn refill(&mut self, now: Millis) {
        let elapsed = now.saturating_sub(self.at);
        self.at = self.at.max(now);
        self.tokens = self
            .tokens
            .saturating_add(elapsed.saturating_mul(self.rate))
            .min(self.cap);
    }

    pub(super) fn has(&self, bytes: u64) -> bool {
        self.tokens >= bytes.saturating_mul(1_000)
    }

    pub(super) fn take(&mut self, bytes: u64) {
        self.tokens = self.tokens.saturating_sub(bytes.saturating_mul(1_000));
    }

    /// Gives back bytes that were taken for a request that was never sent.
    pub(super) fn refund(&mut self, bytes: u64) {
        self.tokens = self
            .tokens
            .saturating_add(bytes.saturating_mul(1_000))
            .min(self.cap);
    }
}
