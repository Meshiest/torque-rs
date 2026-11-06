//! The arithmetic of the model layer: affine 4x4 matrices laid out as Torque's `MatrixF` (row-major, the translation in elements
//! 3, 7 and 11) and unit quaternions `[x, y, z, w]` in the usual convention (a positive turn about an axis is
//! counter-clockwise looking down it; `v' = q v q*`).

/// A 4x4 affine matrix, row-major, the translation in the last column.
pub type Mat4 = [f32; 16];
/// A quaternion `[x, y, z, w]`.
pub type Quat = [f32; 4];
pub type Vec3 = [f32; 3];

pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// No turn.
pub const NO_TURN: Quat = [0.0, 0.0, 0.0, 1.0];

/// `a * b`.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            out[i * 4 + j] = (0..4).map(|k| a[i * 4 + k] * b[k * 4 + j]).sum();
        }
    }
    out
}

/// Row `r`'s first three elements.
fn row(m: &Mat4, r: usize) -> glam::Vec3 {
    glam::Vec3::new(m[r * 4], m[r * 4 + 1], m[r * 4 + 2])
}

/// The point `p` placed by `m`.
pub fn point(m: &Mat4, p: Vec3) -> Vec3 {
    let p = glam::Vec3::from(p);
    std::array::from_fn(|r| row(m, r).dot(p) + m[r * 4 + 3])
}

/// The direction `v` turned (and scaled) by `m`.
pub fn vector(m: &Mat4, v: Vec3) -> Vec3 {
    let v = glam::Vec3::from(v);
    std::array::from_fn(|r| row(m, r).dot(v))
}

/// The rotation matrix of a unit quaternion.
pub fn rotation(q: Quat) -> Mat4 {
    let [x, y, z, w] = q;
    let mut m = IDENTITY;
    m[0] = 1.0 - 2.0 * (y * y + z * z);
    m[1] = 2.0 * (x * y - z * w);
    m[2] = 2.0 * (x * z + y * w);
    m[4] = 2.0 * (x * y + z * w);
    m[5] = 1.0 - 2.0 * (x * x + z * z);
    m[6] = 2.0 * (y * z - x * w);
    m[8] = 2.0 * (x * z - y * w);
    m[9] = 2.0 * (y * z + x * w);
    m[10] = 1.0 - 2.0 * (x * x + y * y);
    m
}

/// Translation times rotation times scale.
pub fn trs(t: Vec3, q: Quat, s: Vec3) -> Mat4 {
    let mut m = rotation(q);
    for r in 0..3 {
        for (c, sc) in s.iter().enumerate() {
            m[r * 4 + c] *= sc;
        }
        m[r * 4 + 3] = t[r];
    }
    m
}

/// The inverse of an affine matrix (the identity for a singular one).
pub fn inverse(m: &Mat4) -> Mat4 {
    let a = |r: usize, c: usize| f64::from(m[r * 4 + c]);
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    if det.abs() < 1e-30 || !det.is_finite() {
        return IDENTITY;
    }
    let inv = 1.0 / det;
    let c = |r: usize, k: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        a(r1, k1) * a(r2, k2) - a(r1, k2) * a(r2, k1)
    };
    let mut out = IDENTITY;
    for r in 0..3 {
        for k in 0..3 {
            // The inverse is the transposed cofactor matrix over the determinant.
            out[r * 4 + k] = (c(k, r) * inv) as f32;
        }
    }
    let t = vector(&out, [-m[3], -m[7], -m[11]]);
    out[3] = t[0];
    out[7] = t[1];
    out[11] = t[2];
    out
}

/// The quaternion of a rotation matrix (the upper 3x3 of `m`, assumed orthonormal).
pub fn quat_of(m: &Mat4) -> Quat {
    let (m00, m01, m02) = (m[0], m[1], m[2]);
    let (m10, m11, m12) = (m[4], m[5], m[6]);
    let (m20, m21, m22) = (m[8], m[9], m[10]);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
    };
    normalize_quat(q)
}

/// An affine matrix as translation, rotation and scale (a mirror goes into a negative x scale; shear is dropped).
pub fn decompose(m: &Mat4) -> (Vec3, Quat, Vec3) {
    let t = [m[3], m[7], m[11]];
    let col = |c: usize| glam::Vec3::new(m[c], m[4 + c], m[8 + c]);
    let mut s = [0, 1, 2].map(|c| col(c).length());
    let det = m[0] * (m[5] * m[10] - m[6] * m[9]) - m[1] * (m[4] * m[10] - m[6] * m[8])
        + m[2] * (m[4] * m[9] - m[5] * m[8]);
    if det < 0.0 {
        s[0] = -s[0];
    }
    let mut r = IDENTITY;
    for (c, sc) in s.iter().enumerate() {
        let k = if sc.abs() > 1e-20 { 1.0 / sc } else { 0.0 };
        for row in 0..3 {
            r[row * 4 + c] = m[row * 4 + c] * k;
        }
    }
    (t, quat_of(&r), s)
}

/// `q` at unit length (no turn for a zero or non-finite one).
pub fn normalize_quat(q: Quat) -> Quat {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l > 1e-20 && l.is_finite() {
        q.map(|c| c / l)
    } else {
        NO_TURN
    }
}

/// `a * b`: the turn `b` then `a`.
pub fn quat_mul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The spherical blend from `a` to `b` by `t`, the short way round.
pub fn slerp(a: Quat, b: Quat, t: f32) -> Quat {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let b = if d < 0.0 {
        d = -d;
        b.map(|c| -c)
    } else {
        b
    };
    if d > 0.9995 {
        return normalize_quat(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
    }
    let theta = d.clamp(-1.0, 1.0).acos();
    let s = theta.sin();
    let (wa, wb) = (((1.0 - t) * theta).sin() / s, (t * theta).sin() / s);
    normalize_quat(std::array::from_fn(|i| a[i] * wa + b[i] * wb))
}
