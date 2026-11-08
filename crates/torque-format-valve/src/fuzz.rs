//! The hostile-input fuzz: seeded mutations of small synthetic files of every format (none from a game), each given to its
//! reader under `catch_unwind`; 0 panics. Mutations are byte flips, cuts, insertions, repeats, 32-bit fields set to edge values,
//! and overlapping offsets: a table's count and offset copied over another's (or every offset of a structure pointed at one
//! place), so tables read the same bytes many times. `TORQUE_FUZZ_VALVE` sets the count (20,000 by default).

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::{bsp, kv, mdl, model, pcf, sound, synth, vmt, vpk, vtf, vtx, vvd};

const CASES: usize = 20_000;

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

/// Every reader on `b`, as its format `kind`.
fn read_all(kind: usize, b: &[u8]) {
    match kind {
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
    }
    let mut r = Rng::new(0x5eed_f501_17ce);
    let mut panics = Vec::new();
    for case in 0..cases {
        let (kind, seed) = &seeds[case % seeds.len()];
        let b = mutate(seed, &mut r);
        let kind = *kind;
        if catch_unwind(AssertUnwindSafe(|| read_all(kind, &b))).is_err() {
            panics.push((case, kind));
        }
    }
    assert!(
        panics.is_empty(),
        "{} panics: {:?}",
        panics.len(),
        &panics[..panics.len().min(10)]
    );
}

/// The sheet texture seed, for the loader tests.
pub fn sheet_texture_for_tests() -> Vec<u8> {
    sheet_texture()
}
