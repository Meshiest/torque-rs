//! How deep two convex polytopes overlap, by the separating axis test: the least distance one must move to clear the other,
//! the direction to move it, and the corners of their common volume. Unlike GJK's closest points and the features
//! `getCollisionInfo` builds from them, the answer stays exact while the two overlap, however deep.
//!
//! The axes tried are both polytopes' face normals and the cross products of their edge directions; along each, the two
//! projections give how far one must move each way to clear the other, and the least of those is the answer (a face axis
//! is kept over an edge axis less than [`EDGE_BIAS`] shallower, so a resting contact does not flicker between them). The
//! common volume's corners are each polytope's edges clipped to the other's slabs (the space between its two planes along
//! each face normal).

use crate::convex::ConvexKind;
use crate::map::MapShape;
use crate::math::{F3, Matrix, mul_p, mul_v};
use crate::mesh::HullAccel;

/// How much shallower an edge-pair axis must be than the best face axis to be taken instead (m).
pub const EDGE_BIAS: f32 = 1e-3;
/// The most corners [`penetration`] reports.
pub const MAX_POINTS: usize = 32;
/// The cap on the points, face normals and edges a [`Polytope`] keeps; more are dropped. A hull past it is tested on fewer
/// axes than it has, so a dropped face's axis cannot separate it: [`penetration`] may then report an overlap with something
/// only that face clears (and its corners may miss parts of the hull). A tree trunk or a buggy body has a few dozen.
pub const MAX_FEATURES: usize = 256;

/// A convex polytope: its points, its face normals (unit, no two parallel, either sense) and its edges (pairs of point
/// indices). Built through its methods, which keep every edge on points it holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Polytope {
    points: Vec<F3>,
    normals: Vec<F3>,
    edges: Vec<[u32; 2]>,
}

impl Polytope {
    pub fn points(&self) -> &[F3] {
        &self.points
    }

    pub fn normals(&self) -> &[F3] {
        &self.normals
    }

    pub fn edges(&self) -> &[[u32; 2]] {
        &self.edges
    }

    /// Adds the point `p`, unless the cap is reached.
    pub fn add_point(&mut self, p: F3) {
        if self.points.len() < MAX_FEATURES {
            self.points.push(p);
        }
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.normals.clear();
        self.edges.clear();
    }

    /// Adds the face normal `n` made unit, unless it is zero, not finite, parallel to one already held, or past the cap.
    pub fn add_normal(&mut self, n: F3) {
        if self.normals.len() >= MAX_FEATURES {
            return;
        }
        let Some(n) = unit(n) else { return };
        if !self.normals.iter().any(|m| dot(*m, n).abs() > 1.0 - 1e-5) {
            self.normals.push(n);
        }
    }

    /// Adds the edge from point `a` to point `b`, unless either is not a point, they are the same, it is held already or the
    /// cap is reached.
    pub fn add_edge(&mut self, a: u32, b: u32) {
        let n = self.points.len() as u32;
        if a == b || a >= n || b >= n || self.edges.len() >= MAX_FEATURES {
            return;
        }
        let e = [a.min(b), a.max(b)];
        if !self.edges.contains(&e) {
            self.edges.push(e);
        }
    }

    /// The least and greatest of the points along `axis`.
    pub fn extent(&self, axis: F3) -> (f32, f32) {
        self.points
            .iter()
            .map(|p| dot(*p, axis))
            .fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)))
    }

    /// Every point moved by `by`.
    pub fn translate(&mut self, by: F3) {
        for p in &mut self.points {
            *p = add(*p, by);
        }
    }

    /// A hull's accelerator as a polytope at `pose` scaled by `scale`: its points scaled then moved by `pose`, its face normals
    /// by the inverse transpose (each divided by the scale, then turned by `pose`; a zero scale drops them), and the edges its
    /// emit strings name.
    pub fn set_hull(&mut self, accel: &HullAccel, pose: &Matrix, scale: F3) {
        self.clear();
        let mat = crate::feature::mat_scale(pose, scale);
        self.points.extend(
            accel
                .verts
                .iter()
                .take(MAX_FEATURES)
                .map(|&v| mul_p(&mat, v)),
        );
        for &n in &accel.normals {
            self.add_normal(mul_v(
                pose,
                [n[0] / scale[0], n[1] / scale[1], n[2] / scale[2]],
            ));
        }
        // Each emit string: `nv`, its vertices, `ne`, its edges as pairs of indices into those vertices, then its faces.
        for s in &accel.emit {
            let Some(&nv) = s.first() else { continue };
            let nv = usize::from(nv);
            let Some(verts) = s.get(1..1 + nv) else {
                continue;
            };
            let Some(&ne) = s.get(1 + nv) else { continue };
            for k in 0..usize::from(ne) {
                let at = 2 + nv + 2 * k;
                let (Some(&i), Some(&j)) = (s.get(at), s.get(at + 1)) else {
                    break;
                };
                if let (Some(&a), Some(&b)) = (verts.get(usize::from(i)), verts.get(usize::from(j)))
                {
                    self.add_edge(u32::from(a), u32::from(b));
                }
            }
        }
    }

    /// A map piece as a polytope, in the world: an interior hull's points, its faces' normals and their edges; a terrain
    /// triangle's prism (its top, its sides straight down, its bottom [`TERRAIN_DEPTH`](crate::map::terrain::TERRAIN_DEPTH)
    /// under its lowest corner).
    pub fn set_map_piece(&mut self, shape: &MapShape, kind: &ConvexKind) {
        self.clear();
        self.points = shape.points(kind);
        self.points.truncate(MAX_FEATURES);
        if let ConvexKind::TerrainTri { .. } = kind {
            if self.points.len() != 6 {
                self.points.clear();
                return;
            }
            let p = self.points.clone();
            self.add_normal(cross(sub(p[1], p[0]), sub(p[2], p[0])));
            self.add_normal([0.0, 0.0, 1.0]);
            for k in 0..3u32 {
                let n = (k + 1) % 3;
                self.add_normal(cross(sub(p[n as usize], p[k as usize]), [0.0, 0.0, 1.0]));
                self.add_edge(k, n);
                self.add_edge(3 + k, 3 + n);
                self.add_edge(k, 3 + k);
            }
            return;
        }
        for (plane, verts) in shape.faces(kind) {
            self.add_normal([plane.x, plane.y, plane.z]);
            for k in 0..verts.len() {
                self.add_edge(verts[k], verts[(k + 1) % verts.len()]);
            }
        }
    }

    fn edge(&self, e: [u32; 2]) -> Option<(F3, F3)> {
        Some((
            *self.points.get(e[0] as usize)?,
            *self.points.get(e[1] as usize)?,
        ))
    }
}

/// How `a` and `b` overlap: [`penetration`]'s answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Penetration {
    /// How far `a` must move along `normal` to clear `b` (above 0).
    pub depth: f32,
    /// The unit direction to move `a` in.
    pub normal: F3,
    /// Corners of the common volume (at most [`MAX_POINTS`], all finite).
    pub points: Vec<F3>,
}

/// Whether `a` and `b` overlap (no face normal or edge-pair axis separates them, and each has a face normal and a point); when
/// they do, `out` holds the depth, the direction to move `a` out of `b`, and the corners of their common volume. Touching
/// (a depth of 0) or anything not finite is no overlap.
pub fn penetration(a: &Polytope, b: &Polytope, out: &mut Penetration) -> bool {
    out.points.clear();
    if a.points.is_empty() || b.points.is_empty() || a.normals.is_empty() || b.normals.is_empty() {
        return false;
    }
    let mut best: Option<(f32, F3)> = None;
    // The depth to clear along `axis` and its sense, or None when the axis separates them.
    let along = |axis: F3| -> Option<(f32, F3)> {
        let (alo, ahi) = a.extent(axis);
        let (blo, bhi) = b.extent(axis);
        let up = bhi - alo;
        let down = ahi - blo;
        if !(up > 0.0 && down > 0.0) {
            return None;
        }
        Some(if up <= down {
            (up, axis)
        } else {
            (down, axis.map(|c| -c))
        })
    };
    for &n in a.normals.iter().chain(&b.normals) {
        let (depth, dir) = match along(n) {
            Some(x) => x,
            None => return false,
        };
        if best.is_none_or(|(d, _)| depth < d) {
            best = Some((depth, dir));
        }
    }
    let dirs = |p: &Polytope| -> Vec<F3> {
        let mut out: Vec<F3> = Vec::new();
        for &e in &p.edges {
            let Some((s, t)) = p.edge(e) else { continue };
            if let Some(d) = unit(sub(t, s))
                && !out.iter().any(|m| dot(*m, d).abs() > 1.0 - 1e-5)
            {
                out.push(d);
            }
        }
        out
    };
    let (da, db) = (dirs(a), dirs(b));
    for &ea in &da {
        for &eb in &db {
            let Some(axis) = unit(cross(ea, eb)) else {
                continue;
            };
            let (depth, dir) = match along(axis) {
                Some(x) => x,
                None => return false,
            };
            if best.is_none_or(|(d, _)| depth < d - EDGE_BIAS) {
                best = Some((depth, dir));
            }
        }
    }
    let Some((depth, normal)) = best else {
        return false;
    };
    if !(depth.is_finite() && normal.iter().all(|c| c.is_finite())) {
        return false;
    }
    out.depth = depth;
    out.normal = normal;
    clip_edges(a, b, &mut out.points);
    clip_edges(b, a, &mut out.points);
    true
}

/// Each edge of `p` clipped to the slabs of `q`: the ends of what is left that are finite, while there is room.
fn clip_edges(p: &Polytope, q: &Polytope, out: &mut Vec<F3>) {
    let slabs: Vec<(F3, f32, f32)> = q
        .normals
        .iter()
        .map(|&n| {
            let (lo, hi) = q.extent(n);
            (n, lo, hi)
        })
        .collect();
    for &e in &p.edges {
        let Some((s, t)) = p.edge(e) else { continue };
        let d = sub(t, s);
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for &(n, lo, hi) in &slabs {
            let s0 = dot(n, s);
            let ds = dot(n, d);
            if ds == 0.0 {
                if !(s0 >= lo && s0 <= hi) {
                    t1 = -1.0;
                    break;
                }
                continue;
            }
            let (mut ta, mut tb) = ((lo - s0) / ds, (hi - s0) / ds);
            if ta > tb {
                std::mem::swap(&mut ta, &mut tb);
            }
            t0 = t0.max(ta);
            t1 = t1.min(tb);
            if !(t0 <= t1) {
                break;
            }
        }
        if !(t0 <= t1) {
            continue;
        }
        for t in [t0, t1] {
            if out.len() >= MAX_POINTS {
                return;
            }
            let at = add(s, d.map(|c| c * t));
            if at.iter().all(|c| c.is_finite()) {
                out.push(at);
            }
        }
    }
}

fn dot(a: F3, b: F3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: F3, b: F3) -> F3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn sub(a: F3, b: F3) -> F3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: F3, b: F3) -> F3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `v` made unit, or None when its length squared is not above 1e-6 or it is not finite.
fn unit(v: F3) -> Option<F3> {
    let l2 = dot(v, v);
    if !(l2 > 1e-6 && l2.is_finite()) {
        return None;
    }
    let k = 1.0 / l2.sqrt();
    Some(v.map(|c| c * k))
}

#[cfg(test)]
#[path = "sat_tests.rs"]
mod tests;
