//! `ConvexFeature` and the contact search of the vehicles (`Convex::updateStateList`, `Convex::getCollisionInfo`
//! and `ConvexFeature::collide`).
//!
//! A feature is the part of a convex nearest a direction: a point, an edge or a face of a box (`BoxConvex::getFeatures`),
//! or the vertices, edges and faces an accelerator's emit string names for a hull (`ShapeBaseConvex::getFeatures`, the same
//! code for every hull class). Two features in contact give collisions: each one's
//! vertices against the other's faces (`testVertex` with `isInside`), then edge against edge
//! (`sqrDistanceEdges`, whose determinant is the absolute value of its `S32` cast, as a float).
//!
//! The feature's matrix products and `sqrDistanceEdges` go through the math library table ([`crate::library`]).

use crate::collision::{Collision, CollisionList, MAX_COLLISIONS};
use crate::convex::{
    BOX_CONVEX_CORNERS, BOX_CONVEX_FACE_AXES, BOX_CONVEX_FACES, Convex, ConvexKind, World,
};
use crate::gjk::GjkState;
use crate::hash::{IdMap, IdSet};
use crate::library;
use crate::math::vec3::sub;
use crate::math::{
    F3, IDENTITY, Matrix, Plane, affine_inverse, cross, determinant, dot, inverse, matrix_product,
};
use crate::mesh::{FeatureFace, HullAccel};
use crate::{Exhausted, ObjectId, work};

/// States one mover's list may hold (cap; one per convex of its working set).
pub const MAX_STATES: usize = 4_096;
/// Vertices, edges or faces one feature may hold (cap).
pub const MAX_FEATURE_ITEMS: usize = 4_096;

/// `ConvexFeature`: vertices in the world, edges and faces over them, and the material and object a contact reports.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConvexFeature {
    pub verts: Vec<F3>,
    pub edges: Vec<[u32; 2]>,
    pub faces: Vec<FeatureFace>,
    pub material: u32,
    pub object: Option<ObjectId>,
}

/// `mat.mulP(p)` as the features move their vertices (through the table).
fn feature_mul_p(m: &Matrix, p: F3) -> F3 {
    (library::get().feature_vertex)(m, p)
}

/// `BoxConvex::getVertex`: `center + (+-size)` with corner `k`'s bits (bit 0 for x).
fn box_vertex(c: F3, s: F3, k: u32) -> F3 {
    let pick = |ci: f32, si: f32, bit: u32| ci + if k & bit != 0 { si } else { -si };
    [
        pick(c[0], s[0], 1),
        pick(c[1], s[1], 2),
        pick(c[2], s[2], 4),
    ]
}

impl ConvexFeature {
    /// `BoxConvex::getFeatures`: the corner whose bits follow the signs of `n` (each `>= 0`, a NaN not), the plane
    /// through it along `n` (`PlaneF::set`), and a bit for each of the corner's three neighbours on that plane (`isOnPlane`:
    /// `mDot(plane, p) + d` strictly between -0.1 and 0.1): none emits the corner, one an edge to that neighbour, two the face
    /// they share; all three nothing.
    pub fn of_box(center: F3, size: F3, object: ObjectId, mat: &Matrix, n: F3) -> ConvexFeature {
        let mut cf = ConvexFeature {
            object: Some(object),
            ..Default::default()
        };
        let v = (n[0] >= 0.0) as u32 + 2 * (n[1] >= 0.0) as u32 + 4 * (n[2] >= 0.0) as u32;
        let p = box_vertex(center, size, v);
        let plane = Plane::from_point_normal(p, n);
        let corner = BOX_CONVEX_CORNERS[v as usize];
        let near = |k: u32| {
            let dist = f64::from(dot(plane.normal(), box_vertex(center, size, k)) + plane.d);
            dist < 0.1 && dist > -0.1
        };
        let code =
            near(corner[0]) as u32 | (near(corner[1]) as u32) << 1 | (near(corner[2]) as u32) << 2;
        match code {
            0 => cf.verts.push(feature_mul_p(mat, p)),
            1 => cf.emit_edge(center, size, v, corner[0], mat),
            2 => cf.emit_edge(center, size, v, corner[1], mat),
            3 => cf.emit_face(center, size, corner[3], mat),
            4 => cf.emit_edge(center, size, v, corner[2], mat),
            5 => cf.emit_face(center, size, corner[4], mat),
            6 => cf.emit_face(center, size, corner[5], mat),
            _ => {}
        }
        cf
    }

    /// `emitEdge`: two corners moved by `mat` and the edge between them.
    fn emit_edge(&mut self, c: F3, s: F3, a: u32, b: u32, mat: &Matrix) {
        let vc = self.verts.len() as u32;
        self.verts.push(feature_mul_p(mat, box_vertex(c, s, a)));
        self.verts.push(feature_mul_p(mat, box_vertex(c, s, b)));
        self.edges.push([vc, vc + 1]);
    }

    /// `emitFace`: the face's four corners moved by `mat`, its four edges, and two triangles whose normal is the
    /// matrix's column for the face's axis (negated for a flipped face).
    fn emit_face(&mut self, c: F3, s: F3, face: u32, mat: &Matrix) {
        let Some(fv) = BOX_CONVEX_FACES.get(face as usize) else {
            return;
        };
        let vc = self.verts.len() as u32;
        for &k in fv {
            self.verts.push(feature_mul_p(mat, box_vertex(c, s, k)));
        }
        for e in 0..4u32 {
            self.edges.push([vc + e, vc + ((e + 1) & 3)]);
        }
        let (axis, flip) = BOX_CONVEX_FACE_AXES[face as usize];
        let mut normal = [mat[axis], mat[axis + 4], mat[axis + 8]];
        if flip {
            normal = [-normal[0], -normal[1], -normal[2]];
        }
        self.faces.push(FeatureFace {
            normal,
            vertex: [vc, vc + 1, vc + 2],
        });
        self.faces.push(FeatureFace {
            normal,
            vertex: [vc, vc + 2, vc + 3],
        });
    }

    /// `ShapeBaseConvex::getFeatures` (any hull class): the accelerator vertex with the largest
    /// dot with `n` names an emit string; its vertices moved by `mat`, its edges and its faces (normals turned by `mat`) are the
    /// feature. The indices are not offset by the vertices already in the feature (it starts empty here).
    pub fn of_hull(accel: &HullAccel, object: ObjectId, mat: &Matrix, n: F3) -> ConvexFeature {
        let mut cf = ConvexFeature {
            object: Some(object),
            ..Default::default()
        };
        let Some(s) = accel.emit.get(accel.support_index(n)) else {
            return cf;
        };
        let mut pos = 0usize;
        let mut next = || {
            let b = s.get(pos).copied();
            pos += 1;
            b
        };
        let nv = next().unwrap_or(0);
        for _ in 0..nv {
            let Some(i) = next() else { return cf };
            let v = accel.verts.get(i as usize).copied().unwrap_or([0.0; 3]);
            cf.verts.push(feature_mul_p(mat, v));
        }
        let ne = next().unwrap_or(0);
        for _ in 0..ne {
            let (Some(a), Some(b)) = (next(), next()) else {
                return cf;
            };
            cf.edges.push([u32::from(a), u32::from(b)]);
        }
        let nf = next().unwrap_or(0);
        for _ in 0..nf {
            let (Some(f), Some(a), Some(b), Some(c)) = (next(), next(), next(), next()) else {
                return cf;
            };
            let nrm = accel.normals.get(f as usize).copied().unwrap_or([0.0; 3]);
            let normal = (library::get().feature_normal)(mat, nrm);
            cf.faces.push(FeatureFace {
                normal,
                vertex: [u32::from(a), u32::from(b), u32::from(c)],
            });
        }
        cf
    }

    fn vert(&self, i: u32) -> F3 {
        self.verts.get(i as usize).copied().unwrap_or([0.0; 3])
    }

    /// `inVolume`: no face has the point strictly in front (`mDot(normal, v - p0) > 0`).
    pub fn in_volume(&self, v: F3) -> bool {
        for f in &self.faces {
            let p0 = self.vert(f.vertex[0]);
            if dot(f.normal, sub(v, p0)) > 0.0 {
                return false;
            }
        }
        true
    }

    /// `testVertex`: each face (while the list has room) whose plane the vertex is within `tol` of, with the vertex
    /// inside its three edges, adds a contact at the vertex along the face's normal (negated when `flip`).
    fn test_vertex(&self, v: F3, list: &mut CollisionList, flip: bool, tol: f32) {
        for f in &self.faces {
            if list.count >= MAX_COLLISIONS {
                return;
            }
            let p0 = self.vert(f.vertex[0]);
            let p1 = self.vert(f.vertex[1]);
            let p2 = self.vert(f.vertex[2]);
            let dist = dot(f.normal, sub(v, p0));
            if dist > tol || dist < -tol {
                continue;
            }
            if is_inside(v, p0, p1, f.normal)
                && is_inside(v, p1, p2, f.normal)
                && is_inside(v, p2, p0, f.normal)
            {
                let normal = if flip {
                    [-f.normal[0], -f.normal[1], -f.normal[2]]
                } else {
                    f.normal
                };
                list.push(Collision {
                    object: self.object,
                    point: v,
                    normal,
                    material: self.material,
                    face: 0,
                    face_dot: 0.0,
                    distance: dist,
                });
            }
        }
    }

    /// `ConvexFeature::collide`: this feature's vertices against `other`'s faces, `other`'s against this one's (the
    /// contact reports `other`'s material and object), then every edge pair within `tol` (squared): the closest points,
    /// the distance negated when either point is inside the other volume, the normal `(i1 - i2) / distance` (no zero test).
    ///
    /// Every vertex against the other's faces and every edge pair (with its two volume tests) is paid through `charge` before it
    /// is done; a refused charge stops the call with what the list holds (`Exhausted`), as a full list does.
    pub fn collide(
        &self,
        other: &ConvexFeature,
        list: &mut CollisionList,
        tol: f32,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), Exhausted> {
        let per_vertex_a = work::FEATURE * (other.faces.len() as u64 + 1);
        for &v in &self.verts {
            if list.count >= MAX_COLLISIONS {
                return Ok(());
            }
            if !charge(per_vertex_a) {
                return Err(Exhausted);
            }
            other.test_vertex(v, list, false, tol);
        }
        let per_vertex_b = work::FEATURE * (self.faces.len() as u64 + 1);
        for &v in &other.verts {
            if list.count >= MAX_COLLISIONS {
                return Ok(());
            }
            if !charge(per_vertex_b) {
                return Err(Exhausted);
            }
            let before = list.count;
            self.test_vertex(v, list, true, tol);
            if list.count != before && list.count > 0 {
                let last = &mut list.entries[list.count - 1];
                last.material = other.material;
                last.object = other.object;
            }
        }
        if self.edges.is_empty() {
            return Ok(());
        }
        let tol_sq = tol * tol;
        let per_pair = work::FEATURE * (self.faces.len() as u64 + other.faces.len() as u64 + 1);
        for e in &self.edges {
            let (s1, e1) = (self.vert(e[0]), self.vert(e[1]));
            for oe in &other.edges {
                if list.count >= MAX_COLLISIONS {
                    return Ok(());
                }
                if !charge(per_pair) {
                    return Err(Exhausted);
                }
                let (s2, e2) = (other.vert(oe[0]), other.vert(oe[1]));
                let (sq, i1, i2) = sqr_distance_edges(s1, e1, s2, e2);
                if sq > tol_sq {
                    continue;
                }
                let mut dist = sq.sqrt();
                if other.in_volume(i1) || self.in_volume(i2) {
                    dist = -dist;
                }
                let inv = 1.0 / dist;
                let normal = sub(i1, i2).map(|c| c * inv);
                list.push(Collision {
                    object: other.object,
                    point: i1,
                    normal,
                    material: other.material,
                    face: 0,
                    face_dot: 0.0,
                    distance: dist,
                });
            }
        }
        Ok(())
    }
}

/// `isInside`: `mDot(mCross(n, b - a), p - a) < 0`.
fn is_inside(p: F3, a: F3, b: F3, n: F3) -> bool {
    dot(cross(n, sub(b, a)), sub(p, a)) < 0.0
}

/// `sqrDistanceEdges` (through the table): the squared distance between the segments `s0-e0` and `s1-e1` and their closest
/// points, when those lie inside both segments; else 1e10.
pub fn sqr_distance_edges(s0: F3, e0: F3, s1: F3, e1: F3) -> (f32, F3, F3) {
    (library::get().sqr_distance_edges)(s0, e0, s1, e1)
}

/// `sqrDistanceEdges` as TGE's `convex.cc` writes it: the determinant is `mAbs((S32)(a00*a11 - a01*a01))`, so near-parallel
/// edges with a determinant below 1 count as parallel (the threshold is 1e-5); `2.0*b0` and `2.0*b1` are doubles, so the
/// squared distance is summed in double precision.
pub fn plain_sqr_distance_edges(s0: F3, e0: F3, s1: F3, e1: F3) -> (f32, F3, F3) {
    use crate::math::plain::dot;
    let none = (1e10f32, [0.0; 3], [0.0; 3]);
    let d0 = sub(e0, s0);
    let a00 = dot(d0, d0);
    let d1 = sub(e1, s1);
    let a11 = dot(d1, d1);
    let a01 = -dot(d0, d1);
    let kd = sub(s0, s1);
    let c = dot(kd, kd);
    let b0 = dot(kd, d0);
    let det = ((a00 * a11 - a01 * a01) as i32).wrapping_abs() as f32;
    if !(f64::from(det) >= 0.00001) {
        return none;
    }
    let b1 = -dot(kd, d1);
    let s = a01 * b1 - a11 * b0;
    let t = a01 * b0 - a00 * b1;
    if !(s >= 0.0 && s <= det && t >= 0.0 && t <= det) {
        return none;
    }
    let inv = (1.0 / f64::from(det)) as f32;
    let s = s * inv;
    let t = t * inv;
    let sq = (f64::from(s) * (f64::from(a00 * s + a01 * t) + 2.0 * f64::from(b0))
        + f64::from(t) * (f64::from(a01 * s + a11 * t) + 2.0 * f64::from(b1))
        + f64::from(c)) as f32;
    let is = [s0[0] + d0[0] * s, s0[1] + d0[1] * s, s0[2] + d0[2] * s];
    let it = [s1[0] + d1[0] * t, s1[1] + d1[1] * t, s1[2] + d1[2] * t];
    (sq.abs(), is, it)
}

/// The features of a convex at `mat` toward `n` (`getFeatures`).
pub fn features(world: &dyn World, c: &Convex, mat: &Matrix, n: F3) -> ConvexFeature {
    match c.kind {
        ConvexKind::Box { center, size } | ConvexKind::OrthoBox { center, size } => {
            ConvexFeature::of_box(center, size, c.object, mat, n)
        }
        ConvexKind::Hull { slot, .. } => match world.hull(c.object, slot) {
            Some(h) => match &h.accel {
                Some(a) => ConvexFeature::of_hull(a, c.object, mat, n),
                None => ConvexFeature {
                    object: Some(c.object),
                    ..Default::default()
                },
            },
            None => ConvexFeature {
                object: Some(c.object),
                ..Default::default()
            },
        },
        // A map piece is in the world's space: `mat` is the identity (its transform and unit scale).
        ConvexKind::TerrainTri { .. } | ConvexKind::InteriorHull { .. } => {
            match world.map_shape(c.object) {
                Some(m) => m.feature(&c.kind, c.object, n),
                None => ConvexFeature {
                    object: Some(c.object),
                    ..Default::default()
                },
            }
        }
    }
}

/// `MatrixF::scale` (`m_matF_scale`): the first three columns times the scale (row 3 too).
pub fn mat_scale(m: &Matrix, s: F3) -> Matrix {
    let mut o = *m;
    for r in 0..4 {
        for (c, k) in s.iter().enumerate() {
            o[r * 4 + c] *= *k;
        }
    }
    o
}

/// A mover's collision states (`Convex::mList`): one GJK state per convex of its working set near it, the newest first.
#[derive(Debug, Clone, Default)]
pub struct StateList {
    pub states: Vec<GjkState>,
    /// [`StateList::update`]'s buffers, empty between calls and kept for their capacity (a clone gets none).
    scratch: UpdateScratch,
}

#[derive(Debug, Default)]
struct UpdateScratch {
    /// The working set's index of each id (the last, for an id it holds twice, as a map built in order).
    working: IdMap<u64, u32>,
    /// The ids that have a state.
    with_state: IdSet<u64>,
    /// The states made this call, oldest first.
    added: Vec<GjkState>,
}

/// Empty: a clone takes nothing of the buffers (a replay clones the list every move).
impl Clone for UpdateScratch {
    fn clone(&self) -> UpdateScratch {
        UpdateScratch::default()
    }
}

impl StateList {
    /// `updateStateList` for the mover's convex `me` at `mat` and `scale`: its box there grown by 1; the
    /// states whose other convex's box misses it are dropped; each convex of `working` that has no state and meets it gets one
    /// (at the head), set at the other's transform.
    pub fn update(
        &mut self,
        world: &mut dyn World,
        me: &Convex,
        mat: &Matrix,
        scale: F3,
        working: &[Convex],
    ) {
        let Some(b) = me.bounding_box_at(world, mat, scale) else {
            return;
        };
        let lo = b.min.map(|x| x - 1.0);
        let hi = b.max.map(|x| x + 1.0);
        let meets = |bb: &crate::math::Box3| {
            !(hi[0] < bb.min[0]
                || hi[1] < bb.min[1]
                || hi[2] < bb.min[2]
                || bb.max[0] < lo[0]
                || bb.max[1] < lo[1]
                || bb.max[2] < lo[2])
        };
        // Membership by maps the list keeps (cleared, not made, each call), not by scanning: a state list and a working set
        // both at their caps would be 16 million tests here.
        let UpdateScratch {
            working: by_id,
            with_state,
            added,
        } = &mut self.scratch;
        by_id.clear();
        by_id.extend(working.iter().enumerate().map(|(i, w)| (w.id, i as u32)));
        let in_working = |id: u64| working.get(*by_id.get(&id)? as usize);
        // The state holds pointers to the two convexes, so it sees what a `buildConvex` changed in place (a collision plane's
        // box follows the query's center): each kept state takes its convexes as they are now.
        self.states.retain_mut(|s| {
            let (mine, other) = if s.a.id == me.id {
                (&mut s.a, &mut s.b)
            } else {
                (&mut s.b, &mut s.a)
            };
            let Some(now) = in_working(other.id) else {
                return false;
            };
            *other = *now;
            if mine.id == me.id {
                *mine = *me;
            }
            other.bounding_box(world).is_some_and(|bb| meets(&bb))
        });
        with_state.clear();
        with_state.extend(self.states.iter().flat_map(|s| [s.a.id, s.b.id]));
        // The add test is the other way round (`min <= hi && lo <= max`): a NaN box gets no state.
        let meets_add =
            |bb: &crate::math::Box3| (0..3).all(|i| bb.min[i] <= hi[i] && lo[i] <= bb.max[i]);
        added.clear();
        for w in working {
            if self.states.len() + added.len() >= MAX_STATES {
                break;
            }
            if !world.charge(work::CONVEX) {
                break;
            }
            let has = with_state.contains(&w.id);
            if has || !w.bounding_box(world).is_some_and(|bb| meets_add(&bb)) {
                continue;
            }
            let other_mat = w.transform(world).unwrap_or(crate::math::IDENTITY);
            let state = GjkState::new(world, *me, *w, mat, &other_mat);
            with_state.insert(state.a.id);
            with_state.insert(state.b.id);
            added.push(state);
        }
        // Each new state went in at the head: the newest first.
        self.states.splice(0..0, added.drain(..).rev());
        by_id.clear();
        with_state.clear();
    }

    /// `getCollisionInfo` for `me` at `mat` and `scale`: for each state within `tol` (after swapping so `me` is `a`),
    /// the two features (`me`'s at `mat` scaled, toward `-v` in its space; the other's at its transform scaled, toward `v`) and
    /// their contacts; a state whose other transform has a zero determinant is skipped. True when the list holds any.
    pub fn collision_info(
        &mut self,
        world: &mut dyn World,
        me: &Convex,
        mat: &Matrix,
        scale: F3,
        list: &mut CollisionList,
        tol: f32,
    ) -> bool {
        for s in self.states.iter_mut() {
            if list.count >= MAX_COLLISIONS || !world.charge(work::CONVEX) {
                break;
            }
            if s.a.id != me.id {
                s.swap();
            }
            if !(s.dist <= tol) {
                continue;
            }
            let omat = mat_scale(mat, scale);
            let imat = inverse(&omat);
            let nv = [-s.v[0], -s.v[1], -s.v[2]];
            let va = (library::get().state_mul_v)(&imat, nv);
            let fa = features(world, &s.a, &omat, va);
            let Some(bt) = s.b.transform(world) else {
                continue;
            };
            if determinant(&bt) == 0.0 {
                continue;
            }
            let bscale = s.b.scale(world).unwrap_or([1.0; 3]);
            let bmat = mat_scale(&bt, bscale);
            let ib = inverse(&bmat);
            let vb = (library::get().state_mul_v)(&ib, s.v);
            let fb = features(world, &s.b, &bmat, vb);
            if fa
                .collide(&fb, list, tol, &mut |u| world.charge(u))
                .is_err()
            {
                break;
            }
        }
        list.count != 0
    }

    /// `Convex::findClosestState(mat, scale, dontCare)`, the vehicles' body query: `updateStateList`, then each state
    /// (swapped so `me` is `a`) measured by GJK from `me` at `mat` scaled to the other at its transform scaled, with the inverses
    /// `scale(1/s) * affineInverse(m)`. The nearest
    /// (strictly smaller, from 1e30) is the answer, unless it is not below `dont_care` (or a NaN): then `None`. The index is the
    /// state's in [`StateList::states`]; its `dist` is the distance.
    pub fn find_closest_state(
        &mut self,
        world: &mut dyn World,
        me: &Convex,
        mat: &Matrix,
        scale: F3,
        working: &[Convex],
        dont_care: f32,
    ) -> Option<usize> {
        self.update(world, me, mat, scale, working);
        let mut best = 1e30f32;
        let mut best_state = None;
        let a = mat_scale(mat, scale);
        let a_inv = scaled_inverse(mat, scale);
        for i in 0..self.states.len() {
            let s = &mut self.states[i];
            if s.a.id != me.id {
                s.swap();
            }
            let bt = s.b.transform(world).unwrap_or(IDENTITY);
            let bs = s.b.scale(world).unwrap_or([1.0; 3]);
            let b = mat_scale(&bt, bs);
            let b_inv = scaled_inverse(&bt, bs);
            let dd = s.distance(world, &a, &b, dont_care, Some((&a_inv, &b_inv)));
            if best > dd {
                best = dd;
                best_state = Some(i);
            }
        }
        if dont_care > best { best_state } else { None }
    }

    /// The bounding boxes of every state's other convex (for tests).
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

/// `scale(1/s) * affineInverse(m)` as `findClosestState` builds a world-to-object matrix: the identity scaled by the
/// reciprocals, times the affine inverse.
fn scaled_inverse(m: &Matrix, s: F3) -> Matrix {
    let inv = [1.0 / s[0], 1.0 / s[1], 1.0 / s[2]];
    matrix_product(&mat_scale(&IDENTITY, inv), &affine_inverse(m))
}

#[cfg(test)]
#[path = "feature_tests.rs"]
mod tests;
