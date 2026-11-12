//! A scene object's transform maths: the matrix, the box an object box covers once moved (`m_matF_x_box3F_C`), the world box
//! and sphere `resetWorldBox` keeps, the affine inverse, a direction turned by the rotation, a point in an object's space and
//! the scale a scene object keeps. Each is called through the [`crate::library`] table; [`plain`] holds the plain `f32`
//! versions, in the order TGE's `math/mMath_C.cc` and `math/mMatrix.h` write them (Torque3D's `math/mMath_C.cpp` and
//! `math/mMatrix.h`, `Copyright (c) 2012 GarageGames, LLC`).

pub use crate::math::matrix::{IDENTITY, Matrix};

/// TGE's `m_matF_x_box3F_C`: the axis-aligned box of the box `min`, `max` moved by `m` (its rotation part and its translation).
pub fn mul_box(m: &Matrix, min: [f32; 3], max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    (crate::library::get().mul_box)(m, min, max)
}

/// `SceneObject::resetWorldBox`: the object box scaled by the object's scale, moved by the transform ([`mul_box`]); a result
/// whose max is below its min on any axis (a NaN does not count: the compares are false) is replaced by the box from `-1 -1 -1`
/// to `1 1 1`.
pub fn world_box(
    matrix: &Matrix,
    scale: [f32; 3],
    object_min: [f32; 3],
    object_max: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let scaled = |v: [f32; 3]| [scale[0] * v[0], scale[1] * v[1], scale[2] * v[2]];
    let (low, high) = mul_box(matrix, scaled(object_min), scaled(object_max));
    if high[0] < low[0] || high[1] < low[1] || high[2] < low[2] {
        return ([-1.0; 3], [1.0; 3]);
    }
    (low, high)
}

/// The centre of a box (`Box3F::getCenter`).
pub fn box_center(min: [f32; 3], max: [f32; 3]) -> [f32; 3] {
    (crate::library::get().box_center)(min, max)
}

/// The bounding sphere `SceneObject::resetWorldBox` keeps beside the world box: the centre is [`box_center`], the radius the
/// distance from it to the box's maximum corner.
pub fn world_sphere(min: [f32; 3], max: [f32; 3]) -> ([f32; 3], f32) {
    (crate::library::get().world_sphere)(min, max)
}

/// `MatrixF::affineInverse`: the rotation transposed and the translation the negated rotation-transposed translation (the
/// inverse of a transform whose rotation is orthonormal; the scale is kept apart).
pub fn affine_inverse(m: &Matrix) -> Matrix {
    (crate::library::get().affine_inverse)(m)
}

/// A world point as the object sees it, for a spatial index's ray test: the world-to-object matrix applied to the point, each
/// component divided by the object's scale.
pub fn to_object_space(w2o: &Matrix, scale: [f32; 3], p: [f32; 3]) -> [f32; 3] {
    (crate::library::get().to_object_space)(w2o, scale, p)
}

/// A direction turned by the rotation part of `m` (`MatrixF::mulV`).
pub fn rotate(m: &Matrix, v: [f32; 3]) -> [f32; 3] {
    (crate::library::get().rotate)(m, v)
}

/// What `SceneObject::setScale` keeps of a scale: the scale as given by default; a game may clamp it.
pub fn clamp_scale(scale: [f32; 3]) -> [f32; 3] {
    (crate::library::get().clamp_scale)(scale)
}

/// The transform maths in plain `f32`, in the order Torque's `mMath_C.cpp` and `mMatrix.h` write them.
pub mod plain {
    use super::Matrix;

    pub use crate::math::matrix::mul_box;

    /// `Box3F::getCenter`: `(min + max) * 0.5` per axis.
    pub fn box_center(min: [f32; 3], max: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| (min[i] + max[i]) * 0.5)
    }

    /// `resetWorldBox`'s sphere: the centre and `(max - centre).len()`.
    pub fn world_sphere(min: [f32; 3], max: [f32; 3]) -> ([f32; 3], f32) {
        let center = box_center(min, max);
        let d = [0, 1, 2].map(|i| max[i] - center[i]);
        (center, (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt())
    }

    pub use crate::math::matrix::affine_inverse;

    /// `MatrixF::mulP` then `Point3F::convolveInverse`: `(m[4i] x + m[4i+1] y + m[4i+2] z + m[4i+3]) / scale[i]`.
    pub fn to_object_space(w2o: &Matrix, scale: [f32; 3], p: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| {
            let r = 4 * i;
            (w2o[r] * p[0] + w2o[r + 1] * p[1] + w2o[r + 2] * p[2] + w2o[r + 3]) / scale[i]
        })
    }

    /// `MatrixF::mulV`.
    pub use crate::math::matrix::mul_v as rotate;

    /// The scale as given (TGE's `setScale` keeps it).
    pub fn keep_scale(scale: [f32; 3]) -> [f32; 3] {
        scale
    }
}
