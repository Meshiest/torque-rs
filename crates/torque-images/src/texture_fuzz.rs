//! The hostile-input fuzz of the material formats: KTX 2.0 files (every format, stored and Zstandard, their blocks decoded
//! when they read) and OpenEXR files (scan lines and tiles, every lossless compression), mutated by flips, byte sets, cuts,
//! insertions, repeats and header words set to edge values. Each reader returns a value or an error; a panic is counted and
//! fails the test. `TORQUE_FUZZ_TEXTURES` sets the count of each (2,000 by default; `just texture-verify` runs 300,000).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use exr::prelude::{Compression, FlatSamples};

use crate::ktx2::{self, Ktx2Format, Limits, Supercompression};
use crate::testenc::Rng;

const CASES: usize = 2_000;

trait Pick {
    fn below(&mut self, n: usize) -> usize;
    fn byte(&mut self) -> u8;
}

impl Pick for Rng {
    fn below(&mut self, n: usize) -> usize {
        self.range(n.max(1) as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

fn cases() -> usize {
    std::env::var("TORQUE_FUZZ_TEXTURES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CASES)
}

fn ktx2_seeds() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for format in Ktx2Format::ALL {
        for sc in [Supercompression::None, Supercompression::Zstandard] {
            for (w, h) in [(1, 1), (8, 8), (12, 4)] {
                let t = crate::ktx2::tests::chain(format, w, h);
                out.push(ktx2::write(&t, sc).expect("a seed writes"));
            }
        }
    }
    out
}

fn exr_seeds() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let vals: Vec<f32> = (0..96).map(|i| i as f32 / 95.0).collect();
    for c in [
        Compression::Uncompressed,
        Compression::RLE,
        Compression::ZIP1,
        Compression::ZIP16,
        Compression::PIZ,
    ] {
        for tile in [None, Some(4)] {
            out.push(crate::exr::tests::file(
                "Y",
                12,
                8,
                FlatSamples::F32(vals.clone()),
                c,
                tile,
            ));
            out.push(crate::exr::tests::file(
                "R",
                12,
                8,
                FlatSamples::F16(
                    vals.iter()
                        .map(|&v| exr::prelude::f16::from_f32(v))
                        .collect(),
                ),
                c,
                tile,
            ));
        }
    }
    out
}

/// A copy of `seed` with one to four mutations.
fn mutate(seed: &[u8], rng: &mut Rng, header_words: usize) -> Vec<u8> {
    let mut f = seed.to_vec();
    for _ in 0..1 + rng.below(4) {
        if f.is_empty() {
            break;
        }
        let at = rng.below(f.len());
        match rng.below(9) {
            0 => f[at] ^= 1 << rng.below(8),
            1 => f[at] = rng.byte(),
            2 if rng.below(2) == 0 => f.truncate(at),
            2 | 7 => f[at] = f[at].wrapping_add(1 + rng.byte() % 3),
            3 => f.insert(at, rng.byte()),
            4 => {
                let end = (at + 1 + rng.below(16)).min(f.len());
                let piece = f[at..end].to_vec();
                f.splice(at..at, piece);
            }
            5 => {
                // A header or index word (the first `header_words`) set to an edge value.
                let word = 12 + 4 * rng.below(header_words);
                let edges = [
                    0u32,
                    1,
                    2,
                    3,
                    4,
                    16,
                    255,
                    4096,
                    4097,
                    65_535,
                    1 << 31,
                    u32::MAX,
                    u32::MAX - 3,
                ];
                if word + 4 <= f.len() {
                    f[word..word + 4].copy_from_slice(&edges[rng.below(edges.len())].to_le_bytes());
                }
            }
            _ => {
                let end = (at + 1 + rng.below(8)).min(f.len());
                for b in &mut f[at..end] {
                    *b = rng.byte();
                }
            }
        }
    }
    f
}

/// What a run found: its panics, the cases that read (a mutation the format still takes), and the slowest case.
struct Found {
    panics: usize,
    read: usize,
    slowest: Duration,
}

/// Runs `cases` mutations of `seeds` through `read` (true when the file read).
fn run(
    seeds: &[Vec<u8>],
    cases: usize,
    header_words: usize,
    seed: u64,
    read: impl Fn(&[u8]) -> bool,
) -> Found {
    let mut rng = Rng(seed | 1);
    let mut found = Found {
        panics: 0,
        read: 0,
        slowest: Duration::ZERO,
    };
    for _ in 0..cases {
        let s = &seeds[rng.below(seeds.len())];
        let f = mutate(s, &mut rng, header_words);
        let t = Instant::now();
        match catch_unwind(AssertUnwindSafe(|| read(&f))) {
            Ok(true) => found.read += 1,
            Ok(false) => {}
            Err(_) => found.panics += 1,
        }
        found.slowest = found.slowest.max(t.elapsed());
    }
    found
}

#[test]
fn hostile_ktx2_files_never_panic() {
    let seeds = ktx2_seeds();
    let n = cases();
    let found = run(&seeds, n, 39, 0x6b74_7832, |f| {
        let Ok(t) = ktx2::read(f, &Limits::default()) else {
            return false;
        };
        if let Some(block) = t.format.block() {
            for (k, l) in t.levels.iter().enumerate() {
                let (w, h) = ((t.width >> k).max(1), (t.height >> k).max(1));
                crate::bc::decode(block, l, w, h).expect("a level read is its size");
            }
        }
        true
    });
    eprintln!(
        "ktx2 fuzz: {n} cases, {} read, {} panics, slowest {:?}",
        found.read, found.panics, found.slowest
    );
    assert_eq!(found.panics, 0);
}

#[test]
fn hostile_exr_files_never_panic() {
    let seeds = exr_seeds();
    let n = cases();
    let found = run(&seeds, n, 8, 0x6578_7231, |f| {
        crate::exr::read_r16(f, 4096).is_ok()
    });
    eprintln!(
        "exr fuzz: {n} cases, {} read, {} panics, slowest {:?}",
        found.read, found.panics, found.slowest
    );
    assert_eq!(found.panics, 0);
}
