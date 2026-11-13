//! The voxel scene against the CPU: the voxels a fixed set of triangles makes are the same bytes every time, hold every voxel
//! the exact triangle-box test fills, and the shaders' constants are the clipmap's.

use glam::{Mat3, Vec3};
use torque_render_core::{Gpu, wgpu};
use torque_render_scene::hd::cells::{Batch, Cell, SlotTexture, StaticCells};
use torque_render_scene::hd::{HdScene, HdVertex, material, oct_encode};
use torque_render_scene::{Filter, Sampler};

use super::Gi;
use super::clipmap::{LEVEL_TILES, N, TILE, TILE_VOXELS, voxel_size};
use super::tribox;

pub(crate) fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// A scene of one cell holding `tris`.
fn scene_of(tris: &[[Vec3; 3]]) -> HdScene<StaticCells> {
    scene_with(tris, 0)
}

/// [`scene_of`] with `bits` in every vertex's material word.
pub(crate) fn scene_with(tris: &[[Vec3; 3]], bits: u32) -> HdScene<StaticCells> {
    let mut vertices = Vec::with_capacity(tris.len() * 3);
    for t in tris {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or(Vec3::Z);
        for p in t {
            vertices.push(HdVertex {
                position: p.to_array(),
                normal: oct_encode(n.to_array()),
                uv: [0.0; 2],
                color: [200, 150, 100, 255],
                material: (material::KIND_SHAPE << material::KIND_SHIFT) | bits,
                extra: 0,
            });
        }
    }
    let indices: Vec<u32> = (0..vertices.len() as u32).collect();
    let mut cells = StaticCells::default();
    cells.slots.insert(
        0,
        SlotTexture {
            texture: None,
            sampler: Sampler {
                repeat: true,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        },
    );
    cells.cells.insert(
        [0, 0, 0],
        Cell {
            key: [0, 0, 0],
            bounds: [[-16.0; 3], [16.0; 3]],
            batches: vec![Batch {
                slot: 0,
                translucent: false,
                first: 0,
                count: indices.len() as u32,
            }],
            vertices,
            indices,
            clusters: Vec::new(),
            generation: 1,
            resident: false,
            ext: (),
        },
    );
    HdScene::new(cells)
}

/// The eye the levels are placed about.
const EYE: [f32; 3] = [1.0, 1.0, 1.0];

/// Every tile of the scene made in one frame (the tier's levels: Ultra's four).
fn voxelize(gpu: &Gpu, scene: &HdScene<StaticCells>) -> (Gi, [Vec<u8>; 3]) {
    let (device, queue) = (gpu.device(), gpu.queue());
    let mut gi = Gi::new(device);
    gi.converge = true;
    let dark = super::GiLights {
        bytes: &[],
        count: 0,
        area_start: 0,
        glow: (0, 0),
        sun: None,
    };
    let frame = super::GiFrame {
        scene,
        eye: EYE,
        tier: Some(super::GiTier::ULTRA),
        lights: &dark,
    };
    assert!(gi.prepare(device, queue, None, frame));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    gi.encode_voxels(device, &mut encoder);
    queue.submit([encoder.finish()]);
    assert_eq!(gi.pending(), 0, "every tile made in the frame");
    let bytes = gi.read_voxels(device, queue).expect("the voxels read back");
    (gi, bytes)
}

pub(crate) fn word(bytes: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().expect("four bytes"))
}

/// Whether the GPU's buffers hold world voxel `v` of `level` filled.
pub(crate) fn filled(bytes: &[Vec<u8>; 3], level: u32, v: [i32; 3]) -> bool {
    let t = v.map(|c| c.div_euclid(TILE));
    let s = t.map(|c| c.rem_euclid(N / TILE));
    let tiles = N / TILE;
    let slot =
        level as usize * LEVEL_TILES as usize + (s[0] + tiles * (s[1] + tiles * s[2])) as usize;
    let tag = t.iter().enumerate().fold(0x8000_0000u32, |acc, (k, c)| {
        acc | ((*c as u32 & 1023) << (10 * k))
    });
    if word(&bytes[0], 2 * slot) != tag {
        return false;
    }
    let l = v.map(|c| c.rem_euclid(TILE));
    let i = (l[0] + TILE * (l[1] + TILE * l[2])) as usize;
    word(&bytes[1], slot * (TILE_VOXELS as usize / 32) + i / 32) & (1 << (i % 32)) != 0
}

pub(crate) fn quad(c: Vec3, u: Vec3, v: Vec3) -> [[Vec3; 3]; 2] {
    let p = [c - u - v, c + u - v, c + u + v, c - u + v];
    [[p[0], p[1], p[2]], [p[0], p[2], p[3]]]
}

fn box_tris(center: Vec3, half: Vec3, turn: Mat3) -> Vec<[Vec3; 3]> {
    let mut out = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut n = Vec3::ZERO;
            n[axis] = sign * half[axis];
            let mut u = Vec3::ZERO;
            u[(axis + 1) % 3] = half[(axis + 1) % 3];
            let mut v = Vec3::ZERO;
            v[(axis + 2) % 3] = half[(axis + 2) % 3] * sign;
            out.extend(quad(center + turn * n, turn * u, turn * v));
        }
    }
    out
}

/// A sphere of `r` about `c`: an icosahedron's faces cut in four `levels` times, pushed onto the sphere.
fn sphere(c: Vec3, r: f32, levels: u32) -> Vec<[Vec3; 3]> {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let v = [
        Vec3::new(-1.0, t, 0.0),
        Vec3::new(1.0, t, 0.0),
        Vec3::new(-1.0, -t, 0.0),
        Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t),
        Vec3::new(0.0, 1.0, t),
        Vec3::new(0.0, -1.0, -t),
        Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0),
        Vec3::new(t, 0.0, 1.0),
        Vec3::new(-t, 0.0, -1.0),
        Vec3::new(-t, 0.0, 1.0),
    ];
    let f = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    let mut tris: Vec<[Vec3; 3]> = f.iter().map(|t| t.map(|i| v[i].normalize())).collect();
    for _ in 0..levels {
        tris = tris
            .iter()
            .flat_map(|t| {
                let m = [
                    ((t[0] + t[1]) * 0.5).normalize(),
                    ((t[1] + t[2]) * 0.5).normalize(),
                    ((t[2] + t[0]) * 0.5).normalize(),
                ];
                [
                    [t[0], m[0], m[2]],
                    [m[0], t[1], m[1]],
                    [m[2], m[1], t[2]],
                    [m[0], m[1], m[2]],
                ]
            })
            .collect();
    }
    tris.iter().map(|t| t.map(|p| c + p * r)).collect()
}

/// A hashed number in [0, 1) of `i`.
fn hash01(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// The six test meshes: a box on no voxel boundary, a turned box, slivers, a sphere, a slanted plane, tiny triangles.
fn meshes() -> Vec<(&'static str, Vec<[Vec3; 3]>)> {
    let turned = Mat3::from_rotation_z(0.52) * Mat3::from_rotation_x(0.35);
    let slivers = (0..24)
        .map(|i| {
            let k = i as f32;
            [
                Vec3::new(-2.0, 0.1 * k, 0.03 * k),
                Vec3::new(4.0, 0.1 * k + 0.004, 0.03 * k + 0.37),
                Vec3::new(4.0, 0.1 * k + 0.011, 0.03 * k + 0.36),
            ]
        })
        .collect();
    let slanted = {
        let turn = Mat3::from_axis_angle(Vec3::new(1.0, 2.0, 0.5).normalize(), 0.65);
        quad(
            Vec3::new(0.3, 0.9, 1.1),
            turn * Vec3::new(3.0, 0.0, 0.0),
            turn * Vec3::new(0.0, 3.0, 0.0),
        )
        .to_vec()
    };
    let tiny = (0..500)
        .map(|i| {
            let p = Vec3::new(hash01(3 * i), hash01(3 * i + 1), hash01(3 * i + 2)) * 3.0 - 0.5;
            let d = Vec3::new(hash01(7 * i + 5), hash01(7 * i + 6), hash01(7 * i + 3)) * 0.02;
            [
                p,
                p + Vec3::new(d.x, 0.0, d.z),
                p + Vec3::new(0.0, d.y, d.x),
            ]
        })
        .collect();
    vec![
        (
            "box",
            box_tris(
                Vec3::new(1.4, 1.1, 0.77),
                Vec3::new(1.3, 0.8, 0.565),
                Mat3::IDENTITY,
            ),
        ),
        (
            "turned box",
            box_tris(Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.2, 0.7, 0.5), turned),
        ),
        ("slivers", slivers),
        ("sphere", sphere(Vec3::new(0.9, 1.2, 0.8), 2.3, 2)),
        ("slanted plane", slanted),
        ("tiny triangles", tiny),
    ]
}

#[test]
fn no_voxel_the_cpu_fills_is_missed() {
    let Some(gpu) = gpu() else {
        return;
    };
    for (name, tris) in meshes() {
        let (gi, bytes) = voxelize(&gpu, &scene_of(&tris));
        assert_eq!(
            gi.sources().corners().len(),
            tris.len(),
            "{name}: every triangle kept"
        );
        for level in 0..2 {
            let origin = gi.clipmap().origins[level as usize];
            let cpu = tribox::voxelize(&tris, voxel_size(level), origin, N);
            assert!(
                !cpu.is_empty(),
                "{name}: the CPU fills voxels at level {level}"
            );
            let missed = cpu.iter().filter(|v| !filled(&bytes, level, **v)).count();
            let cpu_set: std::collections::BTreeSet<[i32; 3]> = cpu.iter().copied().collect();
            let mut extra = 0;
            for z in origin[2]..origin[2] + N {
                for y in origin[1]..origin[1] + N {
                    for x in origin[0]..origin[0] + N {
                        if filled(&bytes, level, [x, y, z]) && !cpu_set.contains(&[x, y, z]) {
                            extra += 1;
                        }
                    }
                }
            }
            eprintln!(
                "{name} level {level}: {} voxels by the CPU, {missed} missed, {extra} more on the GPU",
                cpu.len()
            );
            assert_eq!(
                missed, 0,
                "{name} level {level}: voxels the CPU fills left empty"
            );
            assert!(
                extra * 50 <= cpu.len(),
                "{name} level {level}: {extra} voxels more than the CPU's {}",
                cpu.len()
            );
        }
    }
}

#[test]
fn voxelizing_twice_gives_the_same_bytes() {
    let Some(gpu) = gpu() else {
        return;
    };
    let tris: Vec<[Vec3; 3]> = meshes().into_iter().flat_map(|m| m.1).collect();
    let scene = scene_of(&tris);
    let (_, a) = voxelize(&gpu, &scene);
    let (_, b) = voxelize(&gpu, &scene);
    for k in 0..3 {
        assert!(a[k] == b[k], "buffer {k} differs between two runs");
    }
    assert!(a[2].iter().any(|&b| b != 0), "voxels were written");
}

#[test]
fn the_shader_constants_are_the_clipmaps() {
    let text = include_str!("../../shaders/gi_common.wgsl");
    let constant = |name: &str| -> i64 {
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("const {name}:")))
            .unwrap_or_else(|| panic!("{name} in gi_common.wgsl"));
        let value = line
            .split('=')
            .nth(1)
            .expect("a value")
            .trim()
            .trim_end_matches(';')
            .trim_end_matches('u');
        value.parse().expect("a number")
    };
    assert_eq!(constant("GI_N"), i64::from(N));
    assert_eq!(constant("GI_TILE"), i64::from(TILE));
    assert_eq!(constant("GI_TILES"), i64::from(N / TILE));
    assert_eq!(constant("GI_TILE_VOXELS"), TILE_VOXELS as i64);
    assert_eq!(constant("GI_LEVEL_TILES"), LEVEL_TILES as i64);
    assert_eq!(constant("GI_TILE_WORDS"), TILE_VOXELS as i64 / 32);
    assert_eq!(constant("GI_MAX_LEVELS"), super::clipmap::MAX_LEVELS as i64);
}

#[test]
fn rgb9e5_round_trips_within_its_mantissa() {
    for c in [[1.0, 0.5, 0.25], [0.003, 0.2, 7.5], [100.0, 0.0, 1e-4]] {
        let back = super::sources::unpack_rgb9e5(super::sources::pack_rgb9e5(c));
        let m = c.iter().fold(0f32, |a, b| a.max(*b));
        for k in 0..3 {
            assert!((back[k] - c[k]).abs() <= m / 256.0, "{c:?} -> {back:?}");
        }
    }
}

#[test]
fn the_probe_constants_are_the_grids() {
    let text = include_str!("../../shaders/gi_grid.wgsl");
    let constant = |name: &str| -> i64 {
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("const {name}:")))
            .unwrap_or_else(|| panic!("{name} in gi_grid.wgsl"));
        let value = line.split('=').nth(1).expect("a value").trim();
        value
            .trim_end_matches(';')
            .trim_end_matches('u')
            .split('.')
            .next()
            .expect("a number")
            .parse()
            .expect("a number")
    };
    let p = super::probes::PROBES;
    assert_eq!(
        [
            constant("GI_PROBES_X"),
            constant("GI_PROBES_Y"),
            constant("GI_PROBES_Z")
        ],
        p.map(i64::from)
    );
    assert_eq!(
        constant("GI_CASCADE_PROBES"),
        i64::from(super::probes::CASCADE_PROBES)
    );
    assert_eq!(constant("GI_IRR_TILE"), i64::from(super::probes::IRR_TILE));
    assert_eq!(
        constant("GI_DEPTH_TILE"),
        i64::from(super::probes::DEPTH_TILE)
    );
    assert_eq!(
        constant("GI_IRR_INNER"),
        i64::from(super::probes::IRR_TILE) - 2
    );
    assert_eq!(
        constant("GI_DEPTH_INNER"),
        i64::from(super::probes::DEPTH_TILE) - 2
    );
}

#[test]
fn the_forward_pass_changes_one_line() {
    use crate::shaders::{GI_HOOK, Permutation};
    for (clustered, front_layer) in [(true, false), (false, false), (true, true), (false, true)] {
        let plain = Permutation::Forward {
            clustered,
            front_layer,
        }
        .source();
        assert_eq!(plain.matches(GI_HOOK.0).count(), 1, "the hook's line, once");
        assert!(
            !plain.contains("gi_"),
            "the plain forward module holds none of the global illumination"
        );
        let gi = Permutation::GiForward {
            clustered,
            front_layer,
        }
        .source();
        assert!(gi.starts_with(&plain.replacen(GI_HOOK.0, GI_HOOK.1, 1)));
    }
}
