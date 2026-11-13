//! Unit cases worked by hand from the Torque code: the Player's box swept onto an object of ortho boxes and onto an object of
//! mesh hulls (`updatePos`'s `Polyhedron::buildBox` + `ExtrudedPolyList`), a contact search (`findContact`'s
//! `ClippedPolyList`), the convexes the builders make (the Torque classes' and the test classes' through the builder seam), the
//! working set's order, and GJK distances of known pairs.

use crate::common;

use common::{TEST_HULL, TestBoxes, TestGround, TestHulls, custom, view_at};
use torque_collide::convex::{Convex, ConvexKind, ConvexSource, HullSlot, WorkingSet};
use torque_collide::gjk::GjkState;
use torque_collide::math::{Box3, IDENTITY, Matrix, Plane, d};
use torque_collide::mesh::{Hull, MeshPart};
use torque_collide::poly::{ClippedPolyList, ExtrudedPolyList, Polyhedron};
use torque_collide::testing::BoxWorld;
use torque_collide::{ObjectId, World};

/// `StaticObjectType | StaticShapeObjectType`: the type of the test objects.
const STATIC: u32 = 1 | (1 << 13);
const PLAYER: u32 = 1 << 14;

fn translation(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    m[3] = x;
    m[7] = y;
    m[11] = z;
    m
}

fn box_object(
    world: &mut BoxWorld,
    id: u32,
    pos: [f32; 3],
    obj_box: Box3,
    boxes: Vec<([f32; 3], [f32; 3])>,
) {
    world.add(
        ObjectId(id),
        view_at(pos, obj_box, STATIC),
        custom(TestBoxes { boxes }),
    );
}

/// The Player's sweep as `updatePos` sets it up: the box at the start, extruded along `vector`, the velocity, the list reset,
/// then every convex of the working set whose box meets the swept box feeds the list.
fn sweep(
    world: &mut BoxWorld,
    ws: &WorkingSet,
    start: [f32; 3],
    obj_box: &Box3,
    velocity: [f32; 3],
    dt: f32,
) -> ExtrudedPolyList {
    let mut ph = Polyhedron::default();
    ph.build_box(&translation(start[0], start[1], start[2]), obj_box);
    let vector = [
        (d(velocity[0]) * d(dt)) as f32,
        (d(velocity[1]) * d(dt)) as f32,
        (d(velocity[2]) * d(dt)) as f32,
    ];
    let mut list = ExtrudedPolyList::new();
    list.extrude(&ph, vector);
    list.set_velocity(velocity);
    list.collisions.reset();
    for c in ws.iter().copied().collect::<Vec<_>>() {
        c.poly_list(world, &mut list).unwrap();
    }
    list
}

#[test]
fn the_player_falling_onto_a_box_object_meets_its_top() {
    let mut w = BoxWorld::new();
    box_object(
        &mut w,
        1,
        [0.0; 3],
        Box3::new([-1.0, -1.0, -0.3], [1.0, 1.0, 0.3]),
        vec![([0.0; 3], [2.0, 2.0, 0.6])],
    );
    let player_box = Box3::new([-0.5, -0.5, 0.0], [0.5, 0.5, 2.0]);
    w.add(
        ObjectId(2),
        view_at([0.0, 0.0, 0.5], player_box, PLAYER),
        ConvexSource::Player { has_shape: true },
    );
    let mut ws = WorkingSet::new();
    ws.update(&mut w, ObjectId(2), &Box3::new([-4.0; 3], [4.0; 3]), STATIC)
        .unwrap();
    // One ortho box: the object box's centre plus the offset, half the 2 x 2 x 0.6 size (0.5 * 0.6f32 is exactly 0.3f32).
    let entries: Vec<Convex> = ws.iter().copied().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].kind,
        ConvexKind::OrthoBox {
            center: [0.0, 0.0, 0.0],
            size: [1.0, 1.0, 0.3]
        }
    );

    // Falling at 10 for a tick of 0.032: the vector is f32(-10 * 0.032f32) = -0.32000002f32 (0.032f32 is a hair above 0.032),
    // and the reach is the bottom face's dot with it, 0.32000002f32.
    let list = sweep(
        &mut w,
        &ws,
        [0.0, 0.0, 0.5],
        &player_box,
        [0.0, 0.0, -10.0],
        0.032,
    );
    let c = &list.collisions;
    assert_eq!(c.count, 1, "{:?}", c.as_slice());
    let hit = c.entries[0];
    assert_eq!(hit.normal, [0.0, 0.0, 1.0]);
    assert_eq!(hit.face_dot, 1.0);
    assert_eq!(hit.object, Some(ObjectId(1)));
    // The bottom face is (0, 0, -1, 0.5); the object's top is 0.3f32 (the corner sum 0 + 0.3f32).
    // First pass: 0.5 - 0.3f32 = 0.19999998807907104, a float: the reach is not met yet, so a hit.
    // Second pass: the reach times 0.2f32 is 0.064, above 0.01, so the top is pushed by 0.01f32 along its normal:
    // f32(0.01f32 + 0.3f32) = 0.31f32, at a distance f32(0.5 - 0.31f32) = 0.19f32.
    // t1 = 0.19999999 / 0.32 and t2 = 0.19 / 0.32 are 0.03 apart (< 0.1), so the time is t2.
    let reach = (10.0 * d(0.032f32)) as f32;
    assert_eq!(reach, 0.320_000_02);
    let t2 = (d(0.19f32) / d(reach)) as f32;
    assert_eq!(c.t, t2);
    assert_eq!(c.max_height, 0.3);
}

#[test]
fn the_player_falling_onto_a_mesh_object_meets_both_triangles_at_once() {
    let mut w = BoxWorld::new();
    let bounds = Box3::new([-1.0, -1.0, -0.3], [1.0, 1.0, 0.3]);
    w.add(
        ObjectId(1),
        view_at([0.0; 3], bounds, STATIC),
        custom(TestHulls {
            slots: vec![HullSlot {
                slot: 0,
                present: true,
                bounds,
            }],
        }),
    );
    // A quad at z = 0.3, two triangles facing up (winding: (p1 - p2) x (p3 - p2) points out).
    let verts = vec![
        [-1.0, -1.0, 0.3],
        [1.0, -1.0, 0.3],
        [1.0, 1.0, 0.3],
        [-1.0, 1.0, 0.3],
    ];
    let part = MeshPart {
        node: IDENTITY,
        verts,
        tris: vec![[0, 2, 1], [0, 3, 2]],
        materials: vec![7, 7],
    };
    w.add_hull(ObjectId(1), 0, Hull::new(Some(IDENTITY), vec![part]));
    let player_box = Box3::new([-0.5, -0.5, 0.0], [0.5, 0.5, 2.0]);
    w.add(
        ObjectId(2),
        view_at([0.0, 0.0, 0.5], player_box, PLAYER),
        ConvexSource::Player { has_shape: true },
    );
    let mut ws = WorkingSet::new();
    ws.update(&mut w, ObjectId(2), &Box3::new([-4.0; 3], [4.0; 3]), STATIC)
        .unwrap();
    assert!(matches!(
        ws.iter().next().unwrap().kind,
        ConvexKind::Hull {
            class: TEST_HULL,
            slot: 0,
            ..
        }
    ));

    let list = sweep(
        &mut w,
        &ws,
        [0.0, 0.0, 0.5],
        &player_box,
        [0.0, 0.0, -10.0],
        0.032,
    );
    let c = &list.collisions;
    // Both triangles reach the same time: the second is within EqualEpsilon of the first and joins the list.
    assert_eq!(c.count, 2, "{:?}", c.as_slice());
    assert_eq!(
        c.t,
        (d(0.19f32) / (10.0 * d(0.032f32)) as f32 as f64) as f32
    );
    for hit in c.as_slice() {
        assert_eq!(hit.normal, [0.0, 0.0, 1.0]);
        assert_eq!(hit.material, 7);
    }
}

#[test]
fn a_contact_search_keeps_the_floor_under_the_feet() {
    // findContact's list: six planes around the feet, then the floor's ortho box fed to it.
    let mut w = BoxWorld::new();
    box_object(
        &mut w,
        1,
        [0.0, 0.0, -0.3],
        Box3::new([-1.0, -1.0, -0.3], [1.0, 1.0, 0.3]),
        vec![([0.0; 3], [2.0, 2.0, 0.6])],
    );
    let floor = Convex {
        id: 1,
        object: ObjectId(1),
        kind: ConvexKind::OrthoBox {
            center: [0.0; 3],
            size: [1.0, 1.0, 0.3],
        },
    };
    let mut l = ClippedPolyList::new();
    l.clear();
    l.base.reset();
    l.base.set_interest_normal(Some([0.0, 0.0, -1.0]));
    l.planes = vec![
        Plane::new(-1.0, 0.0, 0.0, -0.5),
        Plane::new(0.0, 1.0, 0.0, -0.5),
        Plane::new(1.0, 0.0, 0.0, -0.5),
        Plane::new(0.0, -1.0, 0.0, -0.5),
        Plane::new(0.0, 0.0, -1.0, -0.1),
        Plane::new(0.0, 0.0, 1.0, -0.1),
    ];
    floor.poly_list(&mut w, &mut l).unwrap();
    assert_eq!(l.polys.len(), 1);
    let p = l.polys[0];
    assert_eq!(p.plane.normal(), [0.0, 0.0, 1.0]);
    assert_eq!(p.object, Some(ObjectId(1)));
    for &i in l.poly_indices(&p) {
        let v = l.verts[i as usize].point;
        assert!(
            v[0].abs() <= 0.5 && v[1].abs() <= 0.5 && v[2] == 0.0,
            "{v:?}"
        );
    }
}

#[test]
fn the_builders_make_the_expected_boxes() {
    let mut w = BoxWorld::new();
    // A Player's convex: an ortho box of its object box, centred on it.
    let pbox = Box3::new([-0.6, -0.6, 0.0], [0.6, 0.6, 2.4]);
    w.add(
        ObjectId(2),
        view_at([1.0, 2.0, 3.0], pbox, PLAYER),
        ConvexSource::Player { has_shape: true },
    );
    // A Player with no shape adds nothing.
    w.add(
        ObjectId(3),
        view_at([1.0, 2.0, 3.0], pbox, PLAYER),
        ConvexSource::Player { has_shape: false },
    );
    // An object of one offset box: the centre is the offset plus the object box's centre, the size half the box's.
    w.add(
        ObjectId(4),
        view_at(
            [0.0; 3],
            Box3::new([-2.0, -1.0, 0.0], [2.0, 1.0, 1.2]),
            STATIC,
        ),
        custom(TestBoxes {
            boxes: vec![([0.25, -0.5, 0.1], [2.0, 1.0, 1.2])],
        }),
    );
    // A ground: a 5 x 5 box under the query's centre.
    w.add(
        ObjectId(5),
        view_at(
            [0.0; 3],
            Box3::new([-1000.0, -1000.0, -0.5], [1000.0, 1000.0, 0.0]),
            1,
        ),
        custom(TestGround),
    );
    // A zone: a box convex of its object box.
    w.add(
        ObjectId(6),
        view_at(
            [0.0; 3],
            Box3::new([-1.0, -2.0, -3.0], [1.0, 2.0, 3.0]),
            1 << 23,
        ),
        ConvexSource::Zone,
    );
    let mut ws = WorkingSet::new();
    let query = Box3::new([-2.0, -1.0, -1.0], [4.0, 5.0, 6.0]);
    ws.update(&mut w, ObjectId(99), &query, PLAYER | STATIC | (1 << 23))
        .unwrap();
    let find = |id: u32| {
        ws.iter()
            .filter(|c| c.object == ObjectId(id))
            .map(|c| c.kind)
            .collect::<Vec<_>>()
    };
    // centre = f32((min + max) * 0.5): (0, 0, 1.2f32); size = f32(f32(max - min) * 0.5): f32(0.6f32 + 0.6f32) is exactly
    // 2 * 0.6f32, so x and y are 0.6f32, and z is half of 2.4f32, 1.2f32.
    assert_eq!(
        find(2),
        vec![ConvexKind::OrthoBox {
            center: [0.0, 0.0, 1.2],
            size: [0.6, 0.6, 1.2]
        }]
    );
    assert!(find(3).is_empty());
    // x = f32(0.25 + 0) = 0.25, y = f32(-0.5 + 0) = -0.5, z = f32(0.1 + 0.6) = 0.7f32; size (1.0, 0.5, 0.6).
    assert_eq!(
        find(4),
        vec![ConvexKind::OrthoBox {
            center: [0.25, -0.5, (d(0.1f32) + d(0.6f32)) as f32],
            size: [1.0, 0.5, 0.6]
        }]
    );
    // The query's centre: (1, 2); z the object box's centre and half height.
    assert_eq!(
        find(5),
        vec![ConvexKind::OrthoBox {
            center: [1.0, 2.0, -0.25],
            size: [5.0, 5.0, 0.25]
        }]
    );
    assert_eq!(
        find(6),
        vec![ConvexKind::Box {
            center: [0.0; 3],
            size: [1.0, 2.0, 3.0]
        }]
    );
    // A second update keeps every entry once and moves the ground's box to the new query's centre.
    let query2 = Box3::new([-2.0, -1.0, -1.0], [6.0, 5.0, 6.0]);
    ws.update(&mut w, ObjectId(99), &query2, PLAYER | STATIC | (1 << 23))
        .unwrap();
    assert_eq!(ws.len(), 4);
    assert_eq!(
        ws.iter()
            .filter(|c| c.object == ObjectId(5))
            .map(|c| c.kind)
            .collect::<Vec<_>>(),
        vec![ConvexKind::OrthoBox {
            center: [2.0, 2.0, -0.25],
            size: [5.0, 5.0, 0.25]
        }]
    );
}

#[test]
fn the_working_set_takes_objects_in_the_containers_order_newest_first() {
    let mut w = BoxWorld::new();
    let b = Box3::new([-0.5; 3], [0.5; 3]);
    for (id, x) in [(1, 2.0f32), (2, -3.0), (3, 5.0)] {
        box_object(&mut w, id, [x, 0.0, 0.0], b, vec![([0.0; 3], [1.0; 3])]);
    }
    let mut ws = WorkingSet::new();
    ws.update(
        &mut w,
        ObjectId(99),
        &Box3::new([-10.0; 3], [10.0; 3]),
        STATIC,
    )
    .unwrap();
    // The container's order (1, 2, 3, not sorted), each added at the head.
    let order: Vec<u32> = ws.iter().map(|c| c.object.0).collect();
    assert_eq!(order, vec![3, 2, 1]);
    // A box that no longer meets the query leaves; the rest keep their places.
    ws.update(
        &mut w,
        ObjectId(99),
        &Box3::new([-10.0; 3], [3.0, 10.0, 10.0]),
        STATIC,
    )
    .unwrap();
    let order: Vec<u32> = ws.iter().map(|c| c.object.0).collect();
    assert_eq!(order, vec![2, 1]);
}

#[test]
fn gjk_measures_known_pairs() {
    let mut w = BoxWorld::new();
    w.add(
        ObjectId(1),
        view_at([0.0; 3], Box3::new([-1.0; 3], [1.0; 3]), STATIC),
        ConvexSource::None,
    );
    w.add(
        ObjectId(2),
        view_at([0.0; 3], Box3::new([-1.0; 3], [1.0; 3]), STATIC),
        ConvexSource::None,
    );
    let unit = ConvexKind::Box {
        center: [0.0; 3],
        size: [1.0; 3],
    };
    let a = Convex {
        id: 1,
        object: ObjectId(1),
        kind: unit,
    };
    let b = Convex {
        id: 2,
        object: ObjectId(2),
        kind: unit,
    };
    let cases: [([f32; 3], f32); 3] = [
        // Face to face along x: the gap is 3.
        ([5.0, 0.0, 0.0], 3.0),
        // Edge to edge: a gap of 1 in x and in y, sqrt(2) as mSqrt rounds it.
        ([3.0, 3.0, 0.0], 2.0f32.sqrt()),
        // Corner to corner: 1 in each axis.
        ([3.0, 3.0, 3.0], 3.0f32.sqrt()),
    ];
    for (pos, want) in cases {
        let b2w = translation(pos[0], pos[1], pos[2]);
        let mut s = GjkState::new(&w, a, b, &IDENTITY, &b2w);
        let got = s.distance(&mut w, &IDENTITY, &b2w, 1e10, None);
        assert!(
            (got - want).abs() <= want * 1e-5,
            "{pos:?}: {got} vs {want}"
        );
        assert!(s.iterations <= torque_collide::gjk::MAX_GJK_ITERATIONS);
    }
    // Overlapping boxes: the simplex fills and the distance is zero.
    let b2w = translation(0.5, 0.25, 0.0);
    let mut s = GjkState::new(&w, a, b, &IDENTITY, &b2w);
    let got = s.distance(&mut w, &IDENTITY, &b2w, 1e10, None);
    assert!(got <= 1e-3, "{got}");
    // Past the don't-care distance the estimate returns early.
    let b2w = translation(50.0, 0.0, 0.0);
    let mut s = GjkState::new(&w, a, b, &IDENTITY, &b2w);
    let got = s.distance(&mut w, &IDENTITY, &b2w, 1.0, None);
    assert!(got > 1.0);
    let _ = w.charge(0);
}

/// `Item::buildConvex`: a box convex of the object box (centre `f32((max + min) * 0.5)`, size `f32((max - min) * 0.5)`)
/// when the item has a shape and the query meets its world box; nothing without a shape; one entry across updates.
#[test]
fn item_builder_makes_a_box_of_its_object_box() {
    let mut w = BoxWorld::new();
    let ob = Box3::new([-0.25, -0.25, 0.0], [0.25, 0.75, 0.5]);
    w.add(
        ObjectId(1),
        view_at([1.0, 1.0, 0.0], ob, 1 << 8),
        ConvexSource::Item { has_shape: true },
    );
    w.add(
        ObjectId(2),
        view_at([1.0, 1.0, 0.0], ob, 1 << 8),
        ConvexSource::Item { has_shape: false },
    );
    let mut ws = WorkingSet::new();
    let query = Box3::new([0.0, 0.0, -1.0], [2.0, 2.0, 1.0]);
    ws.update(&mut w, ObjectId(99), &query, 1 << 8).unwrap();
    ws.update(&mut w, ObjectId(99), &query, 1 << 8).unwrap();
    let kinds: Vec<_> = ws.iter().map(|c| (c.object, c.kind)).collect();
    assert_eq!(
        kinds,
        vec![(
            ObjectId(1),
            ConvexKind::Box {
                center: [0.0, 0.25, 0.25],
                size: [0.25, 0.5, 0.25]
            }
        )]
    );
}

/// `Convex::findClosestState`: the state list is updated (states only for working convexes within the mover's box
/// grown by 1), each measured by GJK with the scaled inverses; the nearest wins, unless it is not below `dontCare`.
#[test]
fn find_closest_state_picks_the_nearest_below_dont_care() {
    use torque_collide::feature::StateList;
    let mut w = BoxWorld::new();
    let cube = Box3::new([-1.0; 3], [1.0; 3]);
    w.add(
        ObjectId(1),
        view_at([0.0; 3], cube, STATIC),
        ConvexSource::None,
    );
    // Object 2 is 0.5 away in y, object 3 is 1 away in x; object 4 is beyond the grown box and gets no state.
    w.add(
        ObjectId(2),
        view_at([0.0, 2.5, 0.0], cube, STATIC),
        ConvexSource::None,
    );
    w.add(
        ObjectId(3),
        view_at([3.0, 0.0, 0.0], cube, STATIC),
        ConvexSource::None,
    );
    w.add(
        ObjectId(4),
        view_at([9.0, 0.0, 0.0], cube, STATIC),
        ConvexSource::None,
    );
    let unit = ConvexKind::Box {
        center: [0.0; 3],
        size: [1.0; 3],
    };
    let me = Convex {
        id: 100,
        object: ObjectId(1),
        kind: unit,
    };
    let working: Vec<Convex> = (2..=4)
        .map(|i| Convex {
            id: i,
            object: ObjectId(i as u32),
            kind: unit,
        })
        .collect();
    let mut list = StateList::default();
    let best = list.find_closest_state(&mut w, &me, &IDENTITY, [1.0; 3], &working, 2.0);
    assert_eq!(list.len(), 2);
    let i = best.expect("a state below 2");
    assert_eq!(list.states[i].b.object, ObjectId(2));
    assert!((list.states[i].dist - 0.5).abs() <= 1e-5);
    // Not below the don't-care distance (0.5 itself, or less): none.
    assert_eq!(
        list.find_closest_state(&mut w, &me, &IDENTITY, [1.0; 3], &working, 0.5),
        None
    );
    assert_eq!(
        list.find_closest_state(&mut w, &me, &IDENTITY, [1.0; 3], &working, f32::NAN),
        None
    );
}

/// The GJK state points at its two convexes, so a box a `buildConvex` moves in place (a test ground's 5 by 5 box follows the
/// query's centre) is measured where it is now: `updateStateList` keeps the state and its distance follows.
#[test]
fn a_state_measures_the_plane_box_where_the_working_set_moved_it() {
    use torque_collide::feature::StateList;
    let mut w = BoxWorld::new();
    let cube = Box3::new([-1.0; 3], [1.0; 3]);
    let plane = Box3::new([-1.0e9, -1.0e9, -0.5], [1.0e9, 1.0e9, 0.5]);
    w.add(
        ObjectId(1),
        view_at([0.0, 0.0, 2.0], cube, 0),
        ConvexSource::None,
    );
    w.add(
        ObjectId(2),
        view_at([0.0, 0.0, -0.5], plane, 1),
        custom(TestGround),
    );
    let me = Convex {
        id: 100,
        object: ObjectId(1),
        kind: ConvexKind::Box {
            center: [0.0; 3],
            size: [1.0; 3],
        },
    };
    let at = |x: f32| Convex {
        id: 7,
        object: ObjectId(2),
        kind: ConvexKind::OrthoBox {
            center: [x, 0.0, 0.0],
            size: [5.0, 5.0, 0.5],
        },
    };
    let mover = translation(0.0, 0.0, 2.0);
    let mut list = StateList::default();
    // Under the mover: 1 above the plane's top (z 0).
    let i = list
        .find_closest_state(&mut w, &me, &mover, [1.0; 3], &[at(0.0)], 10.0)
        .expect("a state");
    assert!((list.states[i].dist - 1.0).abs() <= 1e-5);
    // The box made 20 away, then moved under the mover by the working set: the same state, measured at the new centre.
    let mut list = StateList::default();
    list.find_closest_state(&mut w, &me, &mover, [1.0; 3], &[at(20.0)], 100.0);
    let i = list
        .find_closest_state(&mut w, &me, &mover, [1.0; 3], &[at(0.0)], 10.0)
        .expect("a state");
    assert_eq!(list.len(), 1);
    assert!(
        (list.states[i].dist - 1.0).abs() <= 1e-5,
        "{}",
        list.states[i].dist
    );
}
