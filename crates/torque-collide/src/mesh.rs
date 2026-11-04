//! A collision detail's meshes as plain arrays (the host builds them from its shapes), their polygon feed and their hull
//! accelerator.
//!
//! - [`feed_hull`] is `TSShapeInstance::buildPolyList` over `TSMesh::buildPolyList`: the list's transform
//!   times its scale times each part's node transform becomes the list's transform at unit scale; every vertex of the part, then
//!   each triangle (`begin(material, key++)`, three vertices, the plane through them, `end`); the list's transform and scale are
//!   put back at the end.
//! - [`HullAccel`] is `TSShape::ConvexHullAccelerator` as `computeAccelerator` builds it from `TSMesh::getFeatures` at the
//!   identity (the vertex list made unique, the face normals, and per vertex an "emit string" of the vertices, edges and faces
//!   around it), and its `support`.

use crate::library;
use crate::math::{F3, IDENTITY, Matrix, dot, matrix_product, mul_p, plane_from_points};
use crate::poly::PolyList;
use crate::{Exhausted, World, work};

/// Vertices the accelerator takes (cap: the emit strings count and index in bytes, so a hull of more than 256 vertices would
/// wrap there). The build is `O(V * F^2)`, which these two caps bound.
pub const MAX_HULL_VERTS: usize = 256;
/// Faces the accelerator takes (cap).
pub const MAX_HULL_TRIS: usize = 512;

/// One object's mesh of a collision detail: the node transform at the current pose, the first frame's vertices in the mesh's
/// space, and its triangles in the order `buildPolyList` emits them (a list's as they are, a strip's alternating with the
/// repeated-index ones left out), each with its material index (`matIndex & MaterialMask`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MeshPart {
    pub node: Matrix,
    pub verts: Vec<F3>,
    pub tris: Vec<[u32; 3]>,
    pub materials: Vec<u32>,
}

/// A collision slot's meshes: the parts in the detail's object order, the node transform `getTransform` multiplies in (the first
/// object with a mesh at the detail; `None` when there is none), and the accelerator when one is made.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hull {
    pub node: Option<Matrix>,
    pub parts: Vec<MeshPart>,
    pub accel: Option<HullAccel>,
}

impl Hull {
    /// A hull with its accelerator made from the parts.
    pub fn new(node: Option<Matrix>, parts: Vec<MeshPart>) -> Hull {
        let accel = HullAccel::compute(&parts);
        Hull {
            node,
            parts,
            accel: Some(accel),
        }
    }
}

/// `TSShapeInstance::buildPolyList` for one slot: see the module. Charged per vertex and per triangle before each part.
pub fn feed_hull(
    world: &mut dyn World,
    list: &mut dyn PolyList,
    hull: &Hull,
) -> Result<(), Exhausted> {
    let saved = list.base().transform();
    let saved_scale = list.base().scale();
    let mut s = IDENTITY;
    s[0] = saved_scale[0];
    s[5] = saved_scale[1];
    s[10] = saved_scale[2];
    let scaled = matrix_product(&saved, &s);
    let mut key = 0u32;
    let mut result = Ok(());
    for part in &hull.parts {
        let cost = work::VERTEX * part.verts.len() as u64 + work::POLY * part.tris.len() as u64;
        if !world.charge(cost) {
            result = Err(Exhausted);
            break;
        }
        let m = matrix_product(&scaled, &part.node);
        list.set_transform(&m, [1.0, 1.0, 1.0]);
        let mut base = 0u32;
        for (i, v) in part.verts.iter().enumerate() {
            let idx = list.add_point(*v);
            if i == 0 {
                base = idx;
            }
        }
        for (t, tri) in part.tris.iter().enumerate() {
            let material = part.materials.get(t).copied().unwrap_or(0);
            list.begin(material, key);
            key = key.wrapping_add(1);
            let v = tri.map(|x| base.wrapping_add(x));
            list.vertex(v[0]);
            list.vertex(v[1]);
            list.vertex(v[2]);
            list.plane_points(v[0], v[1], v[2]);
            list.end();
        }
    }
    list.set_transform(&saved, saved_scale);
    result
}

/// `ConvexHullAccelerator`: the unique vertices, the face normals, and one emit string per vertex.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HullAccel {
    pub verts: Vec<F3>,
    pub normals: Vec<F3>,
    pub emit: Vec<Vec<u8>>,
}

/// A face of a convex feature: its normal and three vertex indices.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FeatureFace {
    pub normal: F3,
    pub vertex: [u32; 3],
}

impl HullAccel {
    /// `computeAccelerator`: the features of every part at the identity (`TSMesh::getFeatures`: the vertices moved by
    /// the identity, a face per triangle with the normal of [`plane_from_points`], the edges made unique as `(min, max)`); the
    /// vertices any face uses made unique by exact equality (the faces renumbered); then for each vertex the faces that use it,
    /// the faces "flat" with one of those (a dot above 0.999), the vertices of those faces in first-seen order, their
    /// edges `(min, max)` in first-seen order, and the string `[nv, v..., ne, (a, b)..., nf, (face, a, b, c)...]` with every
    /// count and index truncated to a byte (values past 255 wrap).
    pub fn compute(parts: &[MeshPart]) -> HullAccel {
        let mut verts: Vec<F3> = Vec::new();
        let mut faces: Vec<FeatureFace> = Vec::new();
        for part in parts {
            let base = verts.len() as u32;
            for v in &part.verts {
                if verts.len() >= MAX_HULL_VERTS {
                    break;
                }
                verts.push(mul_p(&IDENTITY, *v));
            }
            for tri in &part.tris {
                if faces.len() >= MAX_HULL_TRIS {
                    break;
                }
                let idx = tri.map(|x| base.wrapping_add(x));
                let get = |i: u32| verts.get(i as usize).copied().unwrap_or([0.0; 3]);
                let p = plane_from_points(get(idx[0]), get(idx[1]), get(idx[2]));
                faces.push(FeatureFace {
                    normal: [p.x, p.y, p.z],
                    vertex: idx,
                });
            }
        }
        // The vertices the faces use, made unique.
        let mut fixed: Vec<F3> = Vec::new();
        for i in 0..verts.len() as u32 {
            if !faces.iter().any(|f| f.vertex.contains(&i)) {
                continue;
            }
            let v = verts[i as usize];
            let to = match fixed.iter().position(|x| x == &v) {
                Some(j) => j as u32,
                None => {
                    fixed.push(v);
                    (fixed.len() - 1) as u32
                }
            };
            for f in faces.iter_mut() {
                for x in f.vertex.iter_mut() {
                    if *x == i {
                        *x = to;
                    }
                }
            }
        }
        let normals: Vec<F3> = faces.iter().map(|f| f.normal).collect();
        let mut emit = Vec::with_capacity(fixed.len());
        for i in 0..fixed.len() as u32 {
            let mut around: Vec<usize> = faces
                .iter()
                .enumerate()
                .filter(|(_, f)| f.vertex.contains(&i))
                .map(|(j, _)| j)
                .collect();
            for j in 0..faces.len() {
                if around.contains(&j) {
                    continue;
                }
                let nj = normals[j];
                let mut k = 0;
                while k < around.len() {
                    let nk = normals[around[k]];
                    let dot = (library::get().accel_normal_dot)(nk, nj);
                    if f64::from(dot) > 0.999 {
                        around.push(j);
                        break;
                    }
                    k += 1;
                }
            }
            let mut remap: Vec<u32> = Vec::new();
            for &f in &around {
                for &v in &faces[f].vertex {
                    if !remap.contains(&v) {
                        remap.push(v);
                    }
                }
            }
            let mut edges: Vec<(u32, u32)> = Vec::new();
            for &f in &around {
                let fv = faces[f].vertex;
                for k in 0..3 {
                    let (a, b) = (fv[k], fv[(k + 1) % 3]);
                    let e = (a.min(b), a.max(b));
                    if !edges.contains(&e) {
                        edges.push(e);
                    }
                }
            }
            let pos = |v: u32| remap.iter().position(|&x| x == v);
            let mut s: Vec<u8> =
                Vec::with_capacity(3 + remap.len() + edges.len() * 2 + around.len() * 4);
            s.push(remap.len() as u8);
            s.extend(remap.iter().map(|&v| v as u8));
            s.push(edges.len() as u8);
            for &(a, b) in &edges {
                if let Some(p) = pos(a) {
                    s.push(p as u8);
                }
                if let Some(p) = pos(b) {
                    s.push(p as u8);
                }
            }
            s.push(around.len() as u8);
            for &f in &around {
                s.push(f as u8);
                for &v in &faces[f].vertex {
                    if let Some(p) = pos(v) {
                        s.push(p as u8);
                    }
                }
            }
            emit.push(s);
        }
        HullAccel {
            verts: fixed,
            normals,
            emit,
        }
    }

    /// The index of the vertex with the largest dot with `v`: each dot `mDot(p, v)`, the
    /// first strictly greater one winning (a NaN never wins); 0 for an empty list.
    pub fn support_index(&self, v: F3) -> usize {
        let dot = |p: &F3| dot(*p, v);
        let mut best = 0usize;
        let Some(first) = self.verts.first() else {
            return 0;
        };
        let mut best_dot = dot(first);
        for (i, p) in self.verts.iter().enumerate().skip(1) {
            let dp = dot(p);
            if dp > best_dot {
                best = i;
                best_dot = dp;
            }
        }
        best
    }

    /// `support`: the vertex of [`support_index`](Self::support_index); the origin for an empty hull.
    pub fn support(&self, v: F3) -> F3 {
        self.verts
            .get(self.support_index(v))
            .copied()
            .unwrap_or([0.0; 3])
    }
}

#[cfg(test)]
#[path = "mesh_tests.rs"]
mod tests;
