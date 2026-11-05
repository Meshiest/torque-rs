use shapes::instance::ShapeInstance;
use shapes::synth::hull_shape;

use super::*;

/// A shape with `root`, `run`, `jump` and `hold` (no ground motion: `run` moves along the table's direction at speed 1).
fn shape() -> Shape {
    let b = [-0.5, -0.5, 0.0, 0.5, 0.5, 2.0];
    let mut shape = hull_shape(
        b,
        &[("detail2", b)],
        &["eye"],
        &["root", "run", "jump", "hold"],
    )
    .shape;
    // The synthetic shape names its root node `root` too; a shape's names are unique, so the node gets another.
    shape.names[1] = b"base".to_vec();
    shape
}

fn sequence_of(shape: &Shape, instance: &ShapeInstance, t: ThreadId) -> Option<Vec<u8>> {
    let seq = instance.thread(t)?.sequence?;
    let name = usize::try_from(shape.sequences[seq].name).ok()?;
    Some(shape.names[name].clone())
}

#[test]
fn the_action_list_is_the_table_then_the_shapes_other_sequences() {
    let list = action_list(&shape());
    let names: Vec<&[u8]> = list.iter().map(|a| a.name.as_slice()).collect();
    assert_eq!(
        names,
        [
            &b"root"[..],
            b"run",
            b"back",
            b"side",
            b"fall",
            b"jump",
            b"standjump",
            b"land",
            b"hold"
        ]
    );
    assert!(list[action::FALL].sequence.is_none() && list[action::JUMP].sequence.is_some());
    // No ground motion: the table's direction at speed 1, not scaled by the velocity.
    let run = &list[1].motion;
    assert_eq!(
        (run.dir, run.speed, run.velocity_scale),
        ([0.0, 1.0, 0.0], 1.0, false)
    );
}

#[test]
fn the_velocity_in_the_players_frame_picks_the_moving_action() {
    let shape = shape();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let still = PlayerMotion::default();
    anim.update(&shape, &mut instance, &still, 0.032);
    assert_eq!(anim.action(), (action::ROOT, true));
    // Facing +x (yaw a quarter turn), moving along +x: forward.
    let ahead = PlayerMotion {
        velocity: [3.0, 0.0, 0.0],
        yaw: std::f32::consts::FRAC_PI_2,
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &ahead, 0.032);
    assert_eq!(anim.action(), (1, true));
    // Moving along -x while facing +x: the shape has no `back`, and only `side` plays backwards, so the root.
    let back = PlayerMotion {
        velocity: [-3.0, 0.0, 0.0],
        ..ahead
    };
    anim.update(&shape, &mut instance, &back, 0.032);
    assert_eq!(anim.action(), (action::ROOT, true));
    // Sideways: no `side` sequence and nothing else matches, so the root.
    let side = PlayerMotion {
        velocity: [0.0, 3.0, 0.0],
        ..ahead
    };
    anim.update(&shape, &mut instance, &side, 0.032);
    assert_eq!(anim.action(), (action::ROOT, true));
}

#[test]
fn a_jump_plays_when_the_player_leaves_the_ground_and_falling_keeps_it_without_a_fall_sequence() {
    let shape = shape();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let jump = PlayerMotion {
        velocity: [0.0, 4.0, 3.0],
        falling: true,
        jumping: true,
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &jump, 0.032);
    assert_eq!(anim.action(), (action::JUMP, true));
    let standing_jump = PlayerMotion {
        velocity: [0.0, 0.0, 0.2],
        ..jump
    };
    let mut other = PlayerAnimation::new(&shape, &mut instance).unwrap();
    other.update(&shape, &mut instance, &standing_jump, 0.032);
    // The shape has no `standjump`: `jump` plays.
    assert_eq!(other.action(), (action::JUMP, true));
}

#[test]
fn the_arm_thread_plays_the_arm_action_the_record_names() {
    let shape = shape();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    assert_eq!(
        anim.arm_action(),
        None,
        "no look sequence: no arm thread yet"
    );
    let hold = PlayerMotion {
        arm_action: Some(8),
        look: 0.5,
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &hold, 0.032);
    assert_eq!(anim.arm_action(), Some(8));
    let t = anim.arm_thread.unwrap();
    assert_eq!(
        sequence_of(&shape, &instance, t).as_deref(),
        Some(&b"hold"[..])
    );
    assert_eq!(instance.thread(t).unwrap().pos, 0.5);
}

#[test]
fn a_records_motion_is_its_velocity_falling_flag_jump_and_arm() {
    use ghosts::motion::Velocity;
    use ghosts::player::{Player, PlayerMove};
    let mut mv = PlayerMove {
        falling: true,
        velocity: Velocity::pack([0.0, 2.0, 0.0], VELOCITY_NORMAL_BITS),
        ..PlayerMove::default()
    };
    mv.mv.triggers[2] = true;
    let record = TgeRecord::Player(Player {
        arm: Some(8),
        motion: Some(mv),
        ..Player::default()
    });
    let m = PlayerMotion::of_record(&record, 0.0).unwrap();
    assert!(m.falling && m.jumping);
    assert_eq!(m.arm_action, Some(8));
    assert!((m.velocity[1] - 2.0).abs() < 0.05 && m.velocity[0].abs() < 0.05);
}

/// A shape with the table's moving actions, a death, the hold and the look's `head` (no ground motion: the table's
/// directions at speed 1; `side` points left).
fn moving_shape() -> Shape {
    let b = [-0.5, -0.5, 0.0, 0.5, 0.5, 2.0];
    let mut shape = hull_shape(
        b,
        &[("detail2", b)],
        &["eye"],
        &["root", "run", "back", "side", "death1", "hold", "head"],
    )
    .shape;
    shape.names[1] = b"base".to_vec();
    // Each sequence a second long, played once.
    for seq in &mut shape.sequences {
        seq.flags = 0;
        seq.duration = 1.0;
    }
    shape
}

#[test]
fn walking_back_plays_back_and_strafing_plays_side_either_way() {
    let shape = moving_shape();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    // Facing +y (yaw 0): +x is the right, -x the left.
    let at = |v: [f32; 3]| PlayerMotion {
        velocity: v,
        ..PlayerMotion::default()
    };
    for (v, want, name) in [
        ([0.0, 4.0, 0.0], (1, true), "ahead: run"),
        ([0.0, -4.0, 0.0], (2, true), "backward: back"),
        ([-4.0, 0.0, 0.0], (action::SIDE, true), "left: side"),
        (
            [4.0, 0.0, 0.0],
            (action::SIDE, false),
            "right: side backwards",
        ),
        ([3.0, -1.0, 0.0], (action::SIDE, false), "mostly right"),
        ([-1.0, -3.0, 0.0], (2, true), "mostly backward"),
    ] {
        // Long enough for the transition to end.
        for _ in 0..10 {
            anim.update(&shape, &mut instance, &at(v), 0.032);
        }
        assert_eq!(anim.action(), want, "{name}");
        let seq = sequence_of(&shape, &instance, anim.action_thread);
        let action = &anim.actions()[want.0].name;
        assert_eq!(seq.as_ref(), Some(action), "{name}: the thread plays it");
    }
}

#[test]
fn a_death_the_server_sets_plays_to_its_end_and_holds_there() {
    let shape = moving_shape();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let death = anim
        .actions()
        .iter()
        .position(|a| a.name == b"death1")
        .unwrap();
    assert!(death >= 8, "past the table");
    let dying = PlayerMotion {
        server_action: Some(ServerAction {
            action: death as u32,
            hold: true,
            at_end: false,
        }),
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &dying, 0.032);
    assert_eq!(anim.action(), (death, true));
    assert!(anim.held());
    // Moving on (the body pushed along) picks nothing else, at the end or after it.
    let pushed = PlayerMotion {
        velocity: [0.0, 4.0, 0.0],
        ..dying
    };
    for _ in 0..100 {
        anim.update(&shape, &mut instance, &pushed, 0.032);
        assert_eq!(anim.action(), (death, true));
    }
    let t = instance.thread(anim.action_thread).unwrap();
    assert_eq!(t.pos, 1.0, "held at its end");

    // A ghost first seen dead starts at the end.
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let dead = PlayerMotion {
        server_action: Some(ServerAction {
            action: death as u32,
            hold: true,
            at_end: true,
        }),
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &dead, 0.0);
    assert_eq!(anim.action(), (death, true));
    assert_eq!(instance.thread(anim.action_thread).unwrap().pos, 1.0);

    // A table action from the server changes nothing; one the server does not hold gives way at its end.
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let table = PlayerMotion {
        server_action: Some(ServerAction {
            action: 2,
            hold: false,
            at_end: false,
        }),
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &table, 0.032);
    assert_eq!(anim.action(), (action::ROOT, true));
    let once = PlayerMotion {
        server_action: Some(ServerAction {
            action: death as u32,
            hold: false,
            at_end: false,
        }),
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &once, 0.032);
    assert_eq!(anim.action(), (death, true));
    for _ in 0..100 {
        anim.update(&shape, &mut instance, &once, 0.032);
    }
    assert_eq!(anim.action(), (action::ROOT, true), "played out");
}

#[test]
fn a_death_stops_the_arm_and_head_threads_until_another_action_plays() {
    let shape = moving_shape();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let find = |name: &[u8]| anim.actions().iter().position(|a| a.name == name);
    let (death, hold) = (find(b"death1").unwrap(), find(b"hold").unwrap());
    let holding = PlayerMotion {
        arm_action: Some(hold as u32),
        look: 0.2,
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &holding, 0.032);
    assert!(anim.arm_running() && anim.head_running());
    let threads = instance.thread_count();
    let dying = PlayerMotion {
        server_action: Some(ServerAction {
            action: death as u32,
            hold: false,
            at_end: false,
        }),
        ..holding
    };
    anim.update(&shape, &mut instance, &dying, 0.032);
    assert!(anim.dying());
    assert!(!anim.arm_running() && !anim.head_running());
    assert_eq!(anim.arm_action(), None);
    assert_eq!(
        instance.thread_count(),
        threads - 2,
        "only the action thread is left"
    );
    // Played out (the server did not hold it), the hold and the look come back.
    for _ in 0..100 {
        anim.update(&shape, &mut instance, &dying, 0.032);
    }
    assert!(!anim.dying());
    assert!(anim.arm_running() && anim.head_running());
    assert_eq!(anim.arm_action(), Some(hold));
    assert_eq!(instance.thread_count(), threads);
}

#[test]
fn the_head_thread_follows_the_look() {
    let shape = moving_shape();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let (t, _) = anim.head_thread.expect("a head thread");
    assert_eq!(
        sequence_of(&shape, &instance, t).as_deref(),
        Some(&b"head"[..])
    );
    for look in [0.0, 0.3, 1.0] {
        let m = PlayerMotion {
            look,
            ..PlayerMotion::default()
        };
        anim.update(&shape, &mut instance, &m, 0.032);
        assert_eq!(instance.thread(t).unwrap().pos, look);
    }
}

#[test]
fn a_records_action_is_the_servers() {
    use ghosts::player::{Action as RecordAction, Player};
    let record = TgeRecord::Player(Player {
        action: Some(RecordAction {
            action: 9,
            hold_at_end: true,
            at_end: false,
            first_person: false,
            position: None,
        }),
        controlled: true,
        ..Player::default()
    });
    assert_eq!(
        ServerAction::of_record(&record),
        Some(ServerAction {
            action: 9,
            hold: true,
            at_end: false
        })
    );
    // The controlling client's record has no move block; the action still comes.
    assert!(PlayerMotion::of_record(&record, 0.0).is_none());
}

#[test]
fn rising_off_the_ground_plays_the_jump_once() {
    let shape = shape();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let at = |v: [f32; 3]| PlayerMotion {
        velocity: v,
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &at([0.0, 4.0, 0.0]), 0.032);
    assert_eq!(anim.action().0, 1, "running");
    // The jump's impulse: the record's velocity rises past the threshold (no falling flag, no trigger in this record).
    anim.update(&shape, &mut instance, &at([0.0, 4.0, 8.0]), 0.032);
    assert_eq!(anim.action(), (action::JUMP, true));
    // Still rising, then coming down: no new jump; the jump plays to its end first.
    anim.update(&shape, &mut instance, &at([0.0, 4.0, 6.0]), 0.032);
    assert_eq!(anim.action(), (action::JUMP, true));
    // A standing jump plays `jump` too when the shape has no `standjump`.
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    anim.update(&shape, &mut instance, &at([0.0, 0.0, 8.0]), 0.032);
    assert_eq!(anim.action(), (action::JUMP, true));
    // A small step up is no jump.
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    anim.update(&shape, &mut instance, &at([0.0, 4.0, 1.5]), 0.032);
    assert_ne!(anim.action().0, action::JUMP);
}

#[test]
fn an_images_recoil_plays_the_recoil_sequence_from_its_start() {
    let b = [-0.5, -0.5, 0.0, 0.5, 0.5, 2.0];
    let mut shape = hull_shape(b, &[("detail2", b)], &["eye"], &["root", "light_recoil"]).shape;
    shape.names[1] = b"base".to_vec();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let still = PlayerMotion::default();
    anim.update(&shape, &mut instance, &still, 0.032);
    assert!(
        !anim.recoiling(&instance),
        "held at its start until a recoil"
    );
    // A kind the shape has no sequence for changes nothing.
    anim.recoil(&shape, &mut instance, 3);
    assert!(!anim.recoiling(&instance));
    anim.recoil(&shape, &mut instance, 1);
    assert!(anim.recoiling(&instance));
    anim.update(&shape, &mut instance, &still, 10.0);
    assert!(!anim.recoiling(&instance), "played to its end");
    // A shape without one has no recoil thread.
    let plain = self::shape();
    let mut instance = ShapeInstance::new(&plain, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&plain, &mut instance).unwrap();
    anim.recoil(&plain, &mut instance, 1);
    assert!(!anim.recoiling(&instance));
}

#[test]
fn getting_off_a_mount_lets_the_moves_pick_again_and_the_next_mount_sets_its_action_anew() {
    let shape = moving_shape();
    let hold = action_list(&shape)
        .iter()
        .position(|a| a.name == b"hold")
        .unwrap();
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let seated = PlayerMotion {
        server_action: Some(ServerAction {
            action: hold as u32,
            hold: true,
            at_end: false,
        }),
        mounted: true,
        ..PlayerMotion::default()
    };
    anim.update(&shape, &mut instance, &seated, 0.5);
    assert_eq!(anim.action(), (hold, true));
    // Off the mount: the record still names the held action, the figure stands and runs.
    let off = PlayerMotion {
        mounted: false,
        ..seated
    };
    anim.update(&shape, &mut instance, &off, 0.032);
    assert_eq!(anim.action(), (action::ROOT, true));
    assert!(!anim.held());
    let running = PlayerMotion {
        velocity: [0.0, 4.0, 0.0],
        ..off
    };
    anim.update(&shape, &mut instance, &running, 0.032);
    assert_eq!(anim.action(), (action::FIRST_MOVE, true));
    // On again with the same record: the held action plays again.
    anim.update(&shape, &mut instance, &seated, 0.032);
    assert_eq!(anim.action(), (hold, true));
}

/// A shape whose one leg node the gaits swing about x, each furthest forward a quarter into its loop and carrying the shape
/// along y: `run` 45 degrees each way over 0.6 seconds at 5 units a second, `back` 30 degrees over 0.7 seconds at 4 backward,
/// and (`walk` true) `walk` 20 degrees over 0.8 seconds at 2.
fn gait_shape(walk: bool) -> Shape {
    use torque_shape_builder::{NO_TURN, SequenceDef, ShapeBuilder, Track, turn};
    const N: usize = 16;
    let mut b = ShapeBuilder::new();
    let base = b.node("base", None, [0.0; 3]);
    let leg = b.node("leg", Some(base), [0.0, 0.0, 1.0]);
    b.detail("detail1", 1.0);
    let gait = |name, duration: f32, degrees: f32, speed: f32| SequenceDef {
        name,
        duration,
        cyclic: true,
        priority: 0,
        keyframes: N,
        rotations: vec![Track {
            node: leg,
            keys: (0..N)
                .map(|k| {
                    turn(
                        [1.0, 0.0, 0.0],
                        degrees * (std::f32::consts::TAU * k as f32 / N as f32).sin(),
                    )
                })
                .collect(),
        }],
        translations: Vec::new(),
        ground: (1..=N)
            .map(|k| [0.0, speed * duration * k as f32 / N as f32, 0.0])
            .collect(),
    };
    b.sequence(SequenceDef {
        name: "root",
        duration: 1.0,
        cyclic: true,
        priority: 0,
        keyframes: 1,
        rotations: vec![Track {
            node: leg,
            keys: vec![NO_TURN],
        }],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    b.sequence(gait("run", 0.6, 45.0, 5.0));
    b.sequence(gait("back", 0.7, 30.0, -4.0));
    if walk {
        b.sequence(gait("walk", 0.8, 20.0, 2.0));
    }
    Shape::for_server(b.build())
}

/// The leg's swing (degrees forward) as the instance is posed now.
fn leg_swing(shape: &Shape, instance: &mut ShapeInstance) -> f32 {
    instance.animate(shape, 0, None, &mut |_| true).unwrap();
    let n = shape.find_name(b"leg").unwrap();
    let node = shape.nodes.iter().position(|x| x.name == n).unwrap();
    let foot = shapes::math::mul_p(&instance.node_transforms()[node], [0.0, 0.0, -1.0]);
    foot[1].atan2(1.0 - foot[2]).to_degrees()
}

fn moving(speed: f32) -> PlayerMotion {
    PlayerMotion {
        velocity: [0.0, speed, 0.0],
        ..PlayerMotion::default()
    }
}

#[test]
fn walking_pace_walks_forward_and_backward_and_running_pace_runs() {
    let shape = gait_shape(true);
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let walk = anim.actions().iter().position(|a| a.name == WALK).unwrap();
    assert_eq!(anim.walk, Some(walk));
    assert!((walk_limit(2.0, 5.0) - 10f32.sqrt()).abs() < 1e-6);
    for (speed, want, name) in [
        (2.1, (walk, true), "a walking pace ahead walks"),
        (6.0, (1, true), "a running pace runs"),
        (
            -1.4,
            (walk, false),
            "a walking pace backward walks backward",
        ),
        (-4.0, (2, true), "a fast pace backward plays back"),
        (2.1, (walk, true), "and back to the walk"),
    ] {
        for _ in 0..20 {
            anim.update(&shape, &mut instance, &moving(speed), 0.016);
        }
        assert_eq!(anim.action(), want, "{name}");
        let seq = sequence_of(&shape, &instance, anim.action_thread);
        assert_eq!(
            seq.as_deref(),
            Some(&anim.actions()[want.0].name[..]),
            "{name}"
        );
    }
    // The walk's rate is the speed over its own, past maxTimeScale while under the limit.
    anim.update(&shape, &mut instance, &moving(3.2), 0.016);
    assert_eq!(anim.action(), (walk, true));
    let scale = instance.thread(anim.action_thread).unwrap().time_scale;
    assert!((scale - 1.6).abs() < 1e-4, "{scale}");

    // Without a walk sequence: TGE's run at any pace, and back.
    let shape = gait_shape(false);
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    assert_eq!(anim.walk, None);
    for (speed, want) in [(2.1, (1, true)), (-1.4, (2, true))] {
        anim.update(&shape, &mut instance, &moving(speed), 0.016);
        assert_eq!(anim.action(), want, "{speed}");
    }
    // Synced: the server walks by the same rule, so the walk.
    let shape = gait_shape(true);
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    anim.set_synced(true);
    anim.update(&shape, &mut instance, &moving(2.1), 0.016);
    assert_eq!(anim.action(), (anim.walk.unwrap(), true));
    // The record carries the walk the synced server picked (an action past the table): the client picks it itself, so the
    // record changes nothing, and a run picks the run.
    let walk = anim.walk.unwrap();
    let carried = PlayerMotion {
        server_action: Some(ServerAction {
            action: walk as u32,
            hold: false,
            at_end: false,
        }),
        ..moving(6.0)
    };
    anim.update(&shape, &mut instance, &carried, 0.016);
    assert_eq!(anim.action(), (1, true));
}

#[test]
fn speeding_up_changes_walk_to_run_past_the_limit_in_step_and_without_a_pop() {
    let shape = gait_shape(true);
    let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
    let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
    let walk = anim.walk.unwrap();
    let dt = 1.0 / 60.0;
    let limit = walk_limit(2.0, 5.0);
    // The fastest the leg turns in a frame running flat out.
    let mut last = leg_swing(&shape, &mut instance);
    let mut steady = 0.0f32;
    for k in 0..120 {
        anim.update(&shape, &mut instance, &moving(5.0), dt);
        let now = leg_swing(&shape, &mut instance);
        if k > 30 {
            steady = steady.max((now - last).abs());
        }
        last = now;
    }
    // Walk, then speed up from 2 to 5 over 3 seconds and back down over 3.
    for _ in 0..60 {
        anim.update(&shape, &mut instance, &moving(2.0), dt);
    }
    assert_eq!(anim.action(), (walk, true));
    let mut last = leg_swing(&shape, &mut instance);
    let (mut up, mut down, mut worst) = (None, None, 0.0f32);
    for k in 0..360 {
        let speed = if k < 180 {
            2.0 + 3.0 * k as f32 / 180.0
        } else {
            5.0 - 3.0 * (k - 180) as f32 / 180.0
        };
        let (before, pos) = (
            anim.action().0,
            instance.thread(anim.action_thread).unwrap().pos,
        );
        anim.update(&shape, &mut instance, &moving(speed), dt);
        let after = anim.action().0;
        if before == walk && after == 1 {
            assert!(up.is_none(), "one change up");
            up = Some(speed);
            // The run takes up the walk's place in the loop (one frame on).
            let now = instance.thread(anim.action_thread).unwrap().pos;
            assert!((now - pos).rem_euclid(1.0) < 0.05, "{pos} {now}");
        } else if before == 1 && after == walk {
            assert!(down.is_none(), "one change down");
            down = Some(speed);
        }
        let now = leg_swing(&shape, &mut instance);
        worst = worst.max((now - last).abs());
        last = now;
    }
    let (up, down) = (up.unwrap(), down.unwrap());
    assert!(
        up >= limit * (1.0 + WALK_MARGIN) && up < limit * (1.0 + WALK_MARGIN) + 0.03,
        "up at {up}"
    );
    assert!(
        down < limit * (1.0 - WALK_MARGIN) && down > limit * (1.0 - WALK_MARGIN) - 0.03,
        "down at {down}"
    );
    assert!(
        worst <= steady * 1.05,
        "no frame turns the leg faster than running flat out: {worst} > {steady}"
    );

    // A pace wavering at the limit does not flip between them.
    let mut changes = 0;
    let mut was = anim.action();
    for k in 0..240 {
        let speed = limit * (1.0 + 0.03 * (k as f32 * 0.3).sin());
        anim.update(&shape, &mut instance, &moving(speed), dt);
        changes += usize::from(anim.action() != was);
        was = anim.action();
    }
    assert_eq!(changes, 0);
}
