use super::*;

/// A Player walking along y at `speed` and turning at `turn` radians a second, sent every `every` seconds and read a tick later,
/// drawn every `frame` seconds: the largest step a frame makes, the largest backward step and the largest turn.
fn run(speed: f32, turn: f32, every: f32, frame: f32) -> (f32, f32, f32) {
    let at = |t: f32| Update {
        position: [0.0, speed * t, 0.0],
        velocity: [0.0, speed, 0.0],
        yaw: wrap_angle(turn * t),
        head: 0.0,
    };
    let mut m = GhostMotion::new(at(0.0));
    let (mut t, mut next) = (0.0f32, every);
    let (mut max_step, mut max_back, mut max_turn) = (0.0f32, 0.0f32, 0.0f32);
    let mut last = (m.position(), m.yaw());
    while t < 5.0 {
        t += frame;
        m.advance(frame);
        if t >= next {
            m.receive(at(next - TICK));
            next += every;
        }
        let (p, y) = (m.position(), m.yaw());
        max_step = max_step.max(Vec3::from(p).distance(last.0.into()));
        max_back = max_back.max(last.0[1] - p[1]);
        max_turn = max_turn.max(wrap_angle(y - last.1).abs());
        last = (p, y);
    }
    (max_step, max_back, max_turn)
}

#[test]
fn a_steady_player_moves_and_turns_steadily() {
    for (every, frame) in [(0.1, 0.007), (0.096, 0.016), (0.032, 0.007), (0.1, 0.033)] {
        let (step, back, turn) = run(6.0, 2.0, every, frame);
        assert!(back <= 1e-4, "{every} {frame}: a backward step of {back}");
        assert!(
            step <= 6.0 * frame * 1.6 + 1e-3,
            "{every} {frame}: a step of {step}"
        );
        assert!(
            turn <= 2.0 * frame * 1.6 + 1e-3,
            "{every} {frame}: a turn of {turn}"
        );
    }
}

#[test]
fn a_far_update_is_a_respawn_and_a_stopped_player_stays() {
    let still = Update {
        position: [0.0; 3],
        velocity: [0.0, 5.0, 0.0],
        yaw: 0.0,
        head: 0.0,
    };
    let mut m = GhostMotion::new(still);
    for _ in 0..100 {
        m.advance(0.016);
    }
    assert!(m.position()[1] < 5.0 * (FIRST_INTERVAL + MAX_EXTRAPOLATION) + 1e-3);
    assert_eq!(m.velocity(), [0.0; 3]);
    m.receive(Update {
        position: [10.0, 0.0, 0.0],
        ..still
    });
    assert_eq!(m.position(), [10.0, 0.0, 0.0]);
    // A position that is not a number is a respawn too, and a time that is not one moves nothing.
    m.receive(Update {
        position: [f32::NAN, 0.0, 0.0],
        ..still
    });
    m.advance(f32::NAN);
}

#[test]
fn the_yaw_warps_the_short_way_round() {
    let u = Update {
        position: [0.0; 3],
        velocity: [0.0; 3],
        yaw: 3.0,
        head: 0.0,
    };
    let mut m = GhostMotion::new(u);
    m.receive(Update { yaw: -3.0, ..u });
    m.advance(0.05);
    assert!(m.yaw() > 3.0 || m.yaw() < -3.0, "{}", m.yaw());
    m.advance(MAX_WARP);
    assert!(wrap_angle(m.yaw() + 3.0).abs() < 1e-4, "{}", m.yaw());
}

#[test]
fn a_landing_player_is_not_run_on_through_the_floor() {
    let falling = Update {
        position: [0.0, 0.0, 0.01],
        velocity: [0.0, 6.0, -7.0],
        yaw: 0.0,
        head: 0.0,
    };
    let mut m = GhostMotion::new(Update {
        position: [0.0, -0.2, 0.25],
        ..falling
    });
    m.advance(0.03);
    m.receive(falling);
    for _ in 0..20 {
        m.advance(0.016);
        assert!(m.position()[2] >= 0.01 - 1e-6, "{:?}", m.position());
    }
    assert!(m.position()[1] > 0.5, "still runs on along the ground");
}
