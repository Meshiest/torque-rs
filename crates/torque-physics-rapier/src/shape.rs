//! What a body collides as, and how a description becomes a Rapier shape: checked, held to the limits, and a hull that cannot
//! be made (too many points, flat, degenerate) replaced by the box of its points.

use rapier3d::prelude::{Pose, SharedShape, Vector};

use crate::guard;
use crate::limits::Limits;
use crate::work;
use crate::world::Error;

/// A body's collision shape, in the body's space.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A box of half-sizes `half` round the body's origin.
    Box { half: [f32; 3] },
    /// A ball round the body's origin.
    Ball { radius: f32 },
    /// The convex hull of `points` (at most [`Limits::max_hull_vertices`]; more, or a hull that is flat or degenerate, is
    /// replaced by the box of the points).
    Hull { points: Vec<[f32; 3]> },
}

impl Shape {
    /// The hull of a shape's collision mesh (every part's points through its node transform, times `scale`), as the engine's
    /// own collision holds it.
    pub fn from_mesh_hull(hull: &torque_collide::mesh::Hull, scale: [f32; 3]) -> Shape {
        let mut points = Vec::new();
        for part in &hull.parts {
            for &v in &part.verts {
                let p = torque_collide::math::mul_p(&part.node, v);
                points.push([p[0] * scale[0], p[1] * scale[1], p[2] * scale[2]]);
            }
        }
        Shape::Hull { points }
    }
}

/// A shape made: the Rapier shape, where it sits in the body's space, and whether it stands in for a hull that could not be
/// made.
pub(crate) struct Built {
    pub shape: SharedShape,
    pub offset: [f32; 3],
    pub replaced: bool,
}

/// Runs `f`, turning a panic into an error (native builds; a wasm32 build aborts first).
pub(crate) fn caught<T>(f: impl FnOnce() -> T) -> Result<T, Error> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| Error::Panicked)
}

/// A box of half-sizes `half` (each held to the limits) at `offset`.
fn cuboid(half: [f32; 3], offset: [f32; 3], limits: &Limits, replaced: bool) -> Built {
    let h = half.map(|x| x.clamp(limits.min_half_extent, limits.max_half_extent));
    Built {
        shape: SharedShape::cuboid(h[0], h[1], h[2]),
        offset,
        replaced,
    }
}

/// The box of `points` (finite, already held to the limits).
fn box_of(points: &[[f32; 3]], limits: &Limits) -> Built {
    if points.is_empty() {
        return cuboid([limits.min_half_extent; 3], [0.0; 3], limits, true);
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let half = [0, 1, 2].map(|k| (hi[k] - lo[k]) * 0.5);
    let mid = [0, 1, 2].map(|k| lo[k] * 0.5 + hi[k] * 0.5);
    cuboid(half, mid, limits, true)
}

/// `shape` as a Rapier shape (module docs). The points are paid for before they are looked at.
pub(crate) fn build(
    shape: &Shape,
    limits: &Limits,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Built, Error> {
    match shape {
        Shape::Box { half } => {
            if !guard::finite(half) {
                return Err(Error::NonFinite);
            }
            Ok(cuboid(half.map(f32::abs), [0.0; 3], limits, false))
        }
        Shape::Ball { radius } => {
            let r = guard::scalar(radius.abs(), limits.min_half_extent, limits.max_half_extent)
                .ok_or(Error::NonFinite)?;
            Ok(Built {
                shape: SharedShape::ball(r),
                offset: [0.0; 3],
                replaced: false,
            })
        }
        Shape::Hull { points } => {
            if !charge(work::VERTEX.saturating_mul(points.len() as u64)) {
                return Err(Error::Exhausted);
            }
            let mut held = Vec::with_capacity(points.len().min(limits.max_hull_vertices));
            for &p in points {
                held.push(guard::coords(p, limits.max_half_extent).ok_or(Error::NonFinite)?);
            }
            if held.len() > limits.max_hull_vertices || held.len() < 4 {
                return Ok(box_of(&held, limits));
            }
            let fallback = box_of(&held, limits);
            // A hull thinner than the thinnest box along an axis is flat: its box (thickened) is what collides.
            let flat = fallback
                .shape
                .as_cuboid()
                .is_some_and(|c| c.half_extents.min_element() <= limits.min_half_extent);
            if flat {
                return Ok(fallback);
            }
            if !charge(work::HULL_POINT.saturating_mul(held.len() as u64)) {
                return Err(Error::Exhausted);
            }
            // The hull is made about the points' middle, so its corners are small numbers whatever the offset.
            let mid = fallback.offset;
            let local: Vec<Vector> = held
                .iter()
                .map(|p| Vector::new(p[0] - mid[0], p[1] - mid[1], p[2] - mid[2]))
                .collect();
            let made = caught(|| SharedShape::convex_hull(&local))?;
            let Some(hull) = made else {
                return Ok(fallback);
            };
            if !sound(&hull) {
                return Ok(fallback);
            }
            Ok(Built {
                shape: hull,
                offset: mid,
                replaced: false,
            })
        }
    }
}

/// Whether a made hull has a volume and finite mass properties (a sliver can come out of the hull with none).
fn sound(shape: &SharedShape) -> bool {
    let props = shape.mass_properties(1.0);
    let aabb = shape.compute_aabb(&Pose::IDENTITY);
    let mass = props.mass();
    mass.is_finite()
        && mass > 1e-9
        && props.local_com.is_finite()
        && aabb.mins.is_finite()
        && aabb.maxs.is_finite()
}
