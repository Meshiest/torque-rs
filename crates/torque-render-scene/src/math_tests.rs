use super::*;

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

#[test]
fn a_quarter_turn_about_z_takes_x_to_minus_y_as_tge_does() {
    let m = Mat::from_axis_angle([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2);
    assert!(close(m.mul_v([1.0, 0.0, 0.0]), [0.0, -1.0, 0.0]));
    let e = Mat::euler_z(std::f32::consts::FRAC_PI_2);
    assert!(close(e.mul_v([1.0, 0.0, 0.0]), [0.0, -1.0, 0.0]));
}

#[test]
fn a_positive_pitch_looks_down() {
    let m = Mat::mul(&Mat::euler_z(0.0), &Mat::euler_x(0.3));
    let forward = m.column(1);
    assert!(forward[2] < 0.0 && forward[1] > 0.0);
}

#[test]
fn the_affine_inverse_undoes_the_matrix() {
    let mut m = Mat::mul(&Mat::euler_z(-0.588), &Mat::euler_x(0.394));
    m.set_column3([2.0, -3.0, 1.8]);
    let p = [0.5, 0.25, -1.0];
    assert!(close(m.affine_inverse().mul_p(m.mul_p(p)), p));
}
