//! What the container's structures read of a scene object, and how their work is paid for, and the ray tests the walks make.
//!
//! [`plain_ray_box`] follows TGE's `Box3F::collideLine` (Torque3D's `math/mBox.cpp`, `Copyright (c) 2012 GarageGames, LLC`) and
//! [`plain_ray_point`] `m_point3F_interpolate_C` (Torque3D's `math/mMath_C.cpp`, `Copyright (c) 2012 GarageGames, LLC`); the
//! code here is this crate's own.

/// The caller's payment for work: `false` when the run's budget cannot pay any more.
pub type Charge<'a> = &'a mut dyn FnMut(u64) -> bool;

/// The walk could not be paid for: the run's step budget is used up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exhausted;

/// What a walk reads of a scene object, live, each time it needs it (the object's fields).
pub trait Geometry {
    /// The world sphere: its centre and radius.
    fn sphere(&self, object: u32) -> ([f32; 3], f32);
    /// The world box: its minimum and maximum.
    fn world_box(&self, object: u32) -> ([f32; 3], [f32; 3]);
    /// The type mask.
    fn type_mask(&self, object: u32) -> u32;
    /// Whether collisions are disabled for the object: a walk and a ray skip it.
    fn collision_disabled(&self, object: u32) -> bool;
}

/// Whether the segment `a` to `b` passes through the box (`Box3F::collideLine`, the answer only), through the
/// [`crate::library`] table.
pub fn ray_box(min: [f32; 3], max: [f32; 3], a: [f32; 3], b: [f32; 3]) -> bool {
    (crate::library::get().ray_box)(min, max, a, b)
}

/// [`ray_box`] in plain `f32`, as TGE's `Box3F::collideLine` writes it: per axis the entry and exit fractions of the slab, the
/// latest entry and the earliest exit kept; no hit when the segment misses a slab or the exit comes before the entry.
pub fn plain_ray_box(min: [f32; 3], max: [f32; 3], a: [f32; 3], b: [f32; 3]) -> bool {
    let mut first = 0.0f32;
    let mut last = 1.0f32;
    for i in 0..3 {
        let (s, e) = (a[i], b[i]);
        let (st, et);
        if s < e {
            if s > max[i] || e < min[i] {
                return false;
            }
            let di = e - s;
            st = if s < min[i] { (min[i] - s) / di } else { 0.0 };
            et = if e > max[i] { (max[i] - s) / di } else { 1.0 };
        } else {
            if e > max[i] || s < min[i] {
                return false;
            }
            let di = e - s;
            st = if s > max[i] { (max[i] - s) / di } else { 0.0 };
            et = if e < min[i] { (min[i] - s) / di } else { 1.0 };
        }
        if st > first {
            first = st;
        }
        if et < last {
            last = et;
        }
        if last < first {
            return false;
        }
    }
    true
}

/// The "no hit yet" distance of a ray: 2.0 (a hit is within 0 to 1).
pub const NO_HIT: f32 = 2.0;

/// The point at `t` along the segment `start` to `end` (`Point3F::interpolate`), through the [`crate::library`] table.
pub fn ray_point(start: [f32; 3], end: [f32; 3], t: f32) -> [f32; 3] {
    (crate::library::get().ray_point)(start, end, t)
}

/// [`ray_point`] in plain `f32`, as TGE's `m_point3F_interpolate` writes it: `start * (1 - t) + end * t` per axis.
pub fn plain_ray_point(start: [f32; 3], end: [f32; 3], t: f32) -> [f32; 3] {
    let inverse = 1.0 - t;
    [0, 1, 2].map(|i| start[i] * inverse + end[i] * t)
}
