//! Hostile hulls, terrain, interiors, transforms and limits: no call panics or poisons the world, and every count stays within
//! its cap. `TORQUE_FUZZ_RAPIER=<n>` sets the iterations (default 2,000).

mod common;

use std::rc::Rc;

use common::{Rng, box_hull, interior, terrain};
use torque_collide::ObjectId;
use torque_collide::map::MapShape;
use torque_collide::math::Box3;
use torque_mapcore::budget::HEIGHTFIELD_SAMPLES;
use torque_physics_rapier::{
    BodyDesc, BodyId, BodyState, Config, Error, Limits, Motion, Shape, World,
};

/// Floats a hostile input picks from, beside plain ones.
const NASTY: [f32; 16] = [
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    0.0,
    -0.0,
    1e-30,
    f32::MIN_POSITIVE,
    1e-7,
    1e30,
    -1e30,
    f32::MAX,
    f32::MIN,
    1e5,
    -1e5,
    1e7,
    4096.0,
];

fn float(rng: &mut Rng, scale: f32) -> f32 {
    if rng.below(6) == 0 {
        NASTY[rng.below(NASTY.len() as u64) as usize]
    } else {
        rng.range(-scale, scale)
    }
}

fn vec3(rng: &mut Rng, scale: f32) -> [f32; 3] {
    [float(rng, scale), float(rng, scale), float(rng, scale)]
}

fn quat(rng: &mut Rng) -> [f32; 4] {
    [
        float(rng, 1.0),
        float(rng, 1.0),
        float(rng, 1.0),
        float(rng, 1.0),
    ]
}

/// Hull points: random clouds, flat, on a line, repeated, too many, too few, or with hostile values.
fn points(rng: &mut Rng, cap: usize) -> Vec<[f32; 3]> {
    let n = rng.below(2 * cap as u64 + 3) as usize;
    match rng.below(5) {
        0 => (0..n)
            .map(|_| [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), 0.0])
            .collect(),
        1 => (0..n)
            .map(|k| [k as f32 * 1e-3, k as f32 * 2e-3, k as f32 * 3e-3])
            .collect(),
        2 => vec![[rng.range(-1.0, 1.0); 3]; n],
        3 => (0..n).map(|_| vec3(rng, 2.0)).collect(),
        _ => (0..n)
            .map(|_| {
                [
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                ]
            })
            .collect(),
    }
}

fn shape(rng: &mut Rng, cap: usize) -> Shape {
    match rng.below(4) {
        0 => Shape::Box {
            half: vec3(rng, 2.0),
        },
        1 => Shape::Ball {
            radius: float(rng, 2.0),
        },
        _ => Shape::Hull {
            points: points(rng, cap),
        },
    }
}

fn desc(rng: &mut Rng, cap: usize) -> BodyDesc {
    let motion = match rng.below(6) {
        0 => Motion::Fixed,
        1 => Motion::Kinematic,
        _ => Motion::Dynamic,
    };
    let near = rng.below(3) != 0;
    let position = if near {
        [
            rng.range(-30.0, 30.0),
            rng.range(-30.0, 30.0),
            rng.range(-5.0, 30.0),
        ]
    } else {
        vec3(rng, 1e4)
    };
    BodyDesc {
        motion,
        shape: shape(rng, cap),
        position,
        rotation: quat(rng),
        linvel: vec3(rng, 50.0),
        angvel: vec3(rng, 50.0),
        density: float(rng, 10.0),
        friction: float(rng, 2.0),
        restitution: float(rng, 2.0),
        linear_damping: float(rng, 2.0),
        angular_damping: float(rng, 2.0),
        ccd: rng.below(2) == 0,
    }
}

fn limits(rng: &mut Rng) -> Limits {
    if rng.below(4) == 0 {
        return Limits::default();
    }
    let d = Limits::default();
    let pick = |rng: &mut Rng, v: f32| if rng.below(8) == 0 { float(rng, v) } else { v };
    Limits {
        max_bodies: rng.below(10) as usize,
        max_colliders: rng.below(80) as usize,
        max_hull_vertices: rng.below(70) as usize,
        max_contacts: rng.below(80) as usize,
        max_cells_per_query: rng.below(20) as usize,
        max_coordinate: pick(rng, d.max_coordinate),
        max_speed: pick(rng, d.max_speed),
        max_angular_speed: pick(rng, d.max_angular_speed),
        max_half_extent: pick(rng, d.max_half_extent),
        min_half_extent: pick(rng, d.min_half_extent),
        max_density: pick(rng, d.max_density),
        min_density: pick(rng, d.min_density),
        max_impulse: pick(rng, d.max_impulse),
    }
}

/// A terrain of hostile heights, square size, origin and holes.
fn hostile_terrain(rng: &mut Rng) -> Rc<MapShape> {
    let kind = rng.below(4);
    let seed = rng.next();
    let heights = move |i: usize, j: usize| {
        let mut r = Rng(seed ^ (i as u64 * 0x9e37_79b9) ^ ((j as u64) << 32) | 1);
        match kind {
            0 => r.range(0.0, 2048.0),
            1 => float(&mut r, 1e4),
            2 => {
                if (i + j).is_multiple_of(7) {
                    1e30
                } else {
                    0.0
                }
            }
            _ => ((i * 31 + j * 17) % 64) as f32,
        }
    };
    let square = match rng.below(4) {
        0 => rng.below(4096) as i32 + 1,
        1 => -(rng.below(100) as i32),
        2 => 1,
        _ => 8,
    };
    let origin = if rng.below(4) == 0 {
        [float(rng, 1e5), float(rng, 1e5)]
    } else {
        [rng.range(-1100.0, 0.0), rng.range(-1100.0, 0.0)]
    };
    let holes = (0..HEIGHTFIELD_SAMPLES)
        .map(|_| rng.below(5) == 0)
        .collect();
    terrain(
        heights,
        square,
        origin,
        float(rng, 100.0),
        rng.below(2) == 0,
        holes,
    )
}

/// An interior of hostile hulls (boxes of hostile corners and clouds), possibly placed far off.
fn hostile_interior(rng: &mut Rng) -> Rc<MapShape> {
    let n = rng.below(24) as usize + 1;
    let hulls = (0..n)
        .map(|_| {
            let lo = vec3(rng, 40.0);
            let hi = [0, 1, 2].map(|k| lo[k] + float(rng, 8.0).abs());
            let mut h = box_hull(lo, hi);
            if rng.below(4) == 0 {
                for c in &mut h.corners {
                    *c = vec3(rng, 40.0).map(f64::from);
                }
            }
            h
        })
        .collect();
    let at = if rng.below(4) == 0 {
        vec3(rng, 1e5)
    } else {
        vec3(rng, 20.0)
    };
    let at = at.map(|v| if v.is_finite() { v } else { 0.0 });
    interior(hulls, at)
}

#[test]
fn hostile_input_never_panics_and_every_count_stays_within_its_cap() {
    let iterations: u64 = std::env::var("TORQUE_FUZZ_RAPIER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);
    let mut rng = Rng(0x5eed_cafe_f00d_d00d);
    // A few terrains made once (a block is 65,536 samples); a new hostile one now and then.
    let mut terrains: Vec<Rc<MapShape>> = (0..6).map(|_| hostile_terrain(&mut rng)).collect();
    let mut steps = 0u64;
    let mut refused = 0u64;
    for it in 0..iterations {
        if it % 64 == 63 {
            let k = rng.below(terrains.len() as u64) as usize;
            terrains[k] = hostile_terrain(&mut rng);
        }
        let config = Config {
            substeps: rng.below(12) as u8,
            gravity: vec3(&mut rng, 30.0),
            solver_iterations: rng.below(20) as u8,
            sleeping: rng.below(2) == 0,
            limits: limits(&mut rng),
        };
        let mut w = World::new(config);
        let l = *w.limits();
        // Every call is charged, as in the engine (whose tick budget is 50 M units); a few iterations get almost nothing.
        let mut budget = if rng.below(4) == 0 {
            rng.below(5_000)
        } else {
            rng.below(2_000_000)
        };
        let mut charge = move |u: u64| {
            if u > budget {
                return false;
            }
            budget -= u;
            true
        };
        let mut check = |r: Result<(), Error>| match r {
            Err(Error::Panicked | Error::Poisoned) => panic!("iteration {it}: {r:?}"),
            Err(_) => refused += 1,
            Ok(()) => {}
        };
        let maps = rng.below(3);
        for m in 0..maps {
            let shape = if rng.below(2) == 0 {
                Rc::clone(&terrains[rng.below(terrains.len() as u64) as usize])
            } else {
                hostile_interior(&mut rng)
            };
            check(w.add_map(ObjectId(m as u32), shape));
        }
        let bodies = rng.below(8) as u32;
        for k in 0..bodies {
            let d = desc(&mut rng, l.max_hull_vertices);
            check(w.add_body(BodyId(k), &d, &mut charge));
        }
        if rng.below(3) == 0 {
            let region = Box3::new(vec3(&mut rng, 1e4), vec3(&mut rng, 1e4));
            check(w.ensure_region(&region, &mut charge).map(|_| ()));
        }
        for _ in 0..rng.below(4) {
            let id = BodyId(rng.below(u64::from(bodies) + 1) as u32);
            match rng.below(3) {
                0 => check(w.apply_impulse(id, vec3(&mut rng, 1e4))),
                1 => {
                    let s = BodyState {
                        position: vec3(&mut rng, 40.0),
                        rotation: quat(&mut rng),
                        linvel: vec3(&mut rng, 1e3),
                        angvel: vec3(&mut rng, 1e3),
                        sleeping: rng.below(2) == 0,
                    };
                    check(w.set_body(id, &s));
                }
                _ => check(w.set_kinematic_target(id, vec3(&mut rng, 40.0), quat(&mut rng))),
            }
            let stepped = w.step(&mut charge);
            if let Ok(r) = &stepped {
                steps += 1;
                assert!(
                    r.contacts <= l.max_contacts || r.contacts_capped,
                    "iteration {it}: {r:?}"
                );
                assert!(r.substeps <= w.config().substeps, "iteration {it}");
            }
            check(stepped.map(|_| ()));
        }
        assert!(!w.is_poisoned(), "iteration {it}");
        assert!(w.body_count() <= l.max_bodies, "iteration {it}");
        assert!(w.collider_count() <= l.max_colliders, "iteration {it}");
        for id in w.ids().collect::<Vec<_>>() {
            let s = w.body(id).expect("a listed body");
            let words = [s.position.as_slice(), &s.rotation, &s.linvel, &s.angvel].concat();
            assert!(words.iter().all(|v| v.is_finite()), "iteration {it}: {s:?}");
            assert!(
                s.position.iter().all(|v| v.abs() <= l.max_coordinate),
                "iteration {it}: {s:?}"
            );
        }
        // Every made shape's vertices are within the cap.
        for (_, c) in w.rapier().colliders.iter() {
            if let Some(h) = c.shape().as_convex_polyhedron() {
                assert!(
                    h.points().len() <= l.max_hull_vertices.max(8),
                    "iteration {it}"
                );
            }
        }
    }
    println!("{iterations} iterations, {steps} steps, {refused} calls refused");
    assert!(
        steps > iterations / 4,
        "{steps} steps over {iterations} iterations"
    );
}
