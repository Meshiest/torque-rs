use ghosts::F32;

use super::*;

fn at(x: f32, yaw: f32) -> Pose {
    let half = yaw * 0.5;
    Pose::Affine {
        xf: Xf {
            position: [F32::new(x), F32::new(0.0), F32::new(1.0)],
            quat: [F32::new(0.0), F32::new(0.0), F32::new(half.sin())],
            negative_w: false,
        },
        scale: None,
    }
}

fn place(p: Pose) -> ([f32; 3], [f32; 4]) {
    match p {
        Pose::Quat { position, rotation } => (position, rotation),
        Pose::Affine { xf, .. } => parts(&xf),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_ghost_first_seen_is_drawn_exactly_where_its_record_puts_it() {
    let warps = PoseWarps::default();
    for now in [0, 10, 5000] {
        assert_eq!(warps.pose(1, 1, 0, at(2.0, 0.5), now), at(2.0, 0.5));
    }
}

#[test]
fn an_update_is_blended_in_over_the_time_since_the_last_one_and_then_held() {
    let warps = PoseWarps::default();
    let _ = warps.pose(1, 1, 0, at(0.0, 0.0), 1000);
    // An update 64 ms later (two ticks): the warp lasts two ticks.
    let update = at(4.0, 0.0);
    let (p, _) = place(warps.pose(1, 1, 1, update, 1064));
    assert_eq!(p[0], 0.0);
    let (p, _) = place(warps.pose(1, 1, 1, update, 1064 + 32));
    assert!((p[0] - 2.0).abs() < 1e-4, "{p:?}");
    // The warp is over: the record's own pose, with no rounding.
    assert_eq!(warps.pose(1, 1, 1, update, 1064 + 64), update);
    assert_eq!(warps.pose(1, 1, 1, update, 9000), update);
}

#[test]
fn the_warp_lasts_a_tick_to_three_whatever_the_time_between_updates() {
    let warps = PoseWarps::default();
    let _ = warps.pose(1, 1, 0, at(0.0, 0.0), 0);
    // Right after: a tick.
    let _ = warps.pose(1, 1, 1, at(1.0, 0.0), 1);
    assert_eq!(warps.pose(1, 1, 1, at(1.0, 0.0), 1 + 32), at(1.0, 0.0));
    // Long after: three ticks.
    let _ = warps.pose(1, 1, 2, at(5.0, 0.0), 10_000);
    let (p, _) = place(warps.pose(1, 1, 2, at(5.0, 0.0), 10_000 + 48));
    assert!((p[0] - 3.0).abs() < 1e-4, "{p:?}");
    assert_eq!(warps.pose(1, 1, 2, at(5.0, 0.0), 10_000 + 96), at(5.0, 0.0));
}

#[test]
fn a_new_update_starts_from_where_the_last_frame_drew_the_ghost() {
    let warps = PoseWarps::default();
    let _ = warps.pose(1, 1, 0, at(0.0, 0.0), 0);
    let _ = warps.pose(1, 1, 1, at(4.0, 0.0), 32);
    // Half way there when the next update comes.
    let mid = place(warps.pose(1, 1, 1, at(4.0, 0.0), 48)).0[0];
    assert!((mid - 2.0).abs() < 1e-4);
    // The next update (the same place again) starts from there, not from the old update's end.
    let (p, _) = place(warps.pose(1, 1, 2, at(4.0, 0.0), 48));
    assert!((p[0] - 2.0).abs() < 1e-4, "{p:?}");
    // And a third, back at the start, from where that was drawn a half warp on (3, a half of 32 ms into 2 to 4).
    let (p, _) = place(warps.pose(1, 1, 3, at(0.0, 0.0), 64));
    assert!((p[0] - 3.0).abs() < 1e-4, "{p:?}");
}

#[test]
fn asking_twice_at_one_time_gives_one_answer() {
    let warps = PoseWarps::default();
    let _ = warps.pose(1, 1, 0, at(0.0, 0.0), 0);
    let a = warps.pose(1, 1, 1, at(4.0, 0.0), 32 + 16);
    let b = warps.pose(1, 1, 1, at(4.0, 0.0), 32 + 16);
    assert_eq!(a, b);
}

#[test]
fn a_turn_is_blended_the_short_way_round() {
    let warps = PoseWarps::default();
    let _ = warps.pose(1, 1, 0, at(0.0, 3.0), 0);
    let _ = warps.pose(1, 1, 1, at(0.0, -3.0), 32);
    let (_, q) = place(warps.pose(1, 1, 1, at(0.0, -3.0), 48));
    // Three radians to minus three is 0.28 radians the short way round, through half a turn: its z stays near the pole.
    let yaw = 2.0 * q[2].atan2(q[3]);
    assert!(yaw.abs() > 2.9, "{yaw}");
    let len: f32 = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    assert!((len - 1.0).abs() < 1e-4);
}

#[test]
fn a_ghost_made_again_on_its_index_starts_over() {
    let warps = PoseWarps::default();
    let _ = warps.pose(1, 1, 0, at(0.0, 0.0), 0);
    let _ = warps.pose(1, 1, 1, at(4.0, 0.0), 32);
    assert_ne!(warps.pose(1, 1, 1, at(4.0, 0.0), 40), at(4.0, 0.0));
    assert_eq!(warps.pose(1, 2, 0, at(9.0, 0.0), 41), at(9.0, 0.0));
}

#[test]
fn poses_that_are_not_transform_records_pass_through_and_hostile_ones_are_never_blended() {
    let warps = PoseWarps::default();
    let euler = Pose::Euler {
        position: [1.0, 2.0, 3.0],
        pitch: 0.0,
        yaw: 1.0,
    };
    assert_eq!(warps.pose(1, 1, 0, euler, 0), euler);
    assert_eq!(warps.pose(1, 1, 1, euler, 50), euler);
    assert!(warps.is_empty());
    let nan = at(f32::NAN, 0.0);
    assert_eq!(
        format!("{:?}", warps.pose(2, 1, 0, nan, 0)),
        format!("{nan:?}")
    );
    assert!(warps.is_empty());
}

#[test]
fn forgotten_ghosts_leave_no_warp_behind() {
    let warps = PoseWarps::default();
    for index in 0..8 {
        let _ = warps.pose(index, 1, 0, at(0.0, 0.0), 0);
    }
    assert_eq!(warps.len(), 8);
    warps.retain(|i| i < 3);
    assert_eq!(warps.len(), 3);
}
