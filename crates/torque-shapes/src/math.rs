//! The matrix and vector arithmetic of the shape queries: `MatrixF::mul`, `mulP`, `mulV`, `inverse`, `QuatF::setMatrix`,
//! `Point3F::normalize`, `mDot` and `mCross`, as Torque's `mMath_C.cpp`, `mMatrix.cpp` and `mQuat.cpp` write them.
//!
//! The routines with an entry in [`crate::library::ShapeLibrary`] are called through the installed table (by default plain
//! `f32` code, the [`plain`] module); the matrix product is plain `f32` code with no entry.

use crate::library;

/// A 4x4 matrix as `MatrixF` holds it: row-major, the translation in the last column (elements 3, 7 and 11).
pub type Matrix = [f32; 16];

/// The identity (`MatrixF::identity`).
pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// `a * b` (`m_matF_x_matF`, `default_matF_x_matF_C`): row `i` of the result is `a[i0]*b[0j] + a[i1]*b[1j] + a[i2]*b[2j] +
/// a[i3]*b[3j]`, summed left to right.
pub fn mat_mul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            out[i * 4 + j] = a[i * 4] * b[j]
                + a[i * 4 + 1] * b[4 + j]
                + a[i * 4 + 2] * b[8 + j]
                + a[i * 4 + 3] * b[12 + j];
        }
    }
    out
}

/// `MatrixF::mulP`: the point turned and moved by `m` (through the table).
pub fn mul_p(m: &Matrix, p: [f32; 3]) -> [f32; 3] {
    (library::get().mul_p)(m, p)
}

/// `MatrixF::mulV`: the direction turned by `m` (through the table).
pub fn mul_v(m: &Matrix, v: [f32; 3]) -> [f32; 3] {
    (library::get().mul_v)(m, v)
}

/// `m_matF_inverse`: the inverse of the affine matrix `m` (through the table). Elements 12-15 are left as they were.
pub fn mat_inverse(m: &Matrix) -> Matrix {
    (library::get().inverse)(m)
}

/// `QuatF::setMatrix`: the quaternion `(x, y, z, w)` as a matrix with no translation; a vector part whose squared length is
/// below `10e-20` gives the identity (through the table).
pub fn quat_matrix(q: [f32; 4]) -> Matrix {
    (library::get().quat_matrix)(q)
}

/// `Quat16::getQuatF`: each short over 32767.
pub fn quat16(q: [i16; 4]) -> [f32; 4] {
    q.map(|s| f32::from(s) / 32767.0)
}

/// The rotation `animateNodes` gives a node at rest: the matrix of the node's default rotation ([`quat16`]) and its default
/// translation in the last column.
pub fn node_matrix(q: [i16; 4], translation: [f32; 3]) -> Matrix {
    let mut m = quat_matrix(quat16(q));
    m[3] = translation[0];
    m[7] = translation[1];
    m[11] = translation[2];
    m
}

/// `m_point3F_normalize`: zero gives `(0, 0, 1)` (through the table).
pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    (library::get().normalize)(v)
}

/// `mDot` (through the table).
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (library::get().dot)(a, b)
}

/// The cross product `TSMesh::buildConvexHull` makes of a triangle seen from `base` (the three attempts differ only in which
/// vertex is the base): `mCross(q3 - base, q2 - base)` and its squared length.
pub fn triangle_cross(base: &[f32; 3], q2: &[f32; 3], q3: &[f32; 3]) -> ([f32; 3], f32) {
    let e2 = [q2[0] - base[0], q2[1] - base[1], q2[2] - base[2]];
    let e3 = [q3[0] - base[0], q3[1] - base[1], q3[2] - base[2]];
    let c = (library::get().cross)(e3, e2);
    (c, dot(c, c))
}

/// The routines of [`crate::library::PLAIN`]: plain `f32` arithmetic, each written as its Torque routine is.
pub mod plain {
    use super::{IDENTITY, Matrix};

    /// `m_matF_x_point3F`.
    pub fn mul_p(m: &Matrix, p: [f32; 3]) -> [f32; 3] {
        [
            m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
            m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
            m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
        ]
    }

    /// `m_matF_x_vectorF`.
    pub fn mul_v(m: &Matrix, v: [f32; 3]) -> [f32; 3] {
        [
            m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
            m[4] * v[0] + m[5] * v[1] + m[6] * v[2],
            m[8] * v[0] + m[9] * v[1] + m[10] * v[2],
        ]
    }

    /// `m_matF_inverse_C` over `m_matF_determinant_C`.
    pub fn inverse(m: &Matrix) -> Matrix {
        let det = m[0] * (m[5] * m[10] - m[6] * m[9])
            + m[4] * (m[2] * m[9] - m[1] * m[10])
            + m[8] * (m[1] * m[6] - m[2] * m[5]);
        let inv_det = 1.0 / det;
        let mut o = *m;
        o[0] = (m[5] * m[10] - m[6] * m[9]) * inv_det;
        o[1] = (m[9] * m[2] - m[10] * m[1]) * inv_det;
        o[2] = (m[1] * m[6] - m[2] * m[5]) * inv_det;
        o[4] = (m[6] * m[8] - m[4] * m[10]) * inv_det;
        o[5] = (m[10] * m[0] - m[8] * m[2]) * inv_det;
        o[6] = (m[2] * m[4] - m[0] * m[6]) * inv_det;
        o[8] = (m[4] * m[9] - m[5] * m[8]) * inv_det;
        o[9] = (m[8] * m[1] - m[9] * m[0]) * inv_det;
        o[10] = (m[0] * m[5] - m[1] * m[4]) * inv_det;
        let t = mul_v(&o, [-m[3], -m[7], -m[11]]);
        o[3] = t[0];
        o[7] = t[1];
        o[11] = t[2];
        o
    }

    /// `QuatF::setMatrix` over `m_quatF_set_matF_C`.
    pub fn quat_matrix(q: [f32; 4]) -> Matrix {
        let [x, y, z, w] = q;
        if x * x + y * y + z * z < 10e-20 {
            return IDENTITY;
        }
        let xs = x * 2.0;
        let ys = y * 2.0;
        let zs = z * 2.0;
        let wx = w * xs;
        let wy = w * ys;
        let wz = w * zs;
        let xx = x * xs;
        let xy = x * ys;
        let xz = x * zs;
        let yy = y * ys;
        let yz = y * zs;
        let zz = z * zs;
        let mut m = [0.0f32; 16];
        m[0] = 1.0 - (yy + zz);
        m[4] = xy - wz;
        m[8] = xz + wy;
        m[1] = xy + wz;
        m[5] = 1.0 - (xx + zz);
        m[9] = yz - wx;
        m[2] = xz - wy;
        m[6] = yz + wx;
        m[10] = 1.0 - (xx + yy);
        m[15] = 1.0;
        m
    }

    /// `m_point3F_normalize_C`.
    pub fn normalize(p: [f32; 3]) -> [f32; 3] {
        let squared = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        if squared != 0.0 {
            let factor = 1.0 / squared.sqrt();
            [p[0] * factor, p[1] * factor, p[2] * factor]
        } else {
            [0.0, 0.0, 1.0]
        }
    }

    /// `mDot`.
    pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    /// `mCross`.
    pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }
}

#[cfg(test)]
#[path = "math_tests.rs"]
mod tests;
