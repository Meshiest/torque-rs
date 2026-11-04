//! The hostile sound-file fuzz: 50,000 seeded cases, WAV and Ogg, 0 panics. The seeds are WAV files of every shape the reader meets
//! (8 and 16 bits, mono and stereo, an extended `fmt `, extra chunks, `smpl`, two `data` chunks, a non-PCM format) and Ogg streams (an
//! identification header with and without the rest, pages of several packets), plus, when `TORQUE_SAMPLE_DIR` names a directory of
//! real sound files (read only), the smallest WAV and Ogg files found below it. The mutations are structure-aware where that reaches
//! deeper (a chunk's size, a `fmt ` field, an Ogg page's granule, segment count or a packet byte with the page's CRC fixed) plus
//! flips, cuts, insertions and repeats. Every file that reads is also measured (`alxGetWaveLen`) and mixed for a few hundred frames.
//! `TORQUE_FUZZ_AUDIO` sets the count; `TORQUE_FUZZ_AUDIO_TRACE=<file>` writes each case to that file (and its number to stderr)
//! before it runs, so a case that aborts the process is left behind (a failed allocation aborts and is not a panic this test can
//! catch).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use crate::buffer::{Buffer, decode};
use crate::ogg::tests::{crc, ident, page};
use crate::system::{Audio, Description};
use crate::wav::tests::{fmt, pcm_wav, riff};

const CASES: usize = 50_000;
/// The most bytes of a sample file a seed keeps.
const SAMPLE_SEED_BYTES: usize = 48 << 10;
/// How deep below `TORQUE_SAMPLE_DIR` sound files are looked for.
const SAMPLE_DEPTH: usize = 6;

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

fn wav_seeds() -> Vec<Vec<u8>> {
    let mut ext = fmt(1, 1, 22_050, 16);
    ext.extend_from_slice(&[0, 0]);
    vec![
        pcm_wav(1, 22_050, 16, 300),
        pcm_wav(1, 11_025, 8, 200),
        pcm_wav(2, 44_100, 16, 100),
        pcm_wav(2, 8_000, 8, 50),
        riff(&[
            (b"fmt ", ext),
            (b"LIST", vec![7; 9]),
            (b"data", vec![1; 40]),
        ]),
        riff(&[
            (b"fmt ", fmt(1, 1, 22_050, 16)),
            (b"smpl", vec![0; 60]),
            (b"data", vec![2; 20]),
        ]),
        riff(&[
            (b"fmt ", fmt(1, 1, 22_050, 16)),
            (b"data", vec![3; 10]),
            (b"smpl", vec![0; 8]),
        ]),
        riff(&[
            (b"fmt ", fmt(1, 1, 22_050, 16)),
            (b"data", vec![4; 10]),
            (b"data", vec![5; 12]),
        ]),
        riff(&[(b"fmt ", fmt(0x11, 1, 22_050, 4)), (b"data", vec![6; 64])]),
    ]
}

fn ogg_seeds() -> Vec<Vec<u8>> {
    let comment: Vec<u8> = {
        let mut c = vec![3];
        c.extend_from_slice(b"vorbis");
        c.extend_from_slice(&4u32.to_le_bytes());
        c.extend_from_slice(b"test");
        c.extend_from_slice(&0u32.to_le_bytes());
        c.push(1);
        c
    };
    let mut setup = vec![5];
    setup.extend_from_slice(b"vorbis");
    setup.extend_from_slice(&[0, 0x42, 0x43, 0x56, 1, 0, 1, 0, 0]);
    let mut a = page(9, 0, 0, 2, &[&ident(1, 22_050)]);
    a.extend(page(9, 1, 0, 0, &[&comment, &setup]));
    a.extend(page(9, 2, 1_024, 4, &[&[0, 1, 2, 3]]));
    let mut b = page(3, 0, 0, 2, &[&ident(2, 44_100)]);
    b.extend(page(3, 1, 0, 0, &[&comment]));
    vec![a, b, page(1, 0, 0, 2, &[&ident(6, 8_000)])]
}

/// Every file below `dir` (at most `depth` levels down) with its size.
fn files_below(dir: &std::path::Path, depth: usize, out: &mut Vec<(u64, std::path::PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else {
            continue;
        };
        if meta.is_dir() {
            if depth > 0 {
                files_below(&e.path(), depth - 1, out);
            }
        } else {
            out.push((meta.len(), e.path()));
        }
    }
}

/// Sound files below `TORQUE_SAMPLE_DIR` (read only), when it is set: at most 16 of each kind, the smallest.
fn sample_seeds() -> Vec<(String, Vec<u8>)> {
    let Some(root) = std::env::var_os("TORQUE_SAMPLE_DIR").map(std::path::PathBuf::from) else {
        return Vec::new();
    };
    let mut all = Vec::new();
    files_below(&root, SAMPLE_DEPTH, &mut all);
    let mut out = Vec::new();
    for ext in ["wav", "ogg"] {
        let mut files: Vec<_> = all
            .iter()
            .filter(|(_, p)| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
            .collect();
        files.sort();
        for (_, p) in files.into_iter().take(16) {
            if let Ok(mut b) = std::fs::read(p) {
                // The headers and the first seconds: the decoder's paths without minutes of music a case.
                b.truncate(SAMPLE_SEED_BYTES);
                out.push((format!("x.{ext}"), b));
            }
        }
    }
    out
}

/// Refixes every Ogg page's CRC in `b` (so a structural change reaches the decoder instead of its CRC check).
fn fix_pages(b: &mut [u8]) {
    let mut at = 0;
    while at + 27 <= b.len() {
        if &b[at..at + 4] != b"OggS" {
            at += 1;
            continue;
        }
        let n = usize::from(b[at + 26]);
        if at + 27 + n > b.len() {
            return;
        }
        let body: usize = b[at + 27..at + 27 + n]
            .iter()
            .map(|x| usize::from(*x))
            .sum();
        let end = (at + 27 + n + body).min(b.len());
        b[at + 22..at + 26].copy_from_slice(&[0; 4]);
        let c = crc(&b[at..end]);
        b[at + 22..at + 26].copy_from_slice(&c.to_le_bytes());
        at = end;
    }
}

fn mutate(rng: &mut Rng, seed: &[u8], ogg: bool) -> Vec<u8> {
    let mut b = seed.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        let len = b.len().max(1);
        match rng.below(9) {
            0 => {
                let i = rng.below(len);
                if let Some(x) = b.get_mut(i) {
                    *x ^= 1 << rng.below(8);
                }
            }
            1 => {
                let i = rng.below(len);
                if let Some(x) = b.get_mut(i) {
                    *x = rng.next() as u8;
                }
            }
            2 => b.truncate(rng.below(len + 1)),
            3 => {
                let i = rng.below(len + 1).min(b.len());
                let n = rng.below(64);
                let ins: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                b.splice(i..i, ins);
            }
            4 => {
                let i = rng.below(len).min(b.len());
                let n = rng.below(256).min(b.len() - i);
                let piece = b[i..i + n].to_vec();
                b.extend_from_slice(&piece);
            }
            5 => {
                // A 32-bit field (a size, a rate, a granule half) set to an edge.
                let i = rng.below(len.saturating_sub(4) + 1);
                let v: u32 = match rng.below(6) {
                    0 => 0,
                    1 => u32::MAX,
                    2 => 0x7fff_ffff,
                    3 => 1,
                    4 => rng.next() as u32,
                    _ => (rng.below(1 << 20)) as u32,
                };
                if i + 4 <= b.len() {
                    b[i..i + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            6 => {
                // A 16-bit field (channels, bits, format).
                let i = rng.below(len.saturating_sub(2) + 1);
                let v: u16 = [0, 1, 2, 3, 8, 16, 24, 0xffff][rng.below(8)];
                if i + 2 <= b.len() {
                    b[i..i + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
            7 => {
                let i = rng.below(len);
                let n = rng.below(len - i.min(len) + 1);
                if i < b.len() {
                    b.drain(i..(i + n).min(b.len()));
                }
            }
            _ => {
                let id: &[u8; 4] = [b"fmt ", b"data", b"smpl", b"OggS", b"LIST"][rng.below(5)];
                let i = rng.below(len.saturating_sub(4) + 1);
                if i + 4 <= b.len() {
                    b[i..i + 4].copy_from_slice(id);
                }
            }
        }
    }
    if ogg && rng.below(2) == 0 {
        fix_pages(&mut b);
    }
    b
}

/// Whether the file read; `caught` counts the decoder's own panics the Ogg reader caught.
fn exercise(name: &str, bytes: &[u8], caught: &mut usize) -> bool {
    let mut messages = Vec::new();
    let result = decode(name.as_bytes(), bytes, &crate::wav::TgeWav, &mut messages);
    if matches!(&result, Err(e) if e == &crate::ogg::OggError::DecoderPanic.to_string()) {
        *caught += 1;
    }
    match result {
        Ok(pcm) => {
            let b = Arc::new(Buffer {
                name: name.as_bytes().to_vec(),
                pcm,
            });
            let _ = b.wave_len_ms();
            let mut a = Audio::default();
            a.init();
            let d = Description {
                is_looping: b.pcm.frames().is_multiple_of(2),
                ..Description::default()
            };
            let h = a.create_source(&d, Some(b), None, 0);
            a.play(h);
            let mut out = vec![0f32; 512];
            a.render(&mut out, 2, 44_100);
            a.advance(50.0);
            a.update();
            assert!(out.iter().all(|x| x.is_finite()));
            true
        }
        Err(_) => false,
    }
}

#[test]
fn hostile_sound_files_never_panic() {
    let cases = std::env::var("TORQUE_FUZZ_AUDIO")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(CASES);
    let mut seeds: Vec<(String, Vec<u8>)> = wav_seeds()
        .into_iter()
        .map(|b| ("x.wav".to_string(), b))
        .collect();
    seeds.extend(ogg_seeds().into_iter().map(|b| ("x.ogg".to_string(), b)));
    let from_samples = sample_seeds();
    let sampled = from_samples.len();
    seeds.extend(from_samples);
    let trace = std::env::var_os("TORQUE_FUZZ_AUDIO_TRACE");
    let mut rng = Rng(0x00a0_d10f_u64 | 1);
    let (mut panics, mut read, mut ogg_cases, mut caught) = (0usize, 0usize, 0usize, 0usize);
    for i in 0..cases {
        let (name, seed) = &seeds[i % seeds.len()];
        let ogg = name.ends_with(".ogg");
        ogg_cases += usize::from(ogg);
        let bytes = mutate(&mut rng, seed, ogg);
        if let Some(t) = &trace {
            // The case about to run (a decoder that aborts leaves it behind: `TORQUE_FUZZ_AUDIO_TRACE=<file>`).
            let _ = std::fs::write(t, &bytes);
            eprintln!("audio fuzz case {i} {name} ({} bytes)", bytes.len());
        }
        match catch_unwind(AssertUnwindSafe(|| exercise(name, &bytes, &mut caught))) {
            Ok(true) => read += 1,
            Ok(false) => {}
            Err(_) => panics += 1,
        }
    }
    eprintln!(
        "audio fuzz: {cases} cases ({ogg_cases} ogg; {sampled} sample seeds), {panics} panics, {read} read, {caught} decoder panics caught"
    );
    assert_eq!(panics, 0);
}
