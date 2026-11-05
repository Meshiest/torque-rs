use super::*;

#[test]
fn a_wheel_spins_by_its_rate_and_stays_in_one_turn() {
    let tau = std::f32::consts::TAU;
    assert!((spin(0.0, tau, 0.25) - 0.25).abs() < 1e-6);
    assert!((spin(0.9, tau, 0.25) - 0.15).abs() < 1e-5);
    assert!((spin(0.1, -tau, 0.25) - 0.85).abs() < 1e-5);
    assert_eq!(spin(0.5, f32::NAN, 0.1), 0.0);
}

#[test]
fn a_flying_ghost_warps_to_its_update_then_flies_on() {
    let w = FlyingWarp {
        from: ([0.0; 3], [0.0, 0.0, 0.0, 1.0]),
        to: ([2.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
        velocity: [10.0, 0.0, 0.0],
        at_ms: 1_000,
        warp_ms: 64.0,
    };
    assert_eq!(w.pose(1_000).0, [0.0; 3]);
    assert!((w.pose(1_032).0[0] - 1.0).abs() < 1e-5);
    assert!((w.pose(1_064).0[0] - 2.0).abs() < 1e-5);
    assert!((w.pose(1_164).0[0] - 3.0).abs() < 1e-4);
    // Never past the prediction's ticks.
    assert!((w.pose(60_000).0[0] - (2.0 + 10.0 * 0.96)).abs() < 1e-3);
}

#[test]
fn the_drawn_steering_ends_at_the_same_place_in_one_frame_or_in_many() {
    let start = Eased {
        value: 0.1,
        rate: -0.5,
    };
    let one = start.follow(0.4, STEER_EASE_SECONDS, 0.1);
    let mut many = start;
    for _ in 0..10 {
        many = many.follow(0.4, STEER_EASE_SECONDS, 0.01);
    }
    assert!((one.value - many.value).abs() < 1e-5, "{one:?} {many:?}");
    assert!((one.rate - many.rate).abs() < 1e-3, "{one:?} {many:?}");
}

#[test]
fn the_drawn_steering_eases_in_and_back_to_the_middle_without_passing_it() {
    // To a full lock and back, at 60 frames a second: it never passes where it goes, and settles there.
    let mut e = Eased::default();
    for target in [0.55f32, 0.0] {
        let from = e.value;
        for _ in 0..30 {
            e = e.follow(target, STEER_EASE_SECONDS, 1.0 / 60.0);
            assert!(
                (e.value - from) * (target - from) >= 0.0
                    && (target - e.value) * (target - from) >= -1e-6,
                "{e:?} from {from} to {target}"
            );
        }
        assert!((e.value - target).abs() < 1e-3, "{e:?} for {target}");
    }
}

#[test]
fn the_drawn_steering_moves_evenly_frame_to_frame_while_the_ticks_step_it() {
    // The steering the ticks set: a step every 32 ms to the lock; drawn every 16 ms. Drawn as the ticks leave it, a frame
    // moves by a whole step or not at all; eased, one frame's move differs from the next's by a small part of a step.
    let step = 0.044f32;
    let mut e = Eased::default();
    let mut last = (0.0f32, None::<f32>);
    let mut worst = 0.0f32;
    for frame in 0..60 {
        let target = (step * (frame / 2 + 1) as f32).min(0.55);
        e = e.follow(target, STEER_EASE_SECONDS, 0.016);
        let moved = e.value - last.0;
        if let Some(prev) = last.1 {
            worst = worst.max((moved - prev).abs());
        }
        last = (e.value, Some(moved));
    }
    assert!(worst < step * 0.25, "a frame's move changed by {worst}");
}

#[test]
fn a_networked_steering_glides_between_its_updates_and_is_drawn_moving_evenly() {
    // Another's turn: its updates bring a step of 0.13 every 96 ms; drawn every 16 ms through the glide and the follow, one
    // frame's move differs from the next's by a small part of a step once it is under way.
    let step = 0.13f32;
    let mut g = Glide::at_rest(0.0, 0);
    let mut e = Eased::default();
    let mut moves = Vec::new();
    let mut last = 0.0f32;
    for frame in 1..=60u32 {
        let now = frame * 16;
        let value = (step * (now / 96) as f32).min(0.55);
        g = g.take(value, now);
        e = e.follow(g.at(now), STEER_EASE_SECONDS, 0.016);
        moves.push(e.value - last);
        last = e.value;
    }
    let steady = &moves[14..28];
    let worst = steady
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < step * 0.06,
        "a frame's move changed by {worst}: {steady:?}"
    );
    // It ends at the last value, without passing it.
    assert!((e.value - 0.55).abs() < 0.01, "{e:?}");
    assert!(moves.iter().all(|m| *m >= -1e-6), "{moves:?}");
}
