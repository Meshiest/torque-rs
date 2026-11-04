//! The math the vehicle code calls: `QuatF` (`inverse`, `normalize`, `mul`, `mulP`, `set(MatrixF)`), `MatrixF::fullInverse`,
//! `Point3F::len` with its zero test and the quaternion-moved test of `processTick`. `QuatF::setMatrix` is
//! [`torque_collide::math::quat_matrix`]; the matrix product is [`torque_collide::math::matrix_product`].
//!
//! The routines go through the [`library`] table; the `plain_*` functions are this crate's own, plain `f32`
//! arithmetic in the order Torque's source writes it.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `math/mQuat.cpp`, `math/mQuat.h` and
//! `math/mMatrix.cpp` (each Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_collide::math::{F3, Matrix};

use crate::library;

/// A quaternion `(x, y, z, w)` as `QuatF` stores it.
pub type Quat = [f32; 4];

/// The identity quaternion.
pub const QUAT_IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

/// `QuatIsZero`'s epsilon.
const QUAT_EPSILON: f32 = 0.0001;

/// Vector sum, difference, scaling, dot and cross products.
pub(crate) fn add(a: F3, b: F3) -> F3 {
    std::array::from_fn(|i| a[i] + b[i])
}

pub(crate) fn sub(a: F3, b: F3) -> F3 {
    std::array::from_fn(|i| a[i] - b[i])
}

pub(crate) fn scale(a: F3, k: f32) -> F3 {
    a.map(|c| c * k)
}

pub(crate) fn dot(a: F3, b: F3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: F3, b: F3) -> F3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

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

/// `QuatF::mul(a, b)`.
pub fn plain_quat_mul(a: &Quat, b: &Quat) -> Quat {
    let [ax, ay, az, aw] = *a;
    let [bx, by, bz, bw] = *b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by + ay * bw + az * bx - ax * bz,
        aw * bz + az * bw + ax * by - ay * bx,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `QuatF::inverse()`: a unit quaternion's conjugate; else the conjugate over the squared magnitude (1 for a zero one).
pub fn plain_quat_inverse(q: &Quat) -> Quat {
    let [x, y, z, w] = *q;
    let magnitude = w * w + x * x + y * y + z * z;
    if magnitude == 1.0 {
        return [-x, -y, -z, w];
    }
    let inv = if magnitude == 0.0 {
        1.0
    } else {
        1.0 / magnitude
    };
    [x * -inv, y * -inv, z * -inv, w * inv]
}

/// `QuatF::normalize()`: a zero quaternion becomes the identity.
pub fn plain_quat_normalize(q: &Quat) -> Quat {
    let [x, y, z, w] = *q;
    let l = (x * x + y * y + z * z + w * w).sqrt();
    if l == 0.0 {
        return QUAT_IDENTITY;
    }
    [x / l, y / l, z / l, w / l]
}

/// `QuatF::set(const MatrixF&)`: from the trace when it is positive, else from the largest diagonal element.
pub fn plain_quat_from_matrix(m: &Matrix) -> Quat {
    let trace = m[0] + m[5] + m[10];
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt();
        let w = s * 0.5;
        let s = 0.5 / s;
        return [(m[6] - m[9]) * s, (m[8] - m[2]) * s, (m[1] - m[4]) * s, w];
    }
    let mut i = 0usize;
    if m[5] > m[0] {
        i = 1;
    }
    if m[10] > m[5 * i] {
        i = 2;
    }
    let j = (i + 1) % 3;
    let k = (j + 1) % 3;
    let s = ((m[5 * i] - (m[5 * j] + m[5 * k])) + 1.0).sqrt();
    let mut q = [0.0f32; 4];
    q[i] = s * 0.5;
    let s = 0.5 / s;
    q[j] = (m[4 * i + j] + m[4 * j + i]) * s;
    q[k] = (m[4 * i + k] + m[4 * k + i]) * s;
    q[3] = (m[4 * j + k] - m[4 * k + j]) * s;
    q
}

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
pub fn plain_len_sq(v: F3) -> f32 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

/// `MatrixF::fullInverse()`: the 4x4 inverse by cofactors, `mul(1 / det)`.
pub fn plain_full_inverse(m: &mut Matrix) -> bool {
    let row = |r: usize| [m[4 * r], m[4 * r + 1], m[4 * r + 2], m[4 * r + 3]];
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (row(0), row(1));
    let ([cx, cy, cz, cw], [dx, dy, dz, dw]) = (row(2), row(3));
    let det = ax * by * cz * dw - ax * by * cw * dz - ax * cy * bz * dw
        + ax * cy * bw * dz
        + ax * dy * bz * cw
        - ax * dy * bw * cz
        - bx * ay * cz * dw
        + bx * ay * cw * dz
        + bx * cy * az * dw
        - bx * cy * aw * dz
        - bx * dy * az * cw
        + bx * dy * aw * cz
        + cx * ay * bz * dw
        - cx * ay * bw * dz
        - cx * by * az * dw
        + cx * by * aw * dz
        + cx * dy * az * bw
        - cx * dy * aw * bz
        - dx * ay * bz * cw
        + dx * ay * bw * cz
        + dx * by * az * cw
        - dx * by * aw * cz
        - dx * cy * az * bw
        + dx * cy * aw * bz;
    if det.abs() < 0.00001 {
        return false;
    }
    let out = [
        by * cz * dw - by * cw * dz - cy * bz * dw + cy * bw * dz + dy * bz * cw - dy * bw * cz,
        -ay * cz * dw + ay * cw * dz + cy * az * dw - cy * aw * dz - dy * az * cw + dy * aw * cz,
        ay * bz * dw - ay * bw * dz - by * az * dw + by * aw * dz + dy * az * bw - dy * aw * bz,
        -ay * bz * cw + ay * bw * cz + by * az * cw - by * aw * cz - cy * az * bw + cy * aw * bz,
        -bx * cz * dw + bx * cw * dz + cx * bz * dw - cx * bw * dz - dx * bz * cw + dx * bw * cz,
        ax * cz * dw - ax * cw * dz - cx * az * dw + cx * aw * dz + dx * az * cw - dx * aw * cz,
        -ax * bz * dw + ax * bw * dz + bx * az * dw - bx * aw * dz - dx * az * bw + dx * aw * bz,
        ax * bz * cw - ax * bw * cz - bx * az * cw + bx * aw * cz + cx * az * bw - cx * aw * bz,
        bx * cy * dw - bx * cw * dy - cx * by * dw + cx * bw * dy + dx * by * cw - dx * bw * cy,
        -ax * cy * dw + ax * cw * dy + cx * ay * dw - cx * aw * dy - dx * ay * cw + dx * aw * cy,
        ax * by * dw - ax * bw * dy - bx * ay * dw + bx * aw * dy + dx * ay * bw - dx * aw * by,
        -ax * by * cw + ax * bw * cy + bx * ay * cw - bx * aw * cy - cx * ay * bw + cx * aw * by,
        -bx * cy * dz + bx * cz * dy + cx * by * dz - cx * bz * dy - dx * by * cz + dx * bz * cy,
        ax * cy * dz - ax * cz * dy - cx * ay * dz + cx * az * dy + dx * ay * cz - dx * az * cy,
        -ax * by * dz + ax * bz * dy + bx * ay * dz - bx * az * dy - dx * ay * bz + dx * az * by,
        ax * by * cz - ax * bz * cy - bx * ay * cz + bx * az * cy + cx * ay * bz - cx * az * by,
    ];
    let inv = 1.0 / det;
    *m = out.map(|e| e * inv);
    true
}
