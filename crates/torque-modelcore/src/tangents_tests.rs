use super::*;

fn dot3(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: Vec3) -> Vec3 {
    let l = dot3(a, a).sqrt();
    a.map(|v| v / l)
}

/// A flat face's reference: its points `o + a e1 + b e2` for a grid of `a`, `b` in 0..=1, UVs `m (a, b) + c`, and the
/// analytic tangent (`dP/du` made unit) with the handedness of `dP/dv`.
struct Face {
    o: Vec3,
    e1: Vec3,
    e2: Vec3,
    m: [[f32; 2]; 2],
    c: [f32; 2],
}

/// A mesh and its reference tangents.
#[derive(Default)]
struct Mesh {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    reference: Vec<[f32; 4]>,
}

impl Mesh {
    fn add(&mut self, f: &Face, cells: u32) {
        let n = norm(cross3(f.e1, f.e2));
        let det = f.m[0][0] * f.m[1][1] - f.m[0][1] * f.m[1][0];
        // (a, b) = M^-1 (uv - c): dP/du and dP/dv are M^-1's columns over e1 and e2.
        let inv = [
            [f.m[1][1] / det, -f.m[0][1] / det],
            [-f.m[1][0] / det, f.m[0][0] / det],
        ];
        let along = |col: usize| -> Vec3 {
            std::array::from_fn(|k| f.e1[k] * inv[0][col] + f.e2[k] * inv[1][col])
        };
        let (dpdu, dpdv) = (along(0), along(1));
        let t = norm(dpdu);
        let w = if dot3(cross3(n, t), dpdv) < 0.0 {
            -1.0
        } else {
            1.0
        };
        let base = self.positions.len() as u32;
        for j in 0..=cells {
            for i in 0..=cells {
                let (a, b) = (i as f32 / cells as f32, j as f32 / cells as f32);
                self.positions
                    .push(std::array::from_fn(|k| f.o[k] + a * f.e1[k] + b * f.e2[k]));
                self.normals.push(n);
                self.uvs.push([
                    f.m[0][0] * a + f.m[0][1] * b + f.c[0],
                    f.m[1][0] * a + f.m[1][1] * b + f.c[1],
                ]);
                self.reference.push([t[0], t[1], t[2], w]);
            }
        }
        let row = cells + 1;
        for j in 0..cells {
            for i in 0..cells {
                let v = base + j * row + i;
                self.indices
                    .extend([v, v + row, v + 1, v + 1, v + row, v + row + 1]);
            }
        }
    }
}

/// Twenty flat meshes: grids of every orientation with plain, scaled, turned, sheared and mirrored UVs, and boxes (six faces,
/// each its own vertices).
fn meshes() -> Vec<Mesh> {
    let uvs: [([[f32; 2]; 2], [f32; 2]); 5] = [
        ([[1.0, 0.0], [0.0, 1.0]], [0.0, 0.0]),
        ([[4.0, 0.0], [0.0, 2.0]], [0.5, -1.0]),
        ([[0.6, -0.8], [0.8, 0.6]], [0.0, 0.0]),
        ([[1.0, 0.5], [0.0, 1.0]], [0.25, 0.0]),
        ([[-1.0, 0.0], [0.0, 1.0]], [1.0, 0.0]),
    ];
    let planes: [(Vec3, Vec3); 3] = [
        ([2.0, 0.0, 0.0], [0.0, 3.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.5]),
        ([1.0, 1.0, 0.0], [0.0, 0.0, -2.0]),
    ];
    let mut out = Vec::new();
    for (k, (e1, e2)) in planes.iter().enumerate() {
        for (m, c) in uvs.iter().take(5) {
            let mut mesh = Mesh::default();
            mesh.add(
                &Face {
                    o: [k as f32, -1.0, 0.5],
                    e1: *e1,
                    e2: *e2,
                    m: *m,
                    c: *c,
                },
                4 + k as u32,
            );
            out.push(mesh);
        }
    }
    for (m, c) in uvs.iter() {
        let mut mesh = Mesh::default();
        let s = 1.5;
        let faces: [(Vec3, Vec3, Vec3); 6] = [
            ([0.0, 0.0, 0.0], [0.0, s, 0.0], [s, 0.0, 0.0]),
            ([0.0, 0.0, s], [s, 0.0, 0.0], [0.0, s, 0.0]),
            ([0.0, 0.0, 0.0], [s, 0.0, 0.0], [0.0, 0.0, s]),
            ([0.0, s, 0.0], [0.0, 0.0, s], [s, 0.0, 0.0]),
            ([0.0, 0.0, 0.0], [0.0, 0.0, s], [0.0, s, 0.0]),
            ([s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]),
        ];
        for (o, e1, e2) in faces {
            mesh.add(
                &Face {
                    o,
                    e1,
                    e2,
                    m: *m,
                    c: *c,
                },
                2,
            );
        }
        out.push(mesh);
    }
    out
}

#[test]
fn mikktspace_tangents_are_the_reference_within_1e_4_on_twenty_meshes() {
    let meshes = meshes();
    assert_eq!(meshes.len(), 20);
    let mut worst = 0.0f32;
    let mut flipped = 0;
    for (k, m) in meshes.iter().enumerate() {
        let t = generate(&m.positions, &m.normals, &m.uvs, &m.indices);
        assert_eq!(t.len(), m.positions.len());
        for (got, want) in t.iter().zip(&m.reference) {
            let d = (0..3).map(|c| (got[c] - want[c]).abs()).fold(0.0, f32::max);
            worst = worst.max(d);
            if got[3] != want[3] {
                flipped += 1;
                eprintln!("mesh {k}: handedness {} for {}", got[3], want[3]);
            }
        }
    }
    assert!(worst <= 1e-4, "worst component difference {worst}");
    assert_eq!(flipped, 0);
}

#[test]
fn a_spheres_tangents_are_unit_perpendicular_and_follow_its_longitude() {
    let (rings, segments) = (32u32, 64u32);
    let mut m = Mesh::default();
    for i in 0..=rings {
        let theta = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..=segments {
            let phi = std::f32::consts::TAU * j as f32 / segments as f32;
            let n = [
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            ];
            m.positions.push(n);
            m.normals.push(n);
            m.uvs
                .push([j as f32 / segments as f32, i as f32 / rings as f32]);
        }
    }
    let row = segments + 1;
    for i in 0..rings {
        for j in 0..segments {
            let v = i * row + j;
            m.indices
                .extend([v, v + 1, v + row, v + 1, v + row + 1, v + row]);
        }
    }
    let t = generate(&m.positions, &m.normals, &m.uvs, &m.indices);
    let mut worst_angle = 0.0f32;
    for (i, (tv, n)) in t.iter().zip(&m.normals).enumerate() {
        let tt = [tv[0], tv[1], tv[2]];
        assert!((dot3(tt, tt) - 1.0).abs() < 1e-5);
        assert!(dot3(tt, *n).abs() < 1e-4);
        let ring = i as u32 / row;
        if ring == 0 || ring == rings {
            continue;
        }
        let phi = std::f32::consts::TAU * (i as u32 % row) as f32 / segments as f32;
        let along = [-phi.sin(), phi.cos(), 0.0];
        worst_angle = worst_angle.max(dot3(tt, along).clamp(-1.0, 1.0).acos());
    }
    assert!(
        worst_angle.to_degrees() < 3.0,
        "{}",
        worst_angle.to_degrees()
    );
}

#[test]
fn hostile_indices_and_missing_arrays_give_a_frame_not_a_panic() {
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let t = generate(&positions, &[], &[], &[0, 1, 2, 0, 1, 99, 7]);
    assert_eq!(t.len(), 3);
    assert!(t.iter().all(|v| v.iter().all(|c| c.is_finite())));
    assert!(generate(&[], &[], &[], &[0, 1, 2]).is_empty());
}

#[test]
fn nan_and_infinite_inputs_give_finite_frames() {
    let positions = [
        [0.0, 0.0, 0.0],
        [f32::NAN, 0.0, 0.0],
        [0.0, f32::INFINITY, 0.0],
        [1.0, 1.0, f32::NEG_INFINITY],
    ];
    let normals = [
        [0.0, 0.0, 1.0],
        [f32::NAN; 3],
        [0.0, 0.0, f32::INFINITY],
        [0.0, 1.0, 0.0],
    ];
    let uvs = [
        [0.0, 0.0],
        [1.0, f32::NAN],
        [f32::INFINITY, 1.0],
        [0.5, 0.5],
    ];
    let t = generate(&positions, &normals, &uvs, &[0, 1, 2, 1, 2, 3, 0, 2, 3]);
    assert_eq!(t.len(), 4);
    assert!(t.iter().flatten().all(|c| c.is_finite()), "{t:?}");
}

#[test]
fn a_fan_past_the_valence_cap_gets_face_frames_not_a_stack_overflow() {
    // 50,000 triangles about one vertex: MikkTSpace's grouping would recurse 50,000 deep.
    let n = 50_000u32;
    let mut positions = vec![[0.0f32, 0.0, 0.0]];
    let mut uvs = vec![[0.5f32, 0.5]];
    for k in 0..=n {
        let a = std::f32::consts::TAU * k as f32 / n as f32 * 0.999;
        positions.push([a.cos(), a.sin(), 0.0]);
        uvs.push([0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()]);
    }
    let normals = vec![[0.0f32, 0.0, 1.0]; positions.len()];
    let indices: Vec<u32> = (0..n).flat_map(|k| [0, k + 1, k + 2]).collect();
    // On a small stack, as a worker thread might have.
    let t = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || generate(&positions, &normals, &uvs, &indices))
        .expect("a thread")
        .join()
        .expect("no overflow");
    // The planar fan's U runs along +X.
    assert!(
        t.iter().all(|v| (v[0] - 1.0).abs() < 1e-3 && v[3] == 1.0),
        "{:?}",
        &t[..3]
    );
}
