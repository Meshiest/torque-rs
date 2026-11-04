//! The shape queries' math library table: the arithmetic of the node poses and the collision queries, and the sort of the
//! threads, called through one set of function pointers.
//!
//! Torque calls its vector and matrix routines through pointers (`mMathFn.h`), which its installers point at one
//! implementation or another. This crate does the same for the routines a host may want computed differently (another
//! rounding, another evaluation order, another sort): every entry of [`ShapeLibrary`] is called through [`get`]. [`PLAIN`] holds
//! this crate's own code, plain `f32` arithmetic in the order Torque's source writes it and a stable sort; a host installs
//! another table once, before its first query, with [`install`].

use std::sync::OnceLock;

use crate::math::Matrix;

/// A thread comparison as `dQsort` takes one: negative, zero or positive.
pub type ThreadCompare<'a> = &'a dyn Fn(&usize, &usize) -> i32;

/// The routines the shape code calls through the table. Each entry names the Torque routine it stands for.
#[derive(Debug, Clone, Copy)]
pub struct ShapeLibrary {
    /// `MatrixF::mulP`.
    pub mul_p: fn(&Matrix, [f32; 3]) -> [f32; 3],
    /// `MatrixF::mulV`.
    pub mul_v: fn(&Matrix, [f32; 3]) -> [f32; 3],
    /// `m_matF_inverse`.
    pub inverse: fn(&Matrix) -> Matrix,
    /// `QuatF::setMatrix`.
    pub quat_matrix: fn([f32; 4]) -> Matrix,
    /// `m_point3F_normalize`.
    pub normalize: fn([f32; 3]) -> [f32; 3],
    /// `mDot`.
    pub dot: fn([f32; 3], [f32; 3]) -> f32,
    /// `mCross`.
    pub cross: fn([f32; 3], [f32; 3]) -> [f32; 3],
    /// `PlaneF::distToPlane` (the plane as `x, y, z, d`).
    pub plane_dist: fn(&[f32; 4], [f32; 3]) -> f32,
    /// `EarlyOutPolyList::addPoint`'s test: the point strictly in front of the plane.
    pub in_front: fn(&[f32; 4], [f32; 3]) -> bool,
    /// `a * b + c` (the clip point `start + diff * t`).
    pub mul_add: fn(f32, f32, f32) -> f32,
    /// `a * b / c` (`TSThread::advanceTime`'s step `timeScale * delta / duration`).
    pub mul_div: fn(f32, f32, f32) -> f32,
    /// `TSTransform::interpolate(F32)`: `p1 + t * (p2 - p1)`.
    pub lerp: fn(f32, f32, f32) -> f32,
    /// `TSTransform::interpolate(QuatF)`: the blend with a fast renormalisation.
    pub interpolate_quat: fn([f32; 4], [f32; 4], f32) -> [f32; 4],
    /// `TSMesh::castRay`'s test that both ends are on one side of a plane: `dot1 * dot2 > 0`.
    pub same_side: fn(f32, f32) -> bool,
    /// `TSMesh::castRay`'s comparison of two fractions across: `a * b * sgn < c * d * sgn`.
    pub ray_time_less: fn(f32, f32, f32, f32, f32) -> bool,
    /// The `(S32)` cast of a float.
    pub to_int: fn(f32) -> i32,
    /// `TSShapeInstance::sortThreads`' sort of the thread list (`dQsort` with `compareThreads`).
    pub sort_threads: fn(&mut [usize], ThreadCompare<'_>),
}

/// This crate's own routines: plain `f32` arithmetic in the order Torque's source writes it, and a stable sort.
pub static PLAIN: ShapeLibrary = ShapeLibrary {
    mul_p: crate::math::plain::mul_p,
    mul_v: crate::math::plain::mul_v,
    inverse: crate::math::plain::inverse,
    quat_matrix: crate::math::plain::quat_matrix,
    normalize: crate::math::plain::normalize,
    dot: crate::math::plain::dot,
    cross: crate::math::plain::cross,
    plane_dist: plain_plane_dist,
    in_front: plain_in_front,
    mul_add: plain_mul_add,
    mul_div: plain_mul_div,
    lerp: plain_lerp,
    interpolate_quat: crate::instance::plain_interpolate_quat,
    same_side: plain_same_side,
    ray_time_less: plain_ray_time_less,
    to_int: plain_to_int,
    sort_threads: plain_sort_threads,
};

/// `PlaneF::distToPlane`: `(x*p.x + y*p.y + z*p.z) + d`.
pub fn plain_plane_dist(pl: &[f32; 4], p: [f32; 3]) -> f32 {
    (pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2]) + pl[3]
}

/// `distToPlane(p) > 0`.
pub fn plain_in_front(pl: &[f32; 4], p: [f32; 3]) -> bool {
    plain_plane_dist(pl, p) > 0.0
}

/// `a * b + c`.
pub fn plain_mul_add(a: f32, b: f32, c: f32) -> f32 {
    a * b + c
}

/// `a * b / c`.
pub fn plain_mul_div(a: f32, b: f32, c: f32) -> f32 {
    a * b / c
}

/// `p1 + t * (p2 - p1)`.
pub fn plain_lerp(p1: f32, p2: f32, t: f32) -> f32 {
    p1 + t * (p2 - p1)
}

/// `dot1 * dot2 > 0`.
pub fn plain_same_side(dot1: f32, dot2: f32) -> bool {
    dot1 * dot2 > 0.0
}

/// `a * b * sgn < c * d * sgn`.
pub fn plain_ray_time_less(a: f32, b: f32, c: f32, d: f32, sgn: f32) -> bool {
    a * b * sgn < c * d * sgn
}

/// The cast as Rust defines it: toward zero, saturating, NaN to 0.
pub fn plain_to_int(x: f32) -> i32 {
    x as i32
}

/// A stable sort by the comparison: equal threads keep their order.
pub fn plain_sort_threads(v: &mut [usize], cmp: ThreadCompare<'_>) {
    v.sort_by(|a, b| cmp(a, b).cmp(&0));
}

static INSTALLED: OnceLock<&'static ShapeLibrary> = OnceLock::new();

/// The table in use: the installed one, else [`PLAIN`].
#[inline]
pub fn get() -> &'static ShapeLibrary {
    match INSTALLED.get() {
        Some(lib) => lib,
        None => &PLAIN,
    }
}

/// Installs `library` for the rest of the process. Only the first install takes; installing the same table again is a
/// no-op. Returns false when another table was installed first.
pub fn install(library: &'static ShapeLibrary) -> bool {
    std::ptr::eq(*INSTALLED.get_or_init(|| library), library)
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
