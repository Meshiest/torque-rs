use super::matrix::{self, IDENTITY};
use super::{all_finite, finite_clamp, finite_coords, quat, vec3};

#[test]
fn the_guards_refuse_what_is_not_finite_and_hold_the_rest() {
    assert_eq!(finite_clamp(f32::NAN, 0.0, 1.0), None);
    assert_eq!(finite_clamp(f32::INFINITY, 0.0, 1.0), None);
    assert_eq!(finite_clamp(5.0, 0.0, 1.0), Some(1.0));
    assert_eq!(finite_clamp(-5.0, 0.0, 1.0), Some(0.0));
    assert_eq!(finite_coords([f32::NAN, 0.0, 0.0], 10.0), None);
    assert_eq!(
        finite_coords([1e30, -1e30, 3.0], 10.0),
        Some([10.0, -10.0, 3.0])
    );
    assert!(all_finite(&[0.0, -1.0, f32::MAX]));
    assert!(!all_finite(&[0.0, f32::NEG_INFINITY]));
}

#[test]
fn the_vector_forms_agree_on_exact_values() {
    let (a, b) = ([1.0, 2.0, 3.0], [4.0, -5.0, 6.0]);
    assert_eq!(vec3::add(a, b), [5.0, -3.0, 9.0]);
    assert_eq!(vec3::sub(a, b), [-3.0, 7.0, -3.0]);
    assert_eq!(vec3::add_scaled(a, b, 2.0), [9.0, -8.0, 15.0]);
    assert_eq!(vec3::dot(a, b), 12.0);
    assert_eq!(
        vec3::cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        [0.0, 0.0, 1.0]
    );
    assert_eq!(vec3::length([3.0, 4.0, 0.0]), 5.0);
    assert_eq!(vec3::normalize([0.0; 3]), [0.0, 0.0, 1.0]);
    assert_eq!(vec3::unit_or_same([0.0; 3]), [0.0; 3]);
    assert_eq!(vec3::unit_or_same([0.0, 0.0, -2.0]), [0.0, 0.0, -1.0]);
    assert!(vec3::is_zero([0.01, -0.01, 0.0]) && !vec3::is_zero([0.011, 0.0, 0.0]));
    assert_eq!(vec3::interpolate(a, b, 0.0), a);
}

#[test]
fn a_matrix_and_its_inverses_undo_each_other() {
    let mut m = matrix::from_quat(quat::normalize(&[0.0, 0.0, 1.0, 1.0]));
    (m[3], m[7], m[11]) = (1.0, 2.0, 3.0);
    let p = [0.5, -4.0, 2.0];
    let back = |inv: &matrix::Matrix| matrix::mul_p(inv, matrix::mul_p(&m, p));
    for q in [
        back(&matrix::inverse(&m)),
        back(&matrix::affine_inverse(&m)),
        {
            let mut full = m;
            assert!(matrix::full_inverse(&mut full));
            back(&full)
        },
    ] {
        assert!(q.iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-5), "{q:?}");
    }
    let mut flat = [0.0; 16];
    assert!(!matrix::full_inverse(&mut flat));
    assert_eq!(flat, [0.0; 16]);
    assert_eq!(matrix::product(&IDENTITY, &m), m);
    assert_eq!(matrix::set_euler([0.0; 3]), IDENTITY);
}

#[test]
fn the_quaternion_forms_meet_at_their_ends() {
    let a = quat::IDENTITY;
    let b = quat::normalize(&[0.0, 0.0, 1.0, 1.0]);
    assert_eq!(quat::mul(&a, &b), b);
    assert_eq!(quat::mul_wxyz(a, b), b);
    assert_eq!(quat::conjugate(quat::conjugate(b)), b);
    assert_eq!(quat::inverse(&a), [-0.0, -0.0, -0.0, 1.0]);
    assert_eq!(quat::interpolate(a, b, 0.0), a);
    for q in [
        quat::interpolate(a, b, 1.0),
        quat::slerp_f32(a, b, 1.0),
        quat::nlerp(a, b, 1.0),
    ] {
        assert!(q.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6), "{q:?}");
    }
    let back = quat::from_matrix(&matrix::from_quat(b));
    assert!(
        back.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6),
        "{back:?}"
    );
}
