//! The fuzz of the readers: seed files (the synthetic ones, and sample files when a test passes them) cut short, bit-flipped and
//! with words overwritten by hostile counts, each read under `catch_unwind`. A case passes when the reader returns (a value or an
//! error) without a panic, its requests stay under [`MAX_ALLOC_BYTES`] and under [`MAX_ALLOC_PER_BYTE`] times the file's
//! size in each of [`MAX_READINGS`] readings, and a `.dif` that reads also passes or fails [`crate::dif::Interior::check`] without a panic.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use crate::{
    MAX_ALLOC_BYTES, MAX_ALLOC_PER_BYTE, MAX_READINGS, dif, dml, parse_requests,
    reset_parse_requests, synth, ter,
};

/// The kind of a seed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dif,
    Ter,
    Dml,
}

/// A seed file.
#[derive(Debug, Clone)]
pub struct Seed {
    pub kind: Kind,
    pub bytes: Vec<u8>,
}

/// The synthetic seeds: every interior version path (plain and later version 0, 5, 10 to 14), with and without the optional
/// parts, every legacy terrain version, and two material lists.
pub fn synthetic_seeds() -> Vec<Seed> {
    let mut out = Vec::new();
    for (version, plain_surfaces) in [
        (0, true),
        (0, false),
        (3, false),
        (5, false),
        (10, false),
        (12, false),
        (13, false),
        (14, false),
    ] {
        for (everything, border) in [(false, 0), (true, 1), (false, 2)] {
            out.push(Seed {
                kind: Kind::Dif,
                bytes: synth::dif(synth::DifSpec {
                    version,
                    plain_surfaces,
                    everything,
                    border,
                }),
            });
        }
    }
    for version in 1..=ter::NEWEST_VERSION {
        out.push(Seed {
            kind: Kind::Ter,
            bytes: synth::ter(version, &["a", "", "b", "c"]),
        });
    }
    out.push(Seed {
        kind: Kind::Dml,
        bytes: b"sky_0\r\nsky_1\r\nclouds/c1\r\n\r\nx".to_vec(),
    });
    out.push(Seed {
        kind: Kind::Dml,
        bytes: b"\x01\x02\x00\x00\x00\x01a\x02bc".to_vec(),
    });
    out
}

/// What a run did.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub cases: usize,
    pub read: usize,
    pub refused: usize,
    pub checked_ok: usize,
    pub checked_refused: usize,
    pub panics: usize,
    pub over_cap: usize,
    pub largest_request: usize,
    pub longest: Duration,
    pub first_failure: Option<String>,
}

impl Summary {
    pub fn clean(&self) -> bool {
        self.panics == 0 && self.over_cap == 0
    }

    pub fn line(&self) -> String {
        format!(
            "maps fuzz: {} cases, {} read ({} checked, {} refused by the check), {} refused, {} panics, {} over the cap, largest request {} bytes, longest case {} ms",
            self.cases,
            self.read,
            self.checked_ok,
            self.checked_refused,
            self.refused,
            self.panics,
            self.over_cap,
            self.largest_request,
            self.longest.as_millis()
        )
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// One mutation of `seed`: cut, flipped bits, a hostile word, or several of these.
fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut b = seed.to_vec();
    let steps = 1 + rng.below(3);
    for _ in 0..steps {
        if b.is_empty() {
            break;
        }
        match rng.below(4) {
            0 => {
                let n = rng.below(b.len());
                b.truncate(n);
            }
            1 => {
                for _ in 0..1 + rng.below(8) {
                    let i = rng.below(b.len());
                    b[i] ^= 1 << rng.below(8);
                }
            }
            2 => {
                let words = [
                    0xffff_ffffu32,
                    0x7fff_ffff,
                    0x8000_0000,
                    0x0100_0000,
                    0xffff,
                    0x10000,
                    2,
                    1,
                    0,
                ];
                let w = words[rng.below(words.len())].to_le_bytes();
                let i = rng.below(b.len());
                for (k, &x) in w.iter().enumerate() {
                    if let Some(v) = b.get_mut(i + k) {
                        *v = x;
                    }
                }
            }
            _ => {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
        }
    }
    b
}

/// What reading one file gave: None when refused, Some(check result) for a `.dif`, Some(true) for the others.
fn exercise(kind: Kind, bytes: &[u8]) -> Option<bool> {
    match kind {
        Kind::Dif => {
            let d = dif::parse_with_border_pad(bytes).ok()?;
            let mut ok = true;
            for i in d.details.iter().chain(&d.sub_objects) {
                ok &= i.check().is_ok();
                let _ = i.triangle_count();
            }
            // What the importer makes of it (the files it names, its surfaces), so that work tied to the contents, not the size,
            // shows in the longest case.
            use torque_mapcore::Importer as _;
            let _ = crate::legacy::IMPORTER.named_files(torque_mapcore::FileKind::Model, bytes);
            let _ = crate::legacy::IMPORTER.surfaces(bytes);
            Some(ok)
        }
        Kind::Ter => {
            let t = ter::parse(bytes).ok()?;
            for i in [0, 1, 255, 65_535, 70_000] {
                let _ = t.dominant(i);
                let _ = t.height(i, i);
            }
            Some(true)
        }
        Kind::Dml => dml::parse(bytes).ok().map(|_| true),
    }
}

/// Runs `cases` mutations over `seeds` (round-robin), the seeds themselves first.
pub fn run(seeds: &[Seed], cases: usize, seed: u64) -> Summary {
    let mut rng = Rng(seed | 1);
    let mut s = Summary::default();
    let total = cases.max(seeds.len());
    for c in 0..total {
        let Some(base) = seeds.get(c % seeds.len().max(1)) else {
            break;
        };
        let bytes = if c < seeds.len() {
            base.bytes.clone()
        } else {
            mutate(&mut rng, &base.bytes)
        };
        reset_parse_requests();
        let start = Instant::now();
        let got = catch_unwind(AssertUnwindSafe(|| exercise(base.kind, &bytes)));
        s.longest = s.longest.max(start.elapsed());
        s.cases += 1;
        let asked = parse_requests();
        if asked > MAX_READINGS * MAX_ALLOC_PER_BYTE * bytes.len() + 4_096
            || crate::largest_request() > MAX_ALLOC_BYTES
        {
            s.over_cap += 1;
            s.first_failure.get_or_insert_with(|| {
                format!(
                    "case {c} ({:?}, {} bytes) asked for {asked} bytes",
                    base.kind,
                    bytes.len()
                )
            });
        }
        match got {
            Err(_) => {
                s.panics += 1;
                s.first_failure.get_or_insert_with(|| {
                    format!(
                        "case {c} ({:?}, {} bytes) panicked, seed {seed}",
                        base.kind,
                        bytes.len()
                    )
                });
            }
            Ok(None) => s.refused += 1,
            Ok(Some(ok)) => {
                s.read += 1;
                if base.kind == Kind::Dif {
                    if ok {
                        s.checked_ok += 1;
                    } else {
                        s.checked_refused += 1;
                    }
                }
            }
        }
    }
    s.largest_request = crate::largest_request();
    s
}
