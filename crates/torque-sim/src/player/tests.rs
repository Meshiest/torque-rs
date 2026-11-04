//! The Player's step on small synthetic worlds: hand-worked ticks (each expected value is the arithmetic chain written out by
//! hand), determinism, and a hostile fuzz.

use std::rc::Rc;

use torque_collide::convex::ConvexSource;
use torque_collide::math::{Box3, F3, IDENTITY};
use torque_collide::mesh::Hull;
use torque_collide::{Exhausted, ObjectId, ObjectView, World};

use super::host::{LiquidStatics, NoSkeleton, PlayerHost, Skeleton};
use super::*;

/// A solid box: its world box (no rotation).
#[derive(Clone)]
pub(crate) struct Solid {
    pub(crate) id: ObjectId,
    pub(crate) min: F3,
    pub(crate) max: F3,
}

pub(crate) struct TestHost {
    solids: Vec<Solid>,
    pub(crate) skel: NoSkeleton,
    pub(crate) statics: LiquidStatics,
    pub(crate) budget: u64,
    pub(crate) time: u32,
    impacts: u32,
    dismounts: u32,
    /// The subclass the host hands the step (none: Torque's Player).
    class: Option<Rc<dyn PlayerClass>>,
}

impl TestHost {
    pub(crate) fn new(solids: Vec<Solid>) -> TestHost {
        TestHost {
            solids,
            skel: NoSkeleton,
            statics: LiquidStatics::default(),
            budget: u64::MAX,
            time: 0,
            impacts: 0,
            dismounts: 0,
            class: None,
        }
    }

    fn solid(&self, id: ObjectId) -> Option<&Solid> {
        self.solids.iter().find(|b| b.id == id)
    }
}

/// A floor: the top at z = 0, 40 x 40.
pub(crate) fn floor(id: u32) -> Solid {
    Solid {
        id: ObjectId(id),
        min: [-20.0, -20.0, -1.0],
        max: [20.0, 20.0, 0.0],
    }
}

impl World for TestHost {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        let b = self.solid(id)?;
        let c: F3 = std::array::from_fn(|i| (b.min[i] + b.max[i]) * 0.5);
        let h: F3 = std::array::from_fn(|i| (b.max[i] - b.min[i]) * 0.5);
        let mut m = IDENTITY;
        m[3] = c[0];
        m[7] = c[1];
        m[11] = c[2];
        let mut wto = IDENTITY;
        wto[3] = -c[0];
        wto[7] = -c[1];
        wto[11] = -c[2];
        Some(ObjectView {
            obj_to_world: m,
            world_to_obj: wto,
            scale: [1.0; 3],
            obj_box: Box3::new([-h[0], -h[1], -h[2]], h),
            world_box: Box3::new(b.min, b.max),
            world_radius: 1.0,
            type_mask: crate::testworld::SOLID,
            collision_disabled: false,
        })
    }

    fn find_objects(
        &mut self,
        query: &Box3,
        mask: u32,
        exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        for b in &self.solids {
            if out.len() >= limit {
                break;
            }
            let v = self.object(b.id).unwrap();
            if Some(b.id) != exclude && v.type_mask & mask != 0 && v.world_box.overlaps(query) {
                out.push(b.id);
            }
        }
        Ok(())
    }

    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        let Some(b) = self.solid(id) else {
            return ConvexSource::None;
        };
        crate::testworld::TestBox::source(b.min, b.max)
    }

    fn hull(&self, _id: ObjectId, _slot: u32) -> Option<Rc<Hull>> {
        None
    }

    fn charge(&mut self, units: u64) -> bool {
        if self.budget < units {
            self.budget = 0;
            return false;
        }
        self.budget -= units;
        true
    }
}

impl PlayerHost for TestHost {
    fn world(&mut self) -> &mut dyn World {
        self
    }
    fn skeleton(&mut self) -> &mut dyn Skeleton {
        &mut self.skel
    }
    fn sim_time(&self) -> u32 {
        self.time
    }
    fn player_class(&self) -> Option<Rc<dyn PlayerClass>> {
        self.class.clone()
    }
    fn liquid_statics(&mut self) -> &mut LiquidStatics {
        &mut self.statics
    }
    fn on_impact(&mut self, _s: &mut PlayerState, _hit: Option<ObjectId>, _vec: F3) {
        self.impacts += 1;
    }
    fn do_dismount(&mut self, _s: &mut PlayerState) {
        self.dismounts += 1;
    }
}

/// A test datablock: the box (1.25, 1.25, 2.65) and mass 90 (hand-chosen numbers).
pub(crate) fn data() -> PlayerData {
    let mut db = PlayerData {
        mass: 90.0,
        bounding_box: [1.25, 1.25, (10.6f32 * 0.25)],
        run_force: 4320.0,
        max_forward_speed: 7.0,
        max_backward_speed: 4.0,
        max_side_speed: 6.0,
        jump_force: 1080.0,
        max_step_height: 1.0,
        min_impact_speed: 250.0,
        ..PlayerData::default()
    };
    db.preload();
    db
}

pub(crate) fn player(db: &PlayerData, pos: F3) -> Player {
    Player::new(PlayerState::new(ObjectId(1), db, pos, 0.0))
}

fn fwd(y: f32) -> Move {
    Move {
        y,
        ..Move::default()
    }
}

/// A whole turn, in radians.
const TWO_PI: f32 = std::f32::consts::TAU;

#[test]
fn a_walk_tick_on_flat_ground_follows_the_run_force_chain() {
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    p.process_tick(&db, Some(&fwd(1.0)), &mut h).unwrap();
    // Yaw 0, so the wish is (0, 1, 0) and the cap `max(7 * 1, 6 * 0)` = 7.
    // Gravity `-20 * dt`; the floor takes it back with 0.002 more, which stays (it is not under 0.0001).
    let gz = -20.0 * DT;
    let az = gz + (-gz + 0.002);
    assert!(az >= 0.0001);
    // The run acceleration toward (0, 7, 0) from (0, 0, az), cut to the run force per tick: `(4320 / 90) * dt`.
    let run_speed = (49.0 + az * az).sqrt();
    let r = ((4320.0 / 90.0) * DT) / run_speed;
    let vy = 7.0 * r;
    let vz = az + (-az * r);
    assert_eq!(p.s.velocity[1], vy);
    assert_eq!(p.s.velocity[0], 0.0);
    assert_eq!(p.s.velocity[2], vz);
    // The early out moves the whole step.
    let step = |v: f32| v * DT;
    assert_eq!(p.s.position(), [0.0, step(vy), step(vz)]);
    assert!(p.s.contact_found && p.s.contact_run && p.s.contact_jump);
    assert_eq!(p.s.contact_timer, 0);
    assert!(!p.s.falling);
}

#[test]
fn a_jump_from_the_floor_adds_the_jump_force_and_starts_the_delay() {
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    // One tick standing to set the jump surface normal (the jump-surface normal).
    p.process_tick(&db, Some(&Move::default()), &mut h).unwrap();
    assert_eq!(p.s.jump_normal, [0.0, 0.0, 1.0]);
    assert_eq!(p.s.jump_surface_last, 0);
    let mut mv = Move::default();
    mv.triggers[2] = true;
    p.process_tick(&db, Some(&mv), &mut h).unwrap();
    // The jump: scale `1 * 0.25 + 0.75` = 1; impulse `1080 / 90` = 12; ramp 1, up the floor's normal; the floor's push and the
    // gravity cancel out.
    assert_eq!(p.s.velocity[2], 12.0);
    assert_eq!(p.s.jump_delay, db.jump_delay);
    assert_eq!(p.s.jump_surface_last, 8);
    // The words carry min(8, 15).
    let w = p.packet_words(&db, &mut h);
    assert_eq!(w.free.unwrap().contact, 8);
    assert!(p.s.position()[2] > 0.0);
}

#[test]
fn a_fall_onto_a_box_lands_on_its_top() {
    let db = data();
    let solid = Solid {
        id: ObjectId(7),
        min: [-1.0, -1.0, -1.0],
        max: [1.0, 1.0, 0.5],
    };
    let mut h = TestHost::new(vec![solid]);
    let mut p = player(&db, [0.0, 0.0, 3.0]);
    p.s.velocity = [0.0, 0.0, -10.0];
    let mut landed = false;
    for _ in 0..40 {
        p.process_tick(&db, Some(&Move::default()), &mut h).unwrap();
        if p.s.contact_run {
            landed = true;
            break;
        }
    }
    assert!(landed);
    let z = p.s.position()[2];
    // Resting on the top (z 0.5) within the back-off and the traction distance.
    assert!((0.49..0.53).contains(&z), "z = {z}");
    // The slide removed the fall: what is left is the skin push `0.01` up.
    assert!(p.s.velocity[2].abs() < 0.05, "vz = {}", p.s.velocity[2]);
    assert_eq!(p.s.jump_surface_last, 0);
    assert!(!p.s.falling);
}

#[test]
fn a_walk_into_a_low_ledge_steps_up() {
    let db = data();
    let ledge = Solid {
        id: ObjectId(8),
        min: [-5.0, 1.0, 0.0],
        max: [5.0, 6.0, 0.4],
    };
    let mut h = TestHost::new(vec![floor(100), ledge]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    let mut top = 0.0f32;
    for _ in 0..60 {
        p.process_tick(&db, Some(&fwd(1.0)), &mut h).unwrap();
        top = top.max(p.s.position()[2]);
    }
    assert!(top >= 0.4, "top = {top}");
    assert!(p.s.position()[1] > 1.0);
}

#[test]
fn the_pitch_clamps_to_the_look_angles_and_a_nan_takes_the_low_one() {
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    let up = Move {
        pitch: 1.0,
        ..Move::default()
    };
    p.process_tick(&db, Some(&up), &mut h).unwrap();
    assert_eq!(p.s.head[0], 1.0);
    p.process_tick(&db, Some(&up), &mut h).unwrap();
    assert_eq!(p.s.head[0], db.max_look_angle);
    // A pitch above pi wraps by 2 pi first: 4.0 -> 4.0 - 2pi = -2.28..., then the clamp.
    let wrap = Move {
        pitch: 4.0,
        ..Move::default()
    };
    p.process_tick(&db, Some(&wrap), &mut h).unwrap();
    let expect = (4.0 - TWO_PI) + db.max_look_angle;
    assert_eq!(p.s.head[0].to_bits(), expect.to_bits());
    let nan = Move {
        pitch: f32::NAN,
        ..Move::default()
    };
    p.process_tick(&db, Some(&nan), &mut h).unwrap();
    assert_eq!(p.s.head[0], db.min_look_angle);
}

/// The yaw wrap of `updateMove` without the cap: the unbounded loops.
fn unbounded_wrap(mut y: f32) -> f32 {
    if !(0.0 <= y || y.is_nan()) {
        loop {
            y += TWO_PI;
            if !(0.0 > y) {
                break;
            }
        }
    }
    if !(y <= TWO_PI || y.is_nan()) {
        loop {
            y -= TWO_PI;
            if !(y > TWO_PI) {
                break;
            }
        }
    }
    y
}

#[test]
fn the_yaw_wrap_cap_matches_the_unbounded_loops_where_they_finish() {
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut yaw = -400.0f32;
    while yaw <= 400.0 {
        let mut p = player(&db, [0.0, 0.0, 0.0]);
        p.s.rot[2] = yaw;
        p.process_tick(&db, Some(&Move::default()), &mut h).unwrap();
        assert_eq!(
            p.s.rot[2].to_bits(),
            unbounded_wrap(yaw).to_bits(),
            "yaw {yaw}"
        );
        yaw += 0.737;
    }
    // A yaw the unbounded loops never finish on (2^25: adding 2 pi no longer moves it) ends the tick.
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    p.s.rot[2] = 3.4e7;
    p.process_tick(&db, Some(&Move::default()), &mut h).unwrap();
    p.s.rot[2] = f32::INFINITY;
    p.process_tick(&db, Some(&Move::default()), &mut h).unwrap();
}

#[test]
fn the_words_are_thirteen_when_free() {
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    let p = player(&db, [1.0, 2.0, 0.0]);
    let w = p.packet_words(&db, &mut h);
    assert_eq!(w.bits().len(), 13);
    assert_eq!(w.free.unwrap().position, [1.0, 2.0, 0.0]);
}

fn walk_script(n: usize) -> Vec<Option<Move>> {
    (0..n)
        .map(|i| {
            let mut m = Move {
                x: ((i % 7) as f32 - 3.0) / 3.0,
                y: if i % 11 < 8 { 1.0 } else { -0.5 },
                yaw: ((i % 5) as f32 - 2.0) * 0.05,
                pitch: ((i % 3) as f32 - 1.0) * 0.1,
                ..Move::default()
            };
            m.triggers[2] = i % 23 == 0;
            m.triggers[3] = i % 41 > 36;
            (i % 17 != 0).then_some(m)
        })
        .collect()
}

#[test]
fn a_driver_run_is_deterministic() {
    let db = data();
    let solids = vec![
        floor(100),
        Solid {
            id: ObjectId(8),
            min: [-5.0, 3.0, 0.0],
            max: [5.0, 6.0, 0.4],
        },
        Solid {
            id: ObjectId(9),
            min: [2.0, -3.0, 0.0],
            max: [3.0, 3.0, 3.0],
        },
    ];
    let start = player(&db, [0.0, 0.0, 0.0]);
    let moves = walk_script(300);
    let mut h1 = TestHost::new(solids.clone());
    let a = driver::run(&start, &db, &moves, &mut h1);
    let mut h2 = TestHost::new(solids);
    let b = driver::run(&start, &db, &moves, &mut h2);
    assert_eq!(a.len(), 300);
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.words.bits(), y.words.bits());
        assert!(!x.exhausted);
    }
}

/// A small xorshift for the fuzz.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub(crate) fn float(&mut self) -> f32 {
        const SPECIAL: [f32; 10] = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            1e30,
            -1e30,
            0.0,
            -0.0,
            f32::MIN_POSITIVE,
            3.4e38,
            1e-30,
        ];
        let r = self.next();
        if r.is_multiple_of(4) {
            SPECIAL[(r >> 8) as usize % SPECIAL.len()]
        } else {
            ((r >> 16) as i32 as f32) / 1.0e8
        }
    }
}

#[test]
fn hostile_moves_and_states_never_panic_or_run_away() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for case in 0..60 {
        let mut db = data();
        if case % 3 == 0 {
            db.mass = rng.float();
            db.max_step_height = rng.float();
            db.run_force = rng.float();
            db.jump_force = rng.float();
            db.bounding_box = [rng.float(), rng.float(), rng.float()];
            db.horiz_max_speed = rng.float();
            db.up_resist_speed = rng.float();
            db.preload();
        }
        let mut h = TestHost::new(vec![
            floor(100),
            Solid {
                id: ObjectId(8),
                min: [-5.0, 1.0, 0.0],
                max: [5.0, 6.0, 0.4],
            },
        ]);
        h.budget = 50_000_000;
        let mut p = player(&db, [0.0, 0.0, 0.0]);
        if case % 2 == 0 {
            p.s.velocity = [rng.float(), rng.float(), rng.float()];
            p.s.rot[2] = rng.float();
            p.s.head = [rng.float(), rng.float(), rng.float()];
            p.s.scale = [rng.float(), rng.float(), rng.float()];
            p.s.update_scaled_box();
        }
        for _ in 0..40 {
            let mut m = Move {
                x: rng.float(),
                y: rng.float(),
                z: rng.float(),
                yaw: rng.float(),
                pitch: rng.float(),
                roll: rng.float(),
                free_look: rng.next().is_multiple_of(2),
                triggers: [false; crate::MAX_TRIGGERS],
            };
            for t in &mut m.triggers {
                *t = rng.next().is_multiple_of(3);
            }
            let _ = p.process_tick(&db, Some(&m), &mut h);
            let _ = p.packet_words(&db, &mut h);
            assert!(p.s.collisions.len() <= MAX_COLLISION_QUEUE);
            assert!(p.working.len() <= torque_collide::convex::MAX_WORKING_SET);
        }
    }
}

#[test]
fn a_spent_budget_stops_the_tick_cleanly() {
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    h.budget = 3;
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    assert!(p.process_tick(&db, Some(&fwd(1.0)), &mut h).is_err());
}

#[test]
fn the_affine_inverse_undoes_a_yaw_and_a_move() {
    let mut m = torque_collide::math::set_euler([0.0, 0.0, 0.5]);
    m[3] = 3.0;
    m[7] = -2.0;
    m[11] = 1.0;
    let inv = affine_inverse(&m);
    let p = torque_collide::math::mul_p(&inv, torque_collide::math::mul_p(&m, [1.0, 2.0, 3.0]));
    for (a, b) in p.iter().zip([1.0f32, 2.0, 3.0]) {
        assert!((a - b).abs() < 1e-5);
    }
}

#[test]
fn a_world_box_with_a_nan_takes_the_unit_box() {
    // `resetWorldBox`: a per-axis compare takes the -1..1 box for a max below its min or unordered with it.
    let db = data();
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    for axis in 0..3 {
        p.s.obj_box = Box3::new([-1.0, -2.0, 0.0], [1.0, 2.0, 3.0]);
        p.s.obj_box.max[axis] = f32::NAN;
        p.s.reset_world_box();
        assert_eq!(p.s.world_box, Box3::new([-1.0; 3], [1.0; 3]), "axis {axis}");
        p.s.obj_box.max[axis] = 5.0;
        p.s.obj_box.min[axis] = f32::NAN;
        p.s.reset_world_box();
        assert_eq!(
            p.s.world_box,
            Box3::new([-1.0; 3], [1.0; 3]),
            "axis {axis} (min)"
        );
    }
    // A valid box is kept (a flat one too: max equal to min).
    p.s.obj_box = Box3::new([0.0; 3], [0.0, 1.0, 1.0]);
    p.s.reset_world_box();
    assert_eq!(p.s.world_box, Box3::new([0.0; 3], [0.0, 1.0, 1.0]));
}

#[test]
fn the_energy_step_keeps_a_nan() {
    // `e > max` takes the max, else `0 > e` takes 0; a NaN fails both and is stored.
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    p.s.energy = f32::NAN;
    p.process_tick(&db, None, &mut h).unwrap();
    assert!(p.s.energy.is_nan());
    p.s.energy = 0.0;
    p.s.recharge_rate = f32::NAN;
    p.process_tick(&db, None, &mut h).unwrap();
    assert!(p.s.energy.is_nan());
    // The clamps themselves.
    p.s.recharge_rate = 0.0;
    p.s.energy = db.max_energy + 5.0;
    p.process_tick(&db, None, &mut h).unwrap();
    assert_eq!(p.s.energy, db.max_energy);
    p.s.energy = -3.0;
    p.process_tick(&db, None, &mut h).unwrap();
    assert_eq!(p.s.energy, 0.0);
}

#[test]
fn the_scaled_box_is_the_object_box_times_the_scale() {
    let db = data();
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    assert_eq!(s.scaled_box, s.obj_box);
    s.scale = [2.0, 0.5, 3.0];
    s.update_scaled_box();
    let b = s.obj_box;
    assert_eq!(
        s.scaled_box.min,
        [b.min[0] * 2.0, b.min[1] * 0.5, b.min[2] * 3.0]
    );
    assert_eq!(
        s.scaled_box.max,
        [b.max[0] * 2.0, b.max[1] * 0.5, b.max[2] * 3.0]
    );
}

/// Torque's early out: each axis of the step shorter than the object box's side (the boxes are 1.25 wide and 2.65 tall).
#[test]
fn the_early_out_fits_a_step_shorter_than_the_box() {
    let s = PlayerState::new(ObjectId(1), &data(), [0.0; 3], 0.0);
    assert!(TorquePlayer.early_out_fits(&s, [1.2, -1.2, 2.6]));
    assert!(!TorquePlayer.early_out_fits(&s, [1.25, 0.0, 0.0]));
    assert!(!TorquePlayer.early_out_fits(&s, [0.0, 0.0, -2.7]));
    assert!(!TorquePlayer.early_out_fits(&s, [f32::NAN, 0.0, 0.0]));
}

/// A subclass's fields: how many moves it saw.
#[derive(Debug, Clone, PartialEq, Default)]
struct Seen {
    moves: u32,
}

/// A subclass that runs twice as fast forward and counts the moves.
#[derive(Debug)]
struct Fast;

impl PlayerClass for Fast {
    fn on_move(&self, s: &mut PlayerState, _mv: &Move) {
        if s.ext.is_none() {
            s.ext = Some(Box::new(Seen::default()));
        }
        if let Some(seen) = s.fields_mut::<Seen>() {
            seen.moves += 1;
        }
    }
    fn move_speed(&self, s: &PlayerState, db: &PlayerData, mv: &Move) -> f32 {
        let fast = PlayerData {
            max_forward_speed: db.max_forward_speed * 2.0,
            ..db.clone()
        };
        super::update_move::move_speed(s, &fast, mv)
    }
}

/// The step asks the host's subclass at its points: the subclass's speed is the one a Player with that speed runs at, and its
/// fields travel with the state (cloned and compared with it).
#[test]
fn a_subclass_overrides_the_step_where_it_says() {
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    h.class = Some(Rc::new(Fast));
    let mut p = player(&db, [0.0; 3]);
    for _ in 0..3 {
        p.process_tick(&db, Some(&fwd(1.0)), &mut h).unwrap();
    }
    let mut plain_db = data();
    plain_db.max_forward_speed = 14.0;
    let mut plain = player(&plain_db, [0.0; 3]);
    let mut h2 = TestHost::new(vec![floor(100)]);
    for _ in 0..3 {
        plain
            .process_tick(&plain_db, Some(&fwd(1.0)), &mut h2)
            .unwrap();
    }
    assert_eq!(p.s.velocity, plain.s.velocity);
    assert_eq!(p.s.position(), plain.s.position());
    assert_eq!(p.s.fields::<Seen>(), Some(&Seen { moves: 3 }));
    let copy: PlayerState = (*p.s).clone();
    assert_eq!(copy, *p.s);
    let mut other = copy.clone();
    other.fields_mut::<Seen>().unwrap().moves = 9;
    assert_ne!(other, copy);
    assert!(plain.s.fields::<Seen>().is_none());
}

// ---- TGE's updateMove sections (the defaults) ----

fn look(yaw: f32, pitch: f32, free_look: bool) -> Move {
    Move {
        yaw,
        pitch,
        free_look,
        ..Move::default()
    }
}

/// Free look turns the head only when mounted on node 0 or seen from the third person; else the body turns and the head
/// eases back to the front.
#[test]
fn free_look_turns_the_head_when_mounted_or_in_the_third_person() {
    let db = data();
    let fresh = || PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    // On foot, first person: free look turns the body.
    let mut s = fresh();
    s.head[2] = 0.5;
    update_move::head_and_rotation(&mut s, &db, &look(0.25, 0.0, true));
    assert_eq!(s.rot[2], 0.25);
    assert_eq!(s.head[2], 0.25);
    // A third-person client: the head turns, held by maxFreelookAngle (3.0).
    let mut s = fresh();
    s.client = Some(ClientView {
        freelook_value: 1.0,
    });
    update_move::head_and_rotation(&mut s, &db, &look(2.0, 0.0, true));
    update_move::head_and_rotation(&mut s, &db, &look(2.0, 0.0, true));
    assert_eq!(s.head[2], db.max_freelook_angle);
    assert_eq!(s.rot[2], 0.0);
    // Mounted on node 0: the head turns; on another node the body does.
    let mut s = fresh();
    s.mount = Some(ObjectId(5));
    update_move::head_and_rotation(&mut s, &db, &look(-0.5, 0.0, true));
    assert_eq!((s.head[2], s.rot[2]), (-0.5, 0.0));
    s.mount_node = 2;
    update_move::head_and_rotation(&mut s, &db, &look(-0.5, 0.0, true));
    assert_eq!(s.head[2], -0.25);
    assert_eq!(s.rot[2], -0.5 + TWO_PI);
}

/// Controlling another object eases the head's pitch back too; the deltas are the change of the tick.
#[test]
fn the_head_eases_back_while_controlling_and_the_deltas_record_the_turn() {
    let db = data();
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    s.head = [0.8, 0.0, 0.4];
    s.control_object = Some(ObjectId(9));
    update_move::head_and_rotation(&mut s, &db, &look(0.1, 0.2, false));
    // The pitch is added before the halving: (0.8 + 0.2) / 2.
    let pitched = 0.8f32 + 0.2;
    assert_eq!(s.head[0], pitched * 0.5);
    assert_eq!(s.head[2], 0.2);
    assert_eq!(s.delta_head, s.head);
    assert_eq!(s.delta_head_vec, [0.8 - s.head[0], 0.0, 0.2]);
    assert_eq!(s.delta_rot, [0.0, 0.0, 0.1]);
    assert_eq!(s.delta_rot_vec, [0.0, 0.0, -0.1]);
    // A turn across 0 wraps the yaw, and the delta takes the short way.
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    update_move::head_and_rotation(&mut s, &db, &look(-0.1, 0.0, false));
    let yaw = -0.1 + TWO_PI;
    assert_eq!(s.rot[2], yaw);
    assert_eq!(s.delta_rot_vec[2], (0.0 - yaw) + TWO_PI);
}

/// The wish's cap: forward, backward and side speeds by the move's axes, the underwater ones at a coverage of 0.9.
#[test]
fn the_speed_cap_takes_the_underwater_speeds_at_nine_tenths_coverage() {
    let mut db = data();
    db.max_underwater_forward_speed = 3.0;
    db.max_underwater_backward_speed = 2.0;
    db.max_underwater_side_speed = 1.0;
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    let mv = |x: f32, y: f32| Move {
        x,
        y,
        ..Move::default()
    };
    assert_eq!(update_move::move_speed(&s, &db, &mv(0.0, 1.0)), 7.0);
    assert_eq!(update_move::move_speed(&s, &db, &mv(0.0, -1.0)), 4.0);
    assert_eq!(update_move::move_speed(&s, &db, &mv(-1.0, 0.5)), 6.0);
    // The source's `>= 0.9` compares in double precision, and the float 0.9 lies just under the double.
    s.water_coverage = 0.9;
    assert_eq!(update_move::move_speed(&s, &db, &mv(0.0, 1.0)), 7.0);
    s.water_coverage = 0.900_001;
    assert_eq!(update_move::move_speed(&s, &db, &mv(0.0, 1.0)), 3.0);
    assert_eq!(update_move::move_speed(&s, &db, &mv(0.0, -1.0)), 2.0);
    assert_eq!(update_move::move_speed(&s, &db, &mv(1.0, 0.0)), 1.0);
    // The object's scale does not enter the cap.
    s.scale = [2.0, 2.0, 2.0];
    assert_eq!(update_move::move_speed(&s, &db, &mv(0.0, -1.0)), 2.0);
}

/// Below the minimum run energy the run surface brakes the Player toward rest; while recovering the run force is scaled.
#[test]
fn the_run_surface_brakes_without_energy_and_scales_the_force_while_recovering() {
    let mut db = data();
    db.min_run_energy = 5.0;
    db.recover_run_force_scale = 0.5;
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    s.velocity = [0.0, 1.0, 0.0];
    s.action_anim = TGE_ACTION_NUMBERS.jump;
    let max_acc = (4320.0 / 90.0) * DT;
    let floor_n = [0.0, 0.0, 1.0];
    let mut acc = [0.0; 3];
    update_move::run_surface(&mut s, &db, &[0.0, 1.0, 0.0], 7.0, &mut acc, floor_n);
    // No energy: the target is 0, so the run acceleration is the velocity's opposite (under the force cap).
    assert!(max_acc > 1.0);
    assert_eq!(acc, [0.0, -1.0, 0.0]);
    assert_eq!(s.action_anim, TGE_ACTION_NUMBERS.none);
    assert_eq!(s.contact_timer, 0);
    // With energy, recovering: the cap is halved.
    s.energy = 10.0;
    s.velocity = [0.0; 3];
    s.action_state = 2;
    let mut acc = [0.0; 3];
    update_move::run_surface(&mut s, &db, &[0.0, 1.0, 0.0], 7.0, &mut acc, floor_n);
    assert_eq!(acc, [0.0, max_acc * 0.5, 0.0]);
}

/// On a slope the wish is turned parallel to the surface before it is scaled to the cap.
#[test]
fn the_run_surface_turns_the_wish_along_a_slope() {
    let db = data();
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    let n = torque_collide::math::normalize([0.0, -1.0, 1.0]);
    let mut acc = [0.0; 3];
    update_move::run_surface(&mut s, &db, &[0.0, 1.0, 0.0], 1.0, &mut acc, n);
    // Uphill: the target (0, 0.707, 0.707) is reached within the force cap.
    assert!((acc[1] - acc[2]).abs() < 1e-6 && acc[1] > 0.7, "{acc:?}");
}

/// The integration: the applied force, buoyancy (none while standing still on a run surface with a buoyancy of 1 or less),
/// the container drag as `v - (v * drag) * dt`, and the falling flag.
#[test]
fn the_integration_applies_buoyancy_drag_and_the_falling_threshold() {
    let db = data();
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    s.buoyancy = 0.5;
    update_move::integrate(&mut s, &db, &[0.0; 3], true);
    assert_eq!(s.velocity, [0.0; 3]);
    update_move::integrate(&mut s, &db, &[0.0; 3], false);
    let up = 0.0 - 0.5 * -20.0 * 1.0 * DT;
    assert_eq!(s.velocity, [0.0, 0.0, up]);
    // Drag 2 on a velocity of (10, 0, -20), no buoyancy.
    s.buoyancy = 0.0;
    s.drag = 2.0;
    s.velocity = [10.0, 0.0, -20.0];
    update_move::integrate(&mut s, &db, &[0.0; 3], false);
    let dragged = |v: f32| v - v * 2.0 * DT;
    assert_eq!(s.velocity, [dragged(10.0), 0.0, dragged(-20.0)]);
    assert!(s.falling);
    // An applied force adds `force / mass` per tick.
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    s.applied_force = [90.0, 0.0, 0.0];
    update_move::integrate(&mut s, &db, &[0.0; 3], true);
    assert_eq!(s.velocity[0], (90.0 / 90.0) * DT);
    assert!(!s.falling);
}

/// In water there is no swim stroke: the jump trigger at full coverage adds nothing to the acceleration.
#[test]
fn there_is_no_swim_stroke() {
    let db = data();
    let mut h = TestHost::new(vec![]);
    let mut p = player(&db, [0.0, 0.0, 10.0]);
    p.s.water_coverage = 1.0;
    let mut mv = Move::default();
    mv.triggers[2] = true;
    let mut acc = [0.0, 0.0, -0.64];
    let mut w = [0.0; 3];
    TorquePlayer.swim(&mut p.s, &db, &mv, &mut w, &mut acc);
    assert_eq!(acc, [0.0, 0.0, -0.64]);
    p.process_tick(&db, Some(&mv), &mut h).unwrap();
    assert!(p.s.velocity[2] < 0.0);
}

/// The jump trigger dismounts a mounted Player.
#[test]
fn the_jump_trigger_dismounts() {
    let db = data();
    let mut h = TestHost::new(vec![]);
    let mut p = player(&db, [0.0, 0.0, 10.0]);
    p.s.mount = Some(ObjectId(5));
    let mut mv = Move::default();
    mv.triggers[4] = true;
    p.process_tick(&db, Some(&mv), &mut h).unwrap();
    assert_eq!(h.dismounts, 0);
    mv.triggers[2] = true;
    p.process_tick(&db, Some(&mv), &mut h).unwrap();
    assert_eq!(h.dismounts, 1);
}

/// TGE's `canJump`: the move state, enabled, unmounted, no jump delay, the energy, and a jump surface within the last
/// `JumpSkipContactsMax` contacts. Nothing about the speed or a ceiling.
#[test]
fn can_jump_follows_tge() {
    let db = data();
    let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    assert!(can_jump(&s, &db));
    s.velocity = [0.0, 0.0, 50.0];
    s.ceiling = true;
    assert!(can_jump(&s, &db));
    s.jump_surface_last = JUMP_SKIP_CONTACTS_MAX;
    assert!(!can_jump(&s, &db));
    s.jump_surface_last = 0;
    s.action_state = 2;
    assert!(!can_jump(&s, &db));
    s.action_state = 1;
    s.jump_delay = 1;
    assert!(!can_jump(&s, &db));
}

/// `findContact` reaches the traction distance below the feet: a floor 0.02 under them is a run surface, one 0.05 under
/// them is not.
#[test]
fn the_contact_search_reaches_the_traction_distance() {
    let db = data();
    for (z, found) in [(0.02f32, true), (0.05, false)] {
        let mut h = TestHost::new(vec![floor(100)]);
        let mut p = player(&db, [0.0, 0.0, z]);
        contact::update_working_collision_set(&mut p, &db, &mut h).unwrap();
        let (mut run, mut jump, mut normal) = (false, false, [0.0; 3]);
        contact::find_contact(&mut p, &db, &mut run, &mut jump, &mut normal, &mut h).unwrap();
        assert_eq!((run, p.s.contact_found), (found, found), "z {z}");
        if found {
            assert_eq!(normal, [0.0, 0.0, 1.0]);
        }
    }
}

/// Without an installed table the steps are this crate's own.
#[test]
fn the_plain_table_is_in_use_by_default() {
    let lib = crate::library::get();
    assert!(std::ptr::eq(lib, &crate::library::PLAIN));
    let m = torque_collide::math::set_euler([0.0, 0.0, 0.5]);
    assert_eq!((lib.affine_inverse)(&m)[1], m[4]);
}

/// Without a class's own, the action numbers are TGE's: eight fixed actions, jump 5, standjump 6, land 7, none 511; a new
/// Player plays no action.
#[test]
fn the_default_action_numbers_are_tges() {
    let db = PlayerData::default();
    assert_eq!(db.actions, TGE_ACTION_NUMBERS);
    let n = TGE_ACTION_NUMBERS;
    assert_eq!(
        (n.table, n.jump, n.stand_jump, n.land, n.none),
        (8, 5, 6, 7, 511)
    );
    let s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
    assert_eq!(s.action_anim, 511);
}

/// On a run surface the table's jump actions stop (TGE's 5 and 6); landing and the other actions play on.
#[test]
fn a_run_surface_stops_the_jump_actions_only() {
    let db = data();
    for (anim, after) in [(5, 511), (6, 511), (7, 7), (0, 0), (8, 8)] {
        let mut s = PlayerState::new(ObjectId(1), &db, [0.0; 3], 0.0);
        s.action_anim = anim;
        let mut acc = [0.0; 3];
        update_move::run_surface(
            &mut s,
            &db,
            &[0.0, 1.0, 0.0],
            7.0,
            &mut acc,
            [0.0, 0.0, 1.0],
        );
        assert_eq!(s.action_anim, after, "action {anim}");
    }
}

/// TGE's Player keeps the arm position the pitch gives (the look animation clamps it to 0..1 after).
#[test]
fn tges_arm_position_is_the_pitchs() {
    let s = PlayerState::new(ObjectId(1), &data(), [0.0; 3], 0.0);
    for p in [-0.5, 0.25, 1.5] {
        assert_eq!(TorquePlayer.arm_pos(&s, p), p);
    }
}
