use super::*;
use crate::preload::{SLOTS, collision_slots};
use crate::synth::solid_shape;

fn free(_: u64) -> bool {
    true
}

const BIG: [f32; 6] = [-10.0, -10.0, -10.0, 10.0, 10.0, 10.0];
const UNIT: [f32; 6] = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

fn cube_shape(strips: bool) -> (Shape, Slots) {
    let model = solid_shape(
        BIG,
        &[("Collision-1", UNIT)],
        [0, 0, 0, 32767],
        [0.0; 3],
        strips,
    );
    let shape = Shape::for_server(model);
    let slots = collision_slots(&shape.names, &shape.details);
    (shape, slots)
}

#[test]
fn a_cube_made_of_triangle_lists_and_of_strips_has_six_planes() {
    for strips in [false, true] {
        let (shape, slots) = cube_shape(strips);
        let mesh = shape.meshes[0].as_ref().unwrap();
        let hull = Hull::build(mesh, &mut free).unwrap();
        assert_eq!(hull.planes(), 6, "strips {strips}: {hull:?}");
        // Every normal points out of the cube: the dot with the direction from the centre is positive.
        for (n, c) in hull.normals.iter().zip(&hull.constants) {
            let at_centre = n[0] * 0.5 + n[1] * 0.5 + n[2] * 0.5 - c;
            assert!(at_centre < 0.0, "strips {strips}: {n:?} {c}");
        }
        let collision = Collision::build(&shape, &slots, &mut free).unwrap();
        assert_eq!(collision.details(), 1);
    }
}

#[test]
fn a_ray_enters_a_cube_through_the_face_it_meets_first() {
    for strips in [false, true] {
        let (shape, slots) = cube_shape(strips);
        let c = Collision::build(&shape, &slots, &mut free).unwrap();
        let hit = c
            .cast_ray(&slots, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], &mut free)
            .unwrap()
            .expect("a hit");
        assert_eq!(hit.normal, [-1.0, 0.0, 0.0], "strips {strips}");
        assert!((hit.t - 1.0 / 3.0).abs() < 1e-6, "{}", hit.t);
        // From the other side, from above, from below.
        let from = |a: [f32; 3], b: [f32; 3]| c.cast_ray(&slots, a, b, &mut free).unwrap();
        assert_eq!(
            from([2.0, 0.5, 0.5], [-1.0, 0.5, 0.5]).unwrap().normal,
            [1.0, 0.0, 0.0]
        );
        assert_eq!(
            from([0.5, 0.5, 3.0], [0.5, 0.5, -3.0]).unwrap().normal,
            [0.0, 0.0, 1.0]
        );
        assert_eq!(
            from([0.5, 3.0, 0.5], [0.5, -3.0, 0.5]).unwrap().normal,
            [0.0, 1.0, 0.0]
        );
        // Misses: beside the cube, short of it, starting inside it.
        assert!(from([-1.0, 1.5, 0.5], [2.0, 1.5, 0.5]).is_none());
        assert!(from([-1.0, 0.5, 0.5], [-0.5, 0.5, 0.5]).is_none());
        assert!(from([0.5, 0.5, 0.5], [3.0, 0.5, 0.5]).is_none());
    }
}

#[test]
fn a_ray_is_cast_at_the_los_detail_and_a_box_tested_against_the_collision_detail() {
    let small = [0.0, 0.0, 0.0, 0.5, 0.5, 0.5];
    let model = solid_shape(
        BIG,
        &[("Collision-1", UNIT), ("LOS-9", small)],
        [0, 0, 0, 32767],
        [0.0; 3],
        true,
    );
    let shape = Shape::for_server(model);
    let slots = collision_slots(&shape.names, &shape.details);
    assert_eq!((slots.collision[0], slots.los[0]), (0, 1));
    assert_eq!(
        (detail_for_ray(&slots, 0), detail_for_polylist(&slots, 0)),
        (1, 0)
    );
    let c = Collision::build(&shape, &slots, &mut free).unwrap();
    let hit = c
        .cast_ray(&slots, [-1.0, 0.25, 0.25], [2.0, 0.25, 0.25], &mut free)
        .unwrap()
        .expect("the small cube is hit");
    assert!((hit.t - 1.0 / 3.0).abs() < 1e-6);
    // A ray that passes through the unit cube above the small one: the LOS detail misses it.
    assert!(
        c.cast_ray(&slots, [-1.0, 0.75, 0.25], [2.0, 0.75, 0.25], &mut free)
            .unwrap()
            .is_none()
    );
    // The polygon list is the unit cube's: a box across its face x = 1 (the small cube ends at 0.5) is hit, and a box wholly inside
    // it is not (no vertex and no polygon lies in the box).
    let planes = box_planes([0.9, 0.2, 0.2], [1.1, 0.4, 0.4]);
    assert!(
        c.box_hit(&slots, &crate::math::IDENTITY, &planes, &mut free)
            .unwrap()
    );
    let planes = box_planes([0.6, 0.6, 0.6], [0.9, 0.9, 0.9]);
    assert!(
        !c.box_hit(&slots, &crate::math::IDENTITY, &planes, &mut free)
            .unwrap()
    );
}

#[test]
fn the_nodes_pose_moves_the_hull_and_the_hit_normal_is_turned_back() {
    // The quaternion (0, 0, .7071, .7071) is the `QuatF` matrix (a quarter turn that sends x to -y) and a move of
    // (2, -3, 0): the unit cube lands on x 2 to 3, y -4 to -3 (to a few parts in 10,000).
    let model = solid_shape(
        BIG,
        &[("Collision-1", UNIT)],
        [0, 0, 23169, 23169],
        [2.0, -3.0, 0.0],
        false,
    );
    let shape = Shape::for_server(model);
    let slots = collision_slots(&shape.names, &shape.details);
    let c = Collision::build(&shape, &slots, &mut free).unwrap();
    let hit = c
        .cast_ray(&slots, [0.0, -3.5, 0.5], [5.0, -3.5, 0.5], &mut free)
        .unwrap()
        .expect("the moved cube is hit");
    assert!((hit.t - 0.4).abs() < 1e-3, "{}", hit.t);
    assert!(
        (hit.normal[0] + 1.0).abs() < 1e-3 && hit.normal[1].abs() < 1e-3,
        "{:?}",
        hit.normal
    );
    // Where the unturned, unmoved cube would have been: nothing.
    assert!(
        c.cast_ray(&slots, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], &mut free)
            .unwrap()
            .is_none()
    );
    // The polygon list puts the cube in the world with the object's transform times the node's: an object moved by (10, 0, 0).
    let mut world = crate::math::IDENTITY;
    world[3] = 10.0;
    let inside = box_planes([12.4, -3.6, 0.4], [12.6, -3.4, 0.6]);
    let across = box_planes([11.9, -3.6, 0.4], [12.1, -3.4, 0.6]);
    let beside = box_planes([2.4, -3.6, 0.4], [2.6, -3.4, 0.6]);
    assert!(
        !c.box_hit(&slots, &world, &inside, &mut free).unwrap(),
        "wholly inside the cube"
    );
    assert!(
        c.box_hit(&slots, &world, &across, &mut free).unwrap(),
        "across the face x = 12"
    );
    assert!(
        !c.box_hit(&slots, &world, &beside, &mut free).unwrap(),
        "where the cube is not"
    );
}

/// A hull of the given planes (`n . p <= c` inside), for the rules that need a geometry a solid cannot make.
fn half_spaces(planes: &[([f32; 3], f32)]) -> Hull {
    Hull {
        normals: planes.iter().map(|p| p.0).collect(),
        constants: planes.iter().map(|p| p.1).collect(),
    }
}

fn part(hull: Hull) -> Part {
    Part {
        node: crate::math::IDENTITY,
        node_inverse: crate::math::IDENTITY,
        geom: Arc::new(MeshGeom {
            hull,
            verts: Vec::new(),
            tris: Vec::new(),
        }),
    }
}

#[test]
fn two_planes_entered_at_one_time_keep_the_first_and_a_later_object_wins_a_tie() {
    // Through the edge of the cube where the faces x = 0 and y = 0 meet: both planes are entered at 0.5.
    let (shape, slots) = cube_shape(false);
    let c = Collision::build(&shape, &slots, &mut free).unwrap();
    let hit = c
        .cast_ray(&slots, [-1.0, -1.0, 0.5], [1.0, 1.0, 0.5], &mut free)
        .unwrap()
        .expect("the edge is hit");
    assert_eq!(hit.t, 0.5);
    let mesh = shape.meshes[0].as_ref().unwrap();
    let hull = Hull::build(mesh, &mut free).unwrap();
    let first = hull
        .normals
        .iter()
        .position(|n| *n == [-1.0, 0.0, 0.0])
        .unwrap();
    let second = hull
        .normals
        .iter()
        .position(|n| *n == [0.0, -1.0, 0.0])
        .unwrap();
    assert_eq!(
        hit.normal,
        if first < second {
            [-1.0, 0.0, 0.0]
        } else {
            [0.0, -1.0, 0.0]
        }
    );
    // Two objects, each entered at 0.5 through another plane: the later one answers (`info.t <= best`).
    let a = half_spaces(&[([-1.0, 0.0, 0.0], 0.0)]);
    let b = half_spaces(&[([-0.6, -0.8, 0.0], 0.0)]);
    let collision = Collision {
        details: BTreeMap::from([(0, vec![part(a.clone()), part(b.clone())])]),
    };
    let ray = ([-1.0, -1.0, 0.0], [1.0, 1.0, 0.0]);
    let hit = collision
        .cast_detail(0, ray.0, ray.1, &mut free)
        .unwrap()
        .unwrap();
    assert_eq!((hit.t, hit.normal), (0.5, [-0.6, -0.8, 0.0]));
    let swapped = Collision {
        details: BTreeMap::from([(0, vec![part(b), part(a)])]),
    };
    let hit = swapped
        .cast_detail(0, ray.0, ray.1, &mut free)
        .unwrap()
        .unwrap();
    assert_eq!(hit.normal, [-1.0, 0.0, 0.0]);
}

#[test]
fn the_slots_take_the_nearest_hit_and_the_first_slot_on_a_tie() {
    // Three one-plane details: x = 1 (detail 0), and x = 0 twice, with different normals (details 1 and 2).
    let far = half_spaces(&[([-1.0, 0.0, 0.0], -1.0)]);
    let near_a = half_spaces(&[([-1.0, 0.0, 0.0], 0.0)]);
    let near_b = half_spaces(&[([-0.6, -0.8, 0.0], 0.0)]);
    let collision = Collision {
        details: BTreeMap::from([
            (0, vec![part(far)]),
            (1, vec![part(near_a)]),
            (2, vec![part(near_b)]),
        ]),
    };
    let ray = ([-3.0, 0.0, 0.0], [3.0, 0.0, 0.0]);
    let mut slots = Slots {
        collision: vec![-1; SLOTS],
        los: vec![-1; SLOTS],
    };
    slots.los[0] = 0;
    slots.los[1] = 1;
    slots.los[5] = 2;
    let hit = collision
        .cast_ray(&slots, ray.0, ray.1, &mut free)
        .unwrap()
        .unwrap();
    assert_eq!(
        (hit.t, hit.normal),
        (0.5, [-1.0, 0.0, 0.0]),
        "the nearer plane wins over the slot that came first, and of two ties the earlier slot"
    );
    slots.los[1] = -1;
    let hit = collision
        .cast_ray(&slots, ray.0, ray.1, &mut free)
        .unwrap()
        .unwrap();
    assert_eq!(hit.normal, [-0.6, -0.8, 0.0]);
    slots.los[5] = -1;
    let hit = collision
        .cast_ray(&slots, ray.0, ray.1, &mut free)
        .unwrap()
        .unwrap();
    assert_eq!(hit.t, 4.0 / 6.0);
}

#[test]
fn a_ray_that_ends_on_a_face_hits_it_at_one_and_one_that_stops_short_does_not() {
    let (shape, slots) = cube_shape(false);
    let c = Collision::build(&shape, &slots, &mut free).unwrap();
    // The end is exactly on the plane x = 0: a crossing (the product of the distances is 0, not above 0).
    let hit = c
        .cast_ray(&slots, [-1.0, 0.5, 0.5], [0.0, 0.5, 0.5], &mut free)
        .unwrap()
        .expect("the end is on the face");
    assert_eq!(hit.t, 1.0);
    assert!(
        c.cast_ray(&slots, [-1.0, 0.5, 0.5], [-0.001, 0.5, 0.5], &mut free)
            .unwrap()
            .is_none()
    );
    // A ray that starts exactly on a face and goes in: both distances are 0 or inside, the start is not outside: no hit.
    assert!(
        c.cast_ray(&slots, [0.0, 0.5, 0.5], [1.0, 0.5, 0.5], &mut free)
            .unwrap()
            .is_none()
    );
}

/// A hull of two triangles through the origin: the plane z = 0 and one tilted about the y axis so that the dot of the two
/// normals is `cosine` (both constants 0, so only the normals decide whether the second plane is kept).
fn two_planes_at(cosine: f32) -> Hull {
    let sine = (1.0 - cosine * cosine).sqrt();
    let (mut shape, _) = cube_shape(false);
    let mesh = shape.meshes[0].as_mut().unwrap();
    mesh.positions = vec![
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [cosine, 0.0, -sine],
    ]
    .into();
    // (0, 1, 2) has the normal (0, 0, 1); (0, 1, 3) has (sin, 0, cos).
    mesh.indices = vec![0, 1, 2, 0, 1, 3];
    mesh.primitives = vec![crate::shape::Primitive {
        start: 0,
        num_elements: 6,
        material: 0,
    }];
    Hull::build(mesh, &mut free).unwrap()
}

#[test]
fn a_plane_is_the_same_as_one_the_hull_has_when_the_normals_are_within_point_99() {
    // A dot of 0.995 is above the 0.99 (the same plane: dropped), 0.985 is below it (a new plane).
    assert_eq!(two_planes_at(0.995).planes(), 1);
    assert_eq!(two_planes_at(0.985).planes(), 2);
    // Opposite sides of the same constant do not merge, and the constant has its own bound.
    assert_eq!(two_planes_at(-0.995).planes(), 2);
    assert_eq!(SAME_NORMAL, 0.99);
}

#[test]
fn a_ray_lying_in_a_face_plane_is_decided_by_the_other_planes() {
    // The segment (0, -1, .5) to (0, 2, .5) has both ends exactly on the plane x = 0 (a plane that "learns nothing") and
    // crosses y = 0 at 1/3 and y = 1 at 2/3: it enters the cube through the face y = 0.
    let (shape, slots) = cube_shape(false);
    let c = Collision::build(&shape, &slots, &mut free).unwrap();
    let hit = c
        .cast_ray(&slots, [0.0, -1.0, 0.5], [0.0, 2.0, 0.5], &mut free)
        .unwrap()
        .expect("the other planes still hit");
    assert_eq!(hit.normal, [0.0, -1.0, 0.0]);
    assert!((hit.t - 1.0 / 3.0).abs() < 1e-6, "{}", hit.t);
    // The same segment on the face x = 1 (the plane at the other end) and on the face z = 0.
    let hit = c
        .cast_ray(&slots, [1.0, -1.0, 0.5], [1.0, 2.0, 0.5], &mut free)
        .unwrap()
        .expect("x = 1");
    assert_eq!(hit.normal, [0.0, -1.0, 0.0]);
    let hit = c
        .cast_ray(&slots, [0.5, -1.0, 0.0], [0.5, 2.0, 0.0], &mut free)
        .unwrap()
        .expect("z = 0");
    assert_eq!(hit.normal, [0.0, -1.0, 0.0]);
    // A segment in the plane x = 0 that never meets the cube's other faces' range hits nothing.
    assert!(
        c.cast_ray(&slots, [0.0, 2.0, 0.5], [0.0, 3.0, 0.5], &mut free)
            .unwrap()
            .is_none()
    );
}

#[test]
fn strips_and_lists_of_one_cube_answer_every_ray_and_box_alike() {
    let (list, slots) = cube_shape(false);
    let (strip, _) = cube_shape(true);
    let a = Collision::build(&list, &slots, &mut free).unwrap();
    let b = Collision::build(&strip, &slots, &mut free).unwrap();
    let mut rng = crate::synth::Rng(7);
    let f = |rng: &mut crate::synth::Rng| (rng.below(4001) as f32 - 1000.0) / 1000.0;
    let mut boxes = 0;
    for _ in 0..400 {
        let (p, q) = (
            [f(&mut rng), f(&mut rng), f(&mut rng)],
            [f(&mut rng), f(&mut rng), f(&mut rng)],
        );
        let ra = a.cast_ray(&slots, p, q, &mut free).unwrap();
        let rb = b.cast_ray(&slots, p, q, &mut free).unwrap();
        assert_eq!(ra.map(|h| h.normal), rb.map(|h| h.normal), "{p:?} {q:?}");
        let lo = [p[0].min(q[0]), p[1].min(q[1]), p[2].min(q[2])];
        let hi = [p[0].max(q[0]), p[1].max(q[1]), p[2].max(q[2])];
        let planes = box_planes(lo, hi);
        let (x, y) = (
            a.box_hit(&slots, &crate::math::IDENTITY, &planes, &mut free)
                .unwrap(),
            b.box_hit(&slots, &crate::math::IDENTITY, &planes, &mut free)
                .unwrap(),
        );
        assert_eq!(x, y, "{lo:?} {hi:?}");
        boxes += usize::from(x);
    }
    assert!(boxes > 50, "{boxes}");
}

#[test]
fn a_vertex_on_a_plane_is_inside_so_a_box_that_only_touches_the_mesh_is_hit() {
    let (shape, slots) = cube_shape(false);
    let c = Collision::build(&shape, &slots, &mut free).unwrap();
    let hit = |lo: [f32; 3], hi: [f32; 3]| {
        c.box_hit(
            &slots,
            &crate::math::IDENTITY,
            &box_planes(lo, hi),
            &mut free,
        )
        .unwrap()
    };
    // The cube's corner (1, 1, 1) touches the box's corner; a hair further, it does not.
    assert!(hit([1.0, 1.0, 1.0], [2.0, 2.0, 2.0]));
    assert!(!hit([1.001, 1.0, 1.0], [2.0, 2.0, 2.0]));
    // A face touching a face, an edge across an edge, a box through the middle of two faces.
    assert!(hit([1.0, 0.25, 0.25], [2.0, 0.75, 0.75]));
    assert!(hit([-1.0, 0.25, -1.0], [2.0, 0.75, 0.25]));
    assert!(hit([0.25, 0.25, -1.0], [0.75, 0.75, 2.0]));
    // Wholly inside the solid, no vertex or polygon is in the box: not hit. Far away: not hit.
    assert!(!hit([0.25, 0.25, 0.25], [0.75, 0.75, 0.75]));
    assert!(!hit([5.0, 5.0, 5.0], [6.0, 6.0, 6.0]));
}

#[test]
fn work_is_charged_before_it_is_done_and_a_refused_charge_ends_the_query() {
    let (shape, slots) = cube_shape(false);
    let mut spent = 0u64;
    let c = Collision::build(&shape, &slots, &mut |u| {
        spent += u;
        true
    })
    .unwrap();
    assert!(spent >= 12 * TRI_WORK, "{spent}");
    assert_eq!(
        Collision::build(&shape, &slots, &mut |_| false),
        Err(ShapeError::OutOfBudget)
    );
    let mut paid = 0u64;
    c.cast_ray(&slots, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], &mut |u| {
        paid += u;
        true
    })
    .unwrap();
    assert_eq!(paid, RAY_PLANE_WORK * 7);
    assert_eq!(
        c.cast_ray(&slots, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], &mut |_| false),
        Err(ShapeError::OutOfBudget)
    );
    let mut paid = 0u64;
    c.box_hit(
        &slots,
        &crate::math::IDENTITY,
        &box_planes([5.0; 3], [6.0; 3]),
        &mut |u| {
            paid += u;
            true
        },
    )
    .unwrap();
    assert_eq!(paid, PART_WORK + 8 * VERTEX_WORK + 12 * TRI_WORK);
    assert_eq!(
        c.box_hit(
            &slots,
            &crate::math::IDENTITY,
            &box_planes([5.0; 3], [6.0; 3]),
            &mut |_| false
        ),
        Err(ShapeError::OutOfBudget)
    );
}

#[test]
fn a_shape_over_the_triangle_cap_is_refused() {
    let (mut shape, slots) = cube_shape(false);
    let mesh = shape.meshes[0].as_mut().unwrap();
    // Two primitives of 30,000 elements over the cube's indices: 20,000 triangles.
    mesh.indices = (0..30_000).map(|i| (i % 8) as u16).collect();
    mesh.primitives = vec![
        crate::shape::Primitive {
            start: 0,
            num_elements: 30_000,
            material: 0,
        },
        crate::shape::Primitive {
            start: 0,
            num_elements: 30_000,
            material: 0,
        },
    ];
    assert_eq!(slot_triangles(&shape, &slots, &mut free), Ok(20_000));
    assert!(matches!(
        Collision::build(&shape, &slots, &mut free),
        Err(ShapeError::Cap {
            what: "collision triangles",
            ..
        })
    ));
    let facts = crate::preload::facts(&shape, &mut free);
    assert!(matches!(
        facts,
        Err(ShapeError::Cap {
            what: "collision triangles",
            ..
        })
    ));
}

#[test]
fn rays_and_boxes_with_no_number_in_them_answer_nothing_and_do_not_panic() {
    let (shape, slots) = cube_shape(true);
    let c = Collision::build(&shape, &slots, &mut free).unwrap();
    let values = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1e30,
        -1e30,
        0.0,
        0.5,
    ];
    for &a in &values {
        for &b in &values {
            let _ = c
                .cast_ray(&slots, [a, b, 0.5], [b, a, a], &mut free)
                .unwrap();
            let planes = box_planes([a, a, a], [b, b, b]);
            let _ = c
                .box_hit(&slots, &crate::math::IDENTITY, &planes, &mut free)
                .unwrap();
            let mut world = crate::math::IDENTITY;
            world[3] = a;
            let planes = box_planes([0.0; 3], [1.0; 3]);
            let _ = c.box_hit(&slots, &world, &planes, &mut free).unwrap();
        }
    }
    let nan = [f32::NAN; 3];
    assert!(
        c.cast_ray(&slots, nan, [1.0, 1.0, 1.0], &mut free)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_strips_repeated_index_triangle_is_left_out_of_the_polygon_list_but_not_a_lists() {
    // One primitive over the cube's corners 0 (0,0,0) and 1 (1,0,0): the strip 0 1 1 makes the triangle (0, 1, 1), whose edge
    // crosses the box below while every vertex is outside it. A strip leaves it out (it compares the three indices); a list
    // does not.
    let across = box_planes([0.25, -1.0, -1.0], [0.75, 1.0, 1.0]);
    for (material, expect) in [(0x4000_0000u32, false), (0, true)] {
        let (mut shape, slots) = cube_shape(false);
        let mesh = shape.meshes[0].as_mut().unwrap();
        mesh.indices = vec![0, 1, 1];
        mesh.primitives = vec![crate::shape::Primitive {
            start: 0,
            num_elements: 3,
            material,
        }];
        let c = Collision::build(&shape, &slots, &mut free).unwrap();
        let hit = c
            .box_hit(&slots, &crate::math::IDENTITY, &across, &mut free)
            .unwrap();
        assert_eq!(hit, expect, "material {material:#x}");
    }
}

#[test]
fn a_sub_shape_claiming_billions_of_nodes_is_paid_for_by_the_nodes_there_are() {
    let (mut shape, slots) = cube_shape(false);
    shape.sub_shapes[0].num_nodes = i32::MAX;
    let mut spent = 0u64;
    let c = Collision::build(&shape, &slots, &mut |u| {
        spent += u;
        true
    })
    .unwrap();
    assert!(spent < 10_000, "{spent}");
    // A bill past the budget would end every query: this one asks for little.
    let mut paid = 0u64;
    c.cast_ray(&slots, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], &mut |u| {
        paid += u;
        true
    })
    .unwrap();
    assert!(paid < 100, "{paid}");
}
