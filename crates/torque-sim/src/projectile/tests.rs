//! The Projectile's step on small synthetic worlds, after TGE's `Projectile::processTick`: straight and ballistic flight, the
//! explosion past the arming delay (the callbacks in order, the delete 500 ms later), the bounce of a ballistic shot before it,
//! the pass-through of any other, the lifetime, the source's exemption, the validators, determinism and a hostile fuzz.
//! Expected values are written out with the float roundings of the `Point3F` operators.

use torque_collide::math::{Box3, F3};
use torque_collide::{ObjectId, TypeMasks};

use super::driver::run;
use super::*;
use crate::testworld::{Ev, Obj, PLAYER, SOLID, TestHost, floor, solid};

const ID: ObjectId = ObjectId(700);
const FLOOR: ObjectId = ObjectId(1);

fn at(pos: F3, vel: F3) -> Projectile {
    let mut s = ProjectileState::new(ID, Box3::new([-0.1; 3], [0.1; 3]));
    s.position = pos;
    s.velocity = vel;
    let mut p = Projectile::new(s);
    p.on_add();
    p
}

fn ballistic() -> ProjectileData {
    ProjectileData {
        is_ballistic: true,
        lifetime: 100,
        ..ProjectileData::default()
    }
}

fn world() -> TestHost {
    TestHost::new(vec![floor(1)])
}

fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `p + v * 0.032` with each operator rounded to a float.
fn step(p: F3, v: F3) -> F3 {
    let t = 0.032f32;
    [0, 1, 2].map(|i| (d(p[i]) + d((d(v[i]) * d(t)) as f32)) as f32)
}

fn player_box(id: u32, min: F3, max: F3) -> Obj {
    Obj {
        id: ObjectId(id),
        min,
        max,
        type_mask: PLAYER,
        convex: false,
        vel: [0.0; 3],
        disabled: 0,
    }
}

#[test]
fn a_shot_moves_by_its_velocity_each_tick() {
    let db = ProjectileData::default();
    let mut p = at([0.0, 0.0, 10.0], [10.0, 0.0, 0.0]);
    let mut h = world();
    p.process_tick(&db, &mut h);
    let end = step([0.0, 0.0, 10.0], [10.0, 0.0, 0.0]);
    assert_eq!(
        h.rays,
        vec![([0.0, 0.0, 10.0], end, TypeMasks::TGE.projectile_ray_players)]
    );
    let s = &p.s;
    assert_eq!(s.position, end);
    assert_eq!(s.age, 1);
    assert_eq!(s.render_base, end);
    assert_eq!(s.render_delta, [(0.0 - d(end[0])) as f32, 0.0, 0.0]);
    assert_eq!(s.scene.position(), end);
    assert_eq!(s.velocity, [10.0, 0.0, 0.0]);
    assert_eq!(s.mask_bits, 0);
    assert_eq!(h.moved, 1);
    assert!(h.events.is_empty());
}

#[test]
fn gravity_pulls_a_ballistic_shot_before_it_moves() {
    let db = ProjectileData {
        gravity_mod: 0.5,
        ..ballistic()
    };
    let mut p = at([0.0, 0.0, 10.0], [0.0, 0.0, 0.0]);
    let mut h = world();
    p.process_tick(&db, &mut h);
    let vz = (0.0 - (9.81 * 0.5) * d(0.032f32)) as f32;
    assert_eq!(p.s.velocity, [0.0, 0.0, vz]);
    assert_eq!(p.s.position, step([0.0, 0.0, 10.0], [0.0, 0.0, vz]));
}

/// A shot down at the floor past the arming delay: it stops at the hit point with no velocity, `onCollision` sees that, then
/// `onExplode` gets the point moved 0.01 out along the normal, and the delete is 500 ms later. A static hit sends no bounce bit;
/// the stored position is the step's end.
#[test]
fn a_hit_past_the_arming_delay_explodes_at_the_hit_point() {
    let db = ProjectileData::default();
    let mut p = at([0.0, 0.0, 1.0], [0.0, 0.0, -50.0]);
    let mut h = world();
    h.time = 5000;
    p.process_tick(&db, &mut h);
    let end = step([0.0, 0.0, 1.0], [0.0, 0.0, -50.0]);
    let hit = h.cast([0.0, 0.0, 1.0], end, TypeMasks::TGE.projectile_ray_players);
    let point = hit.expect("the floor").point;
    let out = (d(point[2]) + d((1.0 * d(0.01f32)) as f32)) as f32;
    let pos = [point[0], point[1], out];
    assert_eq!(
        h.events,
        vec![
            Ev::Collision {
                col: FLOOR,
                point,
                normal: [0.0, 0.0, 1.0],
                vel: [0.0; 3],
                fade: 1.0,
            },
            Ev::Explode {
                pos,
                fade: 1.0,
                time: 5000,
            },
            Ev::ScheduleDelete { id: ID, at: 5500 },
        ]
    );
    let s = &p.s;
    assert!(s.exploded);
    assert_eq!(s.explosion_pos, pos);
    assert_eq!(s.explosion_normal, [0.0, 0.0, 1.0]);
    assert_eq!(s.explosion_type, SOLID);
    assert_eq!(s.velocity, [0.0; 3]);
    assert_eq!(s.mask_bits, net_mask::EXPLOSION);
    assert_eq!(s.position, end);
    assert_eq!(s.render_delta, [0.0, 0.0, (d(point[2]) - d(end[2])) as f32]);
    // an exploded projectile is no longer simulated
    let before = s.clone();
    p.process_tick(&db, &mut h);
    assert_eq!(p.s.position, before.position);
    assert_eq!(p.s.age, 2);
    assert_eq!(h.rays.len(), 1);
}

#[test]
fn a_hit_on_anything_but_static_world_sets_the_bounce_bit() {
    let db = ProjectileData::default();
    let mut p = at([0.0, 0.0, 3.0], [0.0, 0.0, -50.0]);
    let mut h = TestHost::new(vec![floor(1), player_box(9, [-1.0; 3], [1.0, 1.0, 2.0])]);
    p.process_tick(&db, &mut h);
    assert_eq!(p.s.explosion_type, PLAYER);
    assert_eq!(p.s.mask_bits, net_mask::BOUNCE | net_mask::EXPLOSION);
}

/// A ballistic shot that hits before its arming delay is reflected about the normal, loses `bounceFriction` of its part along
/// the surface, is scaled by `bounceElasticity`, and ends the tick 0.05 off the surface.
#[test]
fn a_ballistic_hit_before_the_arming_delay_bounces() {
    let db = ProjectileData {
        arming_delay: 10,
        bounce_friction: 0.25,
        bounce_elasticity: 0.5,
        gravity_mod: 0.0,
        ..ballistic()
    };
    let v0 = [20.0f32, 0.0, -40.0];
    let mut p = at([0.0, 0.0, 0.5], v0);
    let mut h = world();
    p.process_tick(&db, &mut h);
    // the normal is +z: the reflection flips z, the tangent is the x part
    let r = [20.0f32, 0.0, 40.0];
    let tx = (d(r[0]) - d((d(r[0]) * d(0.25f32)) as f32)) as f32;
    let v = [(d(tx) * 0.5) as f32, 0.0, 20.0];
    assert_eq!(p.s.velocity, v);
    let end = step([0.0, 0.0, 0.5], v0);
    let point = h
        .cast([0.0, 0.0, 0.5], end, TypeMasks::TGE.projectile_ray_players)
        .expect("the floor")
        .point;
    let out = [point[0], point[1], (d(point[2]) + d(0.05f32)) as f32];
    assert_eq!(p.s.position, out);
    assert!(!p.s.exploded);
    assert_eq!(p.s.mask_bits, 0);
    assert!(h.events.is_empty());
}

#[test]
fn a_non_ballistic_hit_before_the_arming_delay_passes_through() {
    let db = ProjectileData {
        arming_delay: 10,
        ..ProjectileData::default()
    };
    let mut p = at([0.0, 0.0, 1.0], [0.0, 0.0, -50.0]);
    let mut h = world();
    p.process_tick(&db, &mut h);
    assert_eq!(p.s.position, step([0.0, 0.0, 1.0], [0.0, 0.0, -50.0]));
    assert_eq!(p.s.velocity, [0.0, 0.0, -50.0]);
    assert!(!p.s.exploded);
    assert!(h.events.is_empty());
}

#[test]
fn the_lifetime_deletes_the_projectile() {
    let db = ProjectileData {
        lifetime: 3,
        ..ProjectileData::default()
    };
    let mut p = at([0.0, 0.0, 50.0], [1.0, 0.0, 0.0]);
    let mut h = world();
    for _ in 0..3 {
        p.process_tick(&db, &mut h);
    }
    assert_eq!(h.events, vec![Ev::Delete(ID)]);
    assert_eq!(h.rays.len(), 2);
}

#[test]
fn the_source_is_exempt_for_seven_ticks() {
    let db = ProjectileData::default();
    let mut p = at([0.0, 0.0, 50.0], [1.0, 0.0, 0.0]);
    p.s.source = Some(ObjectId(5));
    p.s.source_id = 5;
    let mut h = world();
    for _ in 0..8 {
        p.process_tick(&db, &mut h);
    }
    let switches = h
        .events
        .iter()
        .filter(|e| matches!(e, Ev::Source { .. }))
        .count();
    assert_eq!(switches, 14);
    assert_eq!(p.s.source, None);
    assert_eq!(p.s.source_id, 0);
}

#[test]
fn the_validators_clamp_to_tges_ranges() {
    let mut db = ProjectileData::default();
    assert!(db.set_lifetime_ms(1000));
    assert_eq!(db.lifetime, 31);
    assert!(!db.set_lifetime_ms(200_000));
    assert_eq!(db.lifetime, 4095);
    assert!(!db.set_arming_delay_ms(-64));
    assert_eq!(db.arming_delay, 0);
    assert!(db.set_fade_delay_ms(-31));
    assert_eq!(db.fade_delay, 0);
    assert_eq!(
        validate::range(20000.0, validate::MUZZLE_VELOCITY),
        (10000.0, false)
    );
    assert_eq!(
        validate::range(-1.0, validate::BOUNCE_FRICTION),
        (0.0, false)
    );
    assert_eq!(validate::range(0.5, validate::GRAVITY_MOD), (0.5, true));
    assert!(validate::range(f32::NAN, validate::VEL_INHERIT).1);
    let d = ProjectileData::default();
    assert_eq!((d.lifetime, d.fade_delay, d.arming_delay), (625, 625, 0));
    assert_eq!(d.bounce_elasticity, 0.999);
    assert_eq!(d.bounce_friction, 0.3);
}

#[test]
fn replay_is_deterministic() {
    let db = ProjectileData {
        arming_delay: 100,
        ..ballistic()
    };
    let start = at([0.0, 0.0, 1.0], [3.0, 0.0, -50.0]);
    let a = run(&start, &db, 60, &mut world());
    let b = run(&start, &db, 60, &mut world());
    assert_eq!(a, b);
    assert!(a.iter().any(|t| t.velocity[2] > 0.0), "it bounced");
    assert_eq!(a[0].bits().len(), 15);
}

/// A tiny xorshift for the fuzz (no clock, no OS randomness: the run is the same every time).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick(&mut self) -> f32 {
        const SPECIAL: [f32; 13] = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            1.0e30,
            -1.0e30,
            1.0e-30,
            3.4e38,
            -20.0,
        ];
        let r = self.next();
        if r.is_multiple_of(3) {
            SPECIAL[(r >> 8) as usize % SPECIAL.len()]
        } else {
            ((r >> 11) as f64 / (1u64 << 53) as f64 * 200.0 - 100.0) as f32
        }
    }
    fn v3(&mut self) -> F3 {
        [self.pick(), self.pick(), self.pick()]
    }
    /// A sane value in `[-range, range]`, hostile with probability `1/den` (`den == 1`: always the hostile mix).
    fn mix(&mut self, range: f32, den: u64) -> f32 {
        if den == 1 || self.next().is_multiple_of(den) {
            self.pick()
        } else {
            ((self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * range
        }
    }
    fn vec3(&mut self, range: f32, den: u64) -> F3 {
        [
            self.mix(range, den),
            self.mix(range, den),
            self.mix(range, den),
        ]
    }
}

#[test]
fn hostile_input_never_panics() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let cases: u32 = std::env::var("FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(800);
    let (mut collisions, mut exploded) = (0usize, 0usize);
    for case in 0..cases {
        // odd cases are all hostile; even cases are a sane scene with a hostile value here and there
        let den = if case % 2 == 1 { 1 } else { 12 };
        let db = ProjectileData {
            vel_inherit_factor: rng.mix(1.0, den),
            muzzle_velocity: rng.mix(200.0, den),
            is_ballistic: case % 4 < 2,
            bounce_elasticity: rng.mix(1.0, den),
            bounce_friction: rng.mix(1.0, den),
            gravity_mod: rng.mix(1.0, den),
            lifetime: if case % 5 == 0 { rng.next() as u32 } else { 40 },
            arming_delay: if case % 6 == 0 { rng.next() as u32 } else { 3 },
            fade_delay: 0,
        };
        let b = if den == 1 {
            Box3::new(rng.v3(), rng.v3())
        } else {
            Box3::new([-0.1; 3], [0.1; 3])
        };
        let mut s = ProjectileState::new(ID, b);
        s.position = rng.vec3(3.0, den);
        s.position[2] = if den == 1 {
            s.position[2]
        } else {
            s.position[2].abs() + 0.3
        };
        s.velocity = rng.vec3(80.0, den);
        if case % 4 == 0 {
            s.source = Some(ObjectId(5));
        }
        let mut p = Projectile::new(s);
        p.on_add();
        let objs = vec![
            floor(1),
            solid(2, rng.vec3(10.0, den), rng.vec3(10.0, den)),
            player_box(5, rng.vec3(5.0, den), rng.vec3(5.0, den)),
            player_box(6, [-1.0, -1.0, 0.1], [1.0, 1.0, 0.4]),
        ];
        let mut h = TestHost::new(objs);
        for _ in 0..60 {
            h.time = h.time.wrapping_add(32);
            p.process_tick(&db, &mut h);
        }
        collisions += h
            .events
            .iter()
            .filter(|e| matches!(e, Ev::Collision { .. }))
            .count();
        exploded += usize::from(p.s.exploded);
    }
    // the fuzz must reach the hit paths, not only skip the NaN ones
    assert!(collisions > cases as usize / 20, "{collisions} collisions");
    assert!(exploded > cases as usize / 20, "{exploded} explosions");
}
