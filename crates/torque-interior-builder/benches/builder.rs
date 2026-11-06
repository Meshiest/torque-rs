//! The builder's geometry: a convex hull, a brush interior and a mesh interior.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_interior_builder::convex::hull_planes;
use torque_interior_builder::geom::Budget;
use torque_interior_builder::mesh::{MeshInput, MeshOptions, NamedMesh, Tri, build_mesh};
use torque_interior_builder::{Brush, Options, TexGen, V3, build};

/// `n` points on and inside a lumpy ball (a fixed sequence).
fn ball(n: usize) -> Vec<V3> {
    let mut s = 0x2545_f491_u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..n)
        .map(|_| {
            let (u, v, w) = (next() * 2.0 - 1.0, next() * std::f64::consts::TAU, next());
            let r = 10.0 * (0.6 + 0.4 * w.sqrt());
            let q = (1.0 - u * u).sqrt();
            [r * q * v.cos(), r * q * v.sin(), r * u * 0.8].into()
        })
        .collect()
}

fn hull(c: &mut Criterion) {
    let points = ball(4_000);
    c.bench_function("hull 4k points", |b| {
        b.iter(|| {
            let mut work = Budget::new(u64::MAX);
            black_box(hull_planes(&points, &mut work).expect("a hull").len())
        })
    });
}

fn cube(center: [f64; 3], size: [f64; 3], material: &str) -> Brush {
    let min = [0, 1, 2].map(|k| center[k] - size[k] * 0.5);
    let max = [0, 1, 2].map(|k| center[k] + size[k] * 0.5);
    Brush::cuboid(min.into(), max.into(), material, |n| TexGen::world(n, 4.0))
}

/// The example kit's arena: a walled floor, a block in the middle, a ring of cover, two stepped platforms and two stacks of
/// crates.
fn arena() -> Vec<Brush> {
    let (floor, wall) = ("arena_floor", "arena_wall");
    let mut out = vec![
        cube([0.0, 0.0, -0.5], [40.0, 40.0, 1.0], floor),
        cube([0.0, 20.5, 1.5], [42.0, 1.0, 3.0], wall),
        cube([0.0, -20.5, 1.5], [42.0, 1.0, 3.0], wall),
        cube([20.5, 0.0, 1.5], [1.0, 40.0, 3.0], wall),
        cube([-20.5, 0.0, 1.5], [1.0, 40.0, 3.0], wall),
        cube([0.0, 0.0, 1.3], [3.0, 3.0, 2.6], wall),
    ];
    for i in 0..8 {
        let a = f64::from(i) * std::f64::consts::FRAC_PI_4;
        let size = if i % 2 == 0 {
            [1.0, 4.0, 2.4]
        } else {
            [2.5, 2.5, 2.4]
        };
        out.push(cube([a.cos() * 9.0, a.sin() * 9.0, 1.2], size, wall));
    }
    for side in [-1.0, 1.0] {
        let c = 13.0 * side;
        out.push(cube([c, c, 0.75], [6.0, 6.0, 1.5], wall));
        out.push(cube([c - 4.0 * side, c, 0.25], [2.0, 4.0, 0.5], wall));
        out.push(cube([c - 3.5 * side, c, 0.75], [1.0, 4.0, 0.5], wall));
    }
    for side in [-1.0, 1.0] {
        out.push(cube([14.0 * side, -14.0 * side, 0.6], [1.2; 3], wall));
        out.push(cube([15.2 * side, -13.4 * side, 0.6], [1.2; 3], wall));
        out.push(cube([14.6 * side, -13.7 * side, 1.8], [1.2; 3], wall));
    }
    out
}

fn brushes(c: &mut Criterion) {
    let arena = arena();
    c.bench_function("arena brush build", |b| {
        b.iter(|| {
            let built = build(&arena, &Options::default()).expect("the arena builds");
            black_box(built.bytes().expect("its bytes").len())
        })
    });
}

/// A box's twelve triangles, clockwise seen from outside.
fn box_tris(lo: [f64; 3], hi: [f64; 3], out: &mut Vec<Tri>) {
    let c = |i: usize| {
        [
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ]
    };
    let uv = |p: [f64; 3]| [p[0] + p[2], p[1] - p[2]];
    for f in [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ] {
        let [a, b, cc, d] = f.map(c);
        for p in [[a, b, cc], [a, cc, d]] {
            out.push(Tri {
                p: p.map(Into::into),
                uv: p.map(uv),
                material: 0,
            });
        }
    }
}

/// About 200,000 triangles: a field of separate boxes.
fn box_field() -> MeshInput {
    let mut tris = Vec::new();
    let side = 118;
    for i in 0..side {
        for j in 0..side {
            let (x, y) = (i as f64 * 2.0, j as f64 * 2.0);
            let h = 1.0 + ((i * 7 + j * 3) % 5) as f64 * 0.25;
            box_tris([x, y, 0.0], [x + 1.0, y + 1.0, h], &mut tris);
        }
    }
    MeshInput {
        drawn: vec![NamedMesh {
            name: "field".into(),
            tris,
        }],
        materials: vec!["wall".into()],
        ..MeshInput::default()
    }
}

fn mesh(c: &mut Criterion) {
    let input = box_field();
    let mut group = c.benchmark_group("mesh");
    group.sample_size(10);
    group.bench_function("mesh build 200k triangles", |b| {
        b.iter(|| {
            let built = build_mesh(&input, &MeshOptions::default()).expect("the field builds");
            black_box(built.pieces.len())
        })
    });
    group.finish();
}

criterion_group!(benches, hull, brushes, mesh);
criterion_main!(benches);
