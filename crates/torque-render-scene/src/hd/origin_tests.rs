use super::*;
use crate::hd::cells::cell_key;
use crate::hd::clusters::Cluster;
use crate::hd::kit::box_object;
use crate::hd::{HdCamera, HdVertex, oct_encode};
use crate::math::gl_mul;

/// Where the world point `p` lands on a 1920 x 1080 picture from `eye` (fov 90), in double precision throughout.
fn exact_pixel(eye: &Eye, p: [f32; 3]) -> (f64, f64) {
    let (sy, cy) = f64::from(eye.yaw).sin_cos();
    let (sp, cp) = f64::from(eye.pitch).sin_cos();
    // TGE's `EulerF(0, 0, yaw) * EulerF(pitch, 0, 0)` as [`Mat::euler_z`] and [`Mat::euler_x`] lay them out (row-major), then
    // its inverse (the transpose, the eye turned back).
    let rz = [[cy, sy, 0.0], [-sy, cy, 0.0], [0.0, 0.0, 1.0]];
    let rx = [[1.0, 0.0, 0.0], [0.0, cp, sp], [0.0, -sp, cp]];
    let mut r = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| rz[i][k] * rx[k][j]).sum();
        }
    }
    let e = eye.position.map(f64::from);
    // GL column-major: column c of the modelview is row c of the row-major inverse.
    let mut modelview = [0.0f64; 16];
    for c in 0..3 {
        for row in 0..3 {
            modelview[c * 4 + row] = r[c][row];
        }
        modelview[12 + c] = -(0..3).map(|k| r[k][c] * e[k]).sum::<f64>();
    }
    modelview[15] = 1.0;
    let fixed = crate::camera::Camera::from_eye(*eye, 90u8, 1000.0, 1920, 1080);
    let [l, rt, b, t] = fixed.frustum.map(f64::from);
    let (n, f) = (f64::from(fixed.near), f64::from(fixed.far));
    #[rustfmt::skip]
    let proj = [
        2.0 * n / (rt - l), 0.0, 0.0, 0.0,
        0.0, 2.0 * n / (t - b), 0.0, 0.0,
        (rt + l) / (rt - l), (t + b) / (t - b), n / (f - n), -1.0,
        0.0, 0.0, f * n / (f - n), 0.0,
    ];
    #[rustfmt::skip]
    let dark_to_ogl = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 0.0, -1.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    let m = gl_mul(&gl_mul(&proj, &dark_to_ogl), &modelview);
    let q = p.map(f64::from);
    let c: Vec<f64> = (0..4)
        .map(|r| m[r] * q[0] + m[4 + r] * q[1] + m[8 + r] * q[2] + m[12 + r])
        .collect();
    ((c[0] / c[3] + 1.0) * 960.0, (1.0 - c[1] / c[3]) * 540.0)
}

/// The GPU's single-precision arithmetic: the camera's matrix times the point, as the vertex shader multiplies them.
fn gpu_pixel(eye: &Eye, p: [f32; 3]) -> (f64, f64) {
    let (x, y, _) = HdCamera::new(*eye, 90u8, 1000.0, 1920, 1080)
        .project(p)
        .expect("in front of the eye");
    (f64::from(x), f64::from(y))
}

/// The worst frame-to-frame motion of one corner of a 1 m box 3 m ahead against another, beyond the exact motion (pixels), and
/// the worst distance of a corner from where it exactly is, over 48 frames of the eye at `distance` units moving 1 mm a
/// frame; in world coordinates, and measured from the origin near the eye.
fn wobble(distance: f32) -> ([f64; 2], [f64; 2]) {
    let (yaw, pitch) = (0.3f32, -0.1f32);
    let (fx, fy) = (-yaw.sin(), yaw.cos());
    let base = [distance + 0.123 + 3.0 * fx, 0.456 + 3.0 * fy, 1.7];
    let corners: Vec<[f32; 3]> = [-0.5f32, 0.5]
        .iter()
        .flat_map(|&dx| {
            [0.0f32, 1.0].iter().flat_map(move |&dy| {
                [-0.5f32, 0.5]
                    .iter()
                    .map(move |&dz| [base[0] + dx, base[1] + dy, base[2] + dz])
            })
        })
        .collect();
    let mut worst = [0.0f64; 2];
    let mut worst_abs = [0.0f64; 2];
    // The last frame's corners: exact, in world coordinates, from the near origin.
    let mut last: Option<[Pixels; 3]> = None;
    for frame in 0..48 {
        let step = frame as f32 * 0.0007;
        let eye = Eye {
            position: [distance + 0.123 + step, 0.456 + step, 1.7],
            pitch,
            yaw,
            fov: Some(90),
        };
        let origin = RenderOrigin::near(eye.position);
        let mut near_eye = eye;
        near_eye.shift(origin.shift());
        let exact: Pixels = corners.iter().map(|&c| exact_pixel(&eye, c)).collect();
        let world: Pixels = corners.iter().map(|&c| gpu_pixel(&eye, c)).collect();
        let near: Pixels = corners
            .iter()
            .map(|&c| gpu_pixel(&near_eye, origin.relative(c)))
            .collect();
        for (k, got) in [&world, &near].into_iter().enumerate() {
            for (g, e) in got.iter().zip(&exact) {
                worst_abs[k] = worst_abs[k].max((g.0 - e.0).hypot(g.1 - e.1));
            }
            if let Some(then) = &last {
                for a in 0..corners.len() {
                    for b in a + 1..corners.len() {
                        let (gx, gy) = moved(got, &then[k + 1], a, b);
                        let (ex, ey) = moved(&exact, &then[0], a, b);
                        worst[k] = worst[k].max((gx - ex).hypot(gy - ey));
                    }
                }
            }
        }
        last = Some([exact, world, near]);
    }
    (worst, worst_abs)
}

type Pixels = Vec<(f64, f64)>;

/// How far corner `a` moved against corner `b` from `then` to `now`.
fn moved(now: &[(f64, f64)], then: &[(f64, f64)], a: usize, b: usize) -> (f64, f64) {
    (
        (now[a].0 - now[b].0) - (then[a].0 - then[b].0),
        (now[a].1 - now[b].1) - (then[a].1 - then[b].1),
    )
}

#[test]
fn a_box_fifty_kilometers_out_wobbles_in_world_coordinates_and_not_from_a_near_origin() {
    for km in [1.0f32, 10.0, 50.0, 100.0] {
        let (w, a) = wobble(km * 1000.0);
        eprintln!(
            "{km:>5} km: world wobble {:.3} px (off by {:.3}); from a near origin {:.4} px (off by {:.4})",
            w[0], a[0], w[1], a[1]
        );
        // (Up to 512 units from the origin: a 0.06 mm step, 0.02 px at 3 m.)
        assert!(
            w[1] < 0.05 && a[1] < 0.05,
            "{km} km from a near origin: {w:?} {a:?}"
        );
    }
    let (w, a) = wobble(50_000.0);
    assert!(
        w[0] > 0.2 && a[0] > 1.0,
        "50 km in world coordinates: {w:?} {a:?}"
    );
}

#[test]
fn an_origin_sits_on_the_grid_and_measures_exactly() {
    let o = RenderOrigin::near([49_500.0, -600.0, 12.0]);
    assert_eq!(o.position, [49_152.0, -1024.0, 0.0]);
    assert_eq!(o.shift(), [-49_152.0, 1024.0, -0.0]);
    // A position's f32 step out there is 2^-8: a point on that grid is measured exactly.
    let p = [49_152.0 + 799.0 / 256.0, -1024.0 + 0.5, 1.7];
    assert_eq!(o.relative(p), [799.0 / 256.0, 0.5, 1.7]);
    assert!(!o.is_far([49_152.0 + 500.0, -1024.0, 0.0], 512.0));
    assert!(o.is_far([49_152.0 + 600.0, -1024.0, 0.0], 512.0));
    assert!(o.is_far([49_152.0, -1024.0 - 600.0, 3000.0], 512.0));
    assert!(!o.is_far([49_152.0, -1024.0, 3000.0], 512.0));
    assert_eq!(
        RenderOrigin::near([100.0, -100.0, 50.0]),
        RenderOrigin::WORLD
    );
}

fn draw_at(p: [f32; 3], material: u32) -> HdDraw {
    let mut d = HdDraw::empty();
    d.vertices.push(HdVertex {
        position: p,
        normal: oct_encode([0.0, 0.0, 1.0]),
        uv: [0.0; 2],
        color: [255; 4],
        material,
        extra: 0,
    });
    d.indices.extend([0, 0, 0]);
    d
}

#[test]
fn every_position_a_frame_holds_moves_with_it() {
    let p = [49_000.5, -700.25, 12.0];
    let by = [-1024.0, 2048.0, 0.0];
    let moved = shifted(p, by);
    let eye = Eye {
        position: p,
        pitch: 0.1,
        yaw: 0.2,
        fov: Some(90),
    };
    let mut place = Mat::IDENTITY;
    place.set_column3(p);
    let light = HdLight {
        position: p,
        color: [1.0; 3],
        radius: 5.0,
        carrier: None,
    };
    let mut object = box_object(1, p, [0.5; 3]);
    object.transform = place;
    object.prev_transform = place;
    object.previous = Some(vec![p]);
    object.nodes = vec![NodePlace {
        now: place,
        before: place,
        normal: Mat::IDENTITY,
        sphere: [p[0], p[1], p[2], 1.0],
        build: 0,
    }];
    // A vertex in its mesh's own space is placed by its node: it stays.
    object.draws.push(draw_at([0.25, 0.0, 0.0], material::NODE));
    let cell = Cell {
        key: cell_key(p),
        bounds: [p, shifted(p, [1.0; 3])],
        vertices: draw_at(p, 0).vertices,
        indices: vec![0, 0, 0],
        batches: Vec::new(),
        clusters: vec![Cluster {
            slot: 0,
            translucent: false,
            first: 0,
            count: 3,
            bounds: [p, p],
        }],
        generation: 1,
        resident: false,
        ext: (),
    };
    let mut cells = StaticCells::default();
    cells.cells.insert(cell.key, cell);
    let mut s = HdScene::new(cells);
    s.eye = Some(eye);
    s.sky = vec![draw_at(p, material::SKY_EYE)];
    s.sky_eye = p;
    s.planes = vec![draw_at(p, 0)];
    s.water = vec![draw_at(p, 0)];
    s.water_zone = Some(WaterZone {
        min: p,
        max: shifted(p, [1.0; 3]),
        tint: [0.5; 4],
    });
    s.objects = vec![object.clone()];
    s.drawn_objects = Some(vec![object]);
    s.coronas = vec![(5.0, draw_at(p, material::BILLBOARD))];
    s.flare = Some(draw_at(p, 0));
    s.particles = vec![(3.0, draw_at(p, material::PARTICLE))];
    s.lights = vec![light];
    s.spots = vec![HdSpot {
        light,
        direction: [0.0, 0.0, -1.0],
        inner: 0.3,
        outer: 0.5,
        shadows: false,
    }];

    s.shift(by);

    let first = |d: &HdDraw| d.vertices[0].position;
    assert_eq!(s.eye.expect("an eye").position, moved);
    assert_eq!(first(&s.sky[0]), moved);
    assert_eq!(s.sky_eye, moved);
    assert_eq!(first(&s.planes[0]), moved);
    assert_eq!(first(&s.water[0]), moved);
    let zone = s.water_zone.expect("a zone");
    assert_eq!((zone.min, zone.max), (moved, shifted(moved, [1.0; 3])));
    assert_eq!(s.coronas[0], (5.0, draw_at(moved, material::BILLBOARD)));
    assert_eq!(first(s.flare.as_ref().expect("a flare")), moved);
    assert_eq!(first(&s.particles[0].1), moved);
    assert_eq!(s.lights[0].position, moved);
    assert_eq!(s.spots[0].light.position, moved);
    for o in s.objects.iter().chain(s.drawn_objects.iter().flatten()) {
        assert_eq!(o.transform.column(3), moved);
        assert_eq!(o.prev_transform.column(3), moved);
        assert_eq!(o.centre, moved);
        assert_eq!(o.previous.as_deref(), Some(&[moved][..]));
        assert_eq!(o.nodes[0].now.column(3), moved);
        assert_eq!(o.nodes[0].before.column(3), moved);
        assert_eq!(o.nodes[0].normal, Mat::IDENTITY);
        assert_eq!(o.nodes[0].sphere, [p[0], p[1], p[2], 1.0]);
        assert_eq!(first(&o.draws[0]), shifted(shifted(p, [-0.5; 3]), by));
        assert_eq!(first(&o.draws[1]), [0.25, 0.0, 0.0]);
    }
    let key = [cell_key(p)[0] - 32, cell_key(p)[1] + 64, cell_key(p)[2]];
    let cell = s
        .cells
        .cells
        .get(&key)
        .expect("the cell under its moved key");
    assert_eq!(s.cells.cells.len(), 1);
    assert_eq!(cell.key, key);
    assert_eq!(cell.key, cell_key(moved));
    assert_eq!(cell.bounds, [moved, shifted(moved, [1.0; 3])]);
    assert_eq!(cell.vertices[0].position, moved);
    assert_eq!(cell.clusters[0].bounds, [moved, moved]);
}
