//! The Item's step on small synthetic worlds: chains worked by hand, with the expected bit patterns computed independently
//! (`f64` arithmetic with the float roundings written out), the rest/wake/undo state machine, the callbacks, determinism, and a
//! hostile fuzz.

use torque_collide::math::{Box3, F3, IDENTITY, set_euler};
use torque_collide::{ObjectId, TypeMasks};

use super::driver::{ItemAction, run};
use super::*;
use crate::scene::DT;
use crate::testworld::{Obj, PLAYER, TestHost, floor, solid};

fn item_box() -> Box3 {
    Box3::new([-0.5, -0.5, 0.0], [0.5, 0.5, 1.0])
}

fn at(p: F3) -> Matrix {
    let mut m = IDENTITY;
    m[3] = p[0];
    m[7] = p[1];
    m[11] = p[2];
    m
}

/// An awake item at `p`.
fn awake(db: &ItemData, p: F3) -> Item {
    let mut s = ItemState::new(ObjectId(900), db, item_box(), at(p), &TypeMasks::TGE);
    s.at_rest = false;
    Item::new(s)
}

fn bits(v: F3) -> [u32; 3] {
    [v[0].to_bits(), v[1].to_bits(), v[2].to_bits()]
}

fn host_with(objs: Vec<Obj>) -> TestHost {
    TestHost::new(objs)
}

#[test]
fn a_new_item_sleeps_and_a_wake_makes_it_fall() {
    let db = ItemData::default();
    let mut it = Item::new(ItemState::new(
        ObjectId(900),
        &db,
        item_box(),
        at([0.0, 0.0, 5.0]),
        &TypeMasks::TGE,
    ));
    let mut h = host_with(vec![floor(1)]);
    assert!(it.s.at_rest);
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(it.s.position(), [0.0, 0.0, 5.0]);
    assert_eq!(it.s.rest_ticks, 1);
    // setTransform wakes a non-static item (and sets the rotation, position and warp bits).
    let m = it.s.scene.obj_to_world;
    it.s.mask_bits = 0;
    it.set_transform(&m, &mut h);
    assert!(!it.s.at_rest);
    assert_eq!(it.s.mask_bits, 0x0c00_0040);
    assert_eq!(it.s.rest_ticks, 0);
    it.process_tick(&db, &mut h).unwrap();
    assert!(it.s.position()[2] < 5.0);
}

#[test]
fn a_static_item_never_simulates_and_a_sticky_one_never_wakes() {
    let mut db = ItemData::default();
    let mut s = ItemState::new(
        ObjectId(900),
        &db,
        item_box(),
        at([0.0, 0.0, 5.0]),
        &TypeMasks::TGE,
    );
    s.is_static = true;
    s.at_rest = false;
    let mut it = Item::new(s);
    let mut h = host_with(vec![floor(1)]);
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(it.s.position(), [0.0, 0.0, 5.0]);
    assert_eq!(it.s.velocity, [0.0; 3]);
    // sticky: the at-rest counter does not run
    db.sticky = true;
    let mut it = Item::new(ItemState::new(
        ObjectId(900),
        &db,
        item_box(),
        at([0.0, 0.0, 5.0]),
        &TypeMasks::TGE,
    ));
    for _ in 0..200 {
        it.process_tick(&db, &mut h).unwrap();
    }
    assert!(it.s.at_rest);
    assert_eq!(it.s.rest_ticks, 0);
    assert_eq!(it.s.position(), [0.0, 0.0, 5.0]);
}

/// An item 0.01 above a floor, awake: gravity and drag, the first ray hits the floor top, the response leaves 0.002 of rebound
/// speed, the free sweep moves it, and the speed under 0.15 on solid world stops it.
#[test]
fn an_item_falls_onto_a_box_and_comes_to_rest() {
    let db = ItemData::default();
    let p = [0.0, 0.0, 0.01f32];
    let mut it = awake(&db, p);
    let mut h = host_with(vec![floor(1)]);
    it.process_tick(&db, &mut h).unwrap();
    // The first ray: from the top centre of the old box to the bottom centre of the new one.
    assert_eq!(h.rays.len(), 1);
    let (a, b, mask) = h.rays[0];
    assert_eq!(mask, TypeMasks::TGE.item_collide);
    assert_eq!([a[0], a[1]], [0.0, 0.0]);
    assert_eq!(a[2], 1.01f32);
    // Gravity and drag over a tick, and the ray's end at the box bottom after the tick.
    let vz = -20.0 * db.gravity_mod * DT;
    let vz = vz - vz * it.s.drag * DT;
    assert_eq!(b[2], p[2] + vz * DT);
    // The stop: velocity zero, at rest, the position goes out.
    assert_eq!(it.s.velocity, [0.0; 3]);
    assert!(it.s.at_rest);
    assert_eq!(it.s.rest_ticks, 0);
    assert_eq!(it.s.mask_bits, net_mask::POSITION);
    assert_eq!(it.s.position()[0], 0.0);
    // The response sends the normal speed back `elasticity + 1` times, plus 0.002; the free sweep moves by what is left.
    let rebound = vz + ((-vz) * (db.elasticity + 1.0) + 0.002);
    assert_eq!(it.s.position()[2], p[2] + rebound * DT);
    assert_eq!(it.s.render_delta, p);
}

/// After 64 ticks asleep the 65th wakes the item for one tick; it settles again, so the whole move is undone through
/// `setTransform` (it tests the flag at the START of the tick).
#[test]
fn a_resting_item_wakes_every_65_ticks_and_undoes_the_move() {
    let db = ItemData::default();
    let mut it = awake(&db, [0.0, 0.0, 0.01]);
    let mut h = host_with(vec![floor(1)]);
    it.process_tick(&db, &mut h).unwrap();
    let z1 = it.s.position()[2];
    assert!(it.s.at_rest);
    for k in 1..=64 {
        it.process_tick(&db, &mut h).unwrap();
        assert_eq!(it.s.rest_ticks, k);
        assert_eq!(it.s.position()[2], z1);
    }
    h.rays.clear();
    it.s.mask_bits = 0;
    it.process_tick(&db, &mut h).unwrap();
    // The 65th tick did simulate (a ray was cast) and the move was undone.
    assert_eq!(h.rays.len(), 1);
    assert_eq!(h.rays[0].1[2].to_bits(), (-0.010_088_322f32).to_bits());
    assert_eq!(it.s.position(), [0.0, 0.0, z1]);
    assert_eq!(it.s.velocity, [0.0; 3]);
    assert!(it.s.at_rest);
    assert_eq!(it.s.rest_ticks, 0);
    assert_eq!(it.s.render_base, [0.0, 0.0, z1]);
    // updatePos set 0x4000000, setTransform 0xc000040, clearMaskBits took 0x4000000 away.
    assert_eq!(it.s.mask_bits, 0x0800_0040);
}

/// A fast drop onto a bouncy, grippy floor: the tangential speed is clipped to `friction * impact`, the normal speed is
/// `elasticity * impact` (+ 0.002 of margin). A hit on solid world at speed keeps the item awake and sends nothing.
#[test]
fn an_item_bounces_with_elasticity_and_friction() {
    let db = ItemData {
        friction: 0.2,
        elasticity: 0.5,
        ..ItemData::default()
    };
    let mut it = awake(&db, [0.0, 0.0, 2.0]);
    it.set_velocity([30.0, 0.0, -100.0]);
    let mut h = host_with(vec![floor(1)]);
    it.s.mask_bits = 0;
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(bits(it.s.velocity), [0x411b_6cbe, 0, 0x4246_114a]);
    let pos = it.s.position();
    assert_eq!(bits(pos), [0x3e9f_27ad, 0, 0x4065_691c]);
    assert!(!it.s.at_rest);
    // moving on solid world while awake: no position bit
    assert_eq!(it.s.mask_bits, 0);
}

/// The sweep loop's response: an item thrown at a wall loses its normal speed (to the 0.002 margin) and keeps its fall;
/// the box stops at the wall, not past it.
#[test]
fn an_item_stops_at_a_wall_in_the_sweep_loop() {
    let db = ItemData::default();
    let mut it = awake(&db, [0.0, 0.0, 5.0]);
    it.set_velocity([20.0, 0.0, 0.0]);
    let wall = solid(2, [1.0, -20.0, -1.0], [1.5, 20.0, 50.0]);
    let mut h = host_with(vec![floor(1), wall]);
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(bits(it.s.velocity), [0xbb03_2000, 0, 0xbf21_37f4]);
    let x = it.s.position()[0];
    // the right face of the box (x + 0.5) rests against the wall at 1.0
    assert!((0.48..=0.5).contains(&x), "x = {x}");
    assert!(!it.s.at_rest);
}

/// Two velocity responses in one `updatePos`: the first ray bounces the item off the floor, then the sweep loop runs it into a
/// wall with the bounced velocity (the floor response, then the wall's).
#[test]
fn the_first_ray_and_the_sweep_loop_each_respond_once() {
    let db = ItemData {
        elasticity: 0.5,
        ..ItemData::default()
    };
    let mut it = awake(&db, [0.0, 0.0, 2.0]);
    it.set_velocity([20.0, 0.0, -100.0]);
    let wall = solid(2, [1.0, -20.0, -1.0], [1.5, 20.0, 50.0]);
    let mut h = host_with(vec![floor(1), wall]);
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(bits(it.s.velocity), [0xc11d_78d6, 0, 0x4246_114a]);
    assert!(!it.s.at_rest);
}

#[test]
fn the_third_sub_step_that_still_hits_zeroes_the_velocity() {
    // A box wedged in a corridor of two walls closer than the item is wide: every sub-step finds a hit.
    let db = ItemData::default();
    let mut it = awake(&db, [0.0, 0.0, 5.0]);
    it.set_velocity([10.0, 0.0, 0.0]);
    let a = solid(2, [0.2, -20.0, -1.0], [0.6, 20.0, 50.0]);
    let b = solid(3, [-0.6, -20.0, -1.0], [-0.2, 20.0, 50.0]);
    let mut h = host_with(vec![a, b]);
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(it.s.velocity, [0.0; 3]);
}

#[test]
fn a_shape_hit_queues_one_collision_per_250_ms() {
    let db = ItemData::default();
    let player = Obj {
        id: ObjectId(5),
        min: [-0.3, -0.3, -2.0],
        max: [0.3, 0.3, 0.5],
        type_mask: PLAYER,
        convex: false,
        vel: [1.0, 2.0, 0.0],
        disabled: 0,
    };
    let mut it = awake(&db, [0.0, 0.0, 2.0]);
    it.set_velocity([0.0, 0.0, -60.0]);
    let mut h = host_with(vec![player]);
    h.time = 1000;
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(h.item_collisions.len(), 1);
    let (who, vec) = h.item_collisions[0];
    assert_eq!(who, ObjectId(5));
    // the item's post-response velocity minus the other's
    let v = it.s.velocity;
    assert_eq!(vec[0], (f64::from(v[0]) - 1.0) as f32);
    assert_eq!(vec[1], (f64::from(v[1]) - 2.0) as f32);
    // still inside the 250 ms: the same object is not queued again
    h.time = 1032;
    it.s.position();
    let m = at([0.0, 0.0, 2.0]);
    it.set_transform(&m, &mut h);
    it.set_velocity([0.0, 0.0, -60.0]);
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(h.item_collisions.len(), 1);
    // after it expired
    h.time = 1400;
    it.set_transform(&m, &mut h);
    it.set_velocity([0.0, 0.0, -60.0]);
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(h.item_collisions.len(), 2);
}

#[test]
fn a_callback_that_deletes_the_item_ends_the_tail() {
    let db = ItemData::default();
    let player = Obj {
        id: ObjectId(5),
        min: [-0.3, -0.3, -2.0],
        max: [0.3, 0.3, 0.5],
        type_mask: PLAYER,
        convex: false,
        vel: [0.0; 3],
        disabled: 0,
    };
    let mut it = awake(&db, [0.0, 0.0, 2.0]);
    it.set_velocity([0.0, 0.0, -60.0]);
    let mut h = host_with(vec![player]);
    h.item_alive = false;
    h.coverage = 0.5;
    it.process_tick(&db, &mut h).unwrap();
    // the item was deleted inside onCollision: no liquid callback fires
    assert!(h.enters.is_empty());
    assert!(!it.s.in_liquid);
}

#[test]
fn the_collision_exempt_object_times_out_after_15_ticks_and_is_switched_off_for_the_ray() {
    let db = ItemData::default();
    let mut it = awake(&db, [0.0, 0.0, 5.0]);
    it.set_collision_timeout(ObjectId(77));
    assert_eq!(it.s.mask_bits, net_mask::COLLISION_OBJECT);
    let mut h = host_with(vec![floor(1)]);
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(it.s.exempt_ticks, 14);
    // off around the first ray and around the working-set rebuild, each time back on
    let offs = h
        .switches
        .iter()
        .filter(|s| *s == &(ObjectId(77), true))
        .count();
    let ons = h
        .switches
        .iter()
        .filter(|s| *s == &(ObjectId(77), false))
        .count();
    assert_eq!(offs, 2);
    assert!(ons >= offs);
    for _ in 0..14 {
        it.process_tick(&db, &mut h).unwrap();
    }
    assert_eq!(it.s.exempt, None);
}

#[test]
fn liquid_callbacks_fire_once_on_entering_and_once_on_leaving() {
    let db = ItemData::default();
    let mut it = awake(&db, [0.0, 0.0, 5.0]);
    let mut h = host_with(vec![floor(1)]);
    h.coverage = 0.5;
    it.process_tick(&db, &mut h).unwrap();
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(h.enters, vec![(0.5, 7)]);
    assert!(it.s.in_liquid);
    h.coverage = 0.0;
    it.process_tick(&db, &mut h).unwrap();
    it.process_tick(&db, &mut h).unwrap();
    assert_eq!(h.leaves, vec![7]);
    assert!(!it.s.in_liquid);
}

#[test]
fn set_velocity_stores_the_velocity_as_given_and_wakes_the_item() {
    let db = ItemData::default();
    let mut it = awake(&db, [0.0, 0.0, 5.0]);
    it.s.at_rest = true;
    it.set_velocity([300.0, 400.0, 0.0]);
    assert_eq!(it.s.velocity, [300.0, 400.0, 0.0]);
    assert!(!it.s.at_rest);
    assert_ne!(it.s.mask_bits & net_mask::POSITION, 0);
    it.set_velocity([3.0, 4.0, 0.0]);
    assert_eq!(it.s.velocity, [3.0, 4.0, 0.0]);
    it.set_velocity([f32::NAN, 0.0, 0.0]);
    assert!(it.s.velocity[0].is_nan());
}

#[test]
fn apply_impulse_sets_the_velocity_to_impulse_over_mass() {
    let db = ItemData {
        mass: 3.0,
        ..ItemData::default()
    };
    let mut it = awake(&db, [0.0, 0.0, 5.0]);
    it.s.velocity = [100.0, 100.0, 100.0];
    it.apply_impulse(&db, [10.0, -20.0, 30.0]);
    assert_eq!(bits(it.s.velocity), [0x4055_5555, 0xc0d5_5555, 0x4120_0000]);
    assert!(!it.s.at_rest);
}

#[test]
fn set_transform_keeps_only_the_yaw_unless_the_item_rotates() {
    let db = ItemData::default();
    let mut it = awake(&db, [0.0, 0.0, 5.0]);
    let mut h = host_with(vec![]);
    let mut m = set_euler([0.3, 0.2, 0.5]);
    m[3] = 1.0;
    m[7] = 2.0;
    m[11] = 3.0;
    it.set_transform(&m, &mut h);
    assert_eq!(it.s.position(), [1.0, 2.0, 3.0]);
    let yaw_only = set_euler([0.0, 0.0, 0.5]);
    for i in [0usize, 1, 4, 5, 10] {
        assert!(
            (it.s.scene.obj_to_world[i] - yaw_only[i]).abs() < 0.2,
            "element {i}: {} vs {}",
            it.s.scene.obj_to_world[i],
            yaw_only[i]
        );
    }
    assert_eq!(it.s.scene.obj_to_world[2], 0.0);
    assert_eq!(it.s.scene.obj_to_world[6], 0.0);
    it.s.rotate = true;
    it.set_transform(&m, &mut h);
    assert_eq!(it.s.scene.obj_to_world[0], 1.0);
    assert_eq!(it.s.scene.obj_to_world[5], 1.0);
    // the world box follows
    assert!(it.s.scene.world_box.min[0] < 1.0 && it.s.scene.world_box.max[0] > 1.0);
}

#[test]
fn max_velocity_clamps_the_fall() {
    let db = ItemData {
        max_velocity: 10.0,
        ..ItemData::default()
    };
    let mut it = awake(&db, [0.0, 0.0, 100.0]);
    it.s.velocity = [0.0, 0.0, -50.0];
    it.update_velocity(&db, DT);
    // |v| is brought toward the limit by the tenth of the excess each tick, not clamped at once
    assert!(it.s.velocity[2] > -50.0 && it.s.velocity[2] < -40.0);
}

#[test]
fn the_working_set_cache_rebuilds_only_when_the_box_leaves_it() {
    let db = ItemData::default();
    let mut it = awake(&db, [0.0, 0.0, 5.0]);
    let mut h = host_with(vec![floor(1)]);
    it.process_tick(&db, &mut h).unwrap();
    let c = it.s.working_box;
    assert_ne!(c.min[0], CACHE_SENTINEL);
    let disables = |h: &TestHost| h.switches.iter().filter(|s| s.1).count();
    let before = disables(&h);
    it.process_tick(&db, &mut h).unwrap();
    // inside the cache: no rebuild, so no disable (updatePos' tail enables once, unpaired, as in Torque)
    assert_eq!(disables(&h), before);
    assert_eq!(it.s.working_box, c);
}

#[test]
fn replay_is_deterministic_and_reports_the_packer_fields() {
    let db = ItemData::default();
    let start = awake(&db, [0.0, 0.0, 3.0]);
    let plan: Vec<Vec<ItemAction>> = (0..250)
        .map(|i| {
            if i == 0 {
                vec![ItemAction::ApplyImpulse([5.0, 0.0, 20.0])]
            } else {
                vec![]
            }
        })
        .collect();
    let a = run(&start, &db, &plan, &mut host_with(vec![floor(1)]));
    let b = run(&start, &db, &plan, &mut host_with(vec![floor(1)]));
    assert_eq!(a, b);
    assert_eq!(a.len(), 250);
    assert!(a.iter().any(|t| t.at_rest));
    assert!(a[0].position[2] > 3.0);
    assert_eq!(a[0].bits().len(), 7);
    // it ends on the floor
    assert!(a[249].position[2] >= 0.0 && a[249].position[2] < 0.2);
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
        const SPECIAL: [f32; 14] = [
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
            200.0,
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
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let cases: u32 = std::env::var("FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    let (mut queued, mut stopped) = (0usize, 0usize);
    for case in 0..cases {
        // odd cases are all hostile; even cases are a sane scene with a hostile value here and there
        let den = if case % 2 == 1 { 1 } else { 12 };
        let db = ItemData {
            mass: rng.mix(5.0, den),
            drag: rng.mix(1.0, den),
            density: rng.mix(3.0, den),
            friction: rng.mix(1.0, den),
            elasticity: rng.mix(1.0, den),
            sticky: case % 7 == 0,
            gravity_mod: rng.mix(2.0, den),
            max_velocity: rng.mix(100.0, den),
            dynamic_type: rng.next() as u32,
            has_shape: true,
        };
        let b = if den == 1 {
            Box3::new(rng.v3(), rng.v3())
        } else {
            Box3::new(
                [rng.mix(0.5, den) - 0.5, rng.mix(0.5, den) - 0.5, 0.0],
                [rng.mix(0.5, den) + 0.5, rng.mix(0.5, den) + 0.5, 1.0],
            )
        };
        let mut p = rng.vec3(10.0, den);
        p[2] = if den == 1 { p[2] } else { p[2].abs() + 0.1 };
        let mut s = ItemState::new(ObjectId(900), &db, b, at(p), &TypeMasks::TGE);
        s.at_rest = false;
        s.is_static = case % 11 == 0;
        s.hidden = case % 13 == 0;
        s.scene.scale = if case % 5 == 0 {
            rng.vec3(2.0, den)
        } else {
            [1.0; 3]
        };
        s.zone_gravity = rng.mix(1.0, den);
        s.buoyancy = rng.mix(1.0, den);
        s.drag = rng.mix(1.0, den);
        let mut it = Item::new(s);
        if case % 3 == 0 {
            it.set_velocity(rng.vec3(60.0, den));
        } else {
            it.s.velocity = rng.vec3(60.0, den);
        }
        if case % 4 == 0 {
            it.set_collision_timeout(ObjectId(2));
        }
        let mut objs = vec![
            floor(1),
            solid(2, rng.vec3(10.0, den), rng.vec3(10.0, den)),
            solid(3, [1.0, -5.0, -1.0], [2.0, 5.0, 8.0]),
        ];
        objs.push(Obj {
            id: ObjectId(4),
            min: rng.vec3(5.0, den),
            max: rng.vec3(5.0, den),
            type_mask: PLAYER,
            convex: false,
            vel: rng.vec3(5.0, den),
            disabled: 0,
        });
        let mut h = host_with(objs);
        if case % 9 == 0 {
            h.world.budget = 40 + u64::from(case % 50);
        }
        h.coverage = rng.mix(1.0, den);
        for tick in 0..40 {
            if tick % 8 == 3 {
                let imp = rng.vec3(100.0, den);
                it.apply_impulse(&db, imp);
            }
            if tick % 10 == 9 {
                let mut m = if den == 1 {
                    let mut m = IDENTITY;
                    for e in m.iter_mut() {
                        *e = rng.pick();
                    }
                    m
                } else {
                    set_euler([rng.mix(1.0, den), rng.mix(1.0, den), rng.mix(3.0, den)])
                };
                m[3] = rng.mix(10.0, den);
                m[7] = rng.mix(10.0, den);
                m[11] = rng.mix(10.0, den).abs() + 0.1;
                it.set_transform(&m, &mut h);
            }
            h.time = h.time.wrapping_add(32);
            let was_awake = !it.s.at_rest;
            let _ = it.process_tick(&db, &mut h);
            stopped += usize::from(was_awake && it.s.at_rest && !it.s.is_static);
        }
        queued += h.item_collisions.len();
    }
    // the fuzz must reach the hit and rest paths, not only skip the NaN ones
    assert!(queued > 0, "{queued} queued collisions");
    assert!(stopped > cases as usize / 20, "{stopped} stops");
}
