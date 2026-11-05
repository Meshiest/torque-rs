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
    for pose in Pose::ALL {
        let mut triggers = [true; crate::MAX_TRIGGERS];
        d.show(pose, &mut triggers);
        assert_eq!(d.shown(&triggers), pose);
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
