//! The hostile-input fuzz: seeded mutations of small synthetic files of every format (none from a game), each given to its
//! reader under `catch_unwind`; 0 panics. Mutations are byte flips, cuts, insertions, repeats, 32-bit fields set to edge values,
//! and overlapping offsets: a table's count and offset copied over another's (or every offset of a structure pointed at one
//! place), so tables read the same bytes many times. `TORQUE_FUZZ_VALVE` sets the count (20,000 by default).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use crate::{bsp, kv, mdl, model, pcf, sound, synth, vmt, vpk, vtf, vtx, vvd};

const CASES: usize = 20_000;

/// The longest any one case may take, in a debug build: a reader that works in time tied to its input takes milliseconds, so a
/// case near this is a path whose work grows faster than the input.
const SLOWEST_CASE: Duration = Duration::from_secs(10);

/// The seed kind of the synthetic MP3 files, which get frame-aware mutations.
const MP3_KIND: usize = 11;

/// A seeded xorshift generator.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

const EDGES: [i32; 10] = [0, 1, -1, 2, 255, 256, 0x7fff, 0x10000, i32::MAX, i32::MIN];

/// One mutation of `seed`.
pub fn mutate(seed: &[u8], r: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..1 + r.below(4) {
        if b.is_empty() {
            b.push(r.next() as u8);
            continue;
        }
        let at = r.below(b.len());
        match r.below(8) {
            0 => b[at] ^= 1 << r.below(8),
            1 => b[at] = r.next() as u8,
            2 => b.truncate(at),
            3 => {
                let n = r.below(16) + 1;
                for _ in 0..n {
                    b.insert(at.min(b.len()), r.next() as u8);
                }
            }
            4 => {
                let end = (at + r.below(64) + 1).min(b.len());
                let chunk = b[at..end].to_vec();
                b.splice(at..at, chunk);
            }
            5 | 6 => {
                // A 32-bit field (4-aligned, as tables are) set to an edge value or a small offset.
                let at = at & !3;
                if at + 4 <= b.len() {
                    let v = if r.below(2) == 0 {
                        EDGES[r.below(EDGES.len())]
                    } else {
                        r.below(b.len() + 16) as i32 - 8
                    };
                    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            _ => {
                // Overlap: an 8-byte (count, offset) pair copied over another 4-aligned place, or a run of them.
                let from = r.below(b.len()) & !3;
                let to = r.below(b.len()) & !3;
                if from + 8 <= b.len() && to + 8 <= b.len() {
                    let pair = b[from..from + 8].to_vec();
                    for k in 0..1 + r.below(8) {
                        let at = to + k * 8;
                        if at + 8 <= b.len() {
                            b[at..at + 8].copy_from_slice(&pair);
                        }
                    }
                }
            }
        }
    }
    b
}

/// One mutation of an MP3 `seed` that reaches past the frame sync: the 4 header bytes (version, layer, bitrate, rate, padding,
/// mode), the side information after one (the granule sizes, table selects and scale factor bits) or a stretch of its main data
/// set to a random or edge value, and, as the plain mutations do, cuts and repeats.
fn mutate_mp3(seed: &[u8], r: &mut Rng) -> Vec<u8> {
    let starts: Vec<usize> = (0..seed.len().saturating_sub(4))
        .filter(|&i| seed[i] == 0xff && seed[i + 1] & 0xe0 == 0xe0)
        .collect();
    let mut b = seed.to_vec();
    for _ in 0..1 + r.below(6) {
        let Some(&frame) = starts.get(r.below(starts.len().max(1))) else {
            return mutate(seed, r);
        };
        let last = b.len().saturating_sub(1);
        let at = |off: usize| (frame + off).min(last);
        match r.below(6) {
            // A header byte: any bit flipped, or any value.
            0 => {
                let i = at(1 + r.below(3));
                b[i] = if r.below(2) == 0 {
                    b[i] ^ (1 << r.below(8))
                } else {
                    r.next() as u8
                };
            }
            // Side information (up to 32 bytes after the header): a byte set to an edge value.
            1 | 2 => {
                let i = at(4 + r.below(32));
                b[i] = [0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff][r.below(6)];
            }
            // A run of side information and main data random.
            3 => {
                for _ in 0..1 + r.below(24) {
                    let i = at(4 + r.below(64));
                    b[i] = r.next() as u8;
                }
            }
            // A frame repeated or dropped.
            4 => {
                let start = frame.min(b.len());
                let end = (start + 417).min(b.len());
                let chunk = b[start..end].to_vec();
                b.splice(start..start, chunk);
            }
            _ => {
                let start = frame.min(b.len());
                b.drain(start..(start + 417).min(b.len()));
            }
        }
        if b.len() < 8 {
            return mutate(seed, r);
        }
    }
    b
}

/// Every reader on `b`, as its format `kind`.
fn read_all(kind: usize, b: &[u8]) {
    match kind {
        MP3_KIND => {
            let _ = sound::read_sound("a.mp3", b);
        }
        0 => {
            if let Ok(d) = vpk::VpkDir::parse(b.to_vec()) {
                let paths: Vec<String> = d.paths().map(str::to_string).collect();
                for p in paths.iter().take(8) {
                    if let Some(e) = d.entry(p) {
                        let _ = d.read_with(e, &mut |_, _, _| Ok(vec![0; 4]));
                    }
                }
            }
        }
        1 => {
            let _ = vtf::read(b, 256);
        }
        2 => {
            if let Ok(m) = vmt::Vmt::parse(b) {
                let _ = (m.base_texture(), m.blend(), m.color("$color"), m.tool());
                let again = m.clone();
                let _ = m.patched(&mut |_| Some(again.clone()));
            }
            let _ = kv::parse(b);
        }
        3 => {
            if let Ok(m) = mdl::Mdl::parse(b) {
                for a in 0..m.anims.len().min(4) {
                    let _ = m.frames(a, Some(b), 64);
                }
                let v = vvd::Vvd::parse(&synth::vvd());
                let t = vtx::Vtx::parse(&synth::vtx(1, 3, false));
                if let (Ok(v), Ok(t)) = (v, t)
                    && let Ok(mut sm) = model::assemble(&m, &v, &t, &model::Choice::default())
                {
                    let _ = sm.add_sequences(&m, &m, None, &mut |_| true);
                }
            }
        }
        4 => {
            let _ = vvd::Vvd::parse(b);
        }
        5 => {
            if let Ok(t) = vtx::Vtx::parse(b)
                && let (Ok(m), Ok(v)) = (
                    mdl::Mdl::parse(&synth::mdl(8)),
                    vvd::Vvd::parse(&synth::vvd()),
                )
            {
                let _ = model::assemble(&m, &v, &t, &model::Choice::default());
            }
        }
        7 => {
            if let Ok(p) = pcf::Pcf::parse(b) {
                for s in p.systems() {
                    let mut sim = pcf::Sim::new(&s, 3);
                    for _ in 0..8 {
                        sim.step(0.1);
                    }
                }
            }
            let _ = pcf::Sheet::parse(b);
        }
        8 => {
            let _ = crate::read_texture(b);
        }
        9 => {
            let _ = sound::read_wav(b);
            let _ = sound::read_mp3(b);
            let _ = sound::read_soundscript(b);
            let _ = sound::read_soundscapes(b);
            let _ = sound::read_manifest(b);
        }
        _ => {
            if let Ok(map) = bsp::Bsp::parse(b) {
                if let Ok(faces) = map.world_faces() {
                    let _ = map.build(&faces, [0.0; 3]);
                }
                let _ = map.entities();
                let _ = map.static_props();
                let _ = map.face_areas();
                let _ = map.leaf_at([1.0, 2.0, 3.0]);
                if let Ok(faces) = map.world_faces()
                    && let Ok(atlas) = map.lightmap_atlas(&faces)
                {
                    let _ = map.build_lit(&faces, [0.0; 3], Some(&atlas));
                }
                let _ = map.ambient_at([1.0, 2.0, 3.0]);
                if let Ok(a) = map.ambient() {
                    for p in [[0.0; 3], [-1e30, 5.0, 1e30], [f32::NAN, 0.0, 0.0]] {
                        let _ = a.at(p);
                    }
                }
                let _ = map.sun_agreement(64);
                if let (Ok(t), Ok(lights)) = (map.tracer(), map.world_lights()) {
                    let _ = bsp::direct_lights(&t, &lights, [1.0, 2.0, 3.0], 0.01);
                }
                if let Ok(Some(z)) = map.pak() {
                    let _ = z.read(b"materials/dev/floor.vmt");
                }
            }
        }
    }
}

/// A 7.3 texture of one pixel with a sprite sheet resource after its image.
fn sheet_texture() -> Vec<u8> {
    let mut t = vtf::write(0, 1, 1, 3, &[vec![1, 2, 3, 4]]);
    // Two resources (the image and the sheet): the header grows by one entry, the image moves after it.
    t[68..72].copy_from_slice(&2u32.to_le_bytes());
    let image = 96u32;
    t[84..88].copy_from_slice(&image.to_le_bytes());
    t[12..16].copy_from_slice(&image.to_le_bytes());
    let sheet_at = image + 4;
    t.splice(
        88..88,
        [0x10u8, 0, 0, 0].into_iter().chain(sheet_at.to_le_bytes()),
    );
    let mut sheet = Vec::new();
    for v in [0i32, 1, 0, 1, 1] {
        sheet.extend_from_slice(&v.to_le_bytes());
    }
    for v in [1f32, 1.0, 0.0, 0.0, 1.0, 1.0] {
        sheet.extend_from_slice(&v.to_le_bytes());
    }
    t.extend_from_slice(&(sheet.len() as u32).to_le_bytes());
    t.extend_from_slice(&sheet);
    t
}

fn seeds() -> Vec<(usize, Vec<u8>)> {
    let tex = vtf::write(0, 2, 2, 4, &[vec![7; 16], vec![9; 4]]);
    let dxt1 = vtf::write(
        13,
        8,
        8,
        2,
        &[vec![0x55; 32], vec![0xaa; 8], vec![1; 8], vec![2; 8]],
    );
    let dxt5 = vtf::write(15, 4, 4, 5, &[vec![0x5a; 16]]);
    let bgr = vtf::write(3, 3, 1, 1, &[vec![1, 2, 3, 4, 5, 6, 7, 8, 9]]);
    let ati2n = vtf::write(37, 4, 4, 3, &[vec![0x33; 16]]);
    let mat = b"\"VertexLitGeneric\" { \"$basetexture\" \"a\\b\" $alphatest 1 $color \"[1 0.5 0]\" Proxies { x { y z } } }".to_vec();
    let patch = b"patch { include \"materials/a.vmt\" insert { $envmap env } }".to_vec();
    let dir = vpk::write_dir(
        &[
            ("materials/a.vmt", &mat),
            ("b/c.vtf", &tex),
            ("readme", b"x"),
        ],
        2,
        4,
    );
    vec![
        (0, dir),
        (0, vpk::write_dir(&[("a/b.c", b"hello")], 1, 2)),
        (1, tex),
        (1, dxt1),
        (1, dxt5),
        (1, bgr),
        (1, ati2n),
        (2, mat),
        (2, patch),
        (3, synth::mdl(8)),
        (3, synth::mdl(200)),
        (4, synth::vvd()),
        (5, synth::vtx(2, 6, false)),
        (5, synth::vtx(3, 9, true)),
        (6, bsp::write_test_map()),
        (6, bsp::write_lit_test_map()),
        (7, pcf::write_test_file()),
        (8, sheet_texture()),
        (9, sound::write_wav(22050, 2, &[1, -1, 2, -2, 3, -3], Some(1))),
        (9, sound::write_adpcm(&[0x12, 0x34, 0x56, 0x78])),
        (MP3_KIND, sound::write_mp3(sound::Mpeg::V1, false, 3)),
        (MP3_KIND, sound::write_mp3(sound::Mpeg::V1, true, 3)),
        (MP3_KIND, sound::write_mp3(sound::Mpeg::V2, false, 3)),
        (MP3_KIND, sound::write_mp3(sound::Mpeg::V2, true, 3)),
        (MP3_KIND, sound::write_mp3(sound::Mpeg::V25, false, 3)),
        (MP3_KIND, sound::write_mp3(sound::Mpeg::V25, true, 3)),
        (
            9,
            b"\"A.B\" { \"volume\" \"0.5,0.7\" \"soundlevel\" \"SNDLVL_70dB\" \"rndwave\" { \"wave\" \")a.wav\" } } \"S\" { \"playrandom\" { \"time\" \"1,2\" } \"playsoundscape\" { \"name\" \"S\" } }".to_vec(),
        ),
    ]
}

#[test]
fn fuzz_every_reader_without_a_panic() {
    let cases = std::env::var("TORQUE_FUZZ_VALVE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CASES);
    let seeds = seeds();
    for (kind, s) in &seeds {
        read_all(*kind, s);
        // A mutation of a frame is only worth running when the unmutated file decodes: the seeds are real frames.
        if *kind == MP3_KIND {
            let wave = sound::read_mp3(s).expect("an MP3 seed decodes");
            assert!(wave.samples.len() >= 1152, "{} samples", wave.samples.len());
        }
    }
    let mut r = Rng::new(0x5eed_f501_17ce);
    let mut panics = Vec::new();
    let mut slowest = (Duration::ZERO, 0usize, 0usize);
    let mut mp3_decoded = 0usize;
    for case in 0..cases {
        let (kind, seed) = &seeds[case % seeds.len()];
        let kind = *kind;
        let b = if kind == MP3_KIND {
            mutate_mp3(seed, &mut r)
        } else {
            mutate(seed, &mut r)
        };
        let started = Instant::now();
        match catch_unwind(AssertUnwindSafe(|| {
            if kind == MP3_KIND {
                return sound::read_sound("a.mp3", &b).is_ok();
            }
            read_all(kind, &b);
            false
        })) {
            Ok(decoded) => mp3_decoded += usize::from(decoded),
            Err(_) => panics.push((case, kind)),
        }
        slowest = slowest.max((started.elapsed(), case, kind));
    }
    eprintln!(
        "VALVE-FUZZ {cases} cases, {} panics, {mp3_decoded} mutated MP3s still decode; slowest case {} (kind {}) took {:?}",
        panics.len(),
        slowest.1,
        slowest.2,
        slowest.0
    );
    assert!(
        panics.is_empty(),
        "{} panics: {:?}",
        panics.len(),
        &panics[..panics.len().min(10)]
    );
    assert!(
        slowest.0 < SLOWEST_CASE,
        "case {} (kind {}) took {:?}",
        slowest.1,
        slowest.2,
        slowest.0
    );
    // Some mutations must keep the file decodable, or the mutations never get past the frame sync.
    assert!(
        cases < 2_000 || mp3_decoded > 0,
        "no mutated MP3 decoded in {cases} cases"
    );
}

/// The sheet texture seed, for the loader tests.
pub fn sheet_texture_for_tests() -> Vec<u8> {
    sheet_texture()
}
