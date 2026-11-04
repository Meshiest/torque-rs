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
    // Moving along -x while facing +x: the shape has no `back`, so `run` plays backwards.
    let back = PlayerMotion {
        velocity: [-3.0, 0.0, 0.0],
        ..ahead
    };
    anim.update(&shape, &mut instance, &back, 0.032);
    assert_eq!(anim.action(), (1, false));
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
    // The shape has no `standjump`: nothing changes.
    assert_eq!(other.action(), (action::ROOT, true));
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
