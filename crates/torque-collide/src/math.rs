//! Vector and matrix arithmetic: `Point3F`, `MatrixF`, `PlaneF`, `QuatF` and `Box3F` as Torque's `mMath_C.cpp`,
//! `mMatrix.cpp`, `mQuat.cpp`, `mPlane.h`, `mPlaneTransformer.cpp` and `mBox.h` write them.
//!
//! The routines Torque calls through its math-library pointers go through [`crate::library`] here: the functions below with
//! the same names call the installed table, whose default ([`crate::library::PLAIN`]) is the [`plain`] module, plain `f32`
//! arithmetic in the source's order. The matrix product, the box transform and the transpose are plain `f32` code with no
//! table entry.
//!
//! The plain bodies live in [`torque_scene::math`], the engine's one copy of them, re-exported here: [`vec3`], [`quat`],
//! [`matrix`], [`transcendental`] and the finite-value guards ([`finite_clamp`], [`finite_coords`], [`all_finite`]). The
//! names of this module that go through the table stay table-routed; code that wants the plain arithmetic whatever table is
//! installed calls those modules.

pub use torque_scene::math::{
    all_finite, finite_clamp, finite_coords, matrix, quat, transcendental, vec3,
};

use crate::library;

/// A point or a vector (`Point3F`).
pub use torque_scene::math::vec3::F3;

/// A 4x4 matrix as `MatrixF` holds it (row-major, the translation in elements 3, 7 and 11), and the identity.
pub use torque_scene::math::matrix::{IDENTITY, Matrix};

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

/// `m_matF_x_matF` and `m_matF_transpose`: the matrix product (summed left to right) and the full 4x4 transpose, plain `f32`
/// with no table entry.
pub use torque_scene::math::matrix::{product as matrix_product, transpose};

/// `MatrixF::mulP` (through the table).
pub fn mul_p(m: &Matrix, p: F3) -> F3 {
    (library::get().mul_p)(m, p)
}

/// Where `m` puts the origin: its translation.
pub fn translation(m: &Matrix) -> F3 {
    [m[3], m[7], m[11]]
}

/// `MatrixF::mulV` (through the table).
pub fn mul_v(m: &Matrix, v: F3) -> F3 {
    (library::get().mul_v)(m, v)
}

/// `m_matF_x_box3F` (Graphics Gems I, 548): the box moved by `m` ([`matrix::mul_box`]).
pub fn mul_box(m: &Matrix, b: &Box3) -> Box3 {
    let (min, max) = matrix::mul_box(m, b.min, b.max);
    Box3 { min, max }
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

/// A quaternion `(x, y, z, w)` as `QuatF` stores it, the identity, `QuatF::normalize()` (a zero quaternion becomes the
/// identity) and `QuatF::set(const MatrixF&)` (the inverse of [`quat_matrix`]): [`quat`]'s.
pub use torque_scene::math::quat::{
    IDENTITY as QUAT_IDENTITY, Quat, from_matrix as plain_quat_from_matrix,
    normalize as plain_quat_normalize,
};

/// `m_point3F_interpolate` (through the table): `from * (1 - t) + to * t` per component.
pub fn interpolate(from: F3, to: F3, t: f32) -> F3 {
    (library::get().interpolate)(from, to, t)
}

/// The routines of [`crate::library::PLAIN`]: plain `f32` arithmetic, each written as its Torque routine is. Most are
/// [`torque_scene::math`]'s, re-exported.
pub mod plain {
    use super::{F3, Plane};

    pub use torque_scene::math::matrix::{
        affine_inverse, determinant, from_quat as quat_matrix, inverse, mul_p, mul_v, set_euler,
    };
    pub use torque_scene::math::vec3::{cross, dot, interpolate, is_zero, normalize};

    /// The `(S32)` cast: Rust's `as`, which cuts toward zero, saturates at the ends of the range and gives 0 for a NaN.
    pub fn float_to_int(x: f64) -> i32 {
        x as i32
    }

    /// The sphere around a world box (`resetWorldBox`): the centre `(min + max) * 0.5` and the radius, the length of
    /// `max - centre` (the scene's [`torque_scene::transform::plain::world_sphere`]).
    pub fn world_sphere(w: &super::Box3) -> (F3, f32) {
        torque_scene::transform::plain::world_sphere(w.min, w.max)
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

#[cfg(test)]
#[path = "math_tests.rs"]
mod tests;
