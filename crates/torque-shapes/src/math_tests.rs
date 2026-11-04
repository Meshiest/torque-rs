use super::*;

fn close(a: &Matrix, b: &Matrix) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

#[test]
fn multiplying_by_the_identity_changes_nothing() {
    let m: Matrix = [
        0.5, -0.25, 3.0, 7.0, 1.5, 2.0, -1.0, 0.125, 0.0, 9.0, 1.0, -4.0, 0.0, 0.0, 0.0, 1.0,
    ];
    assert_eq!(mat_mul(&m, &IDENTITY), m);
    assert_eq!(mat_mul(&IDENTITY, &m), m);
}

#[test]
fn a_rotation_and_its_inverse_make_the_identity() {
    let q = [0, 0, 23169, 23169];
    let m = node_matrix(q, [2.0, -3.0, 0.5]);
    let product = mat_mul(&m, &mat_inverse(&m));
    assert!(close(&product, &IDENTITY), "{product:?}");
}

#[test]
fn a_quarter_turn_about_z_sends_x_to_minus_y_for_a_positive_vector_part() {
    // The shorts 0, 0, 23169, 23169 are 0.70710..., not exactly the root of one half: close to a quarter turn.
    let m = node_matrix([0, 0, 23169, 23169], [0.0; 3]);
    let p = mul_p(&m, [1.0, 0.0, 0.0]);
    assert!(
        p[0].abs() < 1e-4 && (p[1] + 1.0).abs() < 1e-4 && p[2] == 0.0,
        "{p:?}"
    );
    let m = node_matrix([0, 0, -23169, 23169], [0.0; 3]);
    let p = mul_p(&m, [1.0, 0.0, 0.0]);
    assert!(
        p[0].abs() < 1e-4 && (p[1] - 1.0).abs() < 1e-4 && p[2] == 0.0,
        "{p:?}"
    );
    assert_eq!(node_matrix([0, 0, 0, 32767], [1.0, 2.0, 3.0])[3], 1.0);
}

#[test]
fn a_tiny_quaternion_is_the_identity() {
    assert_eq!(quat_matrix([0.0, 0.0, 0.0, 1.0]), IDENTITY);
}

#[test]
fn normalize_has_the_zero_and_unit_cases() {
    assert_eq!(normalize([0.0, 0.0, 0.0]), [0.0, 0.0, 1.0]);
    assert_eq!(normalize([0.0, 0.0, -2.0]), [0.0, 0.0, -1.0]);
    let n = normalize([1.0, 1.0, 0.0]);
    assert!((n[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6 && n[0] == n[1]);
}

#[test]
fn the_cross_product_is_p3_minus_p1_by_p2_minus_p1() {
    let (c, squared) = triangle_cross(&[0.0; 3], &[1.0, 0.0, 0.0], &[0.0, 1.0, 0.0]);
    // mCross(q3 - base, q2 - base) = (0, 1, 0) x (1, 0, 0): the normal of a counter-clockwise triangle points the other way.
    assert_eq!(c, [0.0, 0.0, -1.0]);
    assert_eq!(squared, 1.0);
}

#[test]
fn the_plain_inverse_and_product_use_float_arithmetic() {
    let m = node_matrix([0, 0, 12540, 30274], [1.0, -2.0, 0.25]);
    let i = mat_inverse(&m);
    let p = mul_p(&i, mul_p(&m, [0.5, 0.25, -1.0]));
    for (a, b) in p.iter().zip([0.5, 0.25, -1.0]) {
        assert!((a - b).abs() < 1e-6, "{p:?}");
    }
    assert_eq!(quat16([0, 0, 0, 32767]), [0.0, 0.0, 0.0, 1.0]);
}
