use super::*;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-6
}

#[test]
fn without_an_install_the_table_is_the_plain_one() {
    assert!(std::ptr::eq(get(), &PLAIN));
}

#[test]
fn the_camera_pose_turns_by_yaw_then_pitch_and_reads_back() {
    let m = (PLAIN.camera_pose)([1.0, 2.0, 3.0], 0.5, -1.25);
    assert_eq!((m[3], m[7], m[11]), (1.0, 2.0, 3.0));
    let (pitch, yaw) = (PLAIN.camera_rotation)(&m);
    assert!(close(pitch, 0.5) && close(yaw, -1.25), "{pitch} {yaw}");
    // No rotation: the identity with the position.
    let flat = (PLAIN.camera_pose)([0.0; 3], 0.0, 0.0);
    assert_eq!(flat, IDENTITY);
}

#[test]
fn the_plain_rules_are_tges() {
    assert_eq!((PLAIN.set_velocity)([500.0, 0.0, 0.0]), [500.0, 0.0, 0.0]);
    assert!((PLAIN.moving)([0.0; 3]));
    assert_eq!((PLAIN.plane_offset)([1.0, 2.0, 3.0], [0.0, 0.0, 1.0]), -3.0);
    assert_eq!((PLAIN.difference)(0.75, 0.25), 0.5);
}

#[test]
fn ground_frames_split_a_position_into_a_frame_and_a_fraction() {
    assert_eq!((PLAIN.ground_frame)(0.0, 10), (0, 0.0));
    let (frame, k) = (PLAIN.ground_frame)(0.55, 10);
    assert_eq!(frame, 5);
    assert!((k - 0.5).abs() < 1e-4, "{k}");
    // A position of exactly 1 stays inside the last pair.
    assert_eq!((PLAIN.ground_frame)(1.0, 10).0, 9);
    assert_eq!((PLAIN.ground_lerp)(2.0, 4.0, 0.25), 2.5);
}

#[test]
fn ground_motion_is_the_unit_direction_and_the_speed() {
    let (dir, speed) = (PLAIN.ground_motion)([3.0, 0.0, 4.0]).unwrap();
    assert_eq!(speed, 5.0);
    assert!(close(dir[0], 0.6) && close(dir[2], 0.8));
    assert_eq!((PLAIN.ground_motion)([0.001, 0.0, 0.0]), None);
}

#[test]
fn action_rates_fades_and_boxes() {
    assert_eq!((PLAIN.action_dot)([1.0, 2.0, 0.0], [0.0, 1.0, 0.0]), 2.0);
    assert_eq!(
        (PLAIN.action_rate)([0.0, -4.0, 0.0], [0.0, 1.0, 0.0], 2.0),
        2.0
    );
    assert!((PLAIN.fade_over)(1.0, 0.5, 1.75));
    assert!(!(PLAIN.fade_over)(1.0, 0.5, 1.25));
    assert!(!(PLAIN.box_differs)(
        [-0.5, -0.5, 0.0],
        [0.5, 0.5, 2.0],
        [1.0, 1.0, 2.05]
    ));
    assert!((PLAIN.box_differs)(
        [-0.5, -0.5, 0.0],
        [0.5, 0.5, 2.0],
        [1.0, 1.0, 2.5]
    ));
}
