//! The compressed textures' check on 30 images made in code (stone, rust, moss, walls, wood, marble, cells, weave, tiles,
//! ramps; 256 x 256): each is baked as a material (`hd::pbr::bake`: its chains made in linear light, Toksvig and coverage as
//! the loader makes them, then BC7, BC5 and BC4 in KTX 2.0 files with Zstandard levels) and compared, level by level, with the
//! uncompressed chains `hd::pbr::build` makes of the same maps:
//!
//! - BC7: the base color's and the ORM's PSNR over every level, at least 40 dB for each image;
//! - BC5: the normal's mean angle to the uncompressed one over every level, at most 2 degrees for each image;
//! - coverage: a cutout material of the image's alpha, the share of each level's texels (16 x 16 and larger) at or over the
//!   cutoff after BC7, within 2 % of level 0's;
//! - KTX 2.0: every baked file read and written again, 0 differing bytes, and its levels the blocks encoded;
//! - OpenEXR: the image's luminance as 16-bit steps in a float channel (every lossless compression, scan lines and tiles),
//!   read back to R16 within 1 step.
//!
//! The KTX 2.0 and OpenEXR readers' fuzz is `torque-images`' `texture_fuzz` (`just texture-verify` runs both). Exits with an
//! error when any image fails.
//!
//! `cargo run -p torque-render-scene --release --example texture-verify`

use std::io::Cursor;
use std::process::ExitCode;

use exr::prelude::{
    AnyChannel, AnyChannels, Blocks, Compression, Encoding, FlatSamples, Image, Layer,
    LayerAttributes, LineOrder, SmallVec, Vec2, WritableImage,
};
use images::bc;
use images::ktx2::{self, Limits, Supercompression};
use torque_render_scene::hd::TextureTable;
use torque_render_scene::hd::pbr::{
    self, AlphaMode, MapImage, MapImages, MapRole, PbrMaterial, coverage,
};

const SIDE: u32 = 256;
const IMAGES: u32 = 30;
const CUTOFF: f32 = 0.5;
/// The smallest level whose coverage is compared (below it one texel is more than 0.4 % of the level).
const COVERAGE_MIN_TEXELS: usize = 256;

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Smooth value noise of period `p` cells.
fn noise(x: f32, y: f32, p: i32, seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let w = |a: i32| a.rem_euclid(p);
    let (ix, iy) = (fx as i32, fy as i32);
    let c = |dx: i32, dy: i32| hash(w(ix + dx), w(iy + dy), seed);
    let top = c(0, 0) + (c(1, 0) - c(0, 0)) * sx;
    let bottom = c(0, 1) + (c(1, 1) - c(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// Four octaves over the map at `u`, `v` in 0..1, `p` cells across at the first.
fn fbm(u: f32, v: f32, p: i32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut cells) = (0.0, 0.5, p);
    for o in 0..4 {
        sum += amp * noise(u * cells as f32, v * cells as f32, cells, seed + o);
        amp *= 0.5;
        cells *= 2;
    }
    sum / 0.9375
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t.clamp(0.0, 1.0))
}

/// Image `n`'s color (0..1) and height at `u`, `v`.
fn texel(n: u32, u: f32, v: f32) -> ([f32; 3], f32) {
    let seed = 17 + n * 101;
    let tau = std::f32::consts::TAU;
    match n % 10 {
        // Stone, rust and moss: two colors by noise.
        0..=2 => {
            let palettes = [
                ([0.35, 0.33, 0.30], [0.72, 0.70, 0.66]),
                ([0.30, 0.12, 0.05], [0.75, 0.42, 0.18]),
                ([0.10, 0.22, 0.06], [0.45, 0.60, 0.20]),
            ];
            let (a, b) = palettes[(n % 10) as usize];
            let h = fbm(u, v, 4 + (n / 10) as i32 * 2, seed);
            (mix(a, b, h), h)
        }
        // Wall: courses offset by half, soft mortar.
        3 => {
            let rows = 8.0 + (n / 10) as f32 * 2.0;
            let y = v * rows;
            let x = u * rows / 2.0 + if (y as i32) % 2 == 0 { 0.0 } else { 0.5 };
            let edge = |t: f32| {
                let f = t.fract();
                (f.min(1.0 - f) * 12.0).clamp(0.0, 1.0)
            };
            let slab = edge(x) * edge(y);
            let tone = 0.8 + 0.2 * fbm(u, v, 8, seed);
            let c = mix(
                [0.55, 0.52, 0.48],
                [0.62 * tone, 0.24 * tone, 0.16 * tone],
                slab,
            );
            (c, slab * 0.8 + 0.2 * fbm(u, v, 16, seed))
        }
        // Wood: rings around a far center, bent by noise.
        4 => {
            let (dx, dy) = (u - 0.5, v + 1.5);
            let r = (dx * dx + dy * dy).sqrt() * (6.0 + n as f32 / 2.0) + 1.5 * fbm(u, v, 4, seed);
            let ring = 0.5 + 0.5 * (r * tau).sin();
            (mix([0.42, 0.25, 0.12], [0.70, 0.50, 0.30], ring), ring)
        }
        // Marble: veins along a turbulent wave.
        5 => {
            let t = (u * 3.0 + 4.0 * fbm(u, v, 4, seed)) * tau;
            let vein = (0.5 + 0.5 * t.sin()).powf(3.0);
            (
                mix([0.90, 0.89, 0.86], [0.35, 0.36, 0.40], vein),
                1.0 - vein,
            )
        }
        // Cells: the distance to the nearest of jittered points.
        6 => {
            let cells = 6 + (n / 10) as i32;
            let (x, y) = (u * cells as f32, v * cells as f32);
            let mut d = f32::MAX;
            for oy in -1..=1 {
                for ox in -1..=1 {
                    let (cx, cy) = (x.floor() as i32 + ox, y.floor() as i32 + oy);
                    let (wx, wy) = (cx.rem_euclid(cells), cy.rem_euclid(cells));
                    let px = cx as f32 + hash(wx, wy, seed);
                    let py = cy as f32 + hash(wy, wx, seed + 1);
                    d = d.min(((x - px).powi(2) + (y - py).powi(2)).sqrt());
                }
            }
            let h = (d * 1.2).min(1.0);
            (mix([0.20, 0.25, 0.35], [0.70, 0.75, 0.80], h), h)
        }
        // Weave: crossing threads.
        7 => {
            let k = 8.0 + (n / 10) as f32 * 4.0;
            let a = (u * k * tau).sin() * (v * k * tau / 2.0).cos();
            let h = 0.5 + 0.5 * a;
            (mix([0.15, 0.20, 0.45], [0.55, 0.60, 0.80], h), h)
        }
        // Tiles: a checker with soft edges and a glaze.
        8 => {
            let k = 6.0 + (n / 10) as f32 * 2.0;
            let s = (u * k * tau / 2.0).sin() * (v * k * tau / 2.0).sin();
            let t = (0.5 + 2.0 * s).clamp(0.0, 1.0);
            let c = mix([0.12, 0.30, 0.32], [0.88, 0.86, 0.80], t);
            (c, 0.5 + 0.5 * s.abs().sqrt() * s.signum())
        }
        // Ramps: a gradient with a faint grain.
        _ => {
            let g = 0.04 * fbm(u, v, 16, seed);
            // Each ramp runs its own way.
            let (u, v) = match n / 10 {
                0 => (u, v),
                1 => (v, 1.0 - u),
                _ => (1.0 - u, 1.0 - v),
            };
            let c = [u * 0.9 + g, v * 0.8 + g, (1.0 - u) * 0.7 + g];
            (c, (u + 2.0 * v) / 3.0)
        }
    }
}

/// Image `n`: its color and its height (for the normal and the OpenEXR file), both `SIDE` x `SIDE`.
fn image(n: u32) -> (Vec<u8>, Vec<f32>) {
    let mut rgba = Vec::with_capacity((4 * SIDE * SIDE) as usize);
    let mut height = Vec::with_capacity((SIDE * SIDE) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (u, v) = (
                (x as f32 + 0.5) / SIDE as f32,
                (y as f32 + 0.5) / SIDE as f32,
            );
            let (c, h) = texel(n, u, v);
            for ch in c {
                rgba.push((ch.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            rgba.push(255);
            height.push(h.clamp(0.0, 1.0));
        }
    }
    (rgba, height)
}

fn map(rgba: Vec<u8>) -> MapImage {
    MapImage {
        rgba,
        width: SIDE,
        height: SIDE,
    }
}

/// A normal map of `height` (wrapping), and an ORM map of it.
fn normal_and_orm(height: &[f32], strength: f32) -> (Vec<u8>, Vec<u8>) {
    let s = SIDE as i32;
    let at = |x: i32, y: i32| height[(y.rem_euclid(s) * s + x.rem_euclid(s)) as usize];
    let (mut normal, mut orm) = (Vec::new(), Vec::new());
    for y in 0..s {
        for x in 0..s {
            let dx = (at(x + 1, y) - at(x - 1, y)) * strength;
            let dy = (at(x, y + 1) - at(x, y - 1)) * strength;
            let l = (dx * dx + dy * dy + 1.0).sqrt();
            let n = [-dx / l, -dy / l, 1.0 / l];
            normal.extend(n.map(|c| ((c * 0.5 + 0.5) * 255.0).round() as u8));
            normal.push(255);
            let h = at(x, y);
            orm.extend([
                ((0.6 + 0.4 * h) * 255.0) as u8,
                ((0.25 + 0.6 * (1.0 - h)) * 255.0) as u8,
                (((h - 0.75) * 10.0).clamp(0.0, 1.0) * 255.0).round() as u8,
                255,
            ]);
        }
    }
    (normal, orm)
}

/// A cutout's alpha over `rgba`: 1 where the height passes a wavy threshold, 0 elsewhere, soft at the edge.
fn with_alpha(mut rgba: Vec<u8>, height: &[f32]) -> Vec<u8> {
    for (p, &h) in rgba.chunks_mut(4).zip(height) {
        p[3] = (((h - 0.45) * 20.0 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    rgba
}

fn psnr(a: &[u8], b: &[u8], channels: &[usize]) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0u64);
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        for &c in channels {
            let d = f64::from(p[c]) - f64::from(q[c]);
            sum += d * d;
            n += 1;
        }
    }
    if sum == 0.0 {
        return 99.0;
    }
    10.0 * (255.0f64 * 255.0 / (sum / n as f64)).log10()
}

/// The unit normal of an RGBA8 texel's X and Y (Z rebuilt, as the shader does).
fn unit(p: &[u8]) -> [f32; 3] {
    let x = f32::from(p[0]) / 127.5 - 1.0;
    let y = f32::from(p[1]) / 127.5 - 1.0;
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    let l = (x * x + y * y + z * z).sqrt().max(1e-6);
    [x / l, y / l, z / l]
}

/// A baked role's file: read, written again (0 differing bytes wanted) and its levels decoded to RGBA8.
struct Decoded {
    levels: Vec<Vec<u8>>,
    differing: usize,
}

fn decode(file: &[u8]) -> Result<Decoded, String> {
    let t = ktx2::read(file, &Limits::default()).map_err(|e| e.to_string())?;
    let again = ktx2::write(&t, Supercompression::Zstandard).map_err(|e| e.to_string())?;
    let differing =
        again.iter().zip(file).filter(|(a, b)| a != b).count() + again.len().abs_diff(file.len());
    let levels = t
        .levels
        .iter()
        .enumerate()
        .map(|(k, l)| {
            let (w, h) = ((t.width >> k).max(1), (t.height >> k).max(1));
            match t.format.block() {
                Some(b) => bc::decode(b, l, w, h).map_err(|e| e.to_string()),
                None => Ok(l.clone()),
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(Decoded { levels, differing })
}

/// `m`'s uncompressed chains (by role) and its baked files decoded (by role).
type Chains = Vec<(MapRole, Vec<Vec<u8>>)>;

fn chains(m: &PbrMaterial, maps: &MapImages) -> Result<(Chains, Chains, usize), String> {
    let mut table = TextureTable::default();
    let built = pbr::build(m, "ref", maps.clone(), &mut table).map_err(|e| e.to_string())?;
    let ids = [
        (MapRole::Base, built.base),
        (MapRole::Normal, built.normal),
        (MapRole::Orm, built.orm),
    ];
    let reference = ids
        .into_iter()
        .filter_map(|(r, i)| i.map(|i| (r, table.textures[i].levels.to_vec())))
        .collect();
    let mut differing = 0;
    let mut baked = Vec::new();
    for b in pbr::bake(m, maps.clone(), None).map_err(|e| e.to_string())? {
        let want = b.role.baked();
        let t = ktx2::header(&b.file, &Limits::default()).map_err(|e| e.to_string())?;
        if t.format != want {
            return Err(format!(
                "{:?} baked as {:?}, not {want:?}",
                b.role, t.format
            ));
        }
        let d = decode(&b.file)?;
        differing += d.differing;
        baked.push((b.role, d.levels));
    }
    Ok((reference, baked, differing))
}

fn levels_of(c: &Chains, role: MapRole) -> &[Vec<u8>] {
    c.iter().find(|(r, _)| *r == role).map_or(&[], |(_, l)| l)
}

/// An OpenEXR file of `values` as one float channel.
fn exr_file(values: &[f32], compression: Compression, tiles: bool) -> Vec<u8> {
    let layer = Layer::new(
        (SIDE as usize, SIDE as usize),
        LayerAttributes::default(),
        Encoding {
            compression,
            blocks: if tiles {
                Blocks::Tiles(Vec2(64, 64))
            } else {
                Blocks::ScanLines
            },
            line_order: LineOrder::Increasing,
        },
        AnyChannels::sort(SmallVec::from_vec(vec![AnyChannel::new(
            "Y",
            FlatSamples::F32(values.to_vec()),
        )])),
    );
    let mut out = Vec::new();
    Image::from_layer(layer)
        .write()
        .to_buffered(Cursor::new(&mut out))
        .expect("an image made here writes");
    out
}

#[derive(Default)]
struct Worst {
    bc7_db: f64,
    orm_db: f64,
    bc5_degrees: f64,
    coverage: f32,
    differing: usize,
    exr_steps: u16,
    failed: Vec<String>,
}

fn check(n: u32, worst: &mut Worst) -> Result<(), String> {
    let (rgba, height) = image(n);
    let (normal, orm) = normal_and_orm(&height, 1.0 + 0.5 * (n % 5) as f32);
    let opaque = PbrMaterial::default();
    let maps = MapImages {
        base: Some(map(rgba.clone())),
        normal: Some(map(normal)),
        orm: Some(map(orm)),
        ..MapImages::default()
    };
    let (reference, baked, differing) = chains(&opaque, &maps)?;
    let joined = |c: &Chains, r| levels_of(c, r).concat();
    let bc7_db = psnr(
        &joined(&reference, MapRole::Base),
        &joined(&baked, MapRole::Base),
        &[0, 1, 2, 3],
    );
    let orm_db = psnr(
        &joined(&reference, MapRole::Orm),
        &joined(&baked, MapRole::Orm),
        &[0, 1, 2],
    );
    let (mut angle, mut count) = (0.0f64, 0u64);
    for (a, b) in levels_of(&reference, MapRole::Normal)
        .iter()
        .zip(levels_of(&baked, MapRole::Normal))
    {
        for (p, q) in a.chunks(4).zip(b.chunks(4)) {
            let (u, v) = (unit(p), unit(q));
            let dot = (u[0] * v[0] + u[1] * v[1] + u[2] * v[2]).clamp(-1.0, 1.0);
            angle += f64::from(dot.acos().to_degrees());
            count += 1;
        }
    }
    let bc5_degrees = angle / count.max(1) as f64;
    // The cutout: coverage of the BC7 levels against the uncompressed level 0's.
    let cutout = PbrMaterial {
        alpha: AlphaMode::Cutout,
        alpha_cutoff: CUTOFF,
        ..PbrMaterial::default()
    };
    let cut_maps = MapImages {
        base: Some(map(with_alpha(rgba, &height))),
        ..MapImages::default()
    };
    let (cut_ref, cut_baked, cut_differing) = chains(&cutout, &cut_maps)?;
    let texels = |l: &[u8]| -> Vec<[f32; 4]> {
        l.as_chunks::<4>()
            .0
            .iter()
            .map(|p| p.map(|v| f32::from(v) / 255.0))
            .collect()
    };
    let level0 = coverage(&texels(&levels_of(&cut_ref, MapRole::Base)[0]), CUTOFF);
    let mut cov_err = 0.0f32;
    for l in levels_of(&cut_baked, MapRole::Base) {
        if l.len() / 4 >= COVERAGE_MIN_TEXELS {
            cov_err = cov_err.max((coverage(&texels(l), CUTOFF) - level0).abs());
        }
    }
    // The height as an OpenEXR file and back.
    let steps: Vec<u16> = height
        .iter()
        .map(|&h| (h * 65_535.0).round() as u16)
        .collect();
    let values: Vec<f32> = steps.iter().map(|&s| f32::from(s) / 65_535.0).collect();
    let compressions = [
        Compression::Uncompressed,
        Compression::RLE,
        Compression::ZIP1,
        Compression::ZIP16,
        Compression::PIZ,
    ];
    let file = exr_file(&values, compressions[(n % 5) as usize], n % 2 == 1);
    let back = images::exr::read_r16(&file, 4096).map_err(|e| e.to_string())?;
    let exr_steps = back
        .texels
        .iter()
        .zip(&steps)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(u16::MAX);
    println!(
        "image {n:2}: BC7 {bc7_db:5.1} dB, ORM {orm_db:5.1} dB, BC5 {bc5_degrees:4.2} deg, coverage {:4.2} % (level 0 {:4.1} %), \
         KTX2 {} differing, EXR {exr_steps} step",
        100.0 * cov_err,
        100.0 * level0,
        differing + cut_differing
    );
    let mut fail = |what: &str| worst.failed.push(format!("image {n}: {what}"));
    if bc7_db < 40.0 || orm_db < 40.0 {
        fail("BC7 under 40 dB");
    }
    if bc5_degrees > 2.0 {
        fail("BC5 over 2 degrees");
    }
    if cov_err > 0.02 {
        fail("coverage off by more than 2 %");
    }
    if differing + cut_differing > 0 {
        fail("KTX 2.0 bytes differ");
    }
    if exr_steps > 1 {
        fail("OpenEXR off by more than 1 step");
    }
    worst.bc7_db = worst.bc7_db.min(bc7_db);
    worst.orm_db = worst.orm_db.min(orm_db);
    worst.bc5_degrees = worst.bc5_degrees.max(bc5_degrees);
    worst.coverage = worst.coverage.max(cov_err);
    worst.differing += differing + cut_differing;
    worst.exr_steps = worst.exr_steps.max(exr_steps);
    Ok(())
}

fn main() -> ExitCode {
    let mut worst = Worst {
        bc7_db: f64::INFINITY,
        orm_db: f64::INFINITY,
        ..Worst::default()
    };
    let start = std::time::Instant::now();
    for n in 0..IMAGES {
        if let Err(e) = check(n, &mut worst) {
            worst.failed.push(format!("image {n}: {e}"));
        }
    }
    println!(
        "{IMAGES} images in {:.1} s: BC7 at least {:.1} dB (ORM {:.1} dB), BC5 at most {:.2} degrees, coverage within {:.2} %, \
         KTX2 {} differing bytes, EXR to R16 within {} step",
        start.elapsed().as_secs_f64(),
        worst.bc7_db,
        worst.orm_db,
        worst.bc5_degrees,
        100.0 * worst.coverage,
        worst.differing,
        worst.exr_steps
    );
    if worst.failed.is_empty() {
        println!("pass");
        ExitCode::SUCCESS
    } else {
        for f in &worst.failed {
            eprintln!("FAIL {f}");
        }
        ExitCode::FAILURE
    }
}
