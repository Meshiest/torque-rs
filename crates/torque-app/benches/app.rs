//! The view's vector math: shots against an interior's faces, the camera boom and an interior's panes.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_app::camera_boom::CameraBoom;
use torque_app::ghosts::first_hit;
use torque_app::interiors::panes;
use torque_render_scene::Vertex;
use torque_render_scene::decals::Triangle;

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

criterion_group!(benches, shots, booms, interior_panes);
criterion_main!(benches);
