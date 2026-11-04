use super::*;

#[test]
fn the_name_list_keeps_arrival_order_and_swap_removes() {
    let mut l = NameList::default();
    l.update(&[5, 2, 9]);
    assert_eq!(l.order, vec![5, 2, 9]);
    // 5 loses its name: the last moves into its place; 7 arrives at the end.
    l.update(&[2, 9, 7]);
    assert_eq!(l.order, vec![9, 2, 7]);
}

#[test]
fn the_camera_rotation_is_euler_z_times_euler_x() {
    let m = rotation(0.0, std::f32::consts::FRAC_PI_2);
    // Forward (column 1) of a quarter turn left.
    assert!((m[0][1] - 1.0).abs() < 1e-6 && m[1][1].abs() < 1e-6);
    let m = rotation(0.3, 0.0);
    assert!((m[2][1] - (-0.3f32).sin()).abs() < 1e-6 || (m[2][1] - 0.3f32.sin()).abs() < 1e-6);
}
