//! The math library table: the arithmetic the collision code calls through one set of function pointers.
//!
//! Torque calls its vector and matrix routines through pointers (`mMathFn.h`: `m_matF_x_matF`, `m_point3F_normalize` and the
//! rest), which `mInstallLibrary_C` and the processor-specific installers point at one implementation or another. This crate
//! does the same for the routines a host may install other implementations of: every
//! entry of [`MathLibrary`] is called through [`get`]. [`PLAIN`] holds this crate's own code, plain `f32` arithmetic in the
//! order Torque's source writes it; a host installs another table once, before its first query, with [`install`].

use std::sync::OnceLock;

use crate::gjk::GjkState;
use crate::math::{F3, Matrix, Plane};

/// The routines the collision code calls through the table. Each entry names the Torque routine it stands for.
#[derive(Debug, Clone, Copy)]
pub struct MathLibrary {
    /// `MatrixF::mulP`.
    pub mul_p: fn(&Matrix, F3) -> F3,
    /// `MatrixF::mulV`.
    pub mul_v: fn(&Matrix, F3) -> F3,
    /// `m_matF_determinant` (the 3x3 part).
    pub determinant: fn(&Matrix) -> f32,
    /// `m_matF_inverse`.
    pub inverse: fn(&Matrix) -> Matrix,
    /// `m_matF_affineInverse`.
    pub affine_inverse: fn(&Matrix) -> Matrix,
    /// `m_point3F_normalize`.
    pub normalize: fn(F3) -> F3,
    /// `mDot`.
    pub dot: fn(F3, F3) -> f32,
    /// `mCross`.
    pub cross: fn(F3, F3) -> F3,
    /// `Point3F::isZero`.
    pub is_zero: fn(F3) -> bool,
    /// `m_point3F_interpolate`.
    pub interpolate: fn(F3, F3, f32) -> F3,
    /// `m_matF_set_euler`.
    pub set_euler: fn(F3) -> Matrix,
    /// `QuatF::setMatrix`.
    pub quat_matrix: fn([f32; 4]) -> Matrix,
    /// `PlaneF::distToPlane`.
    pub plane_dist: fn(&Plane, F3) -> f32,
    /// `PlaneF::set(point, normal)`.
    pub plane_from_point_normal: fn(F3, F3) -> Plane,
    /// `PlaneF::set(k, j, l)`.
    pub plane_from_points: fn(F3, F3, F3) -> Plane,
    /// `GjkCollisionState::compute_det`.
    pub gjk_compute_det: fn(&mut GjkState),
    /// `GjkCollisionState::distance`'s convergence test: `|dist - mu| <= dist * rel_error`.
    pub gjk_converged: fn(f32, f32) -> bool,
    /// `MatrixF::mulP` as `getFeatures` moves a feature's vertices.
    pub feature_vertex: fn(&Matrix, F3) -> F3,
    /// `MatrixF::mulV` as a hull's `getFeatures` turns its face normals.
    pub feature_normal: fn(&Matrix, F3) -> F3,
    /// `MatrixF::mulV` as `Convex::getCollisionInfo` turns the separating vector into each convex's space.
    pub state_mul_v: fn(&Matrix, F3) -> F3,
    /// `sqrDistanceEdges`: the squared distance of two segments and their closest points (see `feature`).
    pub sqr_distance_edges: fn(F3, F3, F3, F3) -> (f32, F3, F3),
    /// `AbstractPolyList::isInterestedInPlane`'s test: whether a world plane faces against the interest normal.
    pub interested: fn(F3, &Plane) -> bool,
    /// The point where an edge meets a plane (the polygon lists' clip).
    pub clip_point: fn(F3, F3, &Plane) -> F3,
    /// `ExtrudedPolyList::testPoly`'s time from the first and second passes' distances and the face's reach.
    pub extruded_time: fn(f32, f32, f32) -> f32,
    /// `ExtrudedPolyList::end`'s comparison of a face's time with the list's: (later than the list's by more than
    /// `EqualEpsilon`, earlier than it by more than `EqualEpsilon`).
    pub time_against_list: fn(f32, f32) -> (bool, bool),
    /// `TSShape::computeAccelerator`'s test of two face normals (their dot).
    pub accel_normal_dot: fn(F3, F3) -> f32,
    /// The `(S32)` cast of a float: cut toward zero; what a value beyond the 32-bit range gives is the table's choice.
    pub float_to_int: fn(f64) -> i32,
    /// `SceneObject::resetWorldBox`'s sphere of a world box: its centre and radius.
    pub world_sphere: fn(&crate::math::Box3) -> (F3, f32),
}

/// This crate's own routines: plain `f32` arithmetic in the order Torque's source writes it.
pub static PLAIN: MathLibrary = MathLibrary {
    mul_p: crate::math::plain::mul_p,
    mul_v: crate::math::plain::mul_v,
    determinant: crate::math::plain::determinant,
    inverse: crate::math::plain::inverse,
    affine_inverse: crate::math::plain::affine_inverse,
    normalize: crate::math::plain::normalize,
    dot: crate::math::plain::dot,
    cross: crate::math::plain::cross,
    is_zero: crate::math::plain::is_zero,
    interpolate: crate::math::plain::interpolate,
    set_euler: crate::math::plain::set_euler,
    quat_matrix: crate::math::plain::quat_matrix,
    plane_dist: crate::math::plain::plane_dist,
    plane_from_point_normal: crate::math::plain::plane_from_point_normal,
    plane_from_points: crate::math::plain::plane_from_points,
    gjk_compute_det: crate::gjk::plain_compute_det,
    gjk_converged: crate::gjk::plain_converged,
    feature_vertex: crate::math::plain::mul_p,
    feature_normal: crate::math::plain::mul_v,
    state_mul_v: crate::math::plain::mul_v,
    sqr_distance_edges: crate::feature::plain_sqr_distance_edges,
    interested: crate::poly::plain_interested,
    clip_point: crate::poly::plain_clip_point,
    extruded_time: crate::poly::plain_extruded_time,
    time_against_list: crate::poly::plain_time_against_list,
    accel_normal_dot: crate::math::plain::dot,
    float_to_int: crate::math::plain::float_to_int,
    world_sphere: crate::math::plain::world_sphere,
};

static INSTALLED: OnceLock<&'static MathLibrary> = OnceLock::new();

/// The table in use: the installed one, else [`PLAIN`].
#[inline]
pub fn get() -> &'static MathLibrary {
    match INSTALLED.get() {
        Some(lib) => lib,
        None => &PLAIN,
    }
}

/// Installs `library` for the rest of the process. Only the first install takes; installing the same table again is a
/// no-op. Returns false when another table was installed first.
pub fn install(library: &'static MathLibrary) -> bool {
    std::ptr::eq(*INSTALLED.get_or_init(|| library), library)
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
