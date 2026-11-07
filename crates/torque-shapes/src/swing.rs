//! Swung transitions (an addition of this crate, not part of Torque): a blend between two poses of the whole body that turns
//! the limbs in the shape's space rather than each under its parent.
//!
//! Torque blends a transition node by node, each node's rotation under its parent's. That is right while the body keeps
//! its place, but not when the whole body turns over as the limbs move: coming upright out of the water, the body turns 80
//! degrees up while each arm, raised over the head palm down, comes down to hang palm in. Under the chest the arm turns
//! almost half round, and the shorter way round takes it in across the head and through the chest.
//!
//! A swung blend ([`crate::instance::ShapeInstance::transition_to_sequence_swung`]) instead follows each limb that sits to a
//! side of its parent (an arm on the chest, a leg on the pelvis; [`side_of`]): the direction it runs along swings from where
//! it pointed to where it points in the shape's space, leaned out to its own side halfway by [`LEAN`] (more the further it
//! swings), and it turns about its own length the shorter way meanwhile. The other nodes (the elbows, the knees, the
//! chest) blend under their parents as Torque blends them.

use crate::instance::{SET_BITS, interpolate_quat};
use crate::math;
use crate::shape::Shape;

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// How far a limb swinging half round leans out to its side halfway (the tangent of the lean; a shorter swing leans less, by
/// the sine of half its angle).
const LEAN: f32 = 1.2;

/// The rotations of nodes `a..` (`rotations`, the pose the threads give them; at most [`SET_BITS`] of them) blended from
/// `reference` by `weights` (each node's share and whether it swings; none: the node is not in a transition).
pub(crate) fn blend(
    shape: &Shape,
    a: usize,
    weights: &[Option<(f32, bool)>],
    reference: impl Fn(usize) -> [f32; 4],
    rotations: &mut [[f32; 4]],
) {
    let count = weights.len().min(rotations.len()).min(SET_BITS);
    // Each node's turn in the shape's space: the pose it left, the pose it goes to, and the blend.
    let mut from = [IDENTITY; SET_BITS];
    let mut to = [IDENTITY; SET_BITS];
    let mut now = [IDENTITY; SET_BITS];
    for i in 0..count {
        let n = a + i;
        let parent = usize::try_from(shape.nodes[n].parent)
            .ok()
            .filter(|p| (a..n).contains(p))
            .map(|p| p - a);
        let up = |v: &[[f32; 4]]| parent.map_or(IDENTITY, |p| v[p]);
        let (pf, pt, pn) = (up(&from), up(&to), up(&now));
        let target = rotations[i];
        let start = if weights[i].is_some() {
            reference(n)
        } else {
            target
        };
        from[i] = compose(pf, start);
        to[i] = compose(pt, target);
        rotations[i] = match (weights[i], side_of(shape, n)) {
            (Some((p, true)), Some(side)) => {
                let out = Some(apply(pn, side));
                compose(
                    conjugate(pn),
                    limb(from[i], to[i], bone_axis(shape, n), out, p),
                )
            }
            (Some((p, _)), _) => interpolate_quat(start, target, p),
            (None, _) => target,
        };
        now[i] = compose(pn, rotations[i]);
    }
}

/// The rotation `a` then `b` under it, as Torque's quaternions turn: `quat_matrix(compose(a, b))` is
/// `quat_matrix(a) * quat_matrix(b)`.
pub(crate) fn compose(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [
        bw * ax + aw * bx + by * az - bz * ay,
        bw * ay + aw * by + bz * ax - bx * az,
        bw * az + aw * bz + bx * ay - by * ax,
        bw * aw - bx * ax - by * ay - bz * az,
    ]
}

/// The inverse of the unit rotation `q`.
pub(crate) fn conjugate([x, y, z, w]: [f32; 4]) -> [f32; 4] {
    [-x, -y, -z, w]
}

/// Direction `v` turned by `q`.
pub(crate) fn apply(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    math::mul_v(&math::quat_matrix(q), v)
}

/// The turn of `angle` radians about the unit `axis`, right-handed as [`apply`] turns.
pub(crate) fn about(axis: [f32; 3], angle: f32) -> [f32; 4] {
    let (s, c) = (angle / 2.0).sin_cos();
    [-axis[0] * s, -axis[1] * s, -axis[2] * s, c]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `v` made a unit long; none when it is all but zero.
fn unit(v: [f32; 3], least: f32) -> Option<[f32; 3]> {
    let l = dot(v, v).sqrt();
    (l > least).then(|| v.map(|c| c / l))
}

/// Which way node `n` runs (toward its first child, a unit vector), if it has a child away from its origin.
fn bone_axis(shape: &Shape, n: usize) -> Option<[f32; 3]> {
    let child = usize::try_from(shape.nodes.get(n)?.first_child).ok()?;
    unit(*shape.default_translations.get(child)?, 1e-4)
}

/// The side node `n` sits to on its parent (its place on the parent square to the way the parent runs, a unit vector), if
/// any: an arm's shoulder out from the chest, a leg's hip out from the pelvis.
fn side_of(shape: &Shape, n: usize) -> Option<[f32; 3]> {
    let parent = usize::try_from(shape.nodes.get(n)?.parent).ok()?;
    let t = *shape.default_translations.get(n)?;
    let along = bone_axis(shape, parent)?;
    let d = dot(t, along);
    unit([0, 1, 2].map(|k| t[k] - d * along[k]), 0.02)
}

/// The angle from unit direction `a` to `b` and the axis that turns one to the other: none when they point the same way,
/// any axis square to them when they point opposite ways.
fn between(a: [f32; 3], b: [f32; 3]) -> (f32, Option<[f32; 3]>) {
    let c = cross(a, b);
    let angle = dot(c, c).sqrt().atan2(dot(a, b));
    let axis = unit(c, 1e-6).or_else(|| {
        let other = if a[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        (angle > 1.0).then(|| unit(cross(a, other), 1e-6)).flatten()
    });
    (angle, axis)
}

/// Share `p` of the way from rotation `from` to `to` (in the shape's space) of a node running along `axis` whose side is
/// `out` (in the shape's space; see the module docs). Without an axis, the plain blend.
pub(crate) fn limb(
    from: [f32; 4],
    to: [f32; 4],
    axis: Option<[f32; 3]>,
    out: Option<[f32; 3]>,
    p: f32,
) -> [f32; 4] {
    let Some(axis) = axis else {
        return interpolate_quat(from, to, p);
    };
    let (a, b) = (apply(from, axis), apply(to, axis));
    if out.is_none() && dot(a, b) < -0.999 {
        return interpolate_quat(from, to, p);
    }
    // The way the direction goes: straight, or through the direction halfway leaned out to the side.
    let half = unit([0, 1, 2].map(|k| a[k] + b[k]), 1e-6);
    let lean = LEAN * ((1.0 - dot(a, b)) / 2.0).max(0.0).sqrt();
    let mid = match (half, out) {
        (Some(h), Some(o)) => unit([0, 1, 2].map(|k| h[k] + lean * o[k]), 1e-6),
        (None, o) => o,
        (Some(_), None) => None,
    };
    let (turns, legs) = match mid {
        Some(m) => ([between(a, m), between(m, b)], 2),
        None => ([between(a, b), (0.0, None)], 1),
    };
    let turns = &turns[..legs];
    let length: f32 = turns.iter().map(|t| t.0).sum();
    // The swing that share of the way along.
    let swing = |share: f32| {
        let mut left = share * length;
        let mut q = IDENTITY;
        for (angle, axis) in turns {
            let by = left.min(*angle);
            if let Some(axis) = axis {
                q = compose(about(*axis, by), q);
            }
            left -= by;
        }
        q
    };
    // What is left once the direction has swung (a turn about the direction it started in), taken the shorter way. Whatever
    // the swing leaves, the whole way ends on `to`.
    let rest = compose(conjugate(swing(1.0)), compose(to, conjugate(from)));
    compose(swing(p), compose(interpolate_quat(IDENTITY, rest, p), from))
}

#[cfg(test)]
#[path = "swing_tests.rs"]
mod tests;
