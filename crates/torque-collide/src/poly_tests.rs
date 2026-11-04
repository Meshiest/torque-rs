use super::*;

fn unit_box_planes(min: F3, max: F3) -> Vec<Plane> {
    vec![
        Plane::new(-1.0, 0.0, 0.0, min[0]),
        Plane::new(0.0, 1.0, 0.0, -max[1]),
        Plane::new(1.0, 0.0, 0.0, -max[0]),
        Plane::new(0.0, -1.0, 0.0, min[1]),
        Plane::new(0.0, 0.0, -1.0, min[2]),
        Plane::new(0.0, 0.0, 1.0, -max[2]),
    ]
}

#[test]
fn a_clipped_list_keeps_the_top_of_a_box_inside_its_planes() {
    let mut l = ClippedPolyList::new();
    l.planes = unit_box_planes([-0.5, -0.5, 0.5], [0.5, 0.5, 1.5]);
    l.add_box(&Box3::new([-2.0, -2.0, 0.0], [2.0, 2.0, 1.0]));
    // Only the top face (z = 1) reaches into the planes, clipped to the 1x1 square.
    assert_eq!(l.polys.len(), 1);
    let p = l.polys[0];
    assert_eq!(p.plane.normal(), [0.0, 0.0, 1.0]);
    let pts: Vec<F3> = l
        .poly_indices(&p)
        .iter()
        .map(|&i| l.verts[i as usize].point)
        .collect();
    assert_eq!(pts.len(), 4);
    for q in pts {
        assert!(
            (q[0].abs() - 0.5).abs() < 1e-6 && (q[1].abs() - 0.5).abs() < 1e-6 && q[2] == 1.0,
            "{q:?}"
        );
    }
    // The edge from x = 2 to x = -2 meets x = 0.5 at t = 0.375, exact in f32.
    assert!(l.verts.iter().any(|v| v.point == [0.5, -2.0, 1.0]));
}

#[test]
fn an_early_out_list_stops_at_the_first_point_inside() {
    let mut l = EarlyOutPolyList::new();
    l.planes = unit_box_planes([0.0; 3], [1.0; 3]);
    l.add_point([5.0, 5.0, 5.0]);
    assert!(l.is_empty());
    l.add_point([0.5, 0.5, 0.5]);
    assert!(!l.is_empty());
    assert_eq!(l.add_point([0.5; 3]), 0);
}

#[test]
fn a_box_polyhedron_has_outward_planes() {
    let mut ph = Polyhedron::default();
    ph.build_box(&IDENTITY, &Box3::new([0.0; 3], [1.0, 2.0, 3.0]));
    assert_eq!(ph.points[6], [1.0, 2.0, 3.0]);
    assert_eq!(ph.planes[0], Plane::new(-1.0, -0.0, -0.0, 0.0));
    assert_eq!(ph.planes[5], Plane::new(0.0, 0.0, 1.0, -3.0));
    assert_eq!(ph.planes[1], Plane::new(0.0, 1.0, 0.0, -2.0));
    // Every corner is on or behind every plane.
    for p in &ph.points {
        for pl in &ph.planes {
            assert!(pl.dist(*p) <= 0.0);
        }
    }
}

#[test]
fn a_box_falling_onto_a_floor_meets_it_at_the_right_time() {
    let mut ph = Polyhedron::default();
    ph.build_box(&IDENTITY, &Box3::new([-0.5, -0.5, 1.0], [0.5, 0.5, 2.0]));
    let mut l = ExtrudedPolyList::new();
    l.extrude(&ph, [0.0, 0.0, -2.0]);
    l.set_velocity([0.0, 0.0, -2.0]);
    l.collisions.reset();
    // The floor: the top of a big box at z = 0.
    l.add_box(&Box3::new([-10.0, -10.0, -1.0], [10.0, 10.0, 0.0]));
    assert_eq!(l.collisions.count, 1);
    let c = l.collisions.entries[0];
    assert_eq!(c.normal, [0.0, 0.0, 1.0]);
    // Distance 1 of a reach of 2, less the second pass: the floor's vertices pushed 0.01 along its normal (the reach
    // times 0.2 is above 0.01), so the time is 0.99 / 2 and the first pass's 0.5 is within 0.1 of it.
    assert_eq!(l.collisions.t, (f64::from(0.99f32) / 2.0) as f32);
    assert_eq!(c.face_dot, 1.0);
}
