use super::*;
use crate::hd::kit::look_at;

fn camera(yaw: f32) -> HdCamera {
    let mut eye = look_at([3.0, -4.0, 5.0], [10.0, 20.0, 0.0]);
    eye.yaw += yaw;
    HdCamera::new(eye, 90u8, 500.0, 1280, 720)
}

#[test]
fn the_splits_run_from_the_near_plane_to_the_shadow_distance_mostly_logarithmic() {
    let s = practical_splits(0.1, 160.0, 4);
    assert_eq!(s.len(), 5);
    assert!((s[0] - 0.1).abs() < 1e-6 && (s[4] - 160.0).abs() < 1e-3);
    assert!(s.windows(2).all(|w| w[0] < w[1]));
    // Nearer than the linear split, farther than the logarithmic one.
    assert!(s[1] < 40.0 && s[1] > 0.1 * 1600f32.powf(0.25));
}

#[test]
fn cascade_spheres_hold_their_slices_and_keep_their_size_when_the_camera_turns() {
    let q = Quality::High.shadows();
    let towards = [0.5, 0.3, 0.81];
    let a = cascades(
        &camera(0.0),
        towards,
        &q,
        &ShadowRange {
            start: 0.1,
            end: SHADOW_DISTANCE,
            fade: SHADOW_DISTANCE,
        },
    );
    let b = cascades(
        &camera(0.7),
        towards,
        &q,
        &ShadowRange {
            start: 0.1,
            end: SHADOW_DISTANCE,
            fade: SHADOW_DISTANCE,
        },
    );
    assert_eq!(a.len(), 4);
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.radius, y.radius);
        assert_eq!(x.texel, y.texel);
    }
    // Each slice's eight corners are inside the cascade's square and depth range.
    let cam = camera(0.0);
    let (rot, eye) = eye_to_world(&cam);
    let [l, r, bt, t] = cam.frustum.map(f64::from);
    let n = f64::from(cam.near);
    for c in &a {
        for d in [f64::from(c.near_depth), f64::from(c.far_depth)] {
            for (x, y) in [(l, bt), (r, bt), (l, t), (r, t)] {
                let pe = [x * d / n, y * d / n, -d];
                let pw = [0, 1, 2].map(|k| {
                    (eye[k] + rot[0][k] * pe[0] + rot[1][k] * pe[1] + rot[2][k] * pe[2]) as f32
                });
                let q = transform(&c.view_proj, pw);
                assert!(
                    q[0].abs() <= 1.0 + 1e-3 && q[1].abs() <= 1.0 + 1e-3,
                    "{q:?}"
                );
                assert!(q[2] > 0.0 && q[2] < 1.0, "{q:?}");
            }
        }
    }
}

/// Every point the cascades shade (in the view, out to the range's end) that lies inside [`HELD_SHARE`] of a cascade's
/// square lies short of its far side too, so its shadow is looked up in that cascade or a finer one and a moving shape that
/// cascade holds is never left out of it ([`holds_shadow`]). A first-person eye over an open ground (the range fitted to a
/// held item: a small first cascade) and a far one, suns from low to high.
#[test]
fn a_point_inside_a_cascades_square_lies_inside_its_depth_range() {
    let ranges = [
        shadow_range_of(
            &HdCamera::new(
                look_at([0.0, 0.0, 2.2], [0.3, 3.0, 0.0]),
                90u8,
                500.0,
                1280,
                720,
            ),
            Some((0.5, 1.5)),
        ),
        ShadowRange {
            start: 0.1,
            end: SHADOW_DISTANCE,
            fade: SHADOW_DISTANCE,
        },
    ];
    let mut rng = crate::hd::kit::Lcg::new(43);
    let mut checked = 0;
    for quality in [Quality::Low, Quality::Medium, Quality::High, Quality::Ultra] {
        let q = quality.shadows();
        for range in &ranges {
            for (pitch_to, el, az) in [
                (0.0, 20.0, -80.0),
                (3.0, 45.0, 30.0),
                (8.0, 8.0, 100.0),
                (1.0, 80.0, 0.0),
            ] {
                let cam = HdCamera::new(
                    look_at([0.0, 0.0, 2.2], [0.3, 3.0, pitch_to]),
                    90u8,
                    500.0,
                    1280,
                    720,
                );
                let (a, e) = (f32::to_radians(az), f32::to_radians(el));
                let towards = [e.cos() * a.cos(), e.cos() * a.sin(), e.sin()];
                let cs = cascades(&cam, towards, &q, range);
                let (rot, eye) = eye_to_world(&cam);
                let [l, r, b, t] = cam.frustum.map(f64::from);
                let n = f64::from(cam.near);
                for _ in 0..4000 {
                    let d = f64::from(cam.near)
                        + f64::from(rng.unit()).powi(3) * f64::from(range.end - cam.near);
                    let x = l + (r - l) * f64::from(rng.unit());
                    let y = b + (t - b) * f64::from(rng.unit());
                    let pe = [x * d / n, y * d / n, -d];
                    let pw = [0, 1, 2].map(|k| {
                        (eye[k] + rot[0][k] * pe[0] + rot[1][k] * pe[1] + rot[2][k] * pe[2]) as f32
                    });
                    for (i, c) in cs.iter().enumerate() {
                        let p = transform(&c.view_proj, pw);
                        if p[0].abs() <= HELD_SHARE && p[1].abs() <= HELD_SHARE {
                            checked += 1;
                            assert!(
                                p[2] < 1.0,
                                "{quality:?} {range:?} sun {az}/{el}: {pw:?} in cascade {i}'s square past its far side ({})",
                                p[2]
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(checked > 10_000, "{checked}");
}

#[test]
fn a_moving_camera_keeps_the_cascades_texel_grid() {
    let q = Quality::High.shadows();
    let towards = [0.5, 0.3, 0.81];
    let mut eye = look_at([3.0, -4.0, 5.0], [10.0, 20.0, 0.0]);
    let a = cascades(
        &HdCamera::new(eye, 90u8, 500.0, 1280, 720),
        towards,
        &q,
        &ShadowRange {
            start: 0.1,
            end: SHADOW_DISTANCE,
            fade: SHADOW_DISTANCE,
        },
    );
    eye.position[0] += 0.37;
    let b = cascades(
        &HdCamera::new(eye, 90u8, 500.0, 1280, 720),
        towards,
        &q,
        &ShadowRange {
            start: 0.1,
            end: SHADOW_DISTANCE,
            fade: SHADOW_DISTANCE,
        },
    );
    // A world point lands on the same fraction of a texel in both.
    let p = [7.3, 11.1, 0.4];
    for (x, y) in a.iter().zip(&b) {
        let size = q.cascade_size as f32;
        let (u, v) = (transform(&x.view_proj, p), transform(&y.view_proj, p));
        for k in 0..2 {
            let fa = ((u[k] * 0.5 + 0.5) * size).fract();
            let fb = ((v[k] * 0.5 + 0.5) * size).fract();
            let diff = (fa - fb).abs();
            assert!(!(2e-2..=1.0 - 2e-2).contains(&diff), "{fa} {fb}");
        }
    }
}

#[test]
fn cube_faces_put_a_direction_where_the_cube_lookup_finds_it() {
    let light = [1.0, 2.0, 3.0];
    // A direction per face, its expected (u, v) from the face table.
    for (face, d) in [
        (0, [1.0, 0.2, -0.3]),
        (1, [-1.0, 0.25, 0.1]),
        (2, [0.3, 1.0, 0.2]),
        (3, [-0.2, -1.0, 0.4]),
        (4, [0.1, -0.3, 1.0]),
        (5, [0.2, 0.4, -1.0]),
    ] {
        let p = [light[0] + d[0], light[1] + d[1], light[2] + d[2]];
        let m = cube_face(light, 10.0, face);
        let c = transform(&m, p);
        let (x, y) = (c[0] / c[3], c[1] / c[3]);
        let (u, v) = (x * 0.5 + 0.5, 0.5 - y * 0.5);
        let (sc, tc, ma): (f32, f32, f32) = match face {
            0 => (-d[2], -d[1], d[0]),
            1 => (d[2], -d[1], -d[0]),
            2 => (d[0], d[2], d[1]),
            3 => (d[0], -d[2], -d[1]),
            4 => (d[0], -d[1], d[2]),
            _ => (-d[0], -d[1], -d[2]),
        };
        let (eu, ev) = ((sc / ma + 1.0) * 0.5, (tc / ma + 1.0) * 0.5);
        assert!((u - eu).abs() < 1e-5 && (v - ev).abs() < 1e-5, "{face}");
        assert!(c[2] / c[3] > 0.0 && c[2] / c[3] < 1.0);
        assert!(face_sees(light, 10.0, face, p, p));
        assert!(!face_sees(light, 10.0, face ^ 1, p, p));
    }
}

#[test]
fn the_range_fits_the_boxes_seen_in_powers_of_two() {
    let cam = camera(0.0);
    // Near boxes: from the near plane to the farthest box's power of two.
    let near = [
        [[4.0, 0.0, 0.0], [6.0, 2.0, 1.0]],
        [[9.0, 15.0, 0.0], [11.0, 17.0, 1.0]],
    ];
    let r = shadow_range(&cam, near);
    assert_eq!(r.start, cam.near);
    assert_eq!(r.end, 32.0);
    assert_eq!(r.fade, r.end);
    // Far boxes only: the range starts at a power of two short of them.
    let far = [[[100.0, 300.0, 0.0], [120.0, 320.0, 10.0]]];
    let r = shadow_range(&cam, far);
    assert!(r.start >= 128.0 && r.start.log2().fract() == 0.0, "{r:?}");
    assert_eq!(r.end, (4.0 * r.start).min(cam.far));
    // Near and far boxes: cut at the shadow distance, with a fade.
    let r = shadow_range(&cam, [near[0], far[0]]);
    assert_eq!((r.start, r.end), (cam.near, SHADOW_DISTANCE));
    assert!(r.fade < r.end);
    // Nothing seen: the default distance.
    let r = shadow_range(&cam, []);
    assert_eq!((r.start, r.end), (cam.near, SHADOW_DISTANCE));
    // A small move keeps the range.
    let mut eye = look_at([3.0, -4.0, 5.0], [10.0, 20.0, 0.0]);
    eye.position[0] += 0.3;
    let moved = HdCamera::new(eye, 90u8, 500.0, 1280, 720);
    assert_eq!(shadow_range(&moved, near), shadow_range(&cam, near));
}

#[test]
fn a_maps_range_reaches_past_the_shadow_distance_to_the_far_plane() {
    let cam = camera(0.0);
    let near = [[[4.0, 0.0, 0.0], [6.0, 2.0, 1.0]]];
    let far = [[[60.0, 150.0, 0.0], [80.0, 170.0, 10.0]]];
    let reach = seen_reach(&cam, [near[0], far[0]]);
    // The farthest box's power of two, nothing faded.
    let r = shadow_range_within(&cam, reach, 900.0);
    assert_eq!((r.start, r.end, r.fade), (cam.near, 256.0, 256.0));
    // Capped at the far plane, with a fade when cut short.
    let r = shadow_range_within(&cam, Some((0.5, 700.0)), 900.0);
    assert_eq!(r.end, cam.far);
    assert!(r.fade < r.end);
    // The near range is what shadow_range_of keeps.
    assert_eq!(
        shadow_range_within(&cam, reach, SHADOW_DISTANCE),
        shadow_range_of(&cam, reach)
    );
}

#[test]
fn shadow_polygons_and_rays() {
    let b = Aabb {
        lo: [-0.5, -1.0, 0.2],
        hi: [0.5, 1.0, 0.8],
    };
    assert!((0..3).all(|k| (b.lo[k] - [-0.5, -1.0, 0.2][k]).abs() < 1e-6));
    // A sun straight above: the shadow is the footprint.
    let p = sun_shadow_polygon(&b, [0.0, 0.0, 1.0], 0.0);
    assert_eq!(p.len(), 4);
    assert!(inside_convex(&p, [0.4, 0.9]) && !inside_convex(&p, [0.6, 0.0]));
    // A low sun along +x throws it toward -x.
    let el = 30f32.to_radians();
    let p = sun_shadow_polygon(&b, [el.cos(), 0.0, el.sin()], 0.0);
    assert!(inside_convex(&p, [-1.0, 0.0]) && !inside_convex(&p, [0.7, 0.0]));
    let hit = trace(&[b], [0.0, 0.0, 5.0], [0.0, 0.0, -1.0]).unwrap();
    assert!((hit.0 - 4.2).abs() < 1e-5 && hit.2 == [0.0, 0.0, 1.0]);
    assert!(occluded(&[b], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1e-3, 10.0));
    assert!(!occluded(
        &[b],
        [3.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        1e-3,
        10.0
    ));
    let pp = point_shadow_polygon(&b, [0.0, 0.0, 2.0], 0.0);
    assert!(inside_convex(&pp, [0.0, 0.0]) && inside_convex(&pp, [0.7, 0.0]));
    let r = texel_rect(
        &cascades(
            &camera(0.0),
            [0.3, 0.2, 0.9],
            &Quality::High.shadows(),
            &ShadowRange {
                start: 0.1,
                end: 160.0,
                fade: 160.0,
            },
        )[0]
        .view_proj,
        b.lo,
        b.hi,
        2048,
    );
    assert!(r.is_some());
}

#[test]
fn a_maps_range_has_two_more_cascades_spread_logarithmically_past_the_first() {
    let cam = camera(0.0);
    for q in Quality::ALL {
        let base = q.shadows();
        assert_eq!(base.for_distance(SHADOW_DISTANCE), base);
        let map = base.for_distance(900.0);
        assert_eq!(
            map.cascades,
            (base.cascades + MAP_EXTRA_CASCADES).min(MAX_CASCADES)
        );
    }
    let q = Quality::Ultra.shadows().for_distance(400.0);
    assert_eq!(q.cascades, 6);
    let range = ShadowRange {
        start: cam.near,
        end: 400.0,
        fade: 400.0,
    };
    let cs = cascades(&cam, [0.4, 0.3, 0.8], &q, &range);
    assert_eq!(cs.len(), 6);
    assert_eq!(cs[0].far_depth, FIRST_SPLIT_MAX);
    assert!((cs[5].far_depth - 400.0).abs() < 1e-3);
    // Past the first, each cascade reaches the same factor farther (100 ^ (1/5), about 2.51).
    for c in &cs[1..] {
        assert!(
            (c.far_depth / c.near_depth - 100f32.powf(0.2)).abs() < 1e-3,
            "{c:?}"
        );
    }
    // Every cascade's texel within 3 times the pixel at its nearest (the practical split had the second's 17 times at 4 units).
    for c in &cs[1..] {
        let pixel = (cam.frustum[1] - cam.frustum[0]) / cam.near / cam.width as f32 * c.near_depth;
        assert!(c.texel < 3.0 * pixel, "{c:?} pixel {pixel}");
    }
    // A near range keeps the practical split.
    let near = cascades(
        &cam,
        [0.4, 0.3, 0.8],
        &Quality::Ultra.shadows(),
        &ShadowRange {
            start: cam.near,
            end: SHADOW_DISTANCE,
            fade: SHADOW_DISTANCE,
        },
    );
    let practical = practical_splits(cam.near, SHADOW_DISTANCE, 4);
    assert_eq!(near.len(), 4);
    for (c, s) in near[1..].iter().zip(&practical[2..]) {
        assert_eq!(c.far_depth, *s);
    }
}
