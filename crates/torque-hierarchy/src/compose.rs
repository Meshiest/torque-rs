//! World and local poses from each other, by the mount rule: the parent's scale scales the translations (the node's and the
//! child's local one), never the child's rotation or scale, so no shear appears.

use torque_collide::math::{F3, IDENTITY, Matrix, matrix_product, quat_matrix};
use torque_collide::math::{plain_quat_from_matrix, plain_quat_normalize};

use crate::types::{Local, Pose};

/// `matrix` with its translation (elements 3, 7 and 11) multiplied per axis by `by`.
fn scale_translation(mut matrix: Matrix, by: F3) -> Matrix {
    matrix[3] *= by[0];
    matrix[7] *= by[1];
    matrix[11] *= by[2];
    matrix
}

/// The matrix of `local` with its translation scaled by `parent_scale`.
fn local_matrix(local: &Local, parent_scale: F3) -> Matrix {
    let mut m = quat_matrix(local.rotation);
    let [x, y, z] = local.translation;
    m[3] = x * parent_scale[0];
    m[7] = y * parent_scale[1];
    m[11] = z * parent_scale[2];
    m
}

/// The parent's transform at the attachment: its world, then the node if there is one.
fn attachment(parent: &Pose, node: Option<&Matrix>) -> Matrix {
    match node {
        None => parent.world,
        Some(node) => matrix_product(&parent.world, &scale_translation(*node, parent.scale)),
    }
}

/// A child's world transform: the parent's world, the node, then the local pose.
pub(crate) fn compose(parent: &Pose, node: Option<&Matrix>, local: &Local) -> Matrix {
    matrix_product(
        &attachment(parent, node),
        &local_matrix(local, parent.scale),
    )
}

/// The local pose that puts a child of scale `scale` at `world` under `parent`'s attachment: the inverse of [`compose`].
///
/// The inverse and the product are taken in `f64` from the same `f32` attachment [`compose`] will use, so the result is off
/// by the rounding of the local pose alone. A chain of `f32` products leaves a rotation a little off a true one (about
/// 1e-7 per level), which the inverse must undo, not assume away by a transpose; and in `f32` the cancellation between two
/// far world translations costs a few units in the last place of their size.
pub(crate) fn local_from_world(
    world: &Matrix,
    parent: &Pose,
    node: Option<&Matrix>,
    scale: F3,
) -> Local {
    let a = attachment(parent, node).map(f64::from);
    let w = world.map(f64::from);
    let rotation = [[a[0], a[1], a[2]], [a[4], a[5], a[6]], [a[8], a[9], a[10]]];
    let inverse = invert_3x3(&rotation).unwrap_or_else(|| transpose_3x3(&rotation));
    let apply = |v: [f64; 3], row: usize| (0..3).map(|k| inverse[row][k] * v[k]).sum::<f64>();
    let offset = [w[3] - a[3], w[7] - a[7], w[11] - a[11]];
    let mut m = IDENTITY;
    for row in 0..3 {
        for col in 0..3 {
            // `f64` to `f32` rounds to nearest.
            m[row * 4 + col] = apply([w[col], w[4 + col], w[8 + col]], row) as f32;
        }
        m[row * 4 + 3] = apply(offset, row) as f32; // as above
    }
    Local {
        translation: [
            m[3] / parent.scale[0],
            m[7] / parent.scale[1],
            m[11] / parent.scale[2],
        ],
        rotation: plain_quat_normalize(&plain_quat_from_matrix(&m)),
        scale,
    }
}

type Mat3 = [[f64; 3]; 3];

fn transpose_3x3(m: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}

/// The inverse by cofactors; `None` when the determinant is too small to divide by.
fn invert_3x3(m: &Mat3) -> Option<Mat3> {
    let cofactor = |i: usize, j: usize| {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let det = (0..3).map(|j| m[0][j] * cofactor(0, j)).sum::<f64>();
    if det.abs() < 1e-12 {
        return None;
    }
    Some(std::array::from_fn(|i| {
        std::array::from_fn(|j| cofactor(j, i) / det)
    }))
}

/// The local pose an object with no parent has, read off its world transform: what it keeps when it is parented keeping
/// local.
pub(crate) fn local_from_matrix(world: &Matrix, scale: F3) -> Local {
    Local {
        translation: [world[3], world[7], world[11]],
        rotation: plain_quat_normalize(&plain_quat_from_matrix(world)),
        scale,
    }
}

/// The world transform of an object whose parent is gone and which keeps its local pose.
pub(crate) fn matrix_from_local(local: &Local) -> Matrix {
    local_matrix(local, [1.0; 3])
}

#[cfg(test)]
#[path = "compose_tests.rs"]
mod tests;
