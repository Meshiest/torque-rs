//! The poses on small synthetic worlds: the triggers change the pose, each pose has its box and its speeds, a pose with no
//! room is refused, and a datablock without poses is TGE's.

use torque_collide::ObjectId;

use super::super::tests::{Solid, TestHost, data, floor, player};
use super::*;

/// The test datablock with crouching (box 1.25 x 1.25 x 1.4, speeds 2 / 1.5 / 1) and prone (box 2 x 2 x 0.7, speeds 1 / 0.5 /
/// 0), triggers 3 and 4.
fn posed() -> PlayerData {
    let mut db = data();
    db.poses = PoseData {
        crouch: true,
        prone: true,
        sprint: true,
        crouch_box: [1.25, 1.25, 1.4],
        prone_box: [2.0, 2.0, 0.7],
        crouch_speeds: PoseSpeeds {
            forward: 2.0,
            backward: 1.5,
            side: 1.0,
        },
        prone_speeds: PoseSpeeds {
            forward: 1.0,
            backward: 0.5,
            side: 0.0,
        },
        sprint_speeds: PoseSpeeds {
            forward: 12.0,
            backward: 6.0,
            side: 8.0,
        },
        crouch_force: 4320.0,
        prone_force: 4320.0,
        sprint_force: 8640.0,
        ..PoseData::default()
    };
    db
}

fn held(triggers: &[usize], y: f32) -> Move {
    let mut mv = Move {
        y,
        ..Move::default()
    };
    for &t in triggers {
        mv.triggers[t] = true;
    }
    mv
}

/// Ticks `n` times with `mv`; the forward speed after.
fn run(p: &mut Player, db: &PlayerData, h: &mut TestHost, mv: &Move, n: usize) -> f32 {
    for _ in 0..n {
        p.process_tick(db, Some(mv), h).unwrap();
    }
    p.s.velocity[1]
}

#[test]
fn the_triggers_crouch_lie_down_and_stand_with_each_poses_box() {
    let db = posed();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    run(&mut p, &db, &mut h, &Move::default(), 3);
    assert_eq!(p.s.pose, Pose::Stand);
    assert_eq!(p.s.obj_box.max[2], db.bounding_box[2]);
    p.s.mask_bits = 0;
    run(&mut p, &db, &mut h, &held(&[3], 0.0), 1);
    assert_eq!(p.s.pose, Pose::Crouch);
    assert_eq!(p.s.obj_box.max, [0.625, 0.625, 1.4]);
    assert_eq!(p.s.obj_box.min, [-0.625, -0.625, 0.0]);
    assert!(
        (p.s.world_box.max[2] - (p.s.position()[2] + 1.4)).abs() < 1e-5,
        "the world box follows"
    );
    assert_ne!(p.s.mask_bits & net_mask::MOVE, 0, "the clients are told");
    // Prone wins over crouch when both are held, and its box is long and low; it kneels first, having just crouched from
    // standing.
    run(&mut p, &db, &mut h, &held(&[3, 4], 0.0), 1);
    assert_eq!(p.s.pose, Pose::Crouch);
    run(
        &mut p,
        &db,
        &mut h,
        &held(&[3, 4], 0.0),
        KNEEL_TICKS as usize,
    );
    assert_eq!(p.s.pose, Pose::Prone);
    assert_eq!(p.s.obj_box.max, [1.0, 1.0, 0.7]);
    // Prone for its hold.
    run(
        &mut p,
        &db,
        &mut h,
        &held(&[3, 4], 0.0),
        POSE_HOLD_TICKS as usize,
    );
    // Letting go stands it up again by way of the crouch: crouched while it pushes up and two ticks more, then standing.
    let mut poses = Vec::new();
    for _ in 0..PRONE_UP_TICKS + 3 {
        run(&mut p, &db, &mut h, &Move::default(), 1);
        poses.push(p.s.pose);
    }
    assert_eq!(poses[0], Pose::Crouch);
    let kneel = poses.iter().filter(|q| **q == Pose::Crouch).count();
    assert_eq!(kneel, PRONE_UP_TICKS as usize + 2, "{poses:?}");
    assert_eq!(p.s.pose, Pose::Stand);
    assert_eq!(p.s.obj_box.max[2], db.bounding_box[2]);
    run(&mut p, &db, &mut h, &held(&[5], 0.0), 1);
    assert_eq!(p.s.pose, Pose::Sprint);
    assert_eq!(p.s.obj_box.max[2], db.bounding_box[2]);
}

#[test]
fn each_pose_runs_at_its_own_speeds() {
    let db = posed();
    for (triggers, pose, forward, backward, side) in [
        (&[][..], Pose::Stand, 7.0, 4.0, 6.0),
        (&[5][..], Pose::Sprint, 12.0, 6.0, 8.0),
        (&[3][..], Pose::Crouch, 2.0, 1.5, 1.0),
        (&[4][..], Pose::Prone, 1.0, 0.5, 0.0),
    ] {
        let mut h = TestHost::new(vec![floor(100)]);
        let mut p = player(&db, [0.0, 0.0, 0.0]);
        let ahead = run(&mut p, &db, &mut h, &held(triggers, 1.0), 60);
        assert_eq!(p.s.pose, pose);
        assert!(
            (ahead - forward).abs() < 0.05,
            "{pose:?}: forward at {ahead}, its cap {forward}"
        );
        let mut p = player(&db, [0.0, 0.0, 0.0]);
        let back = run(&mut p, &db, &mut h, &held(triggers, -1.0), 60);
        assert!((back + backward).abs() < 0.05, "{pose:?}: back at {back}");
        let mut p = player(&db, [0.0, 0.0, 0.0]);
        let mut mv = held(triggers, 0.0);
        mv.x = 1.0;
        run(&mut p, &db, &mut h, &mv, 60);
        assert!(
            (p.s.velocity[0] - side).abs() < 0.05,
            "{pose:?}: to the side at {}",
            p.s.velocity[0]
        );
    }
}

#[test]
fn a_pose_without_room_is_refused_and_no_one_stands_up_under_a_low_ceiling() {
    let db = posed();
    // A slab 1.6 over the floor: room to crouch (1.4) and lie down, none to stand (2.65).
    let ceiling = Solid {
        id: ObjectId(9),
        min: [-10.0, -10.0, 1.6],
        max: [10.0, 10.0, 2.0],
    };
    let mut h = TestHost::new(vec![floor(100), ceiling]);
    // Crouched in the open first, then under the slab.
    let mut p = player(&db, [0.0, -14.0, 0.0]);
    run(&mut p, &db, &mut h, &held(&[3], 0.0), 2);
    assert_eq!(p.s.pose, Pose::Crouch);
    let mut under = player(&db, [0.0, 0.0, 0.0]);
    under.s.pose = Pose::Crouch;
    under.s.obj_box = object_box(db.poses.crouch_box);
    under.s.update_scaled_box();
    under.s.reset_world_box();
    // Let go of the crouch: it stays crouched (no room to stand), and stays so tick after tick.
    run(&mut under, &db, &mut h, &Move::default(), 10);
    assert_eq!(under.s.pose, Pose::Crouch);
    assert_eq!(under.s.obj_box.max[2], 1.4);
    // Sprinting asks for the standing box: refused too.
    run(&mut under, &db, &mut h, &held(&[5], 0.0), 2);
    assert_eq!(under.s.pose, Pose::Crouch);
    // Lying down is fine under it; letting go of prone crouches (no room to stand).
    run(&mut under, &db, &mut h, &held(&[4], 0.0), 1);
    assert_eq!(under.s.pose, Pose::Prone);
    run(
        &mut under,
        &db,
        &mut h,
        &held(&[4], 0.0),
        POSE_HOLD_TICKS as usize,
    );
    run(&mut under, &db, &mut h, &Move::default(), 1);
    assert_eq!(under.s.pose, Pose::Crouch);
    // Out from under it (crawling crouched to the open) it stands up.
    for _ in 0..400 {
        under
            .process_tick(
                &db,
                Some(&Move {
                    y: -1.0,
                    ..Move::default()
                }),
                &mut h,
            )
            .unwrap();
        if under.s.pose == Pose::Stand {
            break;
        }
    }
    assert_eq!(under.s.pose, Pose::Stand, "at {:?}", under.s.position());
    assert!(
        under.s.position()[1] < -10.5,
        "only once clear: {:?}",
        under.s.position()
    );
    // Lying under a slab at 1.2 there is no room to crouch either.
    let low = Solid {
        id: ObjectId(9),
        min: [-10.0, -10.0, 1.2],
        max: [10.0, 10.0, 2.0],
    };
    let mut h = TestHost::new(vec![floor(100), low]);
    let mut flat = player(&db, [0.0, 0.0, 0.0]);
    flat.s.pose = Pose::Prone;
    flat.s.obj_box = object_box(db.poses.prone_box);
    flat.s.update_scaled_box();
    flat.s.reset_world_box();
    run(&mut flat, &db, &mut h, &held(&[3], 0.0), 3);
    assert_eq!(flat.s.pose, Pose::Prone, "no room to crouch");
}

#[test]
fn a_wall_beside_a_player_leaves_it_room_to_crouch_but_not_to_lie_down_into_it() {
    let db = posed();
    // A wall 0.65 from the Player's middle: touching its standing box's side.
    let wall = Solid {
        id: ObjectId(9),
        min: [0.64, -10.0, 0.0],
        max: [1.0, 10.0, 3.0],
    };
    let mut h = TestHost::new(vec![floor(100), wall]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    run(&mut p, &db, &mut h, &held(&[3], 0.0), 2);
    assert_eq!(
        p.s.pose,
        Pose::Crouch,
        "a wall at the side leaves room to crouch"
    );
    // The prone box (2 wide) reaches into the wall: refused, so it stays crouched (both held).
    run(&mut p, &db, &mut h, &held(&[3, 4], 0.0), 2);
    assert_eq!(p.s.pose, Pose::Crouch);
}

#[test]
fn a_datablock_without_poses_ignores_the_triggers() {
    let db = data();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    let fwd = run(&mut p, &db, &mut h, &held(&[3, 4, 5], 1.0), 60);
    assert_eq!(p.s.pose, Pose::Stand);
    assert_eq!(p.s.obj_box.max[2], db.bounding_box[2]);
    assert!((fwd - 7.0).abs() < 0.05);
}

#[test]
fn no_jump_lying_down_and_a_mounted_player_stands() {
    let db = posed();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    run(
        &mut p,
        &db,
        &mut h,
        &held(&[4], 0.0),
        3 + KNEEL_TICKS as usize,
    );
    assert_eq!(p.s.pose, Pose::Prone);
    assert!(!crate::player::can_jump(&p.s, &db));
    run(&mut p, &db, &mut h, &held(&[4, 2], 0.0), 3);
    assert!(p.s.velocity[2].abs() < 0.1, "no jump: {:?}", p.s.velocity);
    p.s.mount = Some(ObjectId(50));
    run(&mut p, &db, &mut h, &held(&[4], 0.0), 1);
    assert_eq!(p.s.pose, Pose::Stand);
}

#[test]
fn a_record_shows_the_pose_chosen_in_its_triggers() {
    let mut d = posed().poses;
    d.triggers = PoseTriggers {
        crouch: 3,
        prone: 1,
        sprint: 5,
    };
    // Swimming is not shown: the clients derive it from the water as the server does ([`swims`]).
    for pose in Pose::ALL {
        let mut triggers = [true; crate::MAX_TRIGGERS];
        d.show(pose, &mut triggers);
        let shown = if pose == Pose::Swim {
            Pose::Stand
        } else {
            pose
        };
        assert_eq!(d.shown(&triggers), shown);
        assert!(
            triggers[0] && triggers[2] && triggers[4],
            "the other triggers kept"
        );
    }
    // A pose the datablock does not allow keeps its trigger (a game's own) and is never shown.
    d.sprint = false;
    let mut triggers = [false; crate::MAX_TRIGGERS];
    triggers[5] = true;
    d.show(Pose::Crouch, &mut triggers);
    assert!(triggers[5] && triggers[3]);
    assert_eq!(d.shown(&triggers), Pose::Crouch);
    d.show(Pose::Stand, &mut triggers);
    assert_eq!(d.shown(&triggers), Pose::Stand);
}

#[test]
fn getting_down_and_up_between_the_crouch_and_prone_holds_the_player_still() {
    let db = posed();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    run(
        &mut p,
        &db,
        &mut h,
        &held(&[3], 0.0),
        KNEEL_TICKS as usize + 2,
    );
    assert_eq!(p.s.pose, Pose::Crouch);
    // Lying down while pushing ahead: still for the ticks the body takes to get down, then crawling.
    let mut still = 0;
    for _ in 0..PRONE_DOWN_TICKS + 20 {
        run(&mut p, &db, &mut h, &held(&[3, 4], 1.0), 1);
        assert_eq!(p.s.pose, Pose::Prone);
        if p.s.velocity[1].abs() < 1e-3 {
            still += 1;
        }
    }
    assert_eq!(still, PRONE_DOWN_TICKS as usize, "still while getting down");
    assert!(p.s.velocity[1] > 0.5, "then it crawls: {:?}", p.s.velocity);
    // Up to the crouch: still while it pushes up, then moving crouched.
    let mut still = 0;
    for _ in 0..PRONE_UP_TICKS + 20 {
        run(&mut p, &db, &mut h, &held(&[3], 1.0), 1);
        assert_eq!(p.s.pose, Pose::Crouch);
        if p.s.velocity[1].abs() < 1e-2 {
            still += 1;
        }
    }
    assert!(
        still >= PRONE_UP_TICKS as usize - 2,
        "still while pushing up: {still}"
    );
    assert!(
        p.s.velocity[1] > 1.0,
        "then crouch walking: {:?}",
        p.s.velocity
    );
}

#[test]
fn a_change_reaches_through_both_boxes_while_the_body_moves_and_holds_before_the_next() {
    let db = posed();
    let mut h = TestHost::new(vec![floor(100)]);
    let mut p = player(&db, [0.0, 0.0, 0.0]);
    run(&mut p, &db, &mut h, &Move::default(), 3);
    assert_eq!(reach_box(&p.s), p.s.obj_box);
    run(&mut p, &db, &mut h, &held(&[3], 0.0), 1);
    assert_eq!(p.s.pose, Pose::Crouch);
    // The movement box is the crouch's at once; the reach keeps the standing height while the body eases down.
    assert_eq!(p.s.obj_box.max[2], 1.4);
    assert_eq!(reach_box(&p.s).max[2], db.bounding_box[2]);
    // Let go at once: the crouch is held first.
    let mut ticks = 0;
    while p.s.pose == Pose::Crouch {
        run(&mut p, &db, &mut h, &Move::default(), 1);
        ticks += 1;
    }
    assert_eq!(ticks, POSE_HOLD_TICKS as usize, "held from the change");
    // Settled, the reach is the box again.
    run(
        &mut p,
        &db,
        &mut h,
        &Move::default(),
        CROUCH_BLEND_TICKS as usize,
    );
    assert_eq!(reach_box(&p.s), p.s.obj_box);
    // Flipping every tick changes no faster than the hold, and the reach always covers the standing box it moves from.
    let mut changes = Vec::new();
    let mut last = p.s.pose;
    for tick in 0..80 {
        let t: &[usize] = if tick % 2 == 0 { &[3] } else { &[] };
        run(&mut p, &db, &mut h, &held(t, 0.0), 1);
        if p.s.pose != last {
            changes.push(tick);
            last = p.s.pose;
        }
        assert_eq!(reach_box(&p.s).max[2], db.bounding_box[2], "tick {tick}");
    }
    assert!(changes.len() > 3);
    assert!(
        changes
            .windows(2)
            .all(|w| w[1] - w[0] >= POSE_HOLD_TICKS as usize),
        "{changes:?}"
    );
}

// ---- swimming

/// The lake's surface in the swimming tests.
const LEVEL: f32 = 10.0;

/// A lake: water from 20 under the origin up to [`LEVEL`], 200 x 200, of density 1 and viscosity 15.
fn lake() -> super::super::host::ZoneView {
    use torque_collide::math::Plane;
    let (min, max) = ([-100.0, -100.0, -20.0], [100.0, 100.0, LEVEL]);
    super::super::host::ZoneView {
        id: ObjectId(50),
        active: true,
        world_box: Box3::new(min, max),
        planes: vec![
            Plane::new(-1.0, 0.0, 0.0, min[0]),
            Plane::new(1.0, 0.0, 0.0, -max[0]),
            Plane::new(0.0, -1.0, 0.0, min[1]),
            Plane::new(0.0, 1.0, 0.0, -max[1]),
            Plane::new(0.0, 0.0, -1.0, min[2]),
            Plane::new(0.0, 0.0, 1.0, -max[2]),
        ],
        enabled: true,
        velocity_mod: 1.0,
        gravity_mod: 1.0,
        drag: 0.0,
        applied_force: [0.0; 3],
        water: true,
        viscosity: 15.0,
        density: 1.0,
        color: [0.0; 4],
        liquid_type: 0,
    }
}

/// The posed datablock that also swims: swim box 1.25 x 2.3 x 1, swim force 900, underwater speeds 3 / 2 / 2, a density of
/// 0.85 and a drag of 0.3.
fn swimmer() -> PlayerData {
    let mut db = posed();
    db.poses.swim = true;
    db.poses.swim_box = [1.25, 2.3, 1.0];
    db.poses.swim_force = 900.0;
    db.max_underwater_forward_speed = 3.0;
    db.max_underwater_backward_speed = 2.0;
    db.max_underwater_side_speed = 2.0;
    db.density = 0.85;
    db.drag = 0.3;
    db
}

/// A host with the lake and a bed whose top is `bed` under its surface (none: the lake has no bed near).
fn lake_host(bed: Option<f32>) -> TestHost {
    let solids = bed.map_or_else(Vec::new, |d| {
        vec![Solid {
            id: ObjectId(100),
            min: [-20.0, -20.0, LEVEL - d - 1.0],
            max: [20.0, 20.0, LEVEL - d],
        }]
    });
    let mut h = TestHost::new(solids);
    h.water = Some(lake());
    h
}

#[test]
fn swimming_comes_before_every_pose_the_triggers_ask_for() {
    assert_eq!(
        Pose::ALL,
        [
            Pose::Stand,
            Pose::Sprint,
            Pose::Crouch,
            Pose::Prone,
            Pose::Swim
        ]
    );
    assert_eq!(Pose::from_name(b"swim"), Some(Pose::Swim));
    assert_eq!(Pose::Swim.action_prefix(), Some("swim"));
    let db = swimmer();
    // Deep water: swimming whatever is held (prone, crouch, sprint), with the swim box.
    for triggers in [&[][..], &[4], &[3], &[5], &[3, 4, 5]] {
        let mut h = lake_host(None);
        let mut p = player(&db, [0.0, 0.0, LEVEL - 3.0]);
        run(&mut p, &db, &mut h, &held(triggers, 0.0), 12);
        assert_eq!(p.s.pose, Pose::Swim, "{triggers:?} held");
        assert_eq!(p.s.obj_box.max, [0.625, 1.15, 1.0]);
    }
    // In water shallower than the swim depth the triggers have their way; deeper, the Player swims.
    let mut h = lake_host(Some(0.5));
    let mut p = player(&db, [0.0, 0.0, LEVEL - 0.5]);
    run(&mut p, &db, &mut h, &held(&[3], 0.0), 12);
    assert_eq!(p.s.pose, Pose::Crouch);
    let mut h = lake_host(Some(0.8));
    let mut p = player(&db, [0.0, 0.0, LEVEL - 0.8]);
    run(&mut p, &db, &mut h, &held(&[3], 0.0), 12);
    assert_eq!(p.s.pose, Pose::Swim);
    // A datablock that does not swim stands in deep water.
    let db = posed();
    let mut h = lake_host(None);
    let mut p = player(&db, [0.0, 0.0, LEVEL - 3.0]);
    run(&mut p, &db, &mut h, &Move::default(), 12);
    assert_eq!(p.s.pose, Pose::Stand);
}

/// The state of a swimmer at rest whose standing box the water covers by `coverage`, its head pitched by `pitch`.
fn swimming(db: &PlayerData, pitch: f32, coverage: f32) -> PlayerState {
    let mut s = *player(db, [0.0; 3]).s;
    s.pose = Pose::Swim;
    s.water_coverage = coverage;
    s.head[0] = pitch;
    s
}

#[test]
fn the_stroke_accelerates_toward_the_wish_by_at_most_the_swim_force() {
    use super::super::update_move::swim;
    let db = swimmer();
    let most = (900.0 / 90.0) * super::super::DT;
    let mut s = swimming(&db, 0.0, 1.0);
    // From rest toward (0, 3, 0): the whole step, cut to `swimForce / mass` a tick; the gravity's term left alone.
    let mut acc = [0.0, 0.0, -0.64];
    swim(&mut s, &db, &held(&[], 1.0), &mut acc);
    assert_eq!(acc[0], 0.0);
    assert!((acc[1] - most).abs() < 1e-6, "{acc:?}");
    assert_eq!(acc[2], -0.64);
    // Near the wish: just to it.
    s.velocity = [0.0, 2.9, 0.0];
    let mut acc = [0.0; 3];
    swim(&mut s, &db, &held(&[], 1.0), &mut acc);
    assert!((acc[1] - 0.1).abs() < 1e-5, "{acc:?}");
    // No wish: the stroke stops the Player across the water, at most as fast.
    s.velocity = [1.0, 0.0, 0.0];
    let mut acc = [0.0; 3];
    swim(&mut s, &db, &Move::default(), &mut acc);
    assert!((acc[0] + most).abs() < 1e-6, "{acc:?}");
    // A Player that does not swim takes no stroke.
    s.pose = Pose::Stand;
    let mut acc = [0.0; 3];
    swim(&mut s, &db, &held(&[], 1.0), &mut acc);
    assert_eq!(acc, [0.0; 3]);
}

/// The velocity strokes with `mv` bring a swimmer to from rest (no gravity, no buoyancy): what the stroke steers toward.
fn steered(db: &PlayerData, pitch: f32, coverage: f32, mv: Move) -> F3 {
    let mut s = swimming(db, pitch, coverage);
    for _ in 0..60 {
        let mut acc = [0.0; 3];
        super::super::update_move::swim(&mut s, db, &mv, &mut acc);
        s.velocity = std::array::from_fn(|k| s.velocity[k] + acc[k]);
    }
    s.velocity
}

#[test]
fn the_head_steers_the_stroke_but_not_up_out_of_the_water() {
    let db = swimmer();
    let near = |v: F3, want: F3| (0..3).all(|k| (v[k] - want[k]).abs() < 1e-3);
    // Under the water the stroke goes where the head looks: down (a positive pitch) or up.
    let p = 0.5f32;
    let v = steered(&db, p, 1.0, held(&[], 1.0));
    assert!(near(v, [0.0, 3.0 * p.cos(), -3.0 * p.sin()]), "{v:?}");
    let v = steered(&db, -p, 1.0, held(&[], 1.0));
    assert!(near(v, [0.0, 3.0 * p.cos(), 3.0 * p.sin()]), "{v:?}");
    // At the surface (the standing box not all under), looking up: level, no climbing out of the water. Looking down still
    // dives.
    let v = steered(&db, -p, 0.85, held(&[], 1.0));
    assert!(near(v, [0.0, 3.0, 0.0]), "{v:?}");
    let v = steered(&db, p, 0.85, held(&[], 1.0));
    assert!(v[2] < -1.0, "{v:?}");
    // The move's z swims up under the water (at the forward speed) and down anywhere; not up at the surface.
    let up = Move {
        z: 1.0,
        ..Move::default()
    };
    let down = Move {
        z: -1.0,
        ..Move::default()
    };
    assert!(near(steered(&db, 0.0, 1.0, up), [0.0, 0.0, 3.0]));
    assert_eq!(steered(&db, 0.0, 0.85, up), [0.0; 3]);
    assert!(near(steered(&db, 0.0, 0.85, down), [0.0, 0.0, -3.0]));
}

#[test]
fn a_swimmer_does_not_jump() {
    // On the bed in water 1 m over its feet, swimming: the jump trigger does nothing.
    let db = swimmer();
    let mut h = lake_host(Some(1.0));
    let mut p = player(&db, [0.0, 0.0, LEVEL - 1.0]);
    run(&mut p, &db, &mut h, &Move::default(), 40);
    assert_eq!(p.s.pose, Pose::Swim);
    assert!(!super::super::update_move::can_jump(&p.s, &db));
    p.s.mask_bits = 0;
    run(&mut p, &db, &mut h, &held(&[2], 0.0), 1);
    assert_eq!(p.s.mask_bits & net_mask::JUMP, 0, "it jumped");
    assert!(p.s.velocity[2] < 1.0, "{:?}", p.s.velocity);
    // A Player that does not swim jumps from the same bed.
    let db = posed();
    let mut h = lake_host(Some(1.0));
    let mut p = player(&db, [0.0, 0.0, LEVEL - 1.0]);
    run(&mut p, &db, &mut h, &Move::default(), 40);
    p.s.mask_bits = 0;
    run(&mut p, &db, &mut h, &held(&[2], 0.0), 1);
    assert_ne!(
        p.s.mask_bits & net_mask::JUMP,
        0,
        "the control did not jump"
    );
}

#[test]
fn a_swimmer_keeps_swimming_through_the_hysteresis_band() {
    let d = swimmer().poses;
    // Where the feet start swimming: the swim box's share under the surface.
    let at = LEVEL - SWIM_DEPTH_SHARE * d.swim_box[2];
    assert!(!swims(&d, at + 0.01, 1.0, Some(LEVEL), false));
    assert!(swims(&d, at - 0.01, 1.0, Some(LEVEL), false));
    assert!(swims(
        &d,
        at + SWIM_HYSTERESIS - 0.01,
        1.0,
        Some(LEVEL),
        true
    ));
    assert!(!swims(
        &d,
        at + SWIM_HYSTERESIS + 0.01,
        1.0,
        Some(LEVEL),
        true
    ));
    assert!(!swims(&d, at - 1.0, 1.0, None, true), "out of the water");
    let mut dry = d;
    dry.swim = false;
    assert!(!swims(&dry, at - 1.0, 1.0, Some(LEVEL), false));
    // The band is wider than a position's quantization on the wire (a compressed point's 0.01).
    const { assert!(SWIM_HYSTERESIS > 0.02) };
    // In the step: on a bed within the band, a Player that stands keeps standing and one that swims keeps swimming.
    let db = swimmer();
    let depth = SWIM_DEPTH_SHARE * d.swim_box[2] - SWIM_HYSTERESIS * 0.5;
    for start in [Pose::Stand, Pose::Swim] {
        let mut h = lake_host(Some(depth));
        let mut p = player(&db, [0.0, 0.0, LEVEL - depth]);
        set_pose(&mut p.s, &db, start);
        run(&mut p, &db, &mut h, &Move::default(), 40);
        assert_eq!(p.s.pose, start);
    }
}

#[test]
fn a_swimmer_floats_steady_its_coverage_over_its_standing_box() {
    let db = swimmer();
    let mut h = lake_host(None);
    let mut p = player(&db, [0.0, 0.0, LEVEL - 2.0]);
    run(&mut p, &db, &mut h, &Move::default(), 94);
    assert_eq!(p.s.pose, Pose::Swim);
    // Put in the water a little higher than it floats: three seconds on it holds within 2 cm where its buoyancy meets its weight: the coverage of its standing box (2.65 tall,
    // not the swim box's 1) the Player's density.
    let (mut low, mut high) = (f32::MAX, f32::MIN);
    for _ in 0..60 {
        run(&mut p, &db, &mut h, &Move::default(), 1);
        let z = p.s.position()[2];
        (low, high) = (low.min(z), high.max(z));
    }
    assert!(high - low < 0.02, "bobbing {} m", high - low);
    let under = (LEVEL - p.s.position()[2]) / db.bounding_box[2];
    assert!((p.s.water_coverage - under).abs() < 1e-4);
    assert!(
        (p.s.water_coverage - 0.85).abs() < 0.02,
        "{}",
        p.s.water_coverage
    );
}
