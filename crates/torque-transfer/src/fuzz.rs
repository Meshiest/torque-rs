//! The harness for fuzzing what a sink does with an upload: a seeded generator, a mutator that changes a valid sample's sizes,
//! counts and offsets, and a runner that counts panics.
//!
//! Every sink of an upload kind ships a test that runs [`fuzz_sink`] over a valid sample of its content with the cases
//! [`cases_from_env`] reads (`TORQUE_FUZZ_UPLOAD_<NAME>=100000`; a small default so `cargo test` stays quick):
//!
//! ```
//! use torque_transfer::fuzz::{cases_from_env, fuzz_sink};
//!
//! let sample = [3u8, 0, 0, 0, b'a', b'b', b'c'];
//! // The parser under test: a length-prefixed name that must never panic.
//! let parse = |bytes: &[u8]| {
//!     let len = bytes.first().copied().map_or(0, usize::from);
//!     let _ = bytes.get(4..4 + len);
//! };
//! let report = fuzz_sink("DOC", &sample, cases_from_env("TORQUE_FUZZ_UPLOAD_DOC", 200), parse);
//! assert_eq!(report.panics, 0);
//! ```

use std::panic::{AssertUnwindSafe, catch_unwind};

/// A small seeded generator (xorshift), so a failing case repeats.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator for `seed`.
    pub fn new(seed: u64) -> Rng {
        // One round of splitmix64, so nearby seeds give unrelated streams.
        let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Rng((z ^ (z >> 31)) | 1)
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() >> 32) as usize) % n.max(1)
    }

    /// True `percent` times in 100.
    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    /// Between 0 and `max` - 1 random bytes.
    pub fn some_bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max);
        self.bytes(n)
    }

    /// `n` random bytes.
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next_u64() as u8).collect()
    }
}

/// The values a count, size or offset field is changed to: the ones that break arithmetic.
const EDGES: [u32; 10] = [
    0,
    1,
    2,
    0x7f,
    0x80,
    0xff,
    0xffff,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
];

/// A mutant of `sample`: some bytes flipped, a 1, 2 or 4 byte field set to an edge value, a range cut, repeated or zeroed, or
/// the tail truncated or extended.
pub fn mutate(sample: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut out = sample.to_vec();
    for _ in 0..=rng.below(3) {
        if out.is_empty() {
            let n = rng.below(16);
            out.extend(rng.bytes(n));
            continue;
        }
        let at = rng.below(out.len());
        match rng.below(7) {
            0 => out[at] ^= 1 << rng.below(8),
            1 => out[at] = rng.next_u64() as u8,
            2 => {
                let edge = EDGES[rng.below(EDGES.len())].to_le_bytes();
                let width = [1, 2, 4][rng.below(3)];
                for (k, byte) in edge.iter().take(width).enumerate() {
                    if let Some(slot) = out.get_mut(at + k) {
                        *slot = *byte;
                    }
                }
            }
            3 => out.truncate(at),
            4 => {
                let end = (at + rng.below(32)).min(out.len());
                out.drain(at..end);
            }
            5 => {
                let end = (at + rng.below(32)).min(out.len());
                let part = out[at..end].to_vec();
                out.splice(at..at, part);
            }
            _ => {
                let end = (at + rng.below(32)).min(out.len());
                out[at..end].fill(0);
            }
        }
    }
    out
}

/// What [`fuzz_sink`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuzzReport {
    /// Cases run.
    pub cases: u32,
    /// Cases whose parser panicked.
    pub panics: u32,
}

/// Runs `parse` over `cases` mutants of `sample` (and over random bytes now and then) and counts the panics. The name seeds the
/// generator, so each sink's run is its own and repeats.
pub fn fuzz_sink(name: &str, sample: &[u8], cases: u32, parse: impl Fn(&[u8])) -> FuzzReport {
    let seed = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    let mut rng = Rng::new(seed);
    let mut panics = 0;
    for case in 0..cases {
        let input = if case % 16 == 15 {
            {
                let n = rng.below(sample.len().max(8) * 2);
                rng.bytes(n)
            }
        } else {
            mutate(sample, &mut rng)
        };
        if catch_unwind(AssertUnwindSafe(|| parse(&input))).is_err() {
            panics += 1;
        }
    }
    FuzzReport { cases, panics }
}

/// The number of cases the environment variable `var` asks for, else `default`.
pub fn cases_from_env(var: &str, default: u32) -> u32 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
#[path = "fuzz_tests.rs"]
mod tests;
