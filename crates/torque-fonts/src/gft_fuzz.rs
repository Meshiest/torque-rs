//! The hostile `.gft` fuzz: 50,000 seeded cases, 0 panics. The seeds are small fonts of every
//! shape the reader meets (one and two sheets, no sheet, glyphs on a missing sheet, a remap past the table); the mutations are
//! structure-aware where that reaches deeper (the counts, the glyph table's fields, a sheet's PNG chunk changed with its CRC fixed, a
//! remap entry) plus plain flips, cuts, insertions and repeats. Every font that parses is also measured and laid out with every byte.
//! `TORQUE_FUZZ_FONTS` sets the count.

use std::panic::{AssertUnwindSafe, catch_unwind};

use torque_bytes::crc32;

use super::parse_gft;
use super::tests::{gft, grey_png, sample};
use crate::{Glyph, TextColors};

const CASES: usize = 50_000;

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

fn seeds() -> Vec<Vec<u8>> {
    let mut remap = [-1i16; 256];
    for (i, r) in remap.iter_mut().enumerate().skip(32).take(8) {
        *r = (i - 32) as i16;
    }
    let glyphs: Vec<Glyph> = (0..8)
        .map(|i| Glyph {
            bitmap_index: (i % 3) as i16,
            x_offset: (i * 3) as u8,
            y_offset: 1,
            width: 3,
            height: (i % 4) as u8,
            x_origin: -1,
            y_origin: 6,
            x_increment: 4,
        })
        .collect();
    vec![
        sample(),
        gft(
            12,
            9,
            &glyphs,
            &[grey_png(32, 8, |x, y| (x ^ y) as u8)],
            &remap,
        ),
        gft(12, 9, &glyphs, &[], &remap),
        gft(0, 0, &[], &[grey_png(1, 1, |_, _| 7)], &[-1; 256]),
        gft(
            u32::MAX,
            u32::MAX,
            &glyphs,
            &[grey_png(4, 4, |_, _| 1), grey_png(2, 2, |_, _| 3)],
            &[i16::MAX; 256],
        ),
    ]
}

/// Changes one byte inside a PNG chunk of the file and fixes that chunk's CRC.
fn mutate_png_chunk(f: &mut [u8], rng: &mut Rng) {
    let starts: Vec<usize> = f
        .windows(8)
        .enumerate()
        .filter(|(_, w)| *w == [137, 80, 78, 71, 13, 10, 26, 10])
        .map(|(i, _)| i)
        .collect();
    if starts.is_empty() {
        return;
    }
    let mut at = starts[rng.below(starts.len())] + 8;
    let mut chunks = Vec::new();
    while let Some(len) = f.get(at..at + 4) {
        let len = u32::from_be_bytes([len[0], len[1], len[2], len[3]]) as usize;
        let end = at + 12 + len;
        if end > f.len() {
            break;
        }
        chunks.push((at, len));
        if &f[at + 4..at + 8] == b"IEND" {
            break;
        }
        at = end;
    }
    let Some(&(at, len)) = chunks.get(rng.below(chunks.len())) else {
        return;
    };
    if len > 0 {
        let i = at + 8 + rng.below(len);
        f[i] = rng.next() as u8;
    }
    let crc = crc32(&f[at + 4..at + 8 + len]);
    f[at + 8 + len..at + 12 + len].copy_from_slice(&crc.to_be_bytes());
}

fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut f = seed.to_vec();
    for _ in 0..1 + rng.below(3) {
        match rng.below(9) {
            0 => {
                if !f.is_empty() {
                    let i = rng.below(f.len());
                    f[i] ^= 1 << rng.below(8);
                }
            }
            1 => {
                let n = rng.below(f.len() + 1);
                f.truncate(n);
            }
            2 => {
                let i = rng.below(f.len() + 1);
                let n = 1 + rng.below(16);
                let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                f.splice(i..i, bytes);
            }
            3 => {
                // A count or size field (height, baseline, glyph count) set to an edge value.
                let field = [4usize, 8, 12][rng.below(3)];
                let v: u32 = [0, 1, 255, 256, 4096, 4097, u32::MAX, 0x7fff_ffff][rng.below(8)];
                if f.len() >= field + 4 {
                    f[field..field + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            4 => mutate_png_chunk(&mut f, rng),
            5 => {
                // A glyph-table byte (sheet index, offsets, sizes, origins).
                if f.len() > 16 {
                    let i = 16 + rng.below((f.len() - 16).min(9 * 16));
                    f[i] = [0, 1, 0x7f, 0x80, 0xff][rng.below(5)];
                }
            }
            6 => {
                // A remap entry (the last 512 bytes).
                if f.len() >= 512 {
                    let i = f.len() - 512 + 2 * rng.below(256);
                    let v: i16 = [-1, 0, 1, 7, 255, i16::MAX, i16::MIN][rng.below(7)];
                    f[i..i + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
            7 => {
                // A repeated stretch.
                if !f.is_empty() {
                    let a = rng.below(f.len());
                    let b = (a + 1 + rng.below(64)).min(f.len());
                    let s = f[a..b].to_vec();
                    f.splice(b..b, s);
                }
            }
            _ => {
                // The sheet count.
                let at = f
                    .windows(8)
                    .position(|w| w == [137, 80, 78, 71, 13, 10, 26, 10])
                    .unwrap_or(f.len().min(16));
                if at >= 4 {
                    let v: u32 = [0, 1, 2, 3, 256, 257, u32::MAX][rng.below(7)];
                    f[at - 4..at].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
    }
    f
}

fn exercise(bytes: &[u8]) {
    if let Ok(font) = parse_gft(bytes) {
        let text: Vec<u8> = (0..=255u8).collect();
        let _ = font.str_width(&text);
        let table = [[1, 2, 3, 4]; 10];
        let mut colors = TextColors::new([255; 4], [0; 4]);
        let l = font.layout(
            (i32::MAX - 5, i32::MIN + 5),
            &text,
            &mut colors,
            Some(&table),
            9,
        );
        assert!(l.quads.iter().all(|q| q.sheet < font.sheets.len()));
    }
}

#[test]
fn hostile_font_files_never_panic() {
    let cases = std::env::var("TORQUE_FUZZ_FONTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CASES);
    let seeds = seeds();
    for s in &seeds {
        assert!(parse_gft(s).is_ok(), "a seed does not parse");
    }
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut panics, mut parsed) = (0usize, 0usize);
    for case in 0..cases {
        let f = mutate(&seeds[case % seeds.len()], &mut rng);
        match catch_unwind(AssertUnwindSafe(|| exercise(&f))) {
            Ok(()) => parsed += usize::from(parse_gft(&f).is_ok() && case % 64 == 0),
            Err(_) => {
                panics += 1;
                eprintln!("case {case} panicked ({} bytes)", f.len());
            }
        }
    }
    eprintln!("fonts fuzz: {cases} cases, {panics} panics ({parsed} sampled cases parsed)");
    assert_eq!(panics, 0);
}
