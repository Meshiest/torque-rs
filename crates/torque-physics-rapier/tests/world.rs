//! The world's rules: map pieces made as the engine's collision holds them, built by cell on first use; bodies checked, held
//! and capped; the budget paid before work; two worlds given the same calls the same bits.

mod common;

use common::{Rng, box_hull, free, interior, terrain};
use rapier3d::prelude::{QueryFilter, Ray, Vector};
use torque_collide::ObjectId;
use torque_collide::map::MapShape;
use torque_collide::math::Box3;
use torque_physics_rapier::work;
use torque_physics_rapier::world::Cap;
use torque_physics_rapier::{
    BodyDesc, BodyId, BodyState, Config, Error, Limits, Motion, Shape, World,
};

const TERRAIN: ObjectId = ObjectId(1);
const KEEP: ObjectId = ObjectId(2);

/// A checkerboard of heights: the engine's alternating cut puts a valley along every square's diagonal, where one fixed cut
/// would leave a ridge across half of them.
fn checker(i: usize, j: usize) -> f32 {
    if (i + j) % 2 == 1 { 4.0 } else { 0.0 }
}

fn ball(at: [f32; 3]) -> BodyDesc {
    BodyDesc {
        shape: Shape::Ball { radius: 0.5 },
        position: at,
        ..BodyDesc::default()
    }
}

#[test]
fn terrain_triangles_follow_the_engines_diagonal() {
    let shape = terrain(checker, 8, [-1024.0, -1024.0], 10.0, false, Vec::new());
    let MapShape::Terrain(t) = &*shape else {
        unreachable!()
    };
    let t = t.clone();
    let mut w = World::new(Config::default());
    w.add_map(TERRAIN, shape).unwrap();
    let region = Box3::new([-40.0, -40.0, -100.0], [40.0, 40.0, 100.0]);
    assert!(w.ensure_region(&region, &mut free).unwrap() > 0);
    // The broad phase takes the new colliders in on a step.
    w.step(&mut free).unwrap();
    let mut rng = Rng(7);
    let mut worst = 0.0f32;
    for _ in 0..400 {
        let (x, y) = (rng.range(-38.0, 38.0), rng.range(-38.0, 38.0));
        let ray = Ray::new(Vector::new(x, y, 100.0), Vector::new(0.0, 0.0, -1.0));
        let (_, toi) = w
            .rapier()
            .cast_ray(&ray, 200.0, true, QueryFilter::default())
            .expect("the terrain is under every point");
        let expect = t.height(x, y).expect("on the block");
        worst = worst.max(((100.0 - toi) - expect).abs());
    }
    assert!(worst < 2e-3, "worst height difference {worst}");
}

#[test]
fn cells_are_built_on_first_use_and_only_near_moving_bodies() {
    let mut w = World::new(Config::default());
    w.add_map(
        TERRAIN,
        terrain(|_, _| 0.0, 8, [-1024.0, -1024.0], 0.0, false, Vec::new()),
    )
    .unwrap();
    assert_eq!(w.map_stats().cells, 0);
    w.add_body(BodyId(1), &ball([10.0, 10.0, 3.0]), &mut free)
        .unwrap();
    let r = w.step(&mut free).unwrap();
    assert_eq!(r.cells_built, 1, "{r:?}");
    assert_eq!(w.map_stats().triangles, 128);
    // The same cell is not built again; a body over a cell edge builds its neighbors.
    assert_eq!(w.step(&mut free).unwrap().cells_built, 0);
    w.add_body(BodyId(2), &ball([64.2, 0.3, 3.0]), &mut free)
        .unwrap();
    assert_eq!(w.step(&mut free).unwrap().cells_built, 3);
    // Removing the map clears its cells; they are made again when needed.
    assert!(w.remove_map(TERRAIN).unwrap());
    assert_eq!(w.collider_count(), 2);
}

#[test]
fn a_box_comes_to_rest_on_the_terrain_and_one_on_an_interior_floor() {
    let mut w = World::new(Config::default());
    w.add_map(
        TERRAIN,
        terrain(|_, _| 0.0, 8, [-1024.0, -1024.0], 20.0, false, Vec::new()),
    )
    .unwrap();
    // A slab 2 thick, its top at 22, at 100 east.
    w.add_map(
        KEEP,
        interior(
            vec![box_hull([-8.0, -8.0, -2.0], [8.0, 8.0, 0.0])],
            [100.0, 0.0, 22.0],
        ),
    )
    .unwrap();
    let cube = |at| BodyDesc {
        shape: Shape::Box { half: [0.5; 3] },
        position: at,
        ..BodyDesc::default()
    };
    w.add_body(BodyId(1), &cube([0.0, 0.0, 30.0]), &mut free)
        .unwrap();
    w.add_body(BodyId(2), &cube([100.0, 0.0, 30.0]), &mut free)
        .unwrap();
    for _ in 0..200 {
        w.step(&mut free).unwrap();
    }
    let a = w.body(BodyId(1)).unwrap();
    let b = w.body(BodyId(2)).unwrap();
    assert!((a.position[2] - 20.5).abs() < 0.05, "{a:?}");
    assert!((b.position[2] - 22.5).abs() < 0.05, "{b:?}");
    assert!(a.sleeping && b.sleeping, "{a:?} {b:?}");
    assert_eq!(w.map_stats().hulls, 1);
}

#[test]
fn non_finite_values_are_refused_before_they_reach_the_world() {
    let mut w = World::new(Config::default());
    let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    for v in bad {
        let mut d = BodyDesc::default();
        d.position[1] = v;
        assert_eq!(w.add_body(BodyId(1), &d, &mut free), Err(Error::NonFinite));
        let mut d = BodyDesc::default();
        d.linvel[2] = v;
        assert_eq!(w.add_body(BodyId(1), &d, &mut free), Err(Error::NonFinite));
        let d = BodyDesc {
            shape: Shape::Hull {
                points: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, v]],
            },
            ..BodyDesc::default()
        };
        assert_eq!(w.add_body(BodyId(1), &d, &mut free), Err(Error::NonFinite));
    }
    let d = BodyDesc {
        rotation: [0.0; 4],
        ..BodyDesc::default()
    };
    assert_eq!(w.add_body(BodyId(1), &d, &mut free), Err(Error::NonFinite));
    assert_eq!(w.body_count(), 0);
    w.add_body(BodyId(1), &BodyDesc::default(), &mut free)
        .unwrap();
    let before = w.state_words();
    assert_eq!(
        w.apply_impulse(BodyId(1), [f32::NAN, 0.0, 0.0]),
        Err(Error::NonFinite)
    );
    let s = BodyState {
        rotation: [0.0, 0.0, f32::NAN, 1.0],
        ..w.body(BodyId(1)).unwrap()
    };
    assert_eq!(w.set_body(BodyId(1), &s), Err(Error::NonFinite));
    assert_eq!(w.state_words(), before);
    // Huge finite values are held, not refused.
    let s = BodyState {
        linvel: [1e30, 0.0, 0.0],
        ..w.body(BodyId(1)).unwrap()
    };
    w.set_body(BodyId(1), &s).unwrap();
    let held = w.body(BodyId(1)).unwrap().linvel[0];
    assert_eq!(held, w.limits().max_speed);
}

#[test]
fn bodies_colliders_and_hull_vertices_are_capped() {
    let limits = Limits {
        max_bodies: 3,
        max_hull_vertices: 16,
        ..Limits::default()
    };
    let mut w = World::new(Config {
        limits,
        ..Config::default()
    });
    for k in 0..3 {
        w.add_body(BodyId(k), &BodyDesc::default(), &mut free)
            .unwrap();
    }
    assert_eq!(
        w.add_body(BodyId(9), &BodyDesc::default(), &mut free),
        Err(Error::Cap(Cap::Bodies))
    );
    assert_eq!(
        w.add_body(BodyId(1), &BodyDesc::default(), &mut free),
        Err(Error::Duplicate(BodyId(1)))
    );
    // A hull of more points than the cap, and a flat one, become their boxes.
    let mut w = World::new(Config {
        limits,
        ..Config::default()
    });
    let mut rng = Rng(3);
    let mut cloud = |n: usize| -> Vec<[f32; 3]> {
        (0..n)
            .map(|_| {
                [
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                ]
            })
            .collect()
    };
    let many = cloud(17);
    let fits = cloud(16);
    let flat = (0..8)
        .map(|k| [k as f32, (k * k) as f32 * 0.1, 0.0])
        .collect();
    for (k, points) in [many, flat, fits].into_iter().enumerate() {
        let d = BodyDesc {
            shape: Shape::Hull { points },
            ..BodyDesc::default()
        };
        w.add_body(BodyId(k as u32), &d, &mut free).unwrap();
    }
    assert_eq!(w.stats().hulls_replaced, 2);
    // The collider cap holds the map's cells too.
    let mut w = World::new(Config {
        limits: Limits {
            max_colliders: 2,
            ..Limits::default()
        },
        ..Config::default()
    });
    w.add_map(
        TERRAIN,
        terrain(|_, _| 0.0, 8, [-1024.0, -1024.0], 0.0, false, Vec::new()),
    )
    .unwrap();
    w.add_body(BodyId(1), &ball([0.0, 0.0, 2.0]), &mut free)
        .unwrap();
    let wide = Box3::new([-200.0, -200.0, 0.0], [200.0, 200.0, 1.0]);
    assert_eq!(
        w.ensure_region(&wide, &mut free),
        Err(Error::Cap(Cap::Colliders))
    );
    assert_eq!(w.collider_count(), 2);
    let r = w.step(&mut free).unwrap();
    assert!(r.map_capped, "{r:?}");
}

#[test]
fn a_step_is_paid_for_before_it_runs() {
    let mut w = World::new(Config::default());
    w.add_map(
        TERRAIN,
        terrain(|_, _| 0.0, 8, [-1024.0, -1024.0], 0.0, false, Vec::new()),
    )
    .unwrap();
    for k in 0..8 {
        w.add_body(BodyId(k), &ball([k as f32 * 2.0, 0.0, 5.0]), &mut free)
            .unwrap();
    }
    w.step(&mut free).unwrap();
    let before = w.state_words();
    let mut spent = 0u64;
    let mut small = |u: u64| {
        spent += u;
        spent <= 200
    };
    assert_eq!(w.step(&mut small), Err(Error::Exhausted));
    assert_eq!(w.state_words(), before, "a refused step moves nothing");
    let mut total = 0u64;
    w.step(&mut |u| {
        total += u;
        true
    })
    .unwrap();
    assert!(total > 200, "{total}");
}

#[test]
fn a_body_that_leaves_the_worlds_reach_is_put_back() {
    let mut w = World::new(Config {
        limits: Limits {
            max_coordinate: 1_000.0,
            ..Limits::default()
        },
        ..Config::default()
    });
    let d = BodyDesc {
        position: [995.0, 0.0, 0.0],
        linvel: [400.0, 0.0, 0.0],
        ..BodyDesc::default()
    };
    w.add_body(BodyId(4), &d, &mut free).unwrap();
    let r = w.step(&mut free).unwrap();
    assert_eq!(r.restored, vec![BodyId(4)]);
    let s = w.body(BodyId(4)).unwrap();
    assert_eq!(s.position[0], 995.0);
    assert_eq!(s.linvel, [0.0; 3]);
}

/// A drop of mixed bodies onto a terrain and an interior, its state words a tick at a time.
fn drop_run(order: &[u32]) -> Vec<u32> {
    let mut w = World::new(Config::default());
    w.add_map(
        TERRAIN,
        terrain(checker, 8, [-1024.0, -1024.0], 0.0, false, Vec::new()),
    )
    .unwrap();
    w.add_map(
        KEEP,
        interior(
            vec![box_hull([-4.0, -4.0, 0.0], [4.0, 4.0, 1.0])],
            [0.0, 0.0, 3.0],
        ),
    )
    .unwrap();
    let mut rng = Rng(11);
    let descs: Vec<(BodyId, BodyDesc)> = (0..16)
        .map(|k| {
            let shape = match k % 3 {
                0 => Shape::Box {
                    half: [
                        rng.range(0.2, 0.8),
                        rng.range(0.2, 0.8),
                        rng.range(0.2, 0.8),
                    ],
                },
                1 => Shape::Ball {
                    radius: rng.range(0.2, 0.8),
                },
                _ => Shape::Hull {
                    points: (0..10)
                        .map(|_| {
                            [
                                rng.range(-0.7, 0.7),
                                rng.range(-0.7, 0.7),
                                rng.range(-0.7, 0.7),
                            ]
                        })
                        .collect(),
                },
            };
            let d = BodyDesc {
                shape,
                position: [
                    rng.range(-6.0, 6.0),
                    rng.range(-6.0, 6.0),
                    rng.range(8.0, 14.0),
                ],
                rotation: [
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                    1.0,
                ],
                ..BodyDesc::default()
            };
            (BodyId(k), d)
        })
        .collect();
    let mut by_order = Vec::new();
    for &k in order {
        by_order.push(descs[k as usize].clone());
    }
    w.add_bodies(by_order, &mut free).unwrap();
    let mut words = Vec::new();
    for _ in 0..120 {
        w.step(&mut free).unwrap();
        words.extend(w.state_words());
    }
    words
}

#[test]
fn two_worlds_given_the_same_calls_hold_the_same_bits() {
    let ids: Vec<u32> = (0..16).collect();
    let mut shuffled = ids.clone();
    shuffled.reverse();
    let a = drop_run(&ids);
    assert_eq!(a, drop_run(&ids));
    // `add_bodies` inserts in id order whatever order it is handed.
    assert_eq!(a, drop_run(&shuffled));
}

#[test]
fn an_impulse_on_a_body_of_next_to_no_mass_keeps_its_speed_in_range() {
    let mut w = World::new(Config::default());
    let d = BodyDesc {
        shape: Shape::Box { half: [1e-9; 3] },
        density: 0.0,
        ..BodyDesc::default()
    };
    w.add_body(BodyId(1), &d, &mut free).unwrap();
    let r = w.apply_impulse(BodyId(1), [1e30, -1e30, 1e30]);
    assert!(matches!(r, Ok(()) | Err(Error::NonFinite)), "{r:?}");
    let s = w.body(BodyId(1)).unwrap();
    let speed = s.linvel.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!(
        speed.is_finite() && speed <= w.limits().max_speed * 1.0001,
        "{s:?}"
    );
    for _ in 0..4 {
        w.step(&mut free).unwrap();
    }
    let s = w.body(BodyId(1)).unwrap();
    assert!(
        s.linvel.iter().chain(&s.angvel).all(|v| v.is_finite()),
        "{s:?}"
    );
}

#[test]
fn a_kinematic_body_reaches_its_target_in_one_tick_and_stops() {
    let mut w = World::new(Config::default());
    let d = BodyDesc {
        motion: torque_physics_rapier::Motion::Kinematic,
        ..BodyDesc::default()
    };
    w.add_body(BodyId(1), &d, &mut free).unwrap();
    let half = std::f32::consts::FRAC_1_SQRT_2;
    // A quarter turn about z and a meter along x.
    w.set_kinematic_target(BodyId(1), [1.0, 0.0, 0.0], [0.0, 0.0, half, half])
        .unwrap();
    w.step(&mut free).unwrap();
    let s = w.body(BodyId(1)).unwrap();
    assert!((s.position[0] - 1.0).abs() < 1e-4, "{s:?}");
    assert!(
        (s.rotation[2] - half).abs() < 1e-3 && (s.rotation[3] - half).abs() < 1e-3,
        "{s:?}"
    );
    assert_eq!(s.linvel, [0.0; 3]);
    w.step(&mut free).unwrap();
    assert!((w.body(BodyId(1)).unwrap().position[0] - 1.0).abs() < 1e-4);
}

fn floor() -> BodyDesc {
    BodyDesc {
        motion: torque_physics_rapier::Motion::Fixed,
        shape: Shape::Box {
            half: [50.0, 50.0, 0.5],
        },
        position: [0.0, 0.0, -0.5],
        ..BodyDesc::default()
    }
}

fn crate_at(at: [f32; 3]) -> BodyDesc {
    BodyDesc {
        shape: Shape::Box { half: [0.5; 3] },
        position: at,
        ..BodyDesc::default()
    }
}

#[test]
fn a_part_adds_its_mass_to_its_body_and_goes_with_it() {
    use torque_physics_rapier::PartDesc;
    let mut w = World::new(Config::default());
    w.add_body(BodyId(1), &crate_at([0.0, 0.0, 5.0]), &mut free)
        .unwrap();
    let alone = w.mass_of(BodyId(1)).unwrap();
    let part = PartDesc::new(
        Shape::Box { half: [0.5; 3] },
        [2.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    );
    w.add_part(BodyId(2), BodyId(1), &part, &mut free).unwrap();
    assert!((w.mass_of(BodyId(1)).unwrap() - 2.0 * alone).abs() < 1e-4);
    assert_eq!(w.body_count(), 1);
    assert_eq!(w.collider_count(), 2);
    assert!(w.body(BodyId(2)).is_none());
    assert_eq!(
        w.add_part(BodyId(2), BodyId(1), &part, &mut free),
        Err(Error::Duplicate(BodyId(2)))
    );
    assert_eq!(
        w.add_part(BodyId(3), BodyId(9), &part, &mut free),
        Err(Error::Unknown(BodyId(9)))
    );
    assert_eq!(
        w.add_body(BodyId(2), &crate_at([0.0; 3]), &mut free),
        Err(Error::Duplicate(BodyId(2)))
    );
    w.remove_part(BodyId(2)).unwrap();
    w.step(&mut free).unwrap();
    assert!((w.mass_of(BodyId(1)).unwrap() - alone).abs() < 1e-4);
    w.add_part(BodyId(2), BodyId(1), &part, &mut free).unwrap();
    w.remove_body(BodyId(1)).unwrap();
    assert_eq!(w.part_ids().count(), 0);
    assert_eq!(w.collider_count(), 0);
}

#[test]
fn a_parts_pose_in_its_bodys_frame_never_changes_under_load() {
    use torque_physics_rapier::PartDesc;
    let mut w = World::new(Config::default());
    w.add_body(BodyId(1), &floor(), &mut free).unwrap();
    w.add_body(BodyId(2), &crate_at([0.0, 0.0, 3.0]), &mut free)
        .unwrap();
    let turn = [0.0, 0.0, 0.3826834, 0.9238795];
    let part = PartDesc::new(Shape::Box { half: [0.4; 3] }, [1.3, 0.2, 0.1], turn);
    w.add_part(BodyId(3), BodyId(2), &part, &mut free).unwrap();
    let first = w.part(BodyId(3)).unwrap();
    let raw = |w: &World| {
        let (_, c) = w
            .rapier()
            .colliders
            .iter()
            .find(|(_, c)| {
                c.position_wrt_parent()
                    .is_some_and(|p| p.translation.x > 1.0)
            })
            .expect("the part's collider");
        let p = *c.position_wrt_parent().unwrap();
        (p.translation.to_array(), p.rotation.to_array())
    };
    let raw_first = raw(&w);
    // Dropped onto the floor, then hit by a stream of balls: the body tumbles and rests, the part never shifts in its frame.
    for tick in 0..600 {
        if tick % 40 == 20 {
            let id = BodyId(100 + tick);
            let mut d = ball([3.0, 0.2, 2.0]);
            d.linvel = [-14.0, 0.0, 2.0];
            d.density = 4.0;
            w.add_body(id, &d, &mut free).unwrap();
        }
        w.step(&mut free).unwrap();
        let now = w.part(BodyId(3)).unwrap();
        assert_eq!(
            now.position.map(f32::to_bits),
            first.position.map(f32::to_bits)
        );
        assert_eq!(
            now.rotation.map(f32::to_bits),
            first.rotation.map(f32::to_bits)
        );
        let r = raw(&w);
        assert_eq!(r.0.map(f32::to_bits), raw_first.0.map(f32::to_bits));
        assert_eq!(r.1.map(f32::to_bits), raw_first.1.map(f32::to_bits));
    }
    let s = w.body(BodyId(2)).unwrap();
    assert!(s.position[2] < 1.5, "the body came down: {s:?}");
}

#[test]
fn a_detached_part_leaves_at_the_speed_its_body_gave_its_point() {
    use torque_physics_rapier::PartDesc;
    let mut w = World::new(Config {
        gravity: [0.0; 3],
        ..Config::default()
    });
    let mut d = crate_at([0.0; 3]);
    d.linvel = [3.0, 0.0, 1.0];
    d.angvel = [0.0, 0.0, 2.0];
    w.add_body(BodyId(1), &d, &mut free).unwrap();
    let part = PartDesc::new(
        Shape::Box { half: [0.25; 3] },
        [2.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    );
    w.add_part(BodyId(2), BodyId(1), &part, &mut free).unwrap();
    for _ in 0..30 {
        w.step(&mut free).unwrap();
    }
    let at = w.part_world(BodyId(2)).unwrap();
    let body = w.body(BodyId(1)).unwrap();
    let expect = w.velocity_at(BodyId(1), at.position).unwrap();
    let out = w.detach_part(BodyId(2)).unwrap();
    let speed = |v: [f32; 3]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((speed(out.linvel) - speed(expect)).abs() <= 0.01 * speed(expect));
    let extra: [f32; 3] = std::array::from_fn(|k| out.linvel[k] - body.linvel[k]);
    assert!(speed(extra) > 1.0, "the spin adds to the speed: {out:?}");
    assert!(w.part(BodyId(2)).is_none());
}

#[test]
fn removing_a_part_wakes_its_sleeping_body() {
    use torque_physics_rapier::PartDesc;
    let mut w = World::new(Config::default());
    w.add_body(BodyId(1), &crate_at([0.0, 0.0, 5.0]), &mut free)
        .unwrap();
    let part = PartDesc::new(
        Shape::Box { half: [0.5; 3] },
        [2.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    );
    w.add_part(BodyId(2), BodyId(1), &part, &mut free).unwrap();
    let mut state = w.body(BodyId(1)).unwrap();
    state.sleeping = true;
    w.set_body(BodyId(1), &state).unwrap();
    assert!(w.body(BodyId(1)).unwrap().sleeping);
    w.remove_part(BodyId(2)).unwrap();
    assert!(
        !w.body(BodyId(1)).unwrap().sleeping,
        "the body that lost mass is awake"
    );
}

#[test]
fn a_bodys_parts_come_and_go_in_proportion_to_their_number() {
    use torque_physics_rapier::PartDesc;
    // `n` parts added, moved and removed: each call costs `log n`, not a look at every part.
    fn run(n: u32) -> std::time::Duration {
        let mut w = World::new(Config {
            limits: Limits {
                max_colliders: 100_000,
                ..Limits::default()
            },
            ..Config::default()
        });
        w.add_body(BodyId(1), &crate_at([0.0, 0.0, 5.0]), &mut free)
            .unwrap();
        let started = std::time::Instant::now();
        for k in 0..n {
            let part = PartDesc::new(
                Shape::Box { half: [0.1; 3] },
                [k as f32 * 0.01, 0.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            );
            w.add_part(BodyId(10 + k), BodyId(1), &part, &mut free)
                .unwrap();
        }
        for k in 0..n {
            w.move_part(
                BodyId(10 + k),
                [0.0, k as f32 * 0.01, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            )
            .unwrap();
        }
        for k in 0..n {
            w.remove_part(BodyId(10 + k)).unwrap();
        }
        assert_eq!(w.parts_on(BodyId(1)).count(), 0);
        started.elapsed()
    }
    let best = |n: u32| (0..3).map(|_| run(n)).min().unwrap();
    let (small, large) = (best(500), best(2000));
    let ratio = large.as_secs_f64() / small.as_secs_f64();
    assert!(
        ratio < 9.0,
        "the time grew {ratio} times for 4 times the parts"
    );
}

/// The eight corners of the box `lo..hi`.
fn box_points(lo: [f32; 3], hi: [f32; 3]) -> Vec<[f32; 3]> {
    (0..8)
        .map(|i| std::array::from_fn(|k| if i & (1 << k) == 0 { lo[k] } else { hi[k] }))
        .collect()
}

/// An L of two bars two units long and half a unit wide and thick: the second beside the first's end.
fn l_shape() -> Shape {
    Shape::Compound {
        hulls: vec![
            box_points([-1.0, -1.0, -0.25], [1.0, -0.5, 0.25]),
            box_points([-1.0, -0.5, -0.25], [-0.5, 1.0, 0.25]),
        ]
        .into(),
    }
}

#[test]
fn a_compound_of_hulls_holds_their_mass_and_collides_where_any_hull_does() {
    let mut w = World::new(Config::default());
    let fixed = BodyDesc {
        motion: Motion::Fixed,
        shape: l_shape(),
        position: [0.0, 0.0, 0.25],
        ..BodyDesc::default()
    };
    w.add_body(BodyId(1), &fixed, &mut free).unwrap();
    assert_eq!(w.stats().hulls_replaced, 0);
    // Two bars: 1.0 and 0.25 units cubed, at a density of 2.
    let volume = l_shape().volume(w.limits(), &mut free).unwrap();
    assert!(
        (volume - (2.0 * 0.5 * 0.5 + 1.5 * 0.5 * 0.5)).abs() < 1e-3,
        "{volume}"
    );
    // A ball dropped into the notch falls to the floor body below it; one over the arm rests on the arm.
    let floor = BodyDesc {
        motion: Motion::Fixed,
        shape: Shape::Box {
            half: [20.0, 20.0, 0.5],
        },
        position: [0.0, 0.0, -0.5],
        ..BodyDesc::default()
    };
    w.add_body(BodyId(2), &floor, &mut free).unwrap();
    w.add_body(BodyId(3), &ball([0.3, 0.3, 2.0]), &mut free)
        .unwrap();
    w.add_body(BodyId(4), &ball([0.0, -0.75, 2.0]), &mut free)
        .unwrap();
    for _ in 0..300 {
        w.step(&mut free).unwrap();
    }
    let notch = w.body(BodyId(3)).unwrap().position[2];
    let arm = w.body(BodyId(4)).unwrap().position[2];
    assert!((notch - 0.5).abs() < 0.02, "in the notch: {notch}");
    assert!((arm - 1.0).abs() < 0.02, "on the arm: {arm}");
}

#[test]
fn a_compound_past_the_hull_limit_or_with_no_hulls_is_the_box_of_its_points() {
    let limits = Limits {
        max_compound_hulls: 3,
        ..Limits::default()
    };
    let mut w = World::new(Config {
        limits,
        ..Config::default()
    });
    let many = Shape::Compound {
        hulls: (0..4)
            .map(|i| box_points([i as f32; 3], [i as f32 + 0.5; 3]))
            .collect::<Vec<_>>()
            .into(),
    };
    let d = |shape| BodyDesc {
        shape,
        ..BodyDesc::default()
    };
    w.add_body(BodyId(1), &d(many), &mut free).unwrap();
    w.add_body(
        BodyId(2),
        &d(Shape::Compound {
            hulls: Vec::new().into(),
        }),
        &mut free,
    )
    .unwrap();
    // Within the limit nothing is replaced.
    w.add_body(
        BodyId(3),
        &d(Shape::Compound {
            hulls: vec![box_points([0.0; 3], [1.0; 3]); 3].into(),
        }),
        &mut free,
    )
    .unwrap();
    assert_eq!(w.stats().hulls_replaced, 2);
}

#[test]
fn a_compound_with_a_point_that_is_not_a_number_is_refused_and_a_flat_hull_in_it_is_its_box() {
    let mut w = World::new(Config::default());
    let mut bad = box_points([0.0; 3], [1.0; 3]);
    bad[3][1] = f32::NAN;
    let d = |hulls: Vec<Vec<[f32; 3]>>| BodyDesc {
        shape: Shape::Compound {
            hulls: hulls.into(),
        },
        ..BodyDesc::default()
    };
    assert_eq!(
        w.add_body(
            BodyId(1),
            &d(vec![box_points([0.0; 3], [1.0; 3]), bad]),
            &mut free
        ),
        Err(Error::NonFinite)
    );
    assert_eq!(w.body_count(), 0);
    // A flat hull among sound ones is replaced by its (thickened) box; the body is still made.
    let flat = box_points([0.0, 0.0, 0.0], [1.0, 1.0, 0.0]);
    w.add_body(
        BodyId(1),
        &d(vec![box_points([2.0; 3], [3.0; 3]), flat]),
        &mut free,
    )
    .unwrap();
    assert_eq!(w.stats().hulls_replaced, 1);
}

/// `n` points on a plane tilted by `tilt` about both horizontal axes, spread over `spread` units.
fn tilted_plane(n: usize, tilt: f32, spread: f32, rng: &mut Rng) -> Vec<[f32; 3]> {
    (0..n)
        .map(|_| {
            let (x, y) = (rng.range(-spread, spread), rng.range(-spread, spread));
            [x, y, tilt * x - 0.6 * tilt * y + 0.25]
        })
        .collect()
}

/// `n` points on a line in a direction that is none of the axes.
fn tilted_line(n: usize, rng: &mut Rng) -> Vec<[f32; 3]> {
    (0..n)
        .map(|_| {
            let t = rng.range(-2.0, 2.0);
            [0.3 * t, -0.7 * t + 1.0, 0.2 * t - 0.5]
        })
        .collect()
}

#[test]
fn points_in_a_tilted_plane_or_on_a_line_become_their_box_and_never_reach_the_hull_builder() {
    let mut rng = Rng(0x5eed);
    let mut w = World::new(Config::default());
    let mut expected = 0;
    let mut id = 1;
    let mut add = |w: &mut World, shape: Shape| {
        let d = BodyDesc {
            shape,
            position: [id as f32 * 5.0, 0.0, 3.0],
            ..BodyDesc::default()
        };
        w.add_body(BodyId(id), &d, &mut free).unwrap();
        id += 1;
    };
    // Tilted planes (thick in every axis-aligned box), nearly flat ones within the thickness, lines and one point repeated; as
    // bodies of one hull and as hulls of a compound.
    for tilt in [0.1, 0.5, 1.0, 3.0] {
        for n in [4, 12, 60, 200] {
            let points = tilted_plane(n, tilt, 1.0, &mut rng);
            add(
                &mut w,
                Shape::Hull {
                    points: points.clone(),
                },
            );
            add(
                &mut w,
                Shape::Compound {
                    hulls: vec![points].into(),
                },
            );
            expected += 2;
        }
    }
    for n in [4, 9, 100] {
        add(
            &mut w,
            Shape::Hull {
                points: tilted_line(n, &mut rng),
            },
        );
        add(
            &mut w,
            Shape::Hull {
                points: vec![[0.3, 0.4, 0.5]; n],
            },
        );
        expected += 2;
    }
    // A plane that is only nearly flat (4 mm thick, under the 1 cm limit) is still a box; one 0.2 units thick is a hull.
    let mut slab = tilted_plane(30, 0.8, 1.0, &mut rng);
    slab[0][2] += 0.004;
    add(&mut w, Shape::Hull { points: slab });
    expected += 1;
    assert_eq!(w.stats().hulls_replaced, expected);
    let mut thick = tilted_plane(30, 0.8, 1.0, &mut rng);
    thick[0][2] += 0.1;
    thick[1][2] -= 0.1;
    add(&mut w, Shape::Hull { points: thick });
    assert_eq!(w.stats().hulls_replaced, expected);
    for _ in 0..30 {
        w.step(&mut free).unwrap();
    }
    assert!(!w.is_poisoned());
}

#[test]
fn hostile_point_sets_of_every_flatness_never_panic() {
    let mut rng = Rng(0xfeed);
    for round in 0..300 {
        let mut w = World::new(Config::default());
        let n = 4 + (rng.below(60) as usize);
        let tilt = rng.range(-3.0, 3.0);
        let mut points = match round % 3 {
            0 => tilted_plane(n, tilt, rng.range(0.01, 2.0), &mut rng),
            1 => tilted_line(n, &mut rng),
            _ => vec![[rng.range(-1.0, 1.0); 3]; n],
        };
        // Some of them nudged by less than the thickness, or by a lot.
        let scale = if round % 7 == 0 { 100.0 } else { 1.0 };
        for p in &mut points {
            if rng.below(5) == 0 {
                p[rng.below(3) as usize] += rng.range(-1e-3, 1e-3) * scale;
            }
        }
        let d = BodyDesc {
            shape: Shape::Compound {
                hulls: vec![points].into(),
            },
            position: [0.0, 0.0, 2.0],
            ..BodyDesc::default()
        };
        w.add_body(BodyId(1), &d, &mut free).unwrap();
        for _ in 0..3 {
            w.step(&mut free).unwrap();
        }
        assert!(!w.is_poisoned(), "round {round}");
    }
}

/// A compound of `n` small boxes in a row, `step` apart.
fn row_of_boxes(n: usize, step: f32) -> Shape {
    Shape::Compound {
        hulls: (0..n)
            .map(|i| {
                let x = i as f32 * step;
                box_points([x, -0.2, -0.2], [x + 0.4, 0.2, 0.2])
            })
            .collect::<Vec<_>>()
            .into(),
    }
}

/// A world of `count` bodies of `shape` piled on one another in the air (every pair touches), stepped once so the narrow phase
/// holds their pairs.
fn pile(count: u32, shape: &Shape) -> World {
    let mut w = World::new(Config::default());
    for k in 0..count {
        let d = BodyDesc {
            shape: shape.clone(),
            position: [0.05 * k as f32, 0.03 * k as f32, 50.0],
            ..BodyDesc::default()
        };
        w.add_body(BodyId(k + 1), &d, &mut free).unwrap();
    }
    w.step(&mut free).unwrap();
    w
}

/// What the world's next step charges.
fn charged(w: &mut World) -> u64 {
    let mut total = 0;
    let mut count = |u: u64| {
        total += u;
        true
    };
    w.step(&mut count).unwrap();
    total
}

#[test]
fn a_pile_of_compounds_is_charged_by_the_hulls_of_every_pair_and_refused_where_boxes_fit() {
    let boxes = Shape::Box { half: [0.4; 3] };
    let compounds = row_of_boxes(16, 0.1);
    let mut plain = pile(10, &boxes);
    let mut heavy = pile(10, &compounds);
    let (cheap, dear) = (charged(&mut plain), charged(&mut heavy));
    // 45 pairs of 16 hulls each side test 256 hull pairs a pair: far more than a pair of boxes.
    assert!(dear > 40 * cheap, "{dear} against {cheap}");
    // A budget the boxes fit with room to spare refuses the compounds, and a refused step moves nothing.
    let budget = cheap * 4;
    let mut plain = pile(10, &boxes);
    let mut heavy = pile(10, &compounds);
    let before = heavy.state_words();
    let within = |spent: &mut u64, u: u64| {
        *spent += u;
        *spent <= budget
    };
    let (mut a, mut b) = (0, 0);
    assert!(plain.step(&mut |u| within(&mut a, u)).is_ok());
    assert_eq!(
        heavy.step(&mut |u| within(&mut b, u)).err(),
        Some(Error::Exhausted)
    );
    assert_eq!(heavy.state_words(), before);
    // The charge follows the hulls: half as many hulls a side is about a quarter of the pair work.
    let mut half = pile(10, &row_of_boxes(8, 0.1));
    let mid = charged(&mut half);
    assert!(mid < dear / 2 && mid > cheap, "{cheap} < {mid} < {dear}");
}

#[test]
fn a_compound_is_charged_for_each_hull_when_it_is_made() {
    let make = |n: usize| {
        let mut w = World::new(Config::default());
        let mut total = 0;
        let mut count = |u: u64| {
            total += u;
            true
        };
        let d = BodyDesc {
            shape: row_of_boxes(n, 0.5),
            ..BodyDesc::default()
        };
        w.add_body(BodyId(1), &d, &mut count).unwrap();
        total
    };
    let (one, thirty_two) = (make(1), make(32));
    assert!(thirty_two > 20 * one / 2, "{one} and {thirty_two}");
    // Each hull is at least the per-hull charge and its eight points.
    assert!(thirty_two - one >= 31 * (work::HULL + 8 * work::VERTEX));
}

#[test]
fn the_hulls_of_compounds_count_against_the_colliders_cap_and_leave_with_their_body() {
    let limits = Limits {
        max_colliders: 100,
        ..Limits::default()
    };
    let mut w = World::new(Config {
        limits,
        ..Config::default()
    });
    let big = |k: u32| BodyDesc {
        shape: row_of_boxes(32, 0.5),
        position: [k as f32 * 40.0, 0.0, 5.0],
        ..BodyDesc::default()
    };
    // Three of 32 hulls fill 96 of the 100; the fourth does not fit, though only three colliders are in the world.
    for k in 0..3 {
        w.add_body(BodyId(k + 1), &big(k), &mut free).unwrap();
    }
    assert_eq!(w.collider_count(), 3);
    assert_eq!(
        w.add_body(BodyId(4), &big(3), &mut free),
        Err(Error::Cap(Cap::Colliders))
    );
    // A box still fits; one more compound of 32 does not, and of 3 hulls does.
    w.add_body(BodyId(5), &ball([0.0, 100.0, 5.0]), &mut free)
        .unwrap();
    let small = BodyDesc {
        shape: row_of_boxes(3, 0.5),
        ..big(7)
    };
    assert_eq!(
        w.add_body(BodyId(6), &big(6), &mut free),
        Err(Error::Cap(Cap::Colliders))
    );
    w.add_body(BodyId(7), &small, &mut free).unwrap();
    // A removed body gives its hulls back.
    w.remove_body(BodyId(1)).unwrap();
    w.add_body(BodyId(8), &big(8), &mut free).unwrap();
}
