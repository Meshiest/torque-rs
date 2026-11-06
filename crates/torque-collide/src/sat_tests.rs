use super::*;

/// The box `lo`..`hi` as a polytope.
fn cuboid(lo: F3, hi: F3) -> Polytope {
    let mut p = Polytope::default();
    for k in 0..8u32 {
        p.points.push(std::array::from_fn(|a| {
            if k >> a & 1 == 1 { hi[a] } else { lo[a] }
        }));
    }
    for a in 0..3 {
        let mut n = [0.0; 3];
        n[a] = 1.0;
        p.add_normal(n);
    }
    for k in 0..8u32 {
        for a in 0..3 {
            p.add_edge(k, k | 1 << a);
        }
    }
    p
}

/// A vertical prism of `sides` round (`x`, `y`) with corners at `r`, from `z0` to `z1`.
fn prism(x: f32, y: f32, r: f32, sides: u32, z0: f32, z1: f32) -> Polytope {
    let mut p = Polytope::default();
    let corner = |k: u32| {
        let a = std::f32::consts::TAU * k as f32 / sides as f32;
        (x + r * a.cos(), y + r * a.sin())
    };
    for z in [z0, z1] {
        for k in 0..sides {
            let (cx, cy) = corner(k);
            p.points.push([cx, cy, z]);
        }
    }
    p.add_normal([0.0, 0.0, 1.0]);
    for k in 0..sides {
        let ((ax, ay), (bx, by)) = (corner(k), corner((k + 1) % sides));
        p.add_normal([by - ay, ax - bx, 0.0]);
        let n = (k + 1) % sides;
        p.add_edge(k, n);
        p.add_edge(sides + k, sides + n);
        p.add_edge(k, sides + k);
    }
    p
}

fn near(a: F3, b: F3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
}

#[test]
fn apart_boxes_and_touching_ones_do_not_overlap() {
    let a = cuboid([0.0; 3], [1.0; 3]);
    let mut out = Penetration::default();
    assert!(!penetration(
        &a,
        &cuboid([1.5, 0.0, 0.0], [2.0, 1.0, 1.0]),
        &mut out
    ));
    assert!(!penetration(
        &a,
        &cuboid([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
        &mut out
    ));
    // A square post turned 45 degrees past the box's corner.
    assert!(!penetration(
        &a,
        &prism(1.6, 1.6, 0.5, 4, -1.0, 2.0),
        &mut out
    ));
    // Apart only along an edge pair's axis: two turned cubes whose edges pass 0.1 apart.
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let under = turned_cube([0.0, 0.0, -0.05 - s], 1);
    let over = turned_cube([0.0, 0.0, 0.05 + s], 0);
    assert!(!penetration(&under, &over, &mut out));
}

#[test]
fn overlapping_boxes_move_apart_the_short_way_with_the_common_corners() {
    let a = cuboid([0.0; 3], [1.0; 3]);
    let b = cuboid([0.8, 0.1, 0.1], [2.0, 0.9, 0.9]);
    let mut out = Penetration::default();
    assert!(penetration(&a, &b, &mut out));
    assert!((out.depth - 0.2).abs() < 1e-5, "{out:?}");
    assert!(near(out.normal, [-1.0, 0.0, 0.0]), "{out:?}");
    // The common volume is the box 0.8..1 by 0.1..0.9 by 0.1..0.9: every corner reported lies in it.
    assert!(!out.points.is_empty());
    for p in &out.points {
        assert!((0.8 - 1e-5..=1.0 + 1e-5).contains(&p[0]), "{p:?}");
        assert!((0.1 - 1e-5..=0.9 + 1e-5).contains(&p[1]), "{p:?}");
    }
    // And the other way round, the other sense.
    assert!(penetration(&b, &a, &mut out));
    assert!(near(out.normal, [1.0, 0.0, 0.0]), "{out:?}");
}

/// A post pushed deep into a box's face (deeper than a feature's tolerance would see) still comes out through that face, with
/// corners where the post's edges cross the box.
#[test]
fn a_post_deep_in_a_boxs_face_comes_out_through_the_face() {
    let body = cuboid([-0.7, -1.95, 0.18], [0.7, 2.06, 1.9]);
    for depth in [0.05f32, 0.3, 0.6] {
        // Its near face (between its corners at 240 and 300 degrees) is 0.5 sin 60 short of its center.
        let post = prism(0.2, 2.06 + 0.5 * 0.866_025_4 - depth, 0.5, 6, -1.5, 8.0);
        let mut out = Penetration::default();
        assert!(penetration(&body, &post, &mut out), "{depth}");
        assert!(near(out.normal, [0.0, -1.0, 0.0]), "{depth}: {out:?}");
        assert!((out.depth - depth).abs() < 1e-4, "{depth}: {out:?}");
        assert!(
            out.points.iter().any(|p| (p[2] - 0.18).abs() < 1e-4),
            "{out:?}"
        );
        assert!(
            out.points.iter().any(|p| (p[2] - 1.9).abs() < 1e-4),
            "{out:?}"
        );
    }
}

/// A unit cube centered at `c`, turned 45 degrees about axis `axis` (0 x, 1 y): an edge along that axis lowest and highest.
fn turned_cube(c: F3, axis: usize) -> Polytope {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let mut p = Polytope::default();
    for k in 0..8u32 {
        let l: F3 = std::array::from_fn(|a| if k >> a & 1 == 1 { 0.5 } else { -0.5 });
        // The axis kept; the other level one and z turned.
        let (keep, other) = if axis == 0 { (0, 1) } else { (1, 0) };
        let mut w = [0.0; 3];
        w[keep] = l[keep];
        w[other] = (l[other] - l[2]) * s;
        w[2] = (l[other] + l[2]) * s;
        p.points.push([c[0] + w[0], c[1] + w[1], c[2] + w[2]]);
    }
    let mut n = [0.0; 3];
    n[axis] = 1.0;
    p.add_normal(n);
    let other = 1 - axis;
    let mut n = [0.0; 3];
    n[other] = s;
    n[2] = s;
    p.add_normal(n);
    n[other] = -s;
    p.add_normal(n);
    for k in 0..8u32 {
        for a in 0..3 {
            p.add_edge(k, k | 1 << a);
        }
    }
    p
}

/// Two turned cubes crossing edge to edge (one's lowest edge along x over the other's highest along y): no face normal is the
/// way apart, the edges' cross product (z) is.
#[test]
fn crossed_edges_part_across_both() {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let under = turned_cube([0.0, 0.0, 0.05 - s], 1);
    let over = turned_cube([0.0, 0.0, -0.05 + s], 0);
    let mut out = Penetration::default();
    assert!(penetration(&under, &over, &mut out));
    assert!(near(out.normal, [0.0, 0.0, -1.0]), "{out:?}");
    assert!((out.depth - 0.1).abs() < 1e-4, "{out:?}");
    // The common volume is round where the edges cross: its corners are near the origin.
    assert!(!out.points.is_empty());
    assert!(
        out.points.iter().all(|p| p.iter().all(|c| c.abs() < 0.2)),
        "{out:?}"
    );
}

#[test]
fn an_empty_or_unbounded_polytope_never_overlaps() {
    let a = cuboid([0.0; 3], [1.0; 3]);
    let mut out = Penetration::default();
    assert!(!penetration(&a, &Polytope::default(), &mut out));
    let mut flat = cuboid([0.0; 3], [1.0; 3]);
    flat.normals.clear();
    assert!(!penetration(&a, &flat, &mut out));
    let mut nan = cuboid([0.0; 3], [1.0; 3]);
    nan.points[0] = [f32::NAN; 3];
    // A NaN point widens nothing (min and max skip it): the answer is finite, and no corner is NaN.
    assert!(penetration(&a, &nan, &mut out));
    assert!(out.depth.is_finite() && out.normal.iter().all(|c| c.is_finite()));
    assert!(!out.points.is_empty());
    assert!(
        out.points.iter().flatten().all(|c| c.is_finite()),
        "{out:?}"
    );
    // Normals that are zero or not finite are not taken; edges to points that are not there are not either.
    let mut p = Polytope::default();
    p.add_normal([0.0; 3]);
    p.add_normal([f32::INFINITY, 0.0, 0.0]);
    p.add_edge(0, 1);
    assert!(p.normals.is_empty() && p.edges.is_empty());
}

/// A slanted hull scaled twice as wide: its slanted face's normal follows the scale (the inverse transpose), so a box just
/// off that face, which only the face's axis separates (the hull has no edges here), does not overlap it.
#[test]
fn a_scaled_hull_turns_its_normals_by_the_inverse_transpose() {
    use crate::math::IDENTITY;
    use crate::mesh::HullAccel;
    let s = std::f32::consts::FRAC_1_SQRT_2;
    // A wedge: the right triangle (0,0) (1,0) (0,1) from z 0 to 1, its slanted face facing (1, 1, 0).
    let accel = HullAccel {
        verts: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
        ],
        normals: vec![
            [0.0, 0.0, -1.0],
            [0.0, 0.0, 1.0],
            [0.0, -1.0, 0.0],
            [-1.0, 0.0, 0.0],
            [s, s, 0.0],
        ],
        emit: Vec::new(),
    };
    let mut hull = Polytope::default();
    hull.set_hull(&accel, &IDENTITY, [2.0, 1.0, 1.0]);
    // Scaled, the slanted face runs from (2, 0) to (0, 1): its normal is (1, 2, 0) / sqrt 5.
    let k = 1.0 / 5.0f32.sqrt();
    let want = [k, 2.0 * k, 0.0];
    assert!(
        hull.normals.iter().any(|n| near(*n, want)),
        "{:?}",
        hull.normals
    );
    assert!(!hull.normals.iter().any(|n| near(*n, [s, s, 0.0])));
    // A small box 0.13 off the scaled face (x + 2y = 2.3 at its middle).
    let off = cuboid([1.05, 0.55, 0.4], [1.15, 0.65, 0.6]);
    let mut out = Penetration::default();
    assert!(!penetration(&hull, &off, &mut out), "{out:?}");
    // And one pushed into it does.
    let into = cuboid([0.85, 0.45, 0.4], [0.95, 0.55, 0.6]);
    assert!(penetration(&hull, &into, &mut out));
}
