//! TGE's `MatrixF` (row-major 4 x 4 floats; `mulP` is `M * p` with the translation in the last column) and the rotations
//! built with it.
//!
//! Credit: this module follows Torque3D's MIT-licensed `math/mMath_C.cpp`, `math/mQuat.cpp` and `math/mMatrix.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use glam::{DMat4, Vec3};

/// `v` over its length when that is over `min` and finite (a division, as the draws were made: not glam's product with the
/// reciprocal); `None` otherwise.
pub fn unit_over(v: Vec3, min: f32) -> Option<Vec3> {
    let l = v.length();
    (l > min && l.is_finite()).then(|| v / l)
}

/// `v` over its length, or as it is when that is 0.
pub fn unit_or_same(v: Vec3) -> Vec3 {
    let l = v.length();
    if l > 0.0 { v / l } else { v }
}

/// `v` over its length, or up (+z) when that is 0.
pub fn unit_or_up(v: Vec3) -> Vec3 {
    let l = v.length();
    if l > 0.0 { v / l } else { Vec3::Z }
}

/// A TGE `MatrixF`: `m[r * 4 + c]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat {
    pub m: [f32; 16],
}

impl Mat {
    pub const IDENTITY: Mat = Mat {
        m: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    };

    /// `EulerF(x, 0, 0)` (`m_matF_set_euler`'s single-axis case): rows `(1 0 0) (0 c s) (0 -s c)`.
    pub fn euler_x(a: f32) -> Mat {
        if a == 0.0 {
            return Mat::IDENTITY;
        }
        let (s, c) = a.sin_cos();
        let mut r = Mat::IDENTITY;
        r.m[5] = c;
        r.m[6] = s;
        r.m[9] = -s;
        r.m[10] = c;
        r
    }

    /// `EulerF(0, 0, z)`: rows `(c s 0) (-s c 0) (0 0 1)`.
    pub fn euler_z(a: f32) -> Mat {
        if a == 0.0 {
            return Mat::IDENTITY;
        }
        let (s, c) = a.sin_cos();
        let mut r = Mat::IDENTITY;
        r.m[0] = c;
        r.m[1] = s;
        r.m[4] = -s;
        r.m[5] = c;
        r
    }

    /// `QuatF::setMatrix` (`m_quatF_set_matF`) of the quaternion `x y z w`; a quaternion with a tiny vector part is the identity.
    pub fn from_quat(q: [f32; 4]) -> Mat {
        let [x, y, z, w] = q;
        if x * x + y * y + z * z < 10e-20 {
            return Mat::IDENTITY;
        }
        let (xs, ys, zs) = (x * 2.0, y * 2.0, z * 2.0);
        let (wx, wy, wz) = (w * xs, w * ys, w * zs);
        let (xx, xy, xz) = (x * xs, x * ys, x * zs);
        let (yy, yz, zz) = (y * ys, y * zs, z * zs);
        let mut r = Mat::IDENTITY;
        r.m[0] = 1.0 - (yy + zz);
        r.m[4] = xy - wz;
        r.m[8] = xz + wy;
        r.m[1] = xy + wz;
        r.m[5] = 1.0 - (xx + zz);
        r.m[9] = yz - wx;
        r.m[2] = xz - wy;
        r.m[6] = yz + wx;
        r.m[10] = 1.0 - (xx + yy);
        r
    }

    /// `AngAxisF::setMatrix`: `QuatF(AngAxisF)` (sine and cosine of half the angle) then [`Mat::from_quat`].
    pub fn from_axis_angle(axis: [f32; 3], angle: f32) -> Mat {
        let (s, c) = (angle * 0.5).sin_cos();
        Mat::from_quat([axis[0] * s, axis[1] * s, axis[2] * s, c])
    }

    /// `a * b` (`MatrixF::mul(a, b)`).
    pub fn mul(a: &Mat, b: &Mat) -> Mat {
        let mut r = [0.0f32; 16];
        for i in 0..4 {
            for j in 0..4 {
                r[i * 4 + j] = (0..4).map(|k| a.m[i * 4 + k] * b.m[k * 4 + j]).sum();
            }
        }
        Mat { m: r }
    }

    pub fn set_column3(&mut self, p: [f32; 3]) {
        self.m[3] = p[0];
        self.m[7] = p[1];
        self.m[11] = p[2];
    }

    pub fn column(&self, c: usize) -> [f32; 3] {
        [self.m[c], self.m[4 + c], self.m[8 + c]]
    }

    /// The inverse of a rotation plus translation (`MatrixF::affineInverse`): the transposed rotation and `-R^T t`.
    pub fn affine_inverse(&self) -> Mat {
        let m = &self.m;
        let mut r = Mat::IDENTITY;
        for i in 0..3 {
            for j in 0..3 {
                r.m[i * 4 + j] = m[j * 4 + i];
            }
        }
        let t = [m[3], m[7], m[11]];
        for i in 0..3 {
            r.m[i * 4 + 3] = -(r.m[i * 4] * t[0] + r.m[i * 4 + 1] * t[1] + r.m[i * 4 + 2] * t[2]);
        }
        r
    }

    /// `mulP`: `M * (p, 1)`.
    pub fn mul_p(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.m;
        [
            m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
            m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
            m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
        ]
    }

    /// `mulV`: `M * (v, 0)`.
    pub fn mul_v(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.m;
        [
            m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
            m[4] * v[0] + m[5] * v[1] + m[6] * v[2],
            m[8] * v[0] + m[9] * v[1] + m[10] * v[2],
        ]
    }
}

/// A 4 x 4 product of column-major `f64` matrices (GL's convention): `a * b`.
pub fn gl_mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    (DMat4::from_cols_array(a) * DMat4::from_cols_array(b)).to_cols_array()
}

#[cfg(test)]
#[path = "math_tests.rs"]
mod tests;
