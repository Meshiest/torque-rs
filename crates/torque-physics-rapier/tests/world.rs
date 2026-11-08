//! The world's rules: map pieces made as the engine's collision holds them, built by cell on first use; bodies checked, held
//! and capped; the budget paid before work; two worlds given the same calls the same bits.

mod common;

use common::{Rng, box_hull, free, interior, terrain};
use rapier3d::prelude::{QueryFilter, Ray, Vector};
use torque_collide::ObjectId;
use torque_collide::map::MapShape;
use torque_collide::math::Box3;
use torque_physics_rapier::world::Cap;
use torque_physics_rapier::{BodyDesc, BodyId, BodyState, Config, Error, Limits, Shape, World};

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
