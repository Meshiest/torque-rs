//! The view's vector math: shots against an interior's faces, the camera boom, an interior's panes and a frame of the map's
//! interiors.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_app::camera_boom::CameraBoom;
use torque_app::ghosts::first_hit;
use torque_app::interiors::{Interiors, panes};
use torque_app::view::WorldView;
use torque_ghost_world::tge::TgeClientSet;
use torque_ghost_world::world::{ClientWorld, Ghost};
use torque_ghosts::interior::{InteriorInit, InteriorInstance};
use torque_ghosts::{F32, TgeClass, TgeRecord};
use torque_interior_builder::{Brush, Options, TexGen, build, materials};
use torque_render_core::Gpu;
use torque_render_scene::decals::Triangle;
use torque_render_scene::{Assets, Vertex};

/// A room's faces: a wavy floor and four walls, `n` triangles in all.
fn room(n: usize) -> Vec<Triangle> {
    let per_side = n / 5;
    let side = ((per_side / 2) as f32).sqrt().ceil() as usize;
    let step = 40.0 / side as f32;
    let mut out = Vec::with_capacity(n);
    for wall in 0..5 {
        let place = |u: f32, v: f32| -> [f32; 3] {
            let (u, v) = (u - 20.0, v);
            match wall {
                0 => [u, v - 20.0, 0.2 * (u * 0.5).sin()],
                1 => [u, 20.0, v * 0.25],
                2 => [u, -20.0, v * 0.25],
                3 => [20.0, u, v * 0.25],
                _ => [-20.0, u, v * 0.25],
            }
        };
        for i in 0..side {
            for j in 0..side {
                let (u0, v0) = (i as f32 * step, j as f32 * step);
                let (u1, v1) = (u0 + step, v0 + step);
                out.push([place(u0, v0), place(u1, v0), place(u1, v1)]);
                out.push([place(u0, v0), place(u1, v1), place(u0, v1)]);
            }
        }
    }
    out.truncate(n);
    out
}

/// Where 64 players stand and look.
fn players() -> Vec<([f32; 3], [f32; 3])> {
    (0..64)
        .map(|i| {
            let a = i as f32 * 0.37;
            let at = [12.0 * a.cos(), 12.0 * a.sin(), 1.6];
            let d = [-(a * 1.3).cos(), -(a * 1.3).sin(), -0.2 + 0.01 * i as f32];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            (at, d.map(|c| c / l))
        })
        .collect()
}

fn shots(c: &mut Criterion) {
    let faces = room(10_000);
    let players = players();
    c.bench_function("shots 64 players x 10k faces", |b| {
        b.iter(|| {
            let mut hits = 0;
            for &(at, d) in &players {
                let to = [0, 1, 2].map(|k| at[k] + d[k] * 60.0);
                hits += usize::from(first_hit(at, to, &faces).is_some());
            }
            black_box(hits)
        })
    });
}

fn booms(c: &mut Criterion) {
    let faces = room(2_000);
    let players = players();
    c.bench_function("camera boom 64 players x 2k faces", |b| {
        b.iter(|| {
            let mut sum = 0.0;
            for &(pivot, d) in &players {
                let mut boom = CameraBoom::new(0.5, 0.3);
                let back = d.map(|c| -c);
                let p = boom.follow(pivot, back, 4.0, 1.0 / 60.0, |from, to| {
                    first_hit(from, to, &faces)
                });
                sum += p[0] + p[1] + p[2];
            }
            black_box(sum)
        })
    });
}

fn interior_panes(c: &mut Criterion) {
    let vertices: Vec<Vertex> = room(30_000)
        .iter()
        .flatten()
        .map(|&position| Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
            uv: [0.0; 2],
        })
        .collect();
    c.bench_function("interior panes 30k triangles", |b| {
        b.iter(|| black_box(panes(&vertices).len()))
    });
}

fn cube(min: [f64; 3], max: [f64; 3], material: &str) -> Brush {
    Brush::cuboid(min.into(), max.into(), material, |n| TexGen::world(n, 4.0))
}

/// A floor with forty glass blocks on it (see-through: drawn each frame as panes).
fn glass_yard() -> Vec<u8> {
    let mut brushes = vec![cube([-20.0, -20.0, -1.0], [20.0, 20.0, 0.0], "floor")];
    for i in 0..40 {
        let x = -19.0 + f64::from(i) * 0.9;
        brushes.push(cube([x, -2.0, 0.0], [x + 0.5, 2.0, 3.0], "glass"));
    }
    let mut built = build(&brushes, &Options::default()).expect("the yard builds");
    let flags = BTreeMap::from([(
        "glass".to_string(),
        torque_maps::dif::surface_flags::TRANSLUCENT,
    )]);
    materials::mark(&mut built.dif, &flags);
    built.bytes().expect("the yard writes")
}

struct Yard(Vec<u8>);

impl Assets for Yard {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        path.eq_ignore_ascii_case("bench/yard.dif")
            .then(|| self.0.clone())
    }
}

/// 100 interiors of the glass yard, 50 units apart.
fn yards() -> ClientWorld<TgeClientSet> {
    let mut w = ClientWorld::<TgeClientSet>::default();
    for i in 0..100u32 {
        let mut matrix = [F32::new(0.0); 16];
        for k in [0, 5, 10, 15] {
            matrix[k] = F32::new(1.0);
        }
        matrix[3] = F32::new((i % 10) as f32 * 50.0);
        matrix[7] = F32::new((i / 10) as f32 * 50.0);
        let record = InteriorInstance {
            init: Some(InteriorInit {
                crc: 0,
                file: b"bench/yard.dif".to_vec(),
                show_terrain_inside: false,
                matrix,
                scale: [F32::new(1.0); 3],
                alarm: false,
                skin_base: Vec::new(),
                audio_profile: None,
                audio_environment: None,
                gl_lighting: false,
            }),
            transform: None,
            alarm: false,
        };
        let ghost = Ghost {
            class: Some(TgeClass::InteriorInstance),
            serial: i as usize + 1,
            always: false,
            record: Some(TgeRecord::InteriorInstance(record)),
            part: (),
            position: None,
            updates: 0,
        };
        w.ghosts.insert(i + 1, ghost);
    }
    w
}

/// A frame's interiors when 100 are kept and none changed: what the view is handed every frame.
fn interiors_update(c: &mut Criterion) {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("interiors update skipped: {e}");
            return;
        }
    };
    let mut view = WorldView::new(&gpu);
    let files = Yard(glass_yard());
    let world = yards();
    let mut interiors = Interiors::default();
    let panes = interiors.update(&mut view, &files, &world, &mut |_, _| {});
    assert!(!panes.is_empty(), "the yards have panes");
    view.set_objects(panes);
    // Each frame's panes are handed to the view, as a frame does: the next frame's are made from them.
    c.bench_function("interiors update 100 kept", |b| {
        b.iter(|| {
            let panes = interiors.update(&mut view, &files, &world, &mut |_, _| {});
            let n = panes.len();
            view.set_objects(panes);
            black_box(n)
        })
    });
}

criterion_group!(benches, shots, booms, interior_panes, interiors_update);
criterion_main!(benches);
