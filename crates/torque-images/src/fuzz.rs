//! The hostile-input fuzz: 50,000 seeded cases over both decoders, 0 panics. The seeds are
//! small files of every kind ([`crate::testenc`]'s baseline and progressive JPEGs, the `png` crate's PNGs of every colour type), and
//! the mutations are structure-aware where that reaches deeper: a PNG chunk's bytes changed and its CRC fixed (so IHDR, PLTE, tRNS
//! and the zlib data get past the CRC check), a JPEG marker segment's bytes changed (lengths, tables, frame sizes, sampling) or its
//! entropy-coded data; plus plain flips, cuts, insertions and repeats. The JPEG side runs under small [`Limits`] so that a mutated
//! frame size stays cheap; `TORQUE_FUZZ_IMAGES` sets the count.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::jpeg::{self, Limits};
use crate::testenc::{Enc, Rng, coarse_script, full_script};

/// The default number of cases.
const CASES: usize = 50_000;

fn png_seeds() -> Vec<Vec<u8>> {
    use ::png::{BitDepth as D, ColorType as C};
    let mut out = Vec::new();
    let mut add = |w: u32, h: u32, c: C, d: D, plte: Option<Vec<u8>>, trns: Option<Vec<u8>>| {
        let bits_per_px = match c {
            C::Grayscale | C::Indexed => 1,
            C::GrayscaleAlpha => 2,
            C::Rgb => 3,
            C::Rgba => 4,
        } * d as usize;
        let row = (w as usize * bits_per_px).div_ceil(8);
        let raw: Vec<u8> = (0..row * h as usize).map(|i| (i * 29 + 3) as u8).collect();
        let mut f = Vec::new();
        {
            let mut e = ::png::Encoder::new(&mut f, w, h);
            e.set_color(c);
            e.set_depth(d);
            if let Some(p) = plte {
                e.set_palette(p);
            }
            if let Some(t) = trns {
                e.set_trns(t);
            }
            let mut wr = e.write_header().expect("header");
            wr.write_image_data(&raw).expect("data");
        }
        out.push(f);
    };
    add(7, 5, C::Grayscale, D::One, None, None);
    add(7, 5, C::Grayscale, D::Four, None, Some(vec![0, 3]));
    add(7, 5, C::Grayscale, D::Eight, None, None);
    add(5, 3, C::Grayscale, D::Sixteen, None, Some(vec![1, 2]));
    add(6, 4, C::Rgb, D::Eight, None, Some(vec![0, 1, 0, 2, 0, 3]));
    add(3, 3, C::Rgb, D::Sixteen, None, None);
    add(4, 4, C::Rgba, D::Eight, None, None);
    add(4, 2, C::GrayscaleAlpha, D::Eight, None, None);
    add(
        9,
        3,
        C::Indexed,
        D::Two,
        Some((0..12).collect()),
        Some(vec![0, 99]),
    );
    add(8, 8, C::Indexed, D::Eight, Some((0..96).collect()), None);
    out
}

fn jpeg_seeds() -> Vec<Vec<u8>> {
    let ycc = Enc::random(17, 11, &[(2, 2), (1, 1), (1, 1)], 31);
    let mut rst = Enc::random(16, 16, &[(2, 1), (1, 1), (1, 1)], 32);
    rst.restart = 2;
    vec![
        ycc.baseline(&[vec![0, 1, 2]]),
        ycc.baseline(&[vec![0], vec![1, 2]]),
        ycc.progressive(&full_script(3)),
        ycc.progressive(&coarse_script(3)),
        rst.baseline(&[vec![0, 1, 2]]),
        rst.progressive(&full_script(3)),
        Enc::random(9, 13, &[(1, 1)], 33).baseline(&[vec![0]]),
        Enc::random(8, 8, &[(1, 2), (1, 1), (1, 1)], 34).baseline(&[vec![0, 1, 2]]),
    ]
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The PNG's chunks as (start, data length).
fn png_chunks(f: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 8;
    while at + 12 <= f.len() {
        let len = u32::from_be_bytes([f[at], f[at + 1], f[at + 2], f[at + 3]]) as usize;
        if at + 12 + len > f.len() {
            break;
        }
        out.push((at, len));
        at += 12 + len;
    }
    out
}

/// The JPEG's marker segments before the first scan, as (start, length), and where the first scan's data starts.
fn jpeg_segments(f: &[u8]) -> (Vec<(usize, usize)>, usize) {
    let mut out = Vec::new();
    let mut at = 2;
    while at + 4 <= f.len() && f[at] == 0xff {
        let len = usize::from(u16::from_be_bytes([f[at + 2], f[at + 3]]));
        out.push((at, len + 2));
        let marker = f[at + 1];
        at += 2 + len;
        if marker == 0xda {
            break;
        }
    }
    (out, at.min(f.len()))
}

fn generic(rng: &mut Rng, f: &mut Vec<u8>) {
    if f.is_empty() {
        f.push(rng.next() as u8);
        return;
    }
    match rng.range(5) {
        0 => {
            for _ in 0..1 + rng.range(8) {
                let i = rng.range(f.len() as u64) as usize;
                f[i] = rng.next() as u8;
            }
        }
        1 => {
            let n = rng.range(f.len() as u64) as usize;
            f.truncate(n);
        }
        2 => {
            let i = rng.range(f.len() as u64 + 1) as usize;
            let n = 1 + rng.range(16) as usize;
            let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
            f.splice(i..i, bytes);
        }
        3 => {
            let i = rng.range(f.len() as u64) as usize;
            let j = (i + 1 + rng.range(64) as usize).min(f.len());
            let piece = f[i..j].to_vec();
            let at = rng.range(f.len() as u64 + 1) as usize;
            f.splice(at..at, piece);
        }
        _ => {
            let i = rng.range(f.len() as u64) as usize;
            f[i] = [0x00, 0xff, 0x7f, 0x80, 0x01][rng.range(5) as usize];
        }
    }
}

fn mutate_png(rng: &mut Rng, f: &mut Vec<u8>) {
    let chunks = png_chunks(f);
    if rng.range(2) == 0 && !chunks.is_empty() {
        // Change bytes inside one chunk (its length field included, sometimes) and fix the CRC.
        let (at, len) = chunks[rng.range(chunks.len() as u64) as usize];
        if len > 0 {
            for _ in 0..1 + rng.range(4) {
                let i = at + 8 + rng.range(len as u64) as usize;
                f[i] = rng.next() as u8;
            }
        }
        if rng.range(8) == 0 {
            f[at + 3] = f[at + 3].wrapping_add(rng.next() as u8 % 5);
        }
        let crc = crc32(&f[at + 4..(at + 8 + len).min(f.len())]);
        if at + 12 + len <= f.len() {
            f[at + 8 + len..at + 12 + len].copy_from_slice(&crc.to_be_bytes());
        }
    } else {
        generic(rng, f);
    }
}

fn mutate_jpeg(rng: &mut Rng, f: &mut Vec<u8>) {
    let (segments, data) = jpeg_segments(f);
    match rng.range(3) {
        0 if !segments.is_empty() => {
            let (at, len) = segments[rng.range(segments.len() as u64) as usize];
            for _ in 0..1 + rng.range(3) {
                let i = at + 2 + rng.range(len.saturating_sub(2).max(1) as u64) as usize;
                if i < f.len() {
                    f[i] = rng.next() as u8;
                }
            }
        }
        1 if data < f.len() => {
            for _ in 0..1 + rng.range(6) {
                let i = data + rng.range((f.len() - data) as u64) as usize;
                f[i] = match rng.range(4) {
                    0 => 0xff,
                    1 => 0xd0 + rng.range(8) as u8,
                    _ => rng.next() as u8,
                };
            }
        }
        _ => generic(rng, f),
    }
}

#[test]
fn hostile_images_never_panic() {
    let cases = std::env::var("TORQUE_FUZZ_IMAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CASES);
    let pngs = png_seeds();
    let jpegs = jpeg_seeds();
    let limits = Limits {
        max_dimension: 4096,
        max_pixels: 4096,
        max_scans: 64,
    };
    let mut rng = Rng(0x00c0_ffee_1234_5678);
    let (mut ok, mut refused) = (0usize, 0usize);
    let mut panics = Vec::new();
    for case in 0..cases {
        let is_png = case % 2 == 0;
        let seeds = if is_png { &pngs } else { &jpegs };
        let mut f = seeds[rng.range(seeds.len() as u64) as usize].clone();
        for _ in 0..1 + rng.range(3) {
            if is_png {
                mutate_png(&mut rng, &mut f);
            } else {
                mutate_jpeg(&mut rng, &mut f);
            }
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            if is_png {
                crate::png::decode(&f).is_ok()
            } else {
                jpeg::decode_with(&f, &limits).is_ok()
            }
        }));
        match result {
            Ok(true) => ok += 1,
            Ok(false) => refused += 1,
            Err(_) => panics.push(case),
        }
    }
    eprintln!(
        "IMAGES-FUZZ {cases} cases: {ok} decoded, {refused} refused, {} panics",
        panics.len()
    );
    assert!(
        panics.is_empty(),
        "cases that panicked: {:?}",
        &panics[..panics.len().min(20)]
    );
}
