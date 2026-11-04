//! The collision of a Torque map's terrain and interiors: the convexes, polygon lists and rays here are this crate's own,
//! built from the map files as this crate reads them. A map object's
//! [`MapShape`] lives in the world (the server's object, the client's ghost): [`crate::World::map_shape`] hands it to the
//! working sets (pieces become [`ConvexKind::TerrainTri`] and [`ConvexKind::InteriorHull`] convexes, type 2 and 3, numbered as
//! Torque's `TerrainConvexType` and `InteriorConvexType`) and the hosts' rays ask it directly.
//!
//! **In the world's space.** A piece's convex has the identity for its transform and a unit scale: its points, faces and planes
//! are made in the world when asked for, so the server and the client, which build the same shape from the same ghost record,
//! answer every query alike.
//!
//! **Hostile input.** The files are downloads: every index is checked when the geometry is made, every query is bounded by a named
//! cap and charged to the caller's budget, a NaN or an infinite segment answers nothing.

pub mod interior;
pub mod terrain;

use crate::convex::ConvexKind;
use crate::feature::ConvexFeature;
use crate::math::{Box3, F3, IDENTITY, Matrix, d, quat_matrix};
use crate::mesh::FeatureFace;
use crate::poly::PolyList;
use crate::{Exhausted, ObjectId, work};

pub use interior::{InteriorGeom, InteriorShape};
pub use terrain::{HeightField, MapHit, TerrainShape};

/// `TerrainConvexType` (the others are in [`crate::convex`]).
pub const TERRAIN_CONVEX_TYPE: u32 = 2;
/// `InteriorConvexType`.
pub const INTERIOR_CONVEX_TYPE: u32 = 3;

/// The collision of one map object.
#[derive(Debug, Clone, PartialEq)]
pub enum MapShape {
    Terrain(TerrainShape),
    Interior(InteriorShape),
}

/// A transform as a ghost carries it (the position, the quaternion's x, y, z and the sign of its w) as a matrix:
/// `w = sqrt(max(0, 1 - min(1, x^2 + y^2 + z^2)))` (floats), then `QuatF::setMatrix`.
pub fn xf_matrix(position: F3, quat: F3, negative_w: bool) -> Matrix {
    let sum = quat[0] * quat[0] + quat[1] * quat[1] + quat[2] * quat[2];
    let mut w = (1.0 - sum.min(1.0)).max(0.0).sqrt();
    if negative_w {
        w = -w;
    }
    let mut m = quat_matrix([quat[0], quat[1], quat[2], w]);
    m[3] = position[0];
    m[7] = position[1];
    m[11] = position[2];
    m
}

impl MapShape {
    /// The box the object's collision lies in.
    pub fn world_box(&self) -> Box3 {
        match self {
            MapShape::Terrain(t) => t.world_box(),
            MapShape::Interior(i) => i.world_box(),
        }
    }

    /// The pieces whose boxes meet `query`, as convex kinds (`capped` set when a cap cut the list).
    pub fn pieces(
        &self,
        query: &Box3,
        charge: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<ConvexKind>,
        capped: &mut bool,
    ) -> Result<(), Exhausted> {
        match self {
            MapShape::Terrain(t) => {
                let mut list = Vec::new();
                t.pieces(query, charge, &mut list, capped)?;
                out.extend(list.into_iter().map(|(x, y, half)| ConvexKind::TerrainTri {
                    x,
                    y,
                    half,
                }));
            }
            MapShape::Interior(i) => {
                let mut list = Vec::new();
                i.pieces(query, charge, &mut list)?;
                out.extend(
                    list.into_iter()
                        .map(|hull| ConvexKind::InteriorHull { hull }),
                );
            }
        }
        Ok(())
    }

    /// A piece's points in the world (a terrain triangle's three top corners, then the three under them).
    pub fn points(&self, kind: &ConvexKind) -> Vec<F3> {
        match (self, kind) {
            (MapShape::Terrain(t), &ConvexKind::TerrainTri { x, y, half }) => {
                let tri = t.triangle(i64::from(x), i64::from(y), half);
                let z = TerrainShape::bottom(&tri);
                let mut v = tri.to_vec();
                v.extend(tri.iter().map(|p| [p[0], p[1], z]));
                v
            }
            (MapShape::Interior(i), &ConvexKind::InteriorHull { hull }) => i.hull_points(hull),
            _ => Vec::new(),
        }
    }

    /// A piece's box.
    pub fn piece_box(&self, kind: &ConvexKind) -> Option<Box3> {
        match (self, kind) {
            (MapShape::Terrain(t), &ConvexKind::TerrainTri { x, y, half }) => Some(
                TerrainShape::triangle_box(&t.triangle(i64::from(x), i64::from(y), half)),
            ),
            (MapShape::Interior(i), &ConvexKind::InteriorHull { hull }) => i.hull_box(hull),
            _ => None,
        }
    }

    /// The faces a piece's polygon list holds: a terrain triangle's top, an interior hull's every face; each its plane and its
    /// points (indices into [`MapShape::points`]) clockwise seen from outside.
    pub fn faces(&self, kind: &ConvexKind) -> Vec<(crate::math::Plane, Vec<u32>)> {
        match (self, kind) {
            (MapShape::Terrain(t), &ConvexKind::TerrainTri { x, y, half }) => {
                let tri = t.triangle(i64::from(x), i64::from(y), half);
                let plane = up_plane(&tri);
                vec![(plane, vec![0, 2, 1])]
            }
            (MapShape::Interior(i), &ConvexKind::InteriorHull { hull }) => i.hull_faces(hull),
            _ => Vec::new(),
        }
    }

    /// `support`: the piece's point with the greatest dot with `v` (the first of equals; the origin for no point).
    pub fn support(&self, kind: &ConvexKind, v: F3) -> F3 {
        let pts = self.points(kind);
        let dot = |p: &F3| (d(p[0]) * d(v[0]) + d(p[1]) * d(v[1])) + d(p[2]) * d(v[2]);
        let mut best: Option<(f64, F3)> = None;
        for p in pts {
            let dp = dot(&p);
            if best.is_none_or(|(b, _)| dp > b) {
                best = Some((dp, p));
            }
        }
        best.map_or([0.0; 3], |(_, p)| p)
    }

    /// The piece's polygon list: its points and faces at the identity, charged a vertex a point and a polygon a face.
    pub fn poly_list(
        &self,
        kind: &ConvexKind,
        object: ObjectId,
        list: &mut dyn PolyList,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), Exhausted> {
        let pts = self.points(kind);
        let faces = self.faces(kind);
        if !charge(work::VERTEX * pts.len() as u64 + work::POLY * faces.len() as u64) {
            return Err(Exhausted);
        }
        list.set_transform(&IDENTITY, [1.0; 3]);
        list.set_object(Some(object));
        // A terrain triangle feeds its top three corners only.
        let used = if matches!(kind, ConvexKind::TerrainTri { .. }) {
            3
        } else {
            pts.len()
        };
        let mut base = 0u32;
        for (i, p) in pts.iter().take(used).enumerate() {
            let idx = list.add_point(*p);
            if i == 0 {
                base = idx;
            }
        }
        for (key, (plane, verts)) in faces.iter().enumerate() {
            list.begin(0, key as u32);
            for &v in verts {
                list.vertex(base.wrapping_add(v));
            }
            list.plane(plane);
            list.end();
        }
        Ok(())
    }

    /// The piece's feature toward `n` (a world direction): the faces round its support point for `n` (a terrain triangle: its
    /// top), their points and edges, each face cut into a fan.
    pub fn feature(&self, kind: &ConvexKind, object: ObjectId, n: F3) -> ConvexFeature {
        let mut cf = ConvexFeature {
            object: Some(object),
            ..Default::default()
        };
        let pts = self.points(kind);
        let faces = self.faces(kind);
        if pts.is_empty() || faces.is_empty() {
            return cf;
        }
        let dot = |p: &F3| (d(p[0]) * d(n[0]) + d(p[1]) * d(n[1])) + d(p[2]) * d(n[2]);
        let mut top = 0u32;
        for (i, p) in pts.iter().enumerate() {
            if dot(p) > dot(&pts[top as usize]) {
                top = i as u32;
            }
        }
        let around: Vec<&(crate::math::Plane, Vec<u32>)> =
            if matches!(kind, ConvexKind::TerrainTri { .. }) {
                faces.iter().collect()
            } else {
                faces.iter().filter(|(_, v)| v.contains(&top)).collect()
            };
        let mut remap: Vec<u32> = Vec::new();
        let at = |v: u32, remap: &mut Vec<u32>| -> u32 {
            match remap.iter().position(|&x| x == v) {
                Some(p) => p as u32,
                None => {
                    remap.push(v);
                    (remap.len() - 1) as u32
                }
            }
        };
        for (plane, verts) in around {
            let idx: Vec<u32> = verts.iter().map(|&v| at(v, &mut remap)).collect();
            for k in 0..idx.len() {
                let e = [idx[k], idx[(k + 1) % idx.len()]];
                let e = [e[0].min(e[1]), e[0].max(e[1])];
                if !cf.edges.contains(&e) {
                    cf.edges.push(e);
                }
            }
            for k in 1..idx.len().saturating_sub(1) {
                cf.faces.push(FeatureFace {
                    normal: plane.normal(),
                    vertex: [idx[0], idx[k], idx[k + 1]],
                });
            }
        }
        cf.verts = remap
            .iter()
            .map(|&v| pts.get(v as usize).copied().unwrap_or([0.0; 3]))
            .collect();
        cf
    }

    /// The segment `start` to `end` against the object: the fraction and the world normal of the nearest hit.
    pub fn cast_ray(
        &self,
        start: F3,
        end: F3,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<Option<MapHit>, Exhausted> {
        match self {
            MapShape::Terrain(t) => t.cast_ray(start, end, charge),
            MapShape::Interior(i) => i.cast_ray(start, end, charge),
        }
    }
}

/// The upward plane of a terrain triangle (counter-clockwise seen from above), normalised in `f64`.
fn up_plane(tri: &[F3; 3]) -> crate::math::Plane {
    let p = tri.map(|v| v.map(f64::from));
    let a = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let b = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let n = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-30);
    let n = n.map(|v| v / len);
    let dd = -(n[0] * p[0][0] + n[1] * p[0][1] + n[2] * p[0][2]);
    crate::math::Plane::new(n[0] as f32, n[1] as f32, n[2] as f32, dd as f32)
}
