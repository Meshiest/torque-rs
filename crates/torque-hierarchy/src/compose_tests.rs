use torque_collide::math::quat_matrix;

use super::*;

fn pose(world: Matrix, scale: F3) -> Pose {
    Pose { world, scale }
}

#[test]
fn a_quaternion_survives_the_trip_through_a_matrix() {
    let turn = 0.7f32;
    for axis in [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.6, 0.0, 0.8],
    ] {
        let (s, c) = (turn / 2.0).sin_cos();
        let q = [axis[0] * s, axis[1] * s, axis[2] * s, c];
        let back = plain_quat_from_matrix(&quat_matrix(q));
        let same = q.iter().zip(&back).all(|(a, b)| (a - b).abs() < 1e-6);
        let flipped = q.iter().zip(&back).all(|(a, b)| (a + b).abs() < 1e-6);
        assert!(same || flipped, "{q:?} came back as {back:?}");
    }
}

#[test]
fn local_from_world_undoes_compose() {
    let mut parent = quat_matrix([0.1, 0.5, -0.3, 0.8]);
    parent[3] = 100.0;
    parent[7] = -40.0;
    parent[11] = 7.0;
    let parent = pose(plain_normalized(parent), [2.0, 0.5, 1.5]);
    let mut node = quat_matrix([0.0, 0.0, 0.3, 0.95]);
    node[3] = 1.0;
    let local = Local {
        translation: [3.0, -2.0, 1.0],
        rotation: plain_quat_normalize(&[0.3, -0.2, 0.9, 0.1]),
        scale: [1.0; 3],
    };
    for node in [None, Some(&node)] {
        let world = compose(&parent, node, &local);
        let back = local_from_world(&world, &parent, node, [1.0; 3]);
        let again = compose(&parent, node, &back);
        for i in 0..16 {
            assert!(
                (world[i] - again[i]).abs() < 1e-5,
                "{i}: {} {}",
                world[i],
                again[i]
            );
        }
    }
}

/// The matrix with its rotation made exactly orthonormal by a trip through a quaternion.
fn plain_normalized(m: Matrix) -> Matrix {
    let mut out = quat_matrix(plain_quat_normalize(&plain_quat_from_matrix(&m)));
    out[3] = m[3];
    out[7] = m[7];
    out[11] = m[11];
    out
}

#[test]
fn the_3x3_inverse_handles_a_slightly_off_rotation_and_refuses_a_flat_one() {
    let m = [[1.0, 1e-3, 0.0], [0.0, 1.0, 2e-3], [3e-3, 0.0, 1.0]];
    let inverse = invert_3x3(&m).expect("invertible");
    for (i, row) in m.iter().enumerate() {
        for j in 0..3 {
            let product: f64 = row.iter().zip(&inverse).map(|(a, b)| a * b[j]).sum();
            assert!((product - f64::from(u8::from(i == j))).abs() < 1e-12);
        }
    }
    assert!(invert_3x3(&[[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
}
