//! The scene library table: the container's arithmetic and its search sort, called through one set of function pointers.
//!
//! Torque calls its vector and matrix routines through pointers (`mMathFn.h`), which an installer points at one
//! implementation or another. This crate does the same for the routines of the container and of a scene object's transform:
//! every entry of [`SceneLibrary`] is called through [`get`]. [`PLAIN`] holds this crate's own code, plain `f32` arithmetic in
//! the order Torque's source writes it and a stable sort; a host installs another table once, before its first query, with
//! [`install`].

use std::sync::OnceLock;

use crate::bins::{self, BinGeometry, CellVisit};
use crate::geometry::{self, Charge, Exhausted};
use crate::query;
use crate::transform::{self, Matrix};

/// A point or a vector.
pub type Point = [f32; 3];
/// A box's two corners, or a box's minimum and maximum.
pub type Corners = (Point, Point);
/// The cell walk of a ray: the grid, the segment, the budget and the visit of each cell.
pub type RayCells =
    fn(&BinGeometry, Point, Point, Charge, &mut CellVisit<'_>) -> Result<(), Exhausted>;
/// A sort of `(distance, index)` pairs, each compare paid for first through the second argument.
pub type DistanceSort = fn(&mut [(f32, u32)], &mut dyn FnMut() -> bool) -> Result<(), Exhausted>;

/// The routines the container and the transform maths call through the table. Each entry names the Torque routine it stands
/// for.
#[derive(Debug, Clone, Copy)]
pub struct SceneLibrary {
    /// `getBinRange` (`sceneObject.cc`) on one axis of a grid.
    pub bin_range: fn(&BinGeometry, f32, f32) -> (u32, u32),
    /// The cells `Container::castRay` visits after the overflow list: one column, one row, or the general walk.
    pub ray_cells: RayCells,
    /// `MatrixF::mulP` then `Point3F::convolveInverse`: a world point in an object's space as `Container::castRay` moves it.
    pub object_point: fn(&Matrix, Point, Point) -> Point,
    /// The same move for a spatial index's own ray test.
    pub to_object_space: fn(&Matrix, Point, Point) -> Point,
    /// `m_matF_x_box3F`: the box a box covers once moved.
    pub mul_box: fn(&Matrix, Point, Point) -> Corners,
    /// `Box3F::getCenter`.
    pub box_center: fn(Point, Point) -> Point,
    /// `SceneObject::resetWorldBox`'s sphere of the world box: its centre and radius.
    pub world_sphere: fn(Point, Point) -> (Point, f32),
    /// `m_matF_affineInverse`.
    pub affine_inverse: fn(&Matrix) -> Matrix,
    /// `MatrixF::mulV`.
    pub rotate: fn(&Matrix, Point) -> Point,
    /// What `SceneObject::setScale` makes of a scale before it keeps it (Torque keeps it as given).
    pub clamp_scale: fn(Point) -> Point,
    /// `Box3F::collideLine`, the answer only.
    pub ray_box: fn(Point, Point, Point, Point) -> bool,
    /// `Point3F::interpolate`: the point at `t` along a segment.
    pub ray_point: fn(Point, Point, f32) -> Point,
    /// The distance from a point to a world box's centre (the search sort, `containerSearchCurrDist`).
    pub center_distance: fn(Point, Point, Point) -> f32,
    /// `containerSearchCurrRadiusDist`: the centre distance less the box's smallest side, not below zero.
    pub radius_distance: fn(Point, Point, Point) -> f32,
    /// The squared distance from a point to a world box, `initRadiusSearch`'s filter.
    pub box_distance_squared: fn(Point, Point, Point) -> f32,
    /// `initRadiusSearch`'s test of a squared distance against the radius.
    pub within_radius: fn(f32, f32) -> bool,
    /// The search list's sort by distance (`dQsort` with `cmpSearchPointers`): `(distance, index)` pairs, each compare paid for
    /// first through the second argument ([`Exhausted`] when it refuses; the slice then holds the same pairs in some order).
    pub sort_by_distance: DistanceSort,
}

/// This crate's own routines: plain `f32` arithmetic in the order Torque's source writes it, and a stable sort.
pub static PLAIN: SceneLibrary = SceneLibrary {
    bin_range: bins::plain_bin_range,
    ray_cells: bins::plain_ray_cells,
    object_point: transform::plain::to_object_space,
    to_object_space: transform::plain::to_object_space,
    mul_box: transform::plain::mul_box,
    box_center: transform::plain::box_center,
    world_sphere: transform::plain::world_sphere,
    affine_inverse: transform::plain::affine_inverse,
    rotate: transform::plain::rotate,
    clamp_scale: transform::plain::keep_scale,
    ray_box: geometry::plain_ray_box,
    ray_point: geometry::plain_ray_point,
    center_distance: query::plain::center_distance,
    radius_distance: query::plain::radius_distance,
    box_distance_squared: query::plain::box_distance_squared,
    within_radius: query::plain::within_radius,
    sort_by_distance: query::plain::sort_by_distance,
};

static INSTALLED: OnceLock<&'static SceneLibrary> = OnceLock::new();

/// The table in use: the installed one, else [`PLAIN`].
#[inline]
pub fn get() -> &'static SceneLibrary {
    match INSTALLED.get() {
        Some(lib) => lib,
        None => &PLAIN,
    }
}

/// Installs `library` for the rest of the process. Only the first install takes; installing the same table again is a
/// no-op. Returns false when another table was installed first.
pub fn install(library: &'static SceneLibrary) -> bool {
    std::ptr::eq(*INSTALLED.get_or_init(|| library), library)
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
