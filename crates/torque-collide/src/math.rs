//! Vector and matrix arithmetic: `Point3F`, `MatrixF`, `PlaneF`, `QuatF` and `Box3F` as Torque's `mMath_C.cpp`,
//! `mMatrix.cpp`, `mQuat.cpp`, `mPlane.h`, `mPlaneTransformer.cpp` and `mBox.h` write them.
//!
//! The routines Torque calls through its math-library pointers go through [`crate::library`] here: the functions below with
//! the same names call the installed table, whose default ([`crate::library::PLAIN`]) is the [`plain`] module, plain `f32`
//! arithmetic in the source's order. The matrix product, the box transform and the transpose are plain `f32` code with no
//! table entry.

use crate::library;

/// A point or a vector (`Point3F`).
pub type F3 = [f32; 3];

/// A 4x4 matrix as `MatrixF` holds it: row-major, the translation in the last column (elements 3, 7 and 11).
pub type Matrix = [f32; 16];

/// The identity (`MatrixF::identity`).
pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// An axis-aligned box (`Box3F`): `min` then `max`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Box3 {
    pub min: F3,
    pub max: F3,
}

impl Box3 {
    pub const fn new(min: F3, max: F3) -> Box3 {
        Box3 { min, max }
    }

    /// `Box3F::isOverlapped` as the callers inline it: false when any `other.min > self.max` or
    /// `other.max < self.min`; a NaN compares false, so it never separates.
    pub fn overlaps(&self, other: &Box3) -> bool {
        !(self.max[0] < other.min[0]
            || self.max[1] < other.min[1]
            || self.max[2] < other.min[2]
            || other.max[0] < self.min[0]
            || other.max[1] < self.min[1]
            || other.max[2] < self.min[2])
    }
}

/// A plane (`PlaneF`): the normal `x, y, z` and `d`, a point `p` on it when `n . p + d == 0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Plane {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub d: f32,
}

impl Plane {
    pub const fn new(x: f32, y: f32, z: f32, d: f32) -> Plane {
        Plane { x, y, z, d }
    }

    pub fn normal(&self) -> F3 {
        [self.x, self.y, self.z]
    }

    /// `PlaneF::distToPlane`.
    pub fn dist(&self, p: F3) -> f32 {
        (library::get().plane_dist)(self, p)
    }

    /// `PlaneF::set(point, normal)`: the normal normalised, `d = -(p . n)`.
    pub fn from_point_normal(point: F3, normal: F3) -> Plane {
        (library::get().plane_from_point_normal)(point, normal)
    }
}

/// A float widened to `f64` (exact).
#[inline]
pub fn d(x: f32) -> f64 {
    f64::from(x)
}

/// The `(S32)` cast of a float (through the table): truncation toward zero. [`plain`] saturates at the ends of the range and
/// gives 0 for a NaN.
pub fn float_to_int(x: f64) -> i32 {
    (library::get().float_to_int)(x)
}

/// `SceneObject::resetWorldBox`'s sphere of a world box (through the table): the centre and the radius.
pub fn world_sphere(w: &Box3) -> (F3, f32) {
    (library::get().world_sphere)(w)
}

/// `mSqrt`.
pub fn sqrt_f32(x: f32) -> f32 {
    x.sqrt()
}

/// `m_matF_x_matF` (`default_matF_x_matF_C`): row `i` of the result is `a[i0]*b[0j] + a[i1]*b[1j] + a[i2]*b[2j] +
/// a[i3]*b[3j]`, summed left to right.
pub fn matrix_product(a: &Matrix, b: &Matrix) -> Matrix {
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

/// `MatrixF::mulP` (through the table).
pub fn mul_p(m: &Matrix, p: F3) -> F3 {
    (library::get().mul_p)(m, p)
}

/// `MatrixF::mulV` (through the table).
pub fn mul_v(m: &Matrix, v: F3) -> F3 {
    (library::get().mul_v)(m, v)
}

/// `m_matF_x_box3F` (Graphics Gems I, 548): the box moved by `m`. Each axis starts at the translation; for each element `r`
/// of the row `a = r * min[j]`, `b = r * max[j]`; when `a < b` the minimum gains `a` and the maximum `b`, else the minimum
/// gains `b` and the maximum `a`.
pub fn mul_box(m: &Matrix, b: &Box3) -> Box3 {
    let omin = b.min;
    let omax = b.max;
    let mut min = [m[3], m[7], m[11]];
    let mut max = min;
    for i in 0..3 {
        for j in 0..3 {
            let r = m[4 * i + j];
            let a = r * omin[j];
            let bb = r * omax[j];
            if a < bb {
                min[i] += a;
                max[i] += bb;
            } else {
                min[i] += bb;
                max[i] += a;
            }
        }
    }
    Box3 { min, max }
}

/// `m_matF_transpose`: the full 4x4 transpose.
pub fn transpose(m: &Matrix) -> Matrix {
    let mut o = *m;
    for i in 0..4 {
        for j in 0..4 {
            o[i * 4 + j] = m[j * 4 + i];
        }
    }
    o
}

/// `m_matF_determinant` of the 3x3 part (through the table).
pub fn determinant(m: &Matrix) -> f32 {
    (library::get().determinant)(m)
}

/// `m_matF_affineInverse` (through the table): the 3x3 part transposed and the translation `-(R^T t)`. Elements 12-15
/// are left as they were.
pub fn affine_inverse(m: &Matrix) -> Matrix {
    (library::get().affine_inverse)(m)
}

/// `m_matF_inverse` (through the table): the inverse of the affine matrix `m` by cofactors over its determinant and the
/// translation turned back. Elements 12-15 are left as they were.
pub fn inverse(m: &Matrix) -> Matrix {
    (library::get().inverse)(m)
}

/// `m_point3F_normalize` (through the table): a zero vector gives `(0, 0, 1)`.
pub fn normalize(v: F3) -> F3 {
    (library::get().normalize)(v)
}

/// `mDot` (through the table).
pub fn dot(a: F3, b: F3) -> f32 {
    (library::get().dot)(a, b)
}

/// `mCross` (through the table).
pub fn cross(a: F3, b: F3) -> F3 {
    (library::get().cross)(a, b)
}

/// `Point3F::isZero` (through the table): every component's square at most `POINT_EPSILON`.
pub fn is_zero(v: F3) -> bool {
    (library::get().is_zero)(v)
}

/// `PlaneF::set(k, j, l)` (through the table): the normal of `(k - j) x (l - j)` normalised (a zero one gives `(0, 0, 1)`),
/// through `j`.
pub fn plane_from_points(p1: F3, p2: F3, p3: F3) -> Plane {
    (library::get().plane_from_points)(p1, p2, p3)
}

/// `PlaneTransformer` (`set` and `transform`): moves a plane from an object's space to the world as the polygon
/// lists do for `addPlane`, `plane(PlaneF)` and the interest test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneTransformer {
    transform: Matrix,
    scale: F3,
    transpose_inverse: Matrix,
}

impl Default for PlaneTransformer {
    fn default() -> Self {
        PlaneTransformer::new(&IDENTITY, [1.0, 1.0, 1.0])
    }
}

impl PlaneTransformer {
    /// `set`: keeps the matrix and the scale, and `transpose(matrix * diag(scale))` inverted.
    pub fn new(transform: &Matrix, scale: F3) -> PlaneTransformer {
        let mut s = IDENTITY;
        s[0] = scale[0];
        s[5] = scale[1];
        s[10] = scale[2];
        let ti = inverse(&transpose(&matrix_product(transform, &s)));
        PlaneTransformer {
            transform: *transform,
            scale,
            transpose_inverse: ti,
        }
    }

    /// `transform`: the point `n * -d` convolved with the scale and moved by the matrix (`mulP`), the normal turned by the
    /// transposed inverse (`mulV`), then `PlaneF::set(point, normal)`.
    pub fn transform(&self, plane: &Plane) -> Plane {
        let nd = -plane.d;
        let point = [
            plane.x * nd * self.scale[0],
            plane.y * nd * self.scale[1],
            plane.z * nd * self.scale[2],
        ];
        let point = mul_p(&self.transform, point);
        let normal = mul_v(&self.transpose_inverse, [plane.x, plane.y, plane.z]);
        Plane::from_point_normal(point, normal)
    }
}

/// `m_matF_set_euler` (`MatrixF::set(EulerF)`, through the table): an axis counts when its angle is not zero (a NaN counts).
/// No axis: the identity; x, y or z alone: that rotation; any other mix: the general form. No translation.
pub fn set_euler(e: F3) -> Matrix {
    (library::get().set_euler)(e)
}

/// `QuatF::setMatrix` (through the table): the quaternion `(x, y, z, w)` as a matrix with no translation; a vector part
/// whose squared length is below `10e-20` gives the identity.
pub fn quat_matrix(q: [f32; 4]) -> Matrix {
    (library::get().quat_matrix)(q)
}

/// A quaternion `(x, y, z, w)` as `QuatF` stores it.
pub type Quat = [f32; 4];

/// The identity quaternion.
pub const QUAT_IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

/// `QuatF::normalize()`: a zero quaternion becomes the identity.
pub fn plain_quat_normalize(q: &Quat) -> Quat {
    let [x, y, z, w] = *q;
    let l = (x * x + y * y + z * z + w * w).sqrt();
    if l == 0.0 {
        return QUAT_IDENTITY;
    }
    [x / l, y / l, z / l, w / l]
}

/// `QuatF::set(const MatrixF&)`: from the trace when it is positive, else from the largest diagonal element. The inverse of
/// [`quat_matrix`].
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

/// `m_point3F_interpolate` (through the table): `from * (1 - t) + to * t` per component.
pub fn interpolate(from: F3, to: F3, t: f32) -> F3 {
    (library::get().interpolate)(from, to, t)
}

/// The routines of [`crate::library::PLAIN`]: plain `f32` arithmetic, each written as its Torque routine is.
pub mod plain {
    use super::{F3, IDENTITY, Matrix, Plane, transcendental};

    /// The `(S32)` cast: Rust's `as`, which cuts toward zero, saturates at the ends of the range and gives 0 for a NaN.
    pub fn float_to_int(x: f64) -> i32 {
        x as i32
    }

    /// The sphere around a world box (`resetWorldBox`): the centre `(min + max) * 0.5` and the radius, the length of
    /// `max - centre`.
    pub fn world_sphere(w: &super::Box3) -> (F3, f32) {
        let c: F3 = std::array::from_fn(|i| (w.min[i] + w.max[i]) * 0.5);
        let d: F3 = std::array::from_fn(|i| w.max[i] - c[i]);
        (c, (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt())
    }

    /// `m_matF_x_point3F`.
    pub fn mul_p(m: &Matrix, p: F3) -> F3 {
        [
            m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
            m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
            m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
        ]
    }

    /// `m_matF_x_vectorF`.
    pub fn mul_v(m: &Matrix, v: F3) -> F3 {
        [
            m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
            m[4] * v[0] + m[5] * v[1] + m[6] * v[2],
            m[8] * v[0] + m[9] * v[1] + m[10] * v[2],
        ]
    }

    /// `m_matF_determinant_C`.
    pub fn determinant(m: &Matrix) -> f32 {
        m[0] * (m[5] * m[10] - m[6] * m[9])
            + m[4] * (m[2] * m[9] - m[1] * m[10])
            + m[8] * (m[1] * m[6] - m[2] * m[5])
    }

    /// `m_matF_inverse_C`.
    pub fn inverse(m: &Matrix) -> Matrix {
        let det = determinant(m);
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

    /// `m_matF_affineInverse_C`.
    pub fn affine_inverse(m: &Matrix) -> Matrix {
        let t = *m;
        let mut o = *m;
        o[1] = t[4];
        o[4] = t[1];
        o[2] = t[8];
        o[8] = t[2];
        o[6] = t[9];
        o[9] = t[6];
        o[3] = -(t[0] * t[3] + t[4] * t[7] + t[8] * t[11]);
        o[7] = -(t[1] * t[3] + t[5] * t[7] + t[9] * t[11]);
        o[11] = -(t[2] * t[3] + t[6] * t[7] + t[10] * t[11]);
        o
    }

    /// `m_point3F_normalize_C`.
    pub fn normalize(p: F3) -> F3 {
        let squared = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        if squared != 0.0 {
            let factor = 1.0 / squared.sqrt();
            [p[0] * factor, p[1] * factor, p[2] * factor]
        } else {
            [0.0, 0.0, 1.0]
        }
    }

    /// `mDot`.
    pub fn dot(a: F3, b: F3) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    /// `mCross`.
    pub fn cross(a: F3, b: F3) -> F3 {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    /// `Point3F::isZero`: `x*x <= POINT_EPSILON` for each component, `POINT_EPSILON` being the double `1e-4`.
    pub fn is_zero(v: F3) -> bool {
        v.iter().all(|&x| f64::from(x * x) <= 1e-4)
    }

    /// `m_point3F_interpolate_C`.
    pub fn interpolate(from: F3, to: F3, factor: f32) -> F3 {
        let inverse = 1.0 - factor;
        [
            from[0] * inverse + to[0] * factor,
            from[1] * inverse + to[1] * factor,
            from[2] * inverse + to[2] * factor,
        ]
    }

    /// `mSinCos`: `mSin` and `mCos`, the double functions of the float angle, each a float.
    fn sin_cos(angle: f32) -> (f32, f32) {
        let a = f64::from(angle);
        (transcendental::sin(a) as f32, transcendental::cos(a) as f32)
    }

    /// `m_matF_set_euler_C`.
    pub fn set_euler(e: F3) -> Matrix {
        let mut m = IDENTITY;
        let axes = (e[0] != 0.0) as u32 | ((e[1] != 0.0) as u32) << 1 | ((e[2] != 0.0) as u32) << 2;
        match axes {
            0 => {}
            1 => {
                let (sx, cx) = sin_cos(e[0]);
                m[5] = cx;
                m[6] = sx;
                m[9] = -sx;
                m[10] = cx;
            }
            2 => {
                let (sy, cy) = sin_cos(e[1]);
                m[0] = cy;
                m[2] = -sy;
                m[8] = sy;
                m[10] = cy;
            }
            4 => {
                let (sz, cz) = sin_cos(e[2]);
                m[0] = cz;
                m[1] = sz;
                m[4] = -sz;
                m[5] = cz;
            }
            _ => {
                let (sx, cx) = sin_cos(e[0]);
                let (sy, cy) = sin_cos(e[1]);
                let (sz, cz) = sin_cos(e[2]);
                let r1 = cy * cz;
                let r2 = cy * sz;
                let r3 = sy * cz;
                let r4 = sy * sz;
                m[0] = r1 - (r4 * sx);
                m[1] = r2 + (r3 * sx);
                m[2] = -cx * sy;
                m[4] = -cx * sz;
                m[5] = cx * cz;
                m[6] = sx;
                m[8] = r3 + (r2 * sx);
                m[9] = r4 - (r1 * sx);
                m[10] = cx * cy;
            }
        }
        m
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

    /// `PlaneF::distToPlane`: `(x*p.x + y*p.y + z*p.z) + d`.
    pub fn plane_dist(plane: &Plane, p: F3) -> f32 {
        (plane.x * p[0] + plane.y * p[1] + plane.z * p[2]) + plane.d
    }

    /// `PlaneF::set(p, n)`: the normal normalised (`Point3F::normalize`), `d = -(p.x*x + p.y*y + p.z*z)`.
    pub fn plane_from_point_normal(p: F3, n: F3) -> Plane {
        let [x, y, z] = normalize(n);
        Plane::new(x, y, z, -(p[0] * x + p[1] * y + p[2] * z))
    }

    /// `PlaneF::set(k, j, l)`: `a = k - j`, `b = l - j`, the normal `a x b` normalised (`m_point3F_normalize`), through `j`.
    pub fn plane_from_points(k: F3, j: F3, l: F3) -> Plane {
        let ax = k[0] - j[0];
        let ay = k[1] - j[1];
        let az = k[2] - j[2];
        let bx = l[0] - j[0];
        let by = l[1] - j[1];
        let bz = l[2] - j[2];
        let [x, y, z] = normalize([ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx]);
        Plane::new(x, y, z, -(j[0] * x + j[1] * y + j[2] * z))
    }
}

/// Transcendentals, as Torque's `mSin`, `mCos`, `mAcos`, `mAtan` and `mPow` call the C library's double functions.
pub mod transcendental {
    pub fn sin(x: f64) -> f64 {
        x.sin()
    }
    pub fn cos(x: f64) -> f64 {
        x.cos()
    }
    pub fn acos(x: f64) -> f64 {
        x.acos()
    }
    pub fn atan2(y: f64, x: f64) -> f64 {
        y.atan2(x)
    }
    pub fn pow(x: f64, y: f64) -> f64 {
        x.powf(y)
    }
}

#[cfg(test)]
#[path = "math_tests.rs"]
mod tests;
