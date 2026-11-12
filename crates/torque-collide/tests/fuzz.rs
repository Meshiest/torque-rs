//! The hostile-convex fuzz: pairs of convexes with NaN, infinite, huge and degenerate data (boxes, hulls with broken meshes,
//! wild transforms) through GJK, the features and their contacts, the three polygon lists and the working set and state list.
//! It must not panic, and every loop must stay within its cap. `TORQUE_FUZZ_CONVEX` sets the number of pairs (default 2,000;
//! the full run is 100,000).

mod common;

use common::{Rng, TEST_HULL, TestBoxes, TestGround, TestHulls, custom};
use torque_collide::collision::{CollisionList, MAX_COLLISIONS};
use torque_collide::convex::{
    Convex, ConvexKind, ConvexSource, HullClass, HullSlot, MAX_WORKING_SET, WorkingSet,
};
use torque_collide::feature::{StateList, features};
use torque_collide::gjk::{GjkState, MAX_GJK_ITERATIONS};
use torque_collide::math::{Box3, Matrix, Plane, mul_box};
use torque_collide::mesh::{Hull, MeshPart};
use torque_collide::poly::{
    ClippedPolyList, EarlyOutPolyList, ExtrudedPolyList, MAX_POLY_VERTS, MAX_POLYS, PolyList,
    Polyhedron,
};
use torque_collide::testing::BoxWorld;
use torque_collide::{ObjectId, World};

fn count() -> u64 {
    std::env::var("TORQUE_FUZZ_CONVEX")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000)
}

fn matrix(r: &mut Rng) -> Matrix {
    let mut m = torque_collide::math::IDENTITY;
    match r.below(4) {
        0 => {}
        1 => {
            m[3] = r.sane(50.0);
            m[7] = r.sane(50.0);
            m[11] = r.sane(50.0);
        }
        2 => {
            // A turn about z with a move.
            let a = r.sane(3.2) as f64;
            m[0] = a.cos() as f32;
            m[1] = -(a.sin() as f32);
            m[4] = a.sin() as f32;
            m[5] = a.cos() as f32;
            m[3] = r.sane(10.0);
            m[11] = r.sane(10.0);
        }
        _ => {
            for x in m.iter_mut().take(12) {
                *x = r.float();
            }
        }
    }
    m
}

fn hostile_box(r: &mut Rng) -> Box3 {
    if r.below(3) == 0 {
        Box3::new(r.f3(), r.f3())
    } else {
        let c = [r.sane(5.0), r.sane(5.0), r.sane(5.0)];
        let h = [r.sane(3.0).abs(), r.sane(3.0).abs(), r.sane(3.0).abs()];
        Box3::new(
            [c[0] - h[0], c[1] - h[1], c[2] - h[2]],
            [c[0] + h[0], c[1] + h[1], c[2] + h[2]],
        )
    }
}

fn mesh(r: &mut Rng) -> MeshPart {
    let nv = r.below(12) as usize;
    let mut verts = Vec::with_capacity(nv);
    for _ in 0..nv {
        verts.push(if r.below(4) == 0 {
            r.f3()
        } else {
            [r.sane(2.0), r.sane(2.0), r.sane(2.0)]
        });
    }
    let nt = r.below(16) as usize;
    let mut tris = Vec::with_capacity(nt);
    for _ in 0..nt {
        let pick = |r: &mut Rng| {
            if r.below(10) == 0 {
                r.next() as u32
            } else {
                r.below(nv.max(1) as u64) as u32
            }
        };
        tris.push([pick(r), pick(r), pick(r)]);
    }
    let materials = (0..nt).map(|_| r.next() as u32).collect();
    MeshPart {
        node: matrix(r),
        verts,
        tris,
        materials,
    }
}

fn view(r: &mut Rng, mask: u32) -> torque_collide::convex::ObjectView {
    let m = matrix(r);
    let ob = hostile_box(r);
    let scale = if r.below(3) == 0 {
        r.f3()
    } else {
        [1.0, 1.0, 1.0]
    };
    torque_collide::convex::ObjectView {
        obj_to_world: m,
        world_to_obj: torque_collide::math::inverse(&m),
        scale,
        obj_box: ob,
        world_box: mul_box(&m, &ob),
        world_radius: r.float(),
        type_mask: mask,
        collision_disabled: r.below(10) == 0,
    }
}

fn source(r: &mut Rng) -> ConvexSource {
    match r.below(7) {
        0 => ConvexSource::Player {
            has_shape: r.below(5) != 0,
        },
        1 => custom(TestBoxes {
            boxes: (0..r.below(14)).map(|_| (r.f3(), r.f3())).collect(),
        }),
        2 => custom(TestHulls {
            slots: (0..r.below(10))
                .map(|i| HullSlot {
                    slot: i as u32,
                    present: r.below(4) != 0,
                    bounds: hostile_box(r),
                })
                .collect(),
        }),
        3 => ConvexSource::Shape {
            has_shape: r.below(5) != 0,
            slots: (0..r.below(10))
                .map(|i| HullSlot {
                    slot: i as u32,
                    present: r.below(4) != 0,
                    bounds: hostile_box(r),
                })
                .collect(),
        },
        4 => custom(TestGround),
        5 => ConvexSource::Item {
            has_shape: r.below(4) != 0,
        },
        _ => ConvexSource::Zone,
    }
}

fn convex(r: &mut Rng, id: u64, object: u32) -> Convex {
    let kind = match r.below(4) {
        0 => ConvexKind::Box {
            center: r.f3(),
            size: r.f3(),
        },
        1 => ConvexKind::OrthoBox {
            center: [r.sane(5.0), r.sane(5.0), r.sane(5.0)],
            size: [r.sane(2.0).abs(), r.sane(2.0).abs(), r.sane(2.0).abs()],
        },
        2 => ConvexKind::Hull {
            class: TEST_HULL,
            slot: r.below(3) as u32,
            bounds: hostile_box(r),
        },
        _ => ConvexKind::Hull {
            class: HullClass::SHAPE_BASE,
            slot: r.below(3) as u32,
            bounds: hostile_box(r),
        },
    };
    Convex {
        id,
        object: ObjectId(object),
        kind,
    }
}

fn planes(r: &mut Rng) -> Vec<Plane> {
    (0..r.below(40))
        .map(|_| {
            if r.below(4) == 0 {
                Plane::new(r.float(), r.float(), r.float(), r.float())
            } else {
                let n = torque_collide::math::normalize([r.sane(1.0), r.sane(1.0), r.sane(1.0)]);
                Plane::new(n[0], n[1], n[2], r.sane(5.0))
            }
        })
        .collect()
}

fn check_list_caps(verts: usize, polys: usize) {
    assert!(verts <= MAX_POLY_VERTS, "{verts} vertices");
    assert!(polys <= MAX_POLYS + 1, "{polys} polygons");
}

#[test]
fn hostile_convex_pairs_never_panic_and_stay_within_their_caps() {
    let n = count();
    let mut r = Rng(0x00c0_111d_e5ee_d000);
    let mut gjk_max = 0u32;
    let mut contacts = 0u64;
    for case in 0..n {
        let mut w = BoxWorld::new();
        if r.below(8) == 0 {
            w.budget = r.below(200);
        }
        for id in 1..=2u32 {
            let bit = r.below(32);
            let v = view(&mut r, 1 << bit);
            w.add(ObjectId(id), v, source(&mut r));
            for slot in 0..3 {
                if r.below(2) == 0 {
                    let parts = (0..r.below(3)).map(|_| mesh(&mut r)).collect();
                    let node = if r.below(2) == 0 {
                        Some(matrix(&mut r))
                    } else {
                        None
                    };
                    w.add_hull(ObjectId(id), slot, Hull::new(node, parts));
                }
            }
        }
        let a = convex(&mut r, 1, 1);
        let b = convex(&mut r, 2, 2);
        let a2w = matrix(&mut r);
        let b2w = matrix(&mut r);

        // GJK.
        let mut s = GjkState::new(&w, a, b, &a2w, &b2w);
        let dont_care = if r.below(2) == 0 { r.float() } else { 1e10 };
        let inverses = if r.below(2) == 0 {
            None
        } else {
            Some((matrix(&mut r), matrix(&mut r)))
        };
        let _ = s.distance(
            &mut w,
            &a2w,
            &b2w,
            dont_care,
            inverses.as_ref().map(|(x, y)| (x, y)),
        );
        assert!(
            s.iterations <= MAX_GJK_ITERATIONS,
            "case {case}: {} iterations",
            s.iterations
        );
        gjk_max = gjk_max.max(s.iterations);
        let _ = s.closest_points();

        // Features and their contacts.
        let fa = features(&w, &a, &a2w, r.f3());
        let fb = features(&w, &b, &b2w, r.f3());
        let mut cl = CollisionList::new();
        cl.reset();
        let _ = fa.collide(&fb, &mut cl, r.float(), &mut |_| true);
        assert!(cl.count <= MAX_COLLISIONS);
        contacts += cl.count as u64;

        // The swept box.
        let mut ph = Polyhedron::default();
        ph.build_box(&matrix(&mut r), &hostile_box(&mut r));
        let mut ex = ExtrudedPolyList::new();
        ex.extrude(&ph, r.f3());
        ex.set_velocity(r.f3());
        ex.collisions.reset();
        let _ = a.poly_list(&mut w, &mut ex);
        let _ = b.poly_list(&mut w, &mut ex);
        if r.below(2) == 0 {
            ex.add_box(&hostile_box(&mut r));
        }
        assert!(ex.collisions.count <= MAX_COLLISIONS);
        check_list_caps(ex.verts.len(), 0);

        // The clipped and early-out lists.
        let mut cp = ClippedPolyList::new();
        cp.planes = planes(&mut r);
        cp.normal = r.f3();
        cp.normal_test = r.below(2) == 0;
        let _ = a.poly_list(&mut w, &mut cp);
        let _ = b.poly_list(&mut w, &mut cp);
        check_list_caps(cp.verts.len(), cp.polys.len());
        for p in &cp.polys {
            let _ = cp.poly_indices(p);
        }
        let mut eo = EarlyOutPolyList::new();
        eo.planes = planes(&mut r);
        eo.normal = r.f3();
        let _ = a.poly_list(&mut w, &mut eo);
        let _ = b.poly_list(&mut w, &mut eo);
        let _ = eo.is_empty();
        check_list_caps(eo.verts.len(), eo.polys.len());

        // The working set and the state list of a mover.
        let mut ws = WorkingSet::new();
        let query = hostile_box(&mut r);
        let _ = ws.update(&mut w, ObjectId(99), &query, u32::MAX);
        let _ = ws.update(&mut w, ObjectId(99), &hostile_box(&mut r), r.next() as u32);
        assert!(ws.len() <= MAX_WORKING_SET);
        let working = ws.to_vec();
        let me = convex(&mut r, 1_000_000, 1);
        let mut sl = StateList::default();
        let mat = matrix(&mut r);
        sl.update(&mut w, &me, &mat, r.f3(), &working);
        let mut cl2 = CollisionList::new();
        let _ = sl.collision_info(&mut w, &me, &mat, r.f3(), &mut cl2, r.float());
        assert!(cl2.count <= MAX_COLLISIONS);
        for st in &sl.states {
            assert!(st.iterations <= MAX_GJK_ITERATIONS);
        }
        let _ = w.charge(0);
    }
    eprintln!(
        "convex fuzz: {n} pairs, 0 panics, GJK at most {gjk_max} iterations, {contacts} contacts"
    );
}
