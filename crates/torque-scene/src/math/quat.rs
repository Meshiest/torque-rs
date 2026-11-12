//! Quaternion arithmetic (`QuatF`), stored `[x, y, z, w]`.
//!
//! Torque's `QuatF` turns the other way from a physics world's quaternion of the same four numbers (its `setMatrix` writes the
//! transpose of the usual matrix), so code that moves poses between the two [`conjugate`]s on the way.

use super::matrix::Matrix;
use super::transcendental;

/// A quaternion `(x, y, z, w)` as `QuatF` stores it.
pub type Quat = [f32; 4];

/// The identity quaternion.
pub const IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

/// `QuatF::normalize()`: each component over the length (the squares summed `x, y, z, w`); a zero quaternion becomes the
/// identity.
pub fn normalize(q: &Quat) -> Quat {
    let [x, y, z, w] = *q;
    let l = (x * x + y * y + z * z + w * w).sqrt();
    if l == 0.0 {
        return IDENTITY;
    }
    [x / l, y / l, z / l, w / l]
}

/// `QuatF::set(const MatrixF&)`: from the trace when it is positive, else from the largest diagonal element. The inverse of
/// [`super::matrix::from_quat`].
pub fn from_matrix(m: &Matrix) -> Quat {
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

/// `QuatF::mul(a, b)`: the product `a * b` (`b`'s turn, then `a`'s), each component's terms in the order `QuatF::mul` writes
/// them.
pub fn mul(a: &Quat, b: &Quat) -> Quat {
    let [ax, ay, az, aw] = *a;
    let [bx, by, bz, bw] = *b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by + ay * bw + az * bx - ax * bz,
        aw * bz + az * bw + ax * by - ay * bx,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The same product as [`mul`] with each component's terms summed in the order of `a`'s `w, x, y, z`: the `y` and `z`
/// components round differently from [`mul`]'s, so the two are not interchangeable in a simulated path.
pub fn mul_wxyz(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The conjugate `(-x, -y, -z, w)`: the inverse of a unit quaternion, and the same turn in the other handedness convention (see
/// the module's note).
pub fn conjugate([x, y, z, w]: Quat) -> Quat {
    [-x, -y, -z, w]
}

/// `QuatF::inverse()`: a unit quaternion's conjugate; else the conjugate over the squared magnitude (the squares summed
/// `w, x, y, z`; 1 for a zero one).
pub fn inverse(q: &Quat) -> Quat {
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

/// `a` at `t` 0 and `b` at `t` 1, blended straight the short way and made unit again by [`normalize`] (a zero result is no
/// turn).
pub fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
    let d: f32 = (0..4).map(|k| a[k] * b[k]).sum();
    let s = if d < 0.0 { -1.0 } else { 1.0 };
    let q: Quat = std::array::from_fn(|k| a[k] + (b[k] * s - a[k]) * t);
    normalize(&q)
}

fn d(x: f32) -> f64 {
    f64::from(x)
}

/// The threshold (1e-5, a double): below it `1 - cos` takes the linear blend.
const SLERP_LINEAR: f64 = 1.0e-5;

/// `QuatF::interpolate(q1, q2, t)`: the slerp the short way round in double precision (the dot summed `y, x, z, w`), a linear
/// blend when `1 - cos` is at most 1e-5, each component stored as a float and not made unit again. [`slerp_f32`] is the `f32`
/// form, which rounds differently.
pub fn interpolate(q1: Quat, q2: Quat, t: f32) -> Quat {
    let mut cos =
        ((d(q1[1]) * d(q2[1]) + d(q1[0]) * d(q2[0])) + d(q1[2]) * d(q2[2])) + d(q1[3]) * d(q2[3]);
    let sign = if 0.0 > cos {
        cos = -cos;
        -1.0f64
    } else {
        1.0
    };
    let td = d(t);
    let (s1, s2) = if 1.0 - cos <= SLERP_LINEAR {
        (1.0 - td, sign * td)
    } else {
        let omega = transcendental::acos(cos);
        let sin_omega = transcendental::sin(omega);
        (
            transcendental::sin(omega * (1.0 - td)) / sin_omega,
            transcendental::sin(omega * td) * sign / sin_omega,
        )
    };
    std::array::from_fn(|i| (d(q1[i]) * s1 + d(q2[i]) * s2) as f32)
}

/// The rotation between `a` and `b` a fraction `t` of the way, the short way round, in `f32`: a linear blend when `1 - dot` is
/// at most 1e-4, then made unit; `b` when that result has no finite, non-zero length. Not [`interpolate`]'s bits.
pub fn slerp_f32(a: Quat, b: Quat, t: f32) -> Quat {
    let mut dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let sign = if dot < 0.0 {
        dot = -dot;
        -1.0
    } else {
        1.0
    };
    let (s1, s2) = if 1.0 - dot <= 1.0e-4 {
        (1.0 - t, sign * t)
    } else {
        let omega = transcendental::acos_f32(dot.clamp(-1.0, 1.0));
        let sin = transcendental::sin_f32(omega);
        (
            transcendental::sin_f32(omega * (1.0 - t)) / sin,
            sign * transcendental::sin_f32(omega * t) / sin,
        )
    };
    let q: Quat = std::array::from_fn(|i| a[i] * s1 + b[i] * s2);
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > 0.0 && len.is_finite() {
        q.map(|c| c / len)
    } else {
        b
    }
}
