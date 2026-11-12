//! A gallery of physically based materials, drawn offscreen at 1920 x 1080 (Ultra, so the height maps' parallax is on): spheres
//! and shapes in the sun under the physical sky (`1-sun-spheres.png`, `2-sun-shapes.png`), the same under area lights at night
//! (`3-area-lights.png`), a roughness by metalness sweep (`4-sweep.png`) and a close look at the relief (`5-relief.png`). Each
//! picture is a PNG in the directory given.
//!
//! The materials are made in code (a brushed metal, a stone wall with a height map, wood, a woven fabric, a painted plastic, gold
//! and a rough ground) unless `--library <dir>` names a folder of materials: each folder in it that holds a `.mat.toml` sidecar
//! is one (its maps beside it, any material library's that has base color, normal, ORM or separate maps and height). Nothing is
//! read outside that folder.
//!
//! `cargo run -p torque-render-hd --release --example pbr-gallery -- <dir> [--library <dir>]`; with `--cost` it prints the
//! frame's and the opaque pass's GPU times of the shapes in the sun with their materials and with the same draws plain (under
//! the bench lock).

use std::io::Read;
use std::path::{Path, PathBuf};

use torque_modelcore::pbr::{PbrMaterial, Sidecar};
use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::PlainScene;
use torque_render_hd::profile::Profile;
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::pbr::{self, HdMaterial, MapImage, MapImages, MaterialId, PbrDraw};
use torque_render_scene::hd::{
    AreaShape, DynamicObject, HdAreaLight, HdCamera, HdDraw, HdLight, HdMode, HdScene, HdVertex,
    Quality, material, oct_encode,
};
use torque_render_scene::math::Mat;
use torque_render_scene::{Cull, Filter, Sampler, TexEnv};

const SIZE: (u32, u32) = (1920, 1080);
/// Frames drawn for each picture (the anti-aliasing's history and the capture's faces settle over them).
const FRAMES: usize = 32;
/// Frames timed for `--cost` (after 30 that settle).
const COST_FRAMES: usize = 60;
/// The side of the maps made in code.
const MAP: u32 = 256;
/// The most bytes a library map file may have.
const MAX_FILE_BYTES: u64 = 128 << 20;
/// The most library materials drawn.
const MAX_LIBRARY: usize = 8;

/// A material of the gallery: its name, its table id and its base texture (the draw's own, for the passes that read no maps).
#[derive(Clone)]
struct Mat3 {
    name: String,
    id: MaterialId,
    base: Option<usize>,
}

/// A small deterministic value noise.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Value noise of period `p` texels over the map (tiles seamlessly).
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

/// Three octaves of [`noise`] over the map at `x`, `y` in texels (base period `p` cells across the map).
fn fbm(x: u32, y: u32, p: i32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut period = p;
    for o in 0..3 {
        let k = period as f32 / MAP as f32;
        sum += amp * noise(x as f32 * k, y as f32 * k, period, seed + o);
        amp *= 0.5;
        period *= 2;
    }
    sum / 0.875
}

fn image(f: impl Fn(u32, u32) -> [f32; 4]) -> MapImage {
    let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    MapImage {
        rgba: (0..MAP)
            .flat_map(|y| (0..MAP).map(move |x| (x, y)))
            .flat_map(|(x, y)| f(x, y).map(to))
            .collect(),
        width: MAP,
        height: MAP,
    }
}

/// A tangent-space normal map (OpenGL convention) of height `h` (0..1 a texel) at `strength` (texels of relief at 1).
fn normals_of(h: &dyn Fn(u32, u32) -> f32, strength: f32) -> MapImage {
    let at = |x: i64, y: i64| {
        h(
            x.rem_euclid(MAP as i64) as u32,
            y.rem_euclid(MAP as i64) as u32,
        )
    };
    image(|x, y| {
        let (x, y) = (i64::from(x), i64::from(y));
        let dx = (at(x + 1, y) - at(x - 1, y)) * 0.5 * strength;
        // The map's rows run down while V (and +Y of the normal) runs up the image.
        let dy = (at(x, y - 1) - at(x, y + 1)) * 0.5 * strength;
        let l = (dx * dx + dy * dy + 1.0).sqrt();
        [
            -dx / l * 0.5 + 0.5,
            -dy / l * 0.5 + 0.5,
            1.0 / l * 0.5 + 0.5,
            1.0,
        ]
    })
}

fn srgb(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| {
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    })
}

/// The materials made in code: (name, sidecar factors, maps).
fn synthetic() -> Vec<(&'static str, PbrMaterial, MapImages)> {
    let mut out = Vec::new();
    // Brushed steel: streaks along U, rough 0.2-0.4, metal (physicallybased.info's iron albedo).
    let streak = |x: u32, y: u32| {
        noise(x as f32 / 64.0, y as f32 * 0.5, 4, 3) * 0.6 + fbm(x, y, 64, 9) * 0.4
    };
    out.push((
        "brushed-steel",
        PbrMaterial {
            uv_scale: [2.0, 2.0],
            ..PbrMaterial::default()
        },
        MapImages {
            base: Some(image(|x, y| {
                let s = srgb([0.53, 0.51, 0.49].map(|c| c * (0.9 + 0.1 * streak(x, y))));
                [s[0], s[1], s[2], 1.0]
            })),
            normal: Some(normals_of(&|x, y| streak(x, y), 1.5)),
            orm: Some(image(|x, y| [1.0, 0.18 + 0.2 * streak(x, y), 1.0, 1.0])),
            ..MapImages::default()
        },
    ));
    // A wall of blocks: 4 x 8 courses, mortar low and rough, the blocks' faces in relief in the height map.
    let block = |x: u32, y: u32| {
        let (bw, bh) = (MAP / 4, MAP / 8);
        let row = y / bh;
        let xs = (x + if row % 2 == 1 { bw / 2 } else { 0 }) % MAP;
        let (u, v) = ((xs % bw) as f32 / bw as f32, (y % bh) as f32 / bh as f32);
        let edge = (u.min(1.0 - u) * bw as f32).min(v.min(1.0 - v) * bh as f32);
        let face = ((edge - 1.5) / 2.0).clamp(0.0, 1.0);
        let id = (xs / bw) + row * 7;
        (face, id)
    };
    let wall_h = move |x: u32, y: u32| {
        let (face, _) = block(x, y);
        face * (0.85 + 0.15 * fbm(x, y, 16, 5))
    };
    out.push((
        "wall",
        PbrMaterial {
            height_scale: 0.03,
            uv_scale: [3.0, 3.0],
            metal: 0.0,
            ..PbrMaterial::default()
        },
        MapImages {
            base: Some(image(|x, y| {
                let (face, id) = block(x, y);
                let tone = 0.75 + 0.25 * hash(id as i32, 0, 11);
                let n = 0.85 + 0.15 * fbm(x, y, 32, 4);
                let stone = [0.42 * tone * n, 0.16 * tone * n, 0.1 * tone * n];
                let mortar = [0.45 * n, 0.43 * n, 0.4 * n];
                let c: Vec<f32> = (0..3)
                    .map(|k| mortar[k] + (stone[k] - mortar[k]) * face)
                    .collect();
                let s = srgb([c[0], c[1], c[2]]);
                [s[0], s[1], s[2], 1.0]
            })),
            normal: Some(normals_of(&wall_h, 6.0)),
            orm: Some(image(|x, y| {
                let (face, _) = block(x, y);
                [0.55 + 0.45 * face, 0.95 - 0.12 * face, 0.0, 1.0]
            })),
            height: Some(image(|x, y| {
                let h = wall_h(x, y);
                [h, h, h, 1.0]
            })),
            ..MapImages::default()
        },
    ));
    // Wood: rings bent by noise, rough 0.45-0.6.
    let ring = |x: u32, y: u32| {
        let d = x as f32 / MAP as f32 * 18.0 + fbm(x, y, 8, 21) * 3.0;
        (d * std::f32::consts::TAU).sin() * 0.5 + 0.5
    };
    out.push((
        "wood",
        PbrMaterial {
            metal: 0.0,
            uv_scale: [1.0, 1.0],
            ..PbrMaterial::default()
        },
        MapImages {
            base: Some(image(|x, y| {
                let r = ring(x, y);
                let g = fbm(x, y, 128, 2) * 0.15;
                let s = srgb([
                    0.30 + 0.12 * r + g,
                    0.17 + 0.07 * r + g * 0.6,
                    0.08 + 0.03 * r,
                ]);
                [s[0], s[1], s[2], 1.0]
            })),
            normal: Some(normals_of(
                &|x, y| ring(x, y) * 0.3 + fbm(x, y, 128, 3) * 0.2,
                1.0,
            )),
            orm: Some(image(|x, y| [1.0, 0.45 + 0.15 * ring(x, y), 0.0, 1.0])),
            ..MapImages::default()
        },
    ));
    // A woven fabric: over-under threads, very rough.
    let weave = |x: u32, y: u32| {
        let (cx, cy) = (x / 8, y / 8);
        let (u, v) = ((x % 8) as f32 / 8.0, (y % 8) as f32 / 8.0);
        let warp = (cx + cy) % 2 == 0;
        let t = if warp { u } else { v };
        (t * std::f32::consts::PI).sin()
    };
    out.push((
        "fabric",
        PbrMaterial {
            metal: 0.0,
            uv_scale: [3.0, 3.0],
            ..PbrMaterial::default()
        },
        MapImages {
            base: Some(image(|x, y| {
                let w = weave(x, y);
                let s = srgb([0.09 + 0.05 * w, 0.17 + 0.07 * w, 0.32 + 0.1 * w]);
                [s[0], s[1], s[2], 1.0]
            })),
            normal: Some(normals_of(&weave, 2.5)),
            orm: Some(image(|x, y| [0.7 + 0.3 * weave(x, y), 0.95, 0.0, 1.0])),
            ..MapImages::default()
        },
    ));
    // A red painted plastic, glossy with a faint orange peel.
    out.push((
        "painted-plastic",
        PbrMaterial {
            roughness: 0.22,
            metal: 0.0,
            base_factor: [0.55, 0.06, 0.04, 1.0],
            ..PbrMaterial::default()
        },
        MapImages {
            normal: Some(normals_of(&|x, y| fbm(x, y, 32, 31), 0.6)),
            ..MapImages::default()
        },
    ));
    // Gold (physicallybased.info: albedo 1.0, 0.766, 0.336), polished.
    out.push((
        "gold",
        PbrMaterial {
            roughness: 0.18,
            metal: 1.0,
            base_factor: [1.0, 0.766, 0.336, 1.0],
            ..PbrMaterial::default()
        },
        MapImages::default(),
    ));
    // A rough ground: soil and pebbles, the pebbles raised.
    let pebble = |x: u32, y: u32| ((fbm(x, y, 16, 41) - 0.55) * 6.0).clamp(0.0, 1.0);
    let ground_h = move |x: u32, y: u32| pebble(x, y) * 0.7 + fbm(x, y, 32, 43) * 0.3;
    out.push((
        "ground",
        PbrMaterial {
            metal: 0.0,
            height_scale: 0.04,
            uv_scale: [12.0, 12.0],
            ..PbrMaterial::default()
        },
        MapImages {
            base: Some(image(|x, y| {
                let p = pebble(x, y);
                let n = fbm(x, y, 64, 44);
                let soil = [0.16 + 0.06 * n, 0.12 + 0.04 * n, 0.08 + 0.02 * n];
                let stone = [
                    0.38 * (0.8 + 0.2 * n),
                    0.36 * (0.8 + 0.2 * n),
                    0.33 * (0.8 + 0.2 * n),
                ];
                let c: Vec<f32> = (0..3).map(|k| soil[k] + (stone[k] - soil[k]) * p).collect();
                let s = srgb([c[0], c[1], c[2]]);
                [s[0], s[1], s[2], 1.0]
            })),
            normal: Some(normals_of(&ground_h, 4.0)),
            orm: Some(image(|x, y| {
                [
                    0.6 + 0.4 * ground_h(x, y),
                    0.92 - 0.2 * pebble(x, y),
                    0.0,
                    1.0,
                ]
            })),
            height: Some(image(|x, y| {
                let h = ground_h(x, y);
                [h, h, h, 1.0]
            })),
            ..MapImages::default()
        },
    ));
    out
}

/// The library's materials: each folder in `dir` (sorted) holding a `.mat.toml` with one material, its maps read from that
/// folder only (the sidecar's paths are checked to stay inside it), each file within [`MAX_FILE_BYTES`].
fn library(dir: &Path, scene: &mut HdScene<StaticCells>) -> Result<Vec<Mat3>, String> {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    folders.sort();
    let mut out = Vec::new();
    for folder in folders {
        if out.len() >= MAX_LIBRARY {
            break;
        }
        let Some(sidecar) = std::fs::read_dir(&folder)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().ends_with(".mat.toml"))
        else {
            continue;
        };
        // A plain file only (no link followed), within the cap.
        let read_capped = |p: &Path| -> Option<Vec<u8>> {
            let (file, len) = torque_resource::plain::open_plain(p).ok()?;
            if len > MAX_FILE_BYTES {
                return None;
            }
            let mut bytes = Vec::with_capacity(len as usize);
            file.take(MAX_FILE_BYTES).read_to_end(&mut bytes).ok()?;
            Some(bytes)
        };
        let Some(text) = read_capped(&sidecar).and_then(|b| String::from_utf8(b).ok()) else {
            eprintln!("{}: not readable", sidecar.display());
            continue;
        };
        let m = match Sidecar::parse(&text) {
            Ok(Sidecar::One(m)) => *m,
            Ok(Sidecar::Named(_)) => {
                eprintln!("{}: holds named materials, not one", sidecar.display());
                continue;
            }
            Err(e) => {
                eprintln!("{}: {e}", sidecar.display());
                continue;
            }
        };
        let name = folder
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let mut read = |rel: &str| read_capped(&folder.join(rel));
        match pbr::load(
            &m,
            &sidecar.to_string_lossy(),
            &mut read,
            &mut scene.textures,
        ) {
            Ok(built) => {
                let base = built.base;
                let id = scene.materials.intern(built);
                out.push(Mat3 { name, id, base });
            }
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
    Ok(out)
}

fn put_synthetic(scene: &mut HdScene<StaticCells>) -> Vec<Mat3> {
    synthetic()
        .into_iter()
        .map(|(name, m, maps)| {
            let built =
                pbr::build(&m, name, maps, &mut scene.textures).expect("the maps made in code fit");
            let base = built.base;
            Mat3 {
                name: name.to_string(),
                id: scene.materials.intern(built),
                base,
            }
        })
        .collect()
}

const SAMPLER: Sampler = Sampler {
    repeat: true,
    mag: Filter::Linear,
    min: Filter::Linear,
    mip: Filter::Linear,
    mipmaps: true,
};

/// An object of one draw of `mesh` with material `m` (None: plain, its base texture only), `cull` none.
fn object(
    ghost: u32,
    centre: [f32; 3],
    radius: f32,
    mesh: (Vec<HdVertex>, Vec<u32>),
    m: &Mat3,
    with_material: bool,
) -> DynamicObject {
    let (mut vertices, indices) = mesh;
    let word = (material::KIND_SHAPE << material::KIND_SHIFT)
        | if m.base.is_some() {
            material::tex_env(TexEnv::Modulate)
        } else {
            0
        };
    for v in &mut vertices {
        v.material = word;
    }
    let mut d = HdDraw::empty();
    d.label = format!("{} {ghost}", m.name);
    d.texture = m.base.map(|b| (b, SAMPLER));
    d.pbr = with_material.then(|| PbrDraw {
        material: m.id,
        tangents: pbr::tangents_of(&vertices, &indices),
    });
    d.vertices = vertices;
    d.indices = indices;
    d.cull = Cull::None;
    DynamicObject {
        ghost,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent: false,
        centre,
        radius,
        draws: vec![d],
        previous: None,
        stamp: None,
        nodes: Vec::new(),
    }
}

fn vertex(p: [f32; 3], n: [f32; 3], uv: [f32; 2]) -> HdVertex {
    HdVertex {
        position: p,
        normal: oct_encode(n),
        uv,
        color: [255; 4],
        material: 0,
        extra: 0,
    }
}

fn sphere(c: [f32; 3], r: f32) -> (Vec<HdVertex>, Vec<u32>) {
    let (rings, segments) = (48u32, 96u32);
    let mut v = Vec::new();
    for i in 0..=rings {
        let theta = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..=segments {
            let phi = std::f32::consts::TAU * j as f32 / segments as f32;
            let n = [
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            ];
            v.push(vertex(
                [0, 1, 2].map(|k| c[k] + r * n[k]),
                n,
                [2.0 * j as f32 / segments as f32, i as f32 / rings as f32],
            ));
        }
    }
    let mut idx = Vec::new();
    for i in 0..rings {
        for j in 0..segments {
            let a = i * (segments + 1) + j;
            let b = a + segments + 1;
            idx.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    (v, idx)
}

/// A box of half extents `h` about `c`, each face's UV one unit a meter.
fn cuboid(c: [f32; 3], h: [f32; 3]) -> (Vec<HdVertex>, Vec<u32>) {
    let mut v = Vec::new();
    let mut idx = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut n = [0.0f32; 3];
            n[axis] = sign;
            // A side's U runs level and its V up (a map's courses stay level); the top and bottom take X and Y.
            let (u, w) = match axis {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let base = v.len() as u32;
            for (a, b) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let mut p = c;
                p[axis] += sign * h[axis];
                p[u] += a * h[u];
                p[w] += b * h[w];
                v.push(vertex(p, n, [(a + 1.0) * h[u], (1.0 - b) * h[w]]));
            }
            idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    (v, idx)
}

/// An upright cylinder of radius `r` and half height `hh` about `c` (side and caps; the side's UV one unit a meter).
fn cylinder(c: [f32; 3], r: f32, hh: f32) -> (Vec<HdVertex>, Vec<u32>) {
    let segments = 64u32;
    let mut v = Vec::new();
    let mut idx = Vec::new();
    let around = std::f32::consts::TAU * r;
    for j in 0..=segments {
        let phi = std::f32::consts::TAU * j as f32 / segments as f32;
        let n = [phi.cos(), phi.sin(), 0.0];
        for (k, z) in [-hh, hh].into_iter().enumerate() {
            v.push(vertex(
                [c[0] + r * n[0], c[1] + r * n[1], c[2] + z],
                n,
                [
                    around * j as f32 / segments as f32,
                    (1 - k) as f32 * 2.0 * hh,
                ],
            ));
        }
    }
    for j in 0..segments {
        let a = 2 * j;
        idx.extend([a, a + 2, a + 1, a + 1, a + 2, a + 3]);
    }
    for (z, nz) in [(-hh, -1.0f32), (hh, 1.0)] {
        let centre = v.len() as u32;
        v.push(vertex([c[0], c[1], c[2] + z], [0.0, 0.0, nz], [r, r]));
        for j in 0..=segments {
            let phi = std::f32::consts::TAU * j as f32 / segments as f32;
            let (x, y) = (phi.cos(), phi.sin());
            v.push(vertex(
                [c[0] + r * x, c[1] + r * y, c[2] + z],
                [0.0, 0.0, nz],
                [r + r * x, r - r * y],
            ));
        }
        for j in 0..segments {
            idx.extend([centre, centre + 1 + j, centre + 2 + j]);
        }
    }
    (v, idx)
}

/// A floor of half side `h` at height 0 (UV one unit across).
fn floor(h: f32) -> (Vec<HdVertex>, Vec<u32>) {
    let up = [0.0, 0.0, 1.0];
    (
        vec![
            vertex([-h, -h, 0.0], up, [0.0, 1.0]),
            vertex([h, -h, 0.0], up, [1.0, 1.0]),
            vertex([h, h, 0.0], up, [1.0, 0.0]),
            vertex([-h, h, 0.0], up, [0.0, 0.0]),
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
}

fn base_scene(quality: Quality) -> PlainScene {
    let mut s = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(quality)),
        time: 0.0,
    };
    s.visible_distance = 400.0;
    s.fog_distance = 300.0;
    s.fog.start = 250.0;
    s.fog.end = 400.0;
    s
}

fn sun(s: &mut PlainScene, towards: [f32; 3]) {
    s.atmosphere = true;
    s.clear = [0.55, 0.65, 0.8];
    let l = towards.iter().map(|v| v * v).sum::<f32>().sqrt();
    s.sun.towards = towards.map(|v| v / l);
    s.sun.diffuse = [0.95, 0.9, 0.82];
    s.sun.ambient = [0.4, 0.43, 0.5];
    s.sun.shadow = [0.3, 0.33, 0.4];
}

/// The materials in a row of spheres and the ground, with `with_material` false the same draws plain.
fn spheres(s: &mut PlainScene, mats: &[Mat3], ground: &Mat3, with_material: bool) {
    let n = mats.len() as f32;
    s.objects.push(object(
        1,
        [0.0; 3],
        40.0,
        floor(40.0),
        ground,
        with_material,
    ));
    for (k, m) in mats.iter().enumerate() {
        let c = [(k as f32 - (n - 1.0) / 2.0) * 2.4, 0.0, 1.0];
        s.objects.push(object(
            k as u32 + 2,
            c,
            1.0,
            sphere(c, 1.0),
            m,
            with_material,
        ));
    }
}

/// Shapes of the materials: boxes, cylinders and a slab, on the ground.
fn shapes(s: &mut PlainScene, mats: &[Mat3], ground: &Mat3, with_material: bool) {
    s.objects.push(object(
        1,
        [0.0; 3],
        40.0,
        floor(40.0),
        ground,
        with_material,
    ));
    let place: [([f32; 3], u8); 6] = [
        ([-4.2, 1.0, 1.0], 0),
        ([-1.6, -0.6, 0.8], 1),
        ([1.2, 1.2, 1.2], 2),
        ([3.8, -0.4, 0.6], 0),
        ([-0.2, 3.8, 0.5], 1),
        ([4.6, 3.4, 1.5], 2),
    ];
    for (k, (m, (c, kind))) in mats.iter().cycle().zip(place).enumerate() {
        let mesh = match kind {
            0 => cuboid(c, [0.8, 0.8, c[2]]),
            1 => cylinder(c, 0.7, c[2]),
            _ => cuboid(c, [1.2, 0.3, c[2]]),
        };
        s.objects
            .push(object(k as u32 + 2, c, 2.0, mesh, m, with_material));
    }
}

/// A roughness by metalness grid of spheres in a white-gray base (roughness rising to the right, metalness down the rows).
fn sweep(s: &mut PlainScene, ground: &Mat3) -> Vec<Mat3> {
    let mut mats = Vec::new();
    for (row, metal) in [0.0f32, 0.5, 1.0].into_iter().enumerate() {
        for (k, rough) in [0.05f32, 0.25, 0.45, 0.65, 0.85, 1.0]
            .into_iter()
            .enumerate()
        {
            let m = HdMaterial::of_factors(&PbrMaterial {
                roughness: rough,
                metal,
                base_factor: [0.9, 0.9, 0.88, 1.0],
                ..PbrMaterial::default()
            });
            let id = s.materials.intern(m);
            let mat = Mat3 {
                name: format!("sweep {metal} {rough}"),
                id,
                base: None,
            };
            let c = [(k as f32 - 2.5) * 2.2, 0.0, 1.0 + (2 - row) as f32 * 2.2];
            let ghost = 100 + (row * 6 + k) as u32;
            s.objects
                .push(object(ghost, c, 1.0, sphere(c, 1.0), &mat, true));
            mats.push(mat);
        }
    }
    s.objects
        .push(object(99, [0.0; 3], 40.0, floor(40.0), ground, true));
    mats
}

/// The lights of the night picture: a warm panel over the spheres, a cool tube to the side.
fn area_lights(s: &mut PlainScene) {
    s.atmosphere = false;
    s.clear = [0.01, 0.012, 0.02];
    s.sun.diffuse = [0.0; 3];
    s.sun.ambient = [0.03, 0.03, 0.04];
    s.sun.shadow = [0.0; 3];
    s.area_lights = vec![
        HdAreaLight {
            light: HdLight {
                position: [-2.0, -1.5, 4.0],
                color: [1.0, 0.86, 0.7].map(|c| c * 3.0),
                radius: 18.0,
                carrier: None,
            },
            shape: AreaShape::Rect {
                half_u: [0.0, 0.8, 0.0],
                half_v: [2.0, 0.0, 0.0],
            },
            shadows: true,
        },
        HdAreaLight {
            light: HdLight {
                position: [5.0, -2.0, 2.0],
                color: [0.7, 0.85, 1.0].map(|c| c * 2.2),
                radius: 14.0,
                carrier: None,
            },
            shape: AreaShape::Tube {
                half_axis: [0.0, 0.0, 1.2],
                radius: 0.03,
            },
            shadows: true,
        },
    ];
    s.lights_generation += 1;
}

fn camera(s: &mut PlainScene, eye: [f32; 3], at: [f32; 3], fov: u8) -> HdCamera {
    let e = look_at(eye, at);
    s.eye = Some(e);
    s.fov = fov;
    s.size = SIZE;
    HdCamera::new(e, f32::from(fov), s.visible_distance, SIZE.0, SIZE.1)
}

fn draw(
    gpu: &Gpu,
    world: &mut HdWorld,
    s: &mut PlainScene,
    cam: &HdCamera,
    target: &torque_render_core::Offscreen,
    frames: usize,
) -> Result<torque_render_core::Frame, String> {
    let mut frame = None;
    for _ in 0..frames {
        s.generation += 1;
        s.time += 1.0 / 30.0;
        world.upload(s);
        world.draw(
            s,
            cam,
            Some((&target.color, target.color.texture().format(), [0, 0])),
        )?;
        frame = Some(gpu.read_back(target).map_err(|e| e.to_string())?);
    }
    frame.ok_or_else(|| "no frame".into())
}

fn write_png(path: &Path, frame: &torque_render_core::Frame) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&frame.rgb()))
        .map_err(|e| e.to_string())
}

/// The gallery's materials: the library's (when given and it holds any) or those made in code, with the ground's last.
fn materials(
    scene: &mut HdScene<StaticCells>,
    lib: Option<&Path>,
) -> Result<(Vec<Mat3>, Mat3), String> {
    let mut mats = match lib {
        Some(dir) => library(dir, scene)?,
        None => Vec::new(),
    };
    if mats.is_empty() {
        if lib.is_some() {
            eprintln!("the library holds no material: the ones made in code are drawn");
        }
        mats = put_synthetic(scene);
    }
    let ground = mats
        .iter()
        .position(|m| {
            ["ground", "gravel", "soil", "terrain"]
                .iter()
                .any(|k| m.name.contains(k))
        })
        .unwrap_or(mats.len() - 1);
    let ground = if mats.len() > 1 {
        mats.remove(ground)
    } else {
        mats[0].clone()
    };
    Ok((mats, ground))
}

/// The frame's and the opaque pass's GPU medians of the shapes in the sun, with and without their materials.
fn cost(gpu: &Gpu, lib: Option<&Path>, quality: Quality) -> Result<(), String> {
    for with_material in [false, true] {
        let mut s = base_scene(quality);
        let (mats, ground) = materials(&mut s.frame, lib)?;
        sun(&mut s, [-0.5, -0.6, 0.62]);
        shapes(&mut s, &mats, &ground, with_material);
        spheres_far(&mut s, &mats, with_material);
        let cam = camera(&mut s, [2.0, -12.0, 4.0], [0.0, 1.0, 1.0], 60);
        let mut world: HdWorld = HdWorld::new(gpu);
        let target = gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?;
        world.reset_history();
        draw(gpu, &mut world, &mut s, &cam, &target, 30)?;
        let mut p = Profile::begin(&mut world);
        for _ in 0..COST_FRAMES {
            draw(gpu, &mut world, &mut s, &cam, &target, 1)?;
            p.frame(&mut world);
        }
        world.profile_passes = false;
        let (rows, total) = p.gpu_medians();
        let opaque = rows.iter().find(|r| r.0 == "opaque").map_or(0.0, |r| r.1);
        println!(
            "{}: opaque {opaque:.3} ms, frame {total:.3} ms ({}x{}, {})",
            if with_material { "materials" } else { "plain" },
            SIZE.0,
            SIZE.1,
            quality.name()
        );
    }
    Ok(())
}

/// The spheres of the row behind the shapes (the cost's scene fills more of the frame with materials).
fn spheres_far(s: &mut PlainScene, mats: &[Mat3], with_material: bool) {
    for (k, m) in mats.iter().enumerate() {
        let c = [(k as f32 - 3.0) * 2.4, 8.0, 1.0];
        s.objects.push(object(
            50 + k as u32,
            c,
            1.0,
            sphere(c, 1.0),
            m,
            with_material,
        ));
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = None;
    let mut lib = None;
    let mut cost_quality = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--library" => lib = it.next().map(PathBuf::from),
            "--cost" => {
                cost_quality = Some(
                    it.next()
                        .and_then(|q| Quality::named(q))
                        .unwrap_or(Quality::Ultra),
                )
            }
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    if let Some(q) = cost_quality {
        return cost(&gpu, lib.as_deref(), q);
    }
    let dir = dir.unwrap_or_else(|| "pbr-gallery".into());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?;
    type Setup = fn(&mut PlainScene, &[Mat3], &Mat3) -> HdCamera;
    let pictures: [(&str, Setup); 5] = [
        ("1-sun-spheres", |s, mats, ground| {
            sun(s, [-0.5, -0.6, 0.62]);
            spheres(s, mats, ground, true);
            camera(s, [0.0, -12.5, 3.6], [0.0, 0.0, 1.0], 60)
        }),
        ("2-sun-shapes", |s, mats, ground| {
            sun(s, [0.7, -0.5, 0.35]);
            shapes(s, mats, ground, true);
            camera(s, [2.0, -12.0, 4.0], [0.0, 1.0, 1.0], 60)
        }),
        ("3-area-lights", |s, mats, ground| {
            area_lights(s);
            spheres(s, mats, ground, true);
            camera(s, [0.0, -12.5, 3.6], [0.0, 0.0, 1.0], 60)
        }),
        ("4-sweep", |s, _, ground| {
            sun(s, [-0.4, -0.7, 0.6]);
            sweep(s, ground);
            camera(s, [0.0, -15.0, 4.2], [0.0, 0.0, 3.2], 50)
        }),
        ("5-relief", |s, mats, ground| {
            sun(s, [0.9, -0.2, 0.3]);
            // The wall made in code, else the first material with a height map.
            let has_height = |m: &&Mat3| s.materials.get(m.id).is_some_and(|h| h.height.is_some());
            let relief = mats
                .iter()
                .find(|m| m.name == "wall")
                .or_else(|| mats.iter().find(has_height))
                .unwrap_or(&mats[0]);
            s.objects
                .push(object(1, [0.0; 3], 40.0, floor(40.0), ground, true));
            let c = [0.0, 2.0, 1.5];
            s.objects
                .push(object(2, c, 3.0, cuboid(c, [2.5, 0.25, 1.5]), relief, true));
            camera(s, [-1.8, -1.6, 1.0], [0.6, 2.0, 0.6], 60)
        }),
    ];
    for (name, setup) in pictures {
        let mut s = base_scene(Quality::Ultra);
        let (mats, ground) = materials(&mut s.frame, lib.as_deref())?;
        let cam = setup(&mut s, &mats, &ground);
        let mut world: HdWorld = HdWorld::new(&gpu);
        world.reset_history();
        let frame = draw(&gpu, &mut world, &mut s, &cam, &target, FRAMES)
            .map_err(|e| format!("{name}: {e}"))?;
        let path = dir.join(format!("{name}.png"));
        write_png(&path, &frame)?;
        println!("{}", path.display());
    }
    Ok(())
}
