//! The byte buckets that pace requests.

use torque_net::fair::TokenBucket;

use super::Millis;
use crate::blocks::{BLOCK_BYTES, MAX_OUTSTANDING_RUNS, MAX_RUN_BLOCKS};

/// The bytes of the largest run.
const RUN_BYTES: u64 = MAX_RUN_BLOCKS as u64 * BLOCK_BYTES as u64;

/// A byte bucket on the server's clock (milliseconds that only grow).
pub(super) type Bucket = TokenBucket<Millis>;

/// A full bucket at `now` of `rate` bytes a second that holds at least the runs that may be out at once.
pub(super) fn bucket(rate: u64, now: Millis) -> Bucket {
    Bucket::starting_at(rate, rate.max(RUN_BYTES * MAX_OUTSTANDING_RUNS as u64), now)
}
