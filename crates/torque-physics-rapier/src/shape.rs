//! What a body collides as, and how a description becomes a Rapier shape: checked, held to the limits, and a hull that cannot
//! be made (too many points, flat, degenerate) replaced by the box of its points. A compound of hulls is each of its hulls made
//! so, held together at the poses they were given; a compound of more hulls than the limit is replaced by the box of all its
//! points.

use std::rc::Rc;

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
    /// Several convex hulls held together as one shape, each a [`Shape::Hull`] of its points in the body's space (at most
    /// [`Limits::max_compound_hulls`]; more, or none, is replaced by the box of all the points). The hulls may overlap and need
    /// not touch; a body of one collides where any of them does.
    Compound { hulls: Rc<[Vec<[f32; 3]>]> },
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

    /// The volume of the shape as the world would make it (a hull that cannot be made counted as its box; a compound's hulls
    /// that overlap counted twice, as the mass they add is).
    ///
    /// The work of making the shape is charged to `charge`, as a body's is.
    ///
    /// # Errors
    /// [`Error::NonFinite`] for a value that is not finite or past the limits, [`Error::Exhausted`] past the budget.
    pub fn volume(
        &self,
        limits: &Limits,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<f32, Error> {
        let built = build(self, limits, charge)?;
        Ok(built.shape.mass_properties(1.0).mass())
    }
}

/// A shape made: the Rapier shape, where it sits in the body's space, and whether it stands in for a hull that could not be
/// made.
pub(crate) struct Built {
    pub shape: SharedShape,
    pub offset: [f32; 3],
    pub replaced: bool,
    /// Hulls the shape is made of (a compound's; 1 for any other).
    pub parts: usize,
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
        parts: 1,
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
                parts: 1,
            })
        }
        Shape::Hull { points } => build_hull(points, limits, charge),
        Shape::Compound { hulls } => build_compound(hulls, limits, charge),
    }
}

/// The points of a hull, paid for and then held to the limits.
fn held_points(
    points: &[[f32; 3]],
    limits: &Limits,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Vec<[f32; 3]>, Error> {
    if !charge(work::VERTEX.saturating_mul(points.len() as u64)) {
        return Err(Error::Exhausted);
    }
    let mut held = Vec::with_capacity(points.len().min(limits.max_hull_vertices));
    for &p in points {
        held.push(guard::coords(p, limits.max_half_extent).ok_or(Error::NonFinite)?);
    }
    Ok(held)
}

/// The convex hull of `points`, or their box when the hull cannot be made.
fn build_hull(
    points: &[[f32; 3]],
    limits: &Limits,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Built, Error> {
    let held = held_points(points, limits, charge)?;
    hull_of_held(&held, limits, charge)
}

/// [`build_hull`] of points that are paid for and held already.
fn hull_of_held(
    held: &[[f32; 3]],
    limits: &Limits,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Built, Error> {
    if held.len() > limits.max_hull_vertices || held.len() < 4 {
        return Ok(box_of(held, limits));
    }
    let fallback = box_of(held, limits);
    // A hull thinner than the thinnest box along an axis is flat: its box (thickened) is what collides.
    let flat = fallback
        .shape
        .as_cuboid()
        .is_some_and(|c| c.half_extents.min_element() <= limits.min_half_extent);
    if flat || is_degenerate(held, 2.0 * f64::from(limits.min_half_extent)) {
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
        parts: 1,
    })
}

/// A compound of the hulls of `hulls`, each made as [`build_hull`] makes one and placed at its own middle. More hulls than the
/// limit (or none) is the box of every point.
fn build_compound(
    hulls: &[Vec<[f32; 3]>],
    limits: &Limits,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Built, Error> {
    let mut held = Vec::with_capacity(hulls.len());
    for points in hulls {
        held.push(held_points(points, limits, charge)?);
    }
    if held.is_empty() || held.len() > limits.max_compound_hulls {
        let all: Vec<[f32; 3]> = held.into_iter().flatten().collect();
        return Ok(box_of(&all, limits));
    }
    let mut parts = Vec::with_capacity(held.len());
    let mut replaced = false;
    for points in &held {
        if !charge(work::HULL) {
            return Err(Error::Exhausted);
        }
        let built = hull_of_held(points, limits, charge)?;
        replaced |= built.replaced;
        let at = Vector::new(built.offset[0], built.offset[1], built.offset[2]);
        parts.push((Pose::from_translation(at), built.shape));
    }
    let count = parts.len();
    let shape = caught(|| SharedShape::compound(parts))?;
    Ok(Built {
        shape,
        offset: [0.0; 3],
        replaced,
        parts: count,
    })
}

/// Whether `points` lie within `thickness` of one point, one line or one plane at any tilt: no volume for a hull to have. The
/// hull builder must never see them (a panic in it is caught on native builds only; a wasm32 build aborts), so this check, in
/// `f64` and from the points alone, is the guard there.
fn is_degenerate(points: &[[f32; 3]], thickness: f64) -> bool {
    type V = [f64; 3];
    let sub = |a: V, b: V| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: V, b: V| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: V, b: V| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let all: Vec<V> = points.iter().map(|q| q.map(f64::from)).collect();
    let Some(&first) = all.first() else {
        return true;
    };
    // The point farthest from the first, then the one farthest from the line through them, then how far the rest stand from the
    // plane of the three.
    let farthest = |dist: &dyn Fn(V) -> f64| {
        all.iter()
            .map(|&q| (dist(q), q))
            .fold((0.0, first), |a, b| if b.0 > a.0 { b } else { a })
    };
    let (reach, far) = farthest(&|q| {
        let d = sub(q, first);
        dot(d, d).sqrt()
    });
    if reach <= thickness {
        return true;
    }
    let axis = sub(far, first);
    let (off, away) = farthest(&|q| {
        let c = cross(sub(q, first), axis);
        dot(c, c).sqrt() / reach
    });
    if off <= thickness {
        return true;
    }
    let normal = cross(axis, sub(away, first));
    let len = dot(normal, normal).sqrt();
    len <= 0.0
        || all
            .iter()
            .all(|&q| (dot(sub(q, first), normal) / len).abs() <= thickness)
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
