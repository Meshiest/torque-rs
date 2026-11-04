//! AIPlayer after TGE's `getAIMove`, on the Player tests' flat world: the idle move, the turn and pitch toward an aim point,
//! the walk toward a destination (direction, rotation, slowdown, reach), the line-of-sight callbacks, the console setters, a
//! driver run and a hostile fuzz.

use torque_collide::math::{Box3, F3};
use torque_collide::{ObjectId, TypeMasks};

use super::driver::run;
use super::*;
use crate::Move;
use crate::player::host::{LiquidStatics, Skeleton};
use crate::player::tests::{Rng, TestHost, data, floor, player};
use crate::player::{Player, PlayerData, PlayerHost, PlayerState};

/// The logging host: the Player test world, the aim targets, a line-of-sight ray that is blocked or not, and the callbacks with
/// the AI's line-of-sight flag as the script sees it.
struct AiHost {
    inner: TestHost,
    log: Vec<(&'static str, bool)>,
    targets: Vec<(ObjectId, AiTarget)>,
    blocked: bool,
    rays: Vec<(F3, F3, u32)>,
}

impl AiHost {
    fn new() -> AiHost {
        AiHost {
            inner: TestHost::new(vec![floor(100)]),
            log: Vec::new(),
            targets: Vec::new(),
            blocked: false,
            rays: Vec::new(),
        }
    }

    fn names(&self) -> Vec<&'static str> {
        self.log.iter().map(|l| l.0).collect()
    }
}

impl PlayerHost for AiHost {
    fn world(&mut self) -> &mut dyn torque_collide::World {
        self.inner.world()
    }
    fn skeleton(&mut self) -> &mut dyn Skeleton {
        &mut self.inner.skel
    }
    fn sim_time(&self) -> u32 {
        self.inner.time
    }
    fn liquid_statics(&mut self) -> &mut LiquidStatics {
        &mut self.inner.statics
    }
    fn ai_target(&mut self, id: ObjectId) -> Option<AiTarget> {
        self.targets.iter().find(|t| t.0 == id).map(|t| t.1)
    }
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<F3> {
        self.rays.push((start, end, mask));
        self.blocked.then_some(end)
    }
    fn ai_callback(&mut self, s: &mut PlayerState, name: &'static str) {
        let los = s.ai.as_ref().is_some_and(|a| a.target_in_los);
        self.log.push((name, los));
    }
}

fn ai_player(db: &PlayerData, pos: F3) -> Player {
    let mut p = player(db, pos);
    p.s.ai = Some(Box::default());
    p
}

fn ai(p: &mut Player) -> &mut AiState {
    p.s.ai.as_deref_mut().unwrap()
}

fn mv_of(p: &mut Player, h: &mut AiHost) -> Move {
    get_move(&mut p.s, h).unwrap()
}

/// The eye with no skeleton is the position.
fn eye(p: &Player) -> F3 {
    p.s.position()
}

#[test]
fn a_plain_player_has_no_ai_move() {
    let db = data();
    let mut p = player(&db, [0.0; 3]);
    assert_eq!(get_move(&mut p.s, &mut AiHost::new()), None);
}

/// Nothing to aim at and nowhere to go: the null move with the head pitched back level.
#[test]
fn an_idle_ai_levels_its_head_and_stands() {
    let db = data();
    let mut p = ai_player(&db, [0.0; 3]);
    p.s.head[0] = 0.3;
    let mut h = AiHost::new();
    let m = mv_of(&mut p, &mut h);
    assert_eq!(m.pitch, -0.3);
    assert_eq!((m.x, m.y, m.yaw), (0.0, 0.0, 0.0));
    assert!(h.log.is_empty());
}

/// An aim location to the right turns a quarter turn (yaw `atan2(x, y)` against the Player's 0) and pitches toward its height.
#[test]
fn an_aim_location_turns_and_pitches_toward_it() {
    let db = data();
    let mut p = ai_player(&db, [0.0; 3]);
    ai(&mut p).set_aim_location([10.0, 0.0, 10.0]);
    let mut h = AiHost::new();
    let m = mv_of(&mut p, &mut h);
    let e = eye(&p);
    let x = 10.0 - e[0];
    let y = 0.0 - e[1];
    let yaw = x.atan2(y);
    assert_eq!(m.yaw, yaw);
    let vert = 10.0 - e[2];
    let horz = (x * x + y * y).sqrt();
    let pitch = horz.atan2(vert) - std::f32::consts::FRAC_PI_2;
    assert_eq!(m.pitch, pitch);
    assert_eq!(ai(&mut p).aim_location, [10.0, 0.0, 10.0]);
}

/// A destination straight ahead and far: full stick forward (the larger axis), no turn, full speed.
#[test]
fn a_far_destination_ahead_is_full_stick_forward() {
    let db = data();
    let mut p = ai_player(&db, [0.0; 3]);
    ai(&mut p).set_move_destination([0.0, 20.0, 0.0], true);
    let mut h = AiHost::new();
    let m = mv_of(&mut p, &mut h);
    assert_eq!(m.yaw, 0.0);
    assert!(m.x.abs() < 1e-6, "x = {}", m.x);
    assert_eq!(m.y, 1.0);
    // Looking where it walks: the head is levelled.
    assert_eq!(m.pitch, -0.0);
}

/// Within 5 of the destination a slowing AIPlayer scales its stick by `distance / 5`; without slowdown it does not.
#[test]
fn slowdown_scales_the_stick_near_the_destination() {
    let db = data();
    let mut p = ai_player(&db, [0.0; 3]);
    ai(&mut p).set_move_destination([0.0, 2.0, 0.0], true);
    let mut h = AiHost::new();
    let m = mv_of(&mut p, &mut h);
    let dist = 2.0 - eye(&p)[1];
    assert_eq!(m.y, dist / 5.0);
    let mut q = ai_player(&db, [0.0; 3]);
    ai(&mut q).set_move_destination([0.0, 2.0, 0.0], false);
    ai(&mut q).set_move_speed(0.5);
    let m = mv_of(&mut q, &mut h);
    assert_eq!(m.y, 0.5);
}

/// The diagonal walk: the larger axis is full stick, the other the ratio of the two.
#[test]
fn a_diagonal_destination_splits_the_stick_by_the_ratio() {
    let db = data();
    let mut p = ai_player(&db, [0.0; 3]);
    ai(&mut p).set_move_destination([-30.0, 10.0, 0.0], false);
    ai(&mut p).set_aim_location([0.0, 10.0, 0.0]);
    let mut h = AiHost::new();
    let m = mv_of(&mut p, &mut h);
    // Aiming straight ahead: no turn, so the stick is not rotated.
    assert_eq!(m.yaw, 0.0);
    let e = eye(&p);
    let ratio = ((10.0 - e[1]) / (-30.0 - e[0])).abs();
    assert_eq!(m.x, -1.0);
    assert_eq!(m.y, ratio);
}

/// Within the tolerance on both axes the walk stops and `onReachDestination` runs (the state already stopped).
#[test]
fn reaching_the_destination_stops_and_calls_back() {
    let db = data();
    let mut p = ai_player(&db, [0.0; 3]);
    let e = eye(&p);
    ai(&mut p).set_move_destination([e[0] + 0.1, e[1] - 0.2, 0.0], true);
    let mut h = AiHost::new();
    let m = mv_of(&mut p, &mut h);
    assert_eq!(h.names(), vec!["onReachDestination"]);
    assert_eq!(ai(&mut p).move_state, MoveState::Stop);
    assert_eq!((m.x, m.y), (0.0, 0.0));
}

/// The line of sight to the aim object's box centre: entering it and losing it call back once each.
#[test]
fn the_aim_objects_line_of_sight_calls_back_on_change() {
    let db = data();
    let mut p = ai_player(&db, [0.0; 3]);
    let mut h = AiHost::new();
    let target = AiTarget {
        position: [5.0, 5.0, 0.0],
        world_box: Box3::new([4.0, 4.0, 0.0], [6.0, 6.0, 2.0]),
    };
    h.targets.push((ObjectId(9), target));
    ai(&mut p).set_aim_object(Some(ObjectId(9)), [0.0, 0.0, 1.0]);
    mv_of(&mut p, &mut h);
    assert_eq!(h.log, vec![("onTargetEnterLOS", false)]);
    assert_eq!(h.rays[0].1, [5.0, 5.0, 1.0]);
    assert_eq!(h.rays[0].2, TypeMasks::TGE.ai_los);
    assert_eq!(ai(&mut p).aim_location, [5.0, 5.0, 1.0]);
    mv_of(&mut p, &mut h);
    assert_eq!(h.log.len(), 1);
    h.blocked = true;
    mv_of(&mut p, &mut h);
    assert_eq!(h.names(), vec!["onTargetEnterLOS", "onTargetExitLOS"]);
    assert!(!ai(&mut p).target_in_los);
    // A deleted target is dropped.
    h.targets.clear();
    mv_of(&mut p, &mut h);
    assert_eq!(ai(&mut p).aim_object, None);
}

#[test]
fn the_console_setters_clamp_as_torque_does() {
    let mut a = AiState::new();
    a.set_move_speed(2.0);
    assert_eq!(a.move_speed, 1.0);
    a.set_move_speed(-1.0);
    assert_eq!(a.move_speed, 0.0);
    a.set_move_speed(f32::NAN);
    assert!(a.move_speed.is_nan());
    a.set_move_tolerance(0.01);
    assert_eq!(a.move_tolerance, 0.1);
    a.set_move_tolerance(3.0);
    assert_eq!(a.move_tolerance, 3.0);
    a.set_aim_object(Some(ObjectId(3)), [1.0, 2.0, 3.0]);
    a.target_in_los = true;
    a.set_aim_location([1.0; 3]);
    assert_eq!(
        (a.aim_object, a.aim_offset, a.aim_location_set),
        (None, [0.0; 3], true)
    );
    a.set_move_destination([4.0; 3], false);
    assert_eq!(
        (a.move_state, a.move_slowdown, a.move_destination()),
        (MoveState::Move, false, [4.0; 3])
    );
    a.stop();
    a.clear_aim();
    assert_eq!((a.move_state, a.aim_location_set), (MoveState::Stop, false));
}

/// The driver walks an AIPlayer to a destination with its own `processTick`: deterministic, and it gets there. (It starts off the
/// origin: the stuck test compares with the never-updated last location, the origin.)
#[test]
fn a_driver_walk_reaches_the_destination() {
    let db = data();
    let start = ai_player(&db, [1.0, 0.0, 0.0]);
    let mut steps = vec![vec![AiOp::SetMoveDestination([1.0, 4.0, 0.0], true)]];
    steps.extend((0..120).map(|_| Vec::new()));
    let a = run(&start, &db, &steps, &mut AiHost::new());
    let b = run(&start, &db, &steps, &mut AiHost::new());
    assert_eq!(a, b);
    let last = a.last().unwrap();
    assert_eq!(last.ai.move_state, MoveState::Stop);
    assert!(
        (last.position[1] - 4.0).abs() < 0.5,
        "y = {}",
        last.position[1]
    );
}

#[test]
fn hostile_states_never_panic() {
    let db = data();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for case in 0..400u32 {
        let mut p = ai_player(&db, [0.0; 3]);
        p.s.rot[2] = rng.float();
        p.s.head[0] = rng.float();
        let a = ai(&mut p);
        a.set_move_destination([rng.float(), rng.float(), rng.float()], case % 2 == 0);
        a.set_move_speed(rng.float());
        a.set_move_tolerance(rng.float());
        if case % 3 == 0 {
            a.set_aim_location([rng.float(), rng.float(), rng.float()]);
        }
        let mut h = AiHost::new();
        h.blocked = case % 5 == 0;
        if case % 4 == 0 {
            h.targets.push((
                ObjectId(9),
                AiTarget {
                    position: [rng.float(), rng.float(), rng.float()],
                    world_box: Box3::new([rng.float(); 3], [rng.float(); 3]),
                },
            ));
            ai(&mut p).set_aim_object(Some(ObjectId(9)), [0.0; 3]);
        }
        for _ in 0..3 {
            let _ = p.process_tick(&db, None, &mut h);
        }
    }
}

/// Torque never moves the last location from the origin: an AIPlayer walking from exactly there is stuck at once.
#[test]
fn a_walk_from_the_origin_is_stuck_at_once() {
    let db = data();
    let mut p = ai_player(&db, [0.0; 3]);
    ai(&mut p).set_move_destination([0.0, 4.0, 0.0], true);
    let mut h = AiHost::new();
    let m = mv_of(&mut p, &mut h);
    assert_eq!(h.names(), vec!["onMoveStuck"]);
    assert_eq!(ai(&mut p).move_state, MoveState::Stop);
    assert_ne!(m.y, 0.0, "the move of that tick is kept");
}
