//! The math the vehicle code calls: `QuatF` (`inverse`, `normalize`, `mul`, `mulP`, `set(MatrixF)`), `MatrixF::fullInverse`,
//! `Point3F::len` with its zero test and the quaternion-moved test of `processTick`. `QuatF::setMatrix` is
//! [`torque_collide::math::quat_matrix`]; `QuatF::set(MatrixF)` and `normalize` are [`plain_quat_from_matrix`] and
//! [`plain_quat_normalize`], re-exported from `torque_collide::math`; the matrix product is
//! [`torque_collide::math::matrix_product`].
//!
//! The routines go through the [`library`] table; the `plain_*` functions are plain `f32` arithmetic in the order Torque's
//! source writes it, most of them [`torque_collide::math`]'s plain bodies (the scene crate's) re-exported.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `math/mQuat.cpp`, `math/mQuat.h` and
//! `math/mMatrix.cpp` (each Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_collide::math::{F3, Matrix};
pub use torque_collide::math::{QUAT_IDENTITY, Quat, plain_quat_from_matrix, plain_quat_normalize};

use crate::library;

/// `QuatIsZero`'s epsilon.
const QUAT_EPSILON: f32 = 0.0001;

/// Vector sum, difference, scaling, dot and cross products.
pub(crate) use torque_collide::math::vec3::{add, cross, dot, scale, sub};

/// The gravity of the vehicles, in units per second squared (`sVehicleGravity`).
pub const GRAVITY: f32 = -20.0;

/// `QuatF::mul(a, b)`: the product `a * b` (through the table).
pub fn quat_mul(a: &Quat, b: &Quat) -> Quat {
    (library::get().quat_mul)(a, b)
}

/// `QuatF::inverse()` (through the table).
pub fn quat_inverse(q: &Quat) -> Quat {
    (library::get().quat_inverse)(q)
}

/// `QuatF::normalize()` (through the table).
pub fn quat_normalize(q: &Quat) -> Quat {
    (library::get().quat_normalize)(q)
}

/// `QuatF::mulP(p)`: `q^-1 * (p, 0) * q`, the vector part.
pub fn quat_mul_p(q: &Quat, p: F3) -> F3 {
    let qc = quat_inverse(q);
    let t = quat_mul(&qc, &[p[0], p[1], p[2], 0.0]);
    let r = quat_mul(&t, q);
    [r[0], r[1], r[2]]
}

/// `QuatF::set(const MatrixF&)` (through the table).
pub fn quat_from_matrix(m: &Matrix) -> Quat {
    (library::get().quat_from_matrix)(m)
}

/// Both quaternions normalised, `a * inverse(b)`, and true unless it is the identity (through the table).
pub fn quat_rotated(a: &Quat, b: &Quat) -> bool {
    (library::get().quat_rotated)(a, b)
}

/// `Point3F::len()` with its zero test (through the table).
pub fn len_eps(v: F3) -> f32 {
    (library::get().len_eps)(v)
}

/// `Point3F::lenSquared()` (through the table).
pub fn len_sq(v: F3) -> f32 {
    (library::get().len_sq)(v)
}

/// `MatrixF::fullInverse()` (through the table): false, and the matrix left as it was, when the determinant is below 1e-5 in
/// size.
pub fn full_inverse(m: &mut Matrix) -> bool {
    (library::get().full_inverse)(m)
}

/// `QuatF::mul(a, b)` and `QuatF::inverse()` (a unit quaternion's conjugate; else the conjugate over the squared magnitude, 1
/// for a zero one).
pub use torque_collide::math::quat::{inverse as plain_quat_inverse, mul as plain_quat_mul};

/// `QuatF::isEqual`-style test of `processTick`: both normalised, `a *= inverse(b)`, and true unless each of x, y and z is
/// below `QuatIsZero`'s epsilon in size.
pub fn plain_quat_rotated(a: &Quat, b: &Quat) -> bool {
    let a = plain_quat_normalize(a);
    let b = plain_quat_inverse(&plain_quat_normalize(b));
    let r = plain_quat_mul(&a, &b);
    !(r[0].abs() < QUAT_EPSILON && r[1].abs() < QUAT_EPSILON && r[2].abs() < QUAT_EPSILON)
}

/// `Point3F::len()` with the zero test of `Point3F::isZero` (`POINT_EPSILON` on each component's square): 0 for a vector that
/// is zero by that test (a NaN square is not), else the length.
pub fn plain_len_eps(v: F3) -> f32 {
    let e = 1e-4;
    if !(v[0] * v[0] <= e) || !(v[1] * v[1] <= e) || !(v[2] * v[2] <= e) {
        plain_len_sq(v).sqrt()
    } else {
        0.0
    }
}

/// `Point3F::lenSquared()`.
pub use torque_collide::math::vec3::length_squared as plain_len_sq;

/// `MatrixF::fullInverse()`: the 4x4 inverse by cofactors, `mul(1 / det)`; false, and the matrix left as it was, when the
/// determinant is below 1e-5 in size.
pub use torque_collide::math::matrix::full_inverse as plain_full_inverse;
