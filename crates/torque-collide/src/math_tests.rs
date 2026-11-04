use super::*;

#[test]
fn the_matrix_product_rounds_each_step_to_a_float() {
    // 1 + 2^-24 is not a float: the f32 sum loses it.
    let mut a = IDENTITY;
    a[0] = 1.0;
    a[1] = 1.0;
    let mut b = IDENTITY;
    b[0] = 1.0;
    b[4] = f32::EPSILON / 2.0;
    b[5] = 1.0;
    let m = matrix_product(&a, &b);
    assert_eq!(m[0], 1.0);
    assert_eq!(matrix_product(&IDENTITY, &a), a);
}

#[test]
fn float_to_int_truncates_and_saturates() {
    assert_eq!(float_to_int(2.9), 2);
    assert_eq!(float_to_int(-2.9), -2);
    assert_eq!(float_to_int(f64::NAN), 0);
    assert_eq!(float_to_int(3e9), i32::MAX);
    assert_eq!(float_to_int(-3e9), i32::MIN);
    assert_eq!(float_to_int(-2_147_483_648.5), -2_147_483_648);
}

#[test]
fn a_box_moved_by_a_quarter_turn_swaps_its_axes() {
    let mut m = IDENTITY;
    m[0] = 0.0;
    m[1] = -1.0;
    m[4] = 1.0;
    m[5] = 0.0;
    m[3] = 10.0;
    let b = mul_box(&m, &Box3::new([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]));
    assert_eq!(b, Box3::new([8.0, 0.0, 0.0], [10.0, 1.0, 3.0]));
}

#[test]
fn a_plane_from_three_points_faces_by_the_right_hand_rule() {
    let p = plane_from_points([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    // a = (-1, 0, 0), b = (-1, 1, 0): n = (0, 0, -1), d = 0.
    assert_eq!(p, Plane::new(0.0, 0.0, -1.0, 0.0));
    let degenerate = plane_from_points([1.0; 3], [1.0; 3], [1.0; 3]);
    assert_eq!(degenerate, Plane::new(0.0, 0.0, 1.0, -1.0));
}

#[test]
fn a_plane_transformer_moves_and_scales_a_plane() {
    let mut m = IDENTITY;
    m[11] = 5.0;
    let t = PlaneTransformer::new(&m, [1.0, 1.0, 2.0]);
    // z = 1 in object space: scaled to z = 2, moved to z = 7.
    let p = t.transform(&Plane::new(0.0, 0.0, 1.0, -1.0));
    assert_eq!(p, Plane::new(0.0, 0.0, 1.0, -7.0));
}

#[test]
fn the_inverse_undoes_a_move_and_a_turn() {
    let mut m = IDENTITY;
    m[0] = 0.0;
    m[1] = -1.0;
    m[4] = 1.0;
    m[5] = 0.0;
    m[3] = 3.0;
    m[7] = -2.0;
    let p = mul_p(&inverse(&m), mul_p(&m, [1.0, 2.0, 3.0]));
    assert_eq!(p, [1.0, 2.0, 3.0]);
}

#[test]
fn the_plain_routines_follow_the_source_order_in_f32() {
    // (1 + 2^-23) * (1 - 2^-24) summed in f32 rounds each product: the source's order, plain floats.
    let a = [1.0 + f32::EPSILON, 1.0, 1.0];
    let b = [1.0 - f32::EPSILON / 2.0, -1.0, 0.0];
    let expected = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    assert_eq!(dot(a, b), expected);
    assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
    assert_eq!(determinant(&IDENTITY), 1.0);
    let mut m = IDENTITY;
    m[3] = 4.0;
    assert_eq!(affine_inverse(&m)[3], -4.0);
    assert_eq!(mul_v(&m, [1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
}

#[test]
fn is_zero_takes_each_square_against_the_point_epsilon() {
    assert!(is_zero([0.01, -0.01, 0.0]));
    assert!(!is_zero([0.0, 0.0, 0.011]));
    assert!(!is_zero([f32::NAN, 0.0, 0.0]));
}

#[test]
fn normalize_and_the_quaternion_matrix_have_their_identity_cases() {
    assert_eq!(normalize([0.0; 3]), [0.0, 0.0, 1.0]);
    assert_eq!(normalize([0.0, 3.0, 0.0]), [0.0, 1.0, 0.0]);
    assert_eq!(quat_matrix([0.0, 0.0, 0.0, 1.0]), IDENTITY);
    // A half turn about z: x goes to -x.
    let m = quat_matrix([0.0, 0.0, 1.0, 0.0]);
    assert_eq!(mul_v(&m, [1.0, 0.0, 0.0]), [-1.0, 0.0, 0.0]);
}

#[test]
fn set_euler_turns_about_z_alone_and_mixes_axes() {
    let m = set_euler([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
    let v = mul_v(&m, [1.0, 0.0, 0.0]);
    // Torque's rows: x goes to -y.
    assert!(v[0].abs() < 1e-6 && (v[1] + 1.0).abs() < 1e-6, "{v:?}");
    assert_eq!(set_euler([0.0; 3]), IDENTITY);
    let m = set_euler([0.3, 0.2, 0.1]);
    let r = matrix_product(&m, &transpose(&m));
    for (i, x) in r.iter().enumerate() {
        assert!((x - IDENTITY[i]).abs() < 1e-5, "{r:?}");
    }
}

#[test]
fn interpolate_weighs_both_ends() {
    assert_eq!(
        interpolate([0.0; 3], [2.0, 4.0, -8.0], 0.5),
        [1.0, 2.0, -4.0]
    );
}
