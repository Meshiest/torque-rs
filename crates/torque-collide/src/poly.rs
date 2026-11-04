//! The polygon lists (the `AbstractPolyList` family) and `Polyhedron`.
//!
//! A convex feeds a list through [`PolyList`] (the interface of `AbstractPolyList`: `isEmpty`, `addPoint`, `addPlane`,
//! `begin`, the three `plane` overloads, `vertex`, `end`, `getIndexedPlane`): points first, in the object's space, moved to the
//! world by the list's transform and scale, then polygons as indices into them. The lists are:
//! - [`ClippedPolyList`]: keeps what lies inside its planes, clipped (`Player::findContact`, `step`);
//! - [`EarlyOutPolyList`]: stops at the first thing inside its planes (`updatePos`' early out);
//! - [`ExtrudedPolyList`]: sweeps a [`Polyhedron`] along a vector and records the earliest contacts in a
//!   [`CollisionList`] (`updatePos`).
//!
//! **Masks.** A vertex keeps a bit per plane it is in front of. Bits are made three ways: the clipped list's `addPoint`
//! doubles a bit (0 past 32 planes), the clip loops use `1 << (n mod 32)` for plane `n` (wrapping past 32 planes)
//! and rotate for the later planes.
//!
//! **Caps.** Every list holds at most [`MAX_POLY_VERTS`] vertices and [`MAX_POLYS`] polygons; past that it marks itself
//! [`overflowed`](PolyBase::overflowed) and takes nothing more (a polygon indexing past its vertices is dropped). Torque
//! has no such cap (its vectors grow until memory runs out); ordinary feeders stay far below these.

use crate::ObjectId;
use crate::collision::{Collision, CollisionList, MAX_COLLISIONS};
use crate::library;
use crate::math::{
    Box3, F3, IDENTITY, Matrix, Plane, PlaneTransformer, dot, is_zero, matrix_product, mul_p,
    normalize, plane_from_points,
};

/// Vertices one list may hold (cap; a box feeds 8, a collision mesh at most a few hundred).
pub const MAX_POLY_VERTS: usize = 65_536;
/// Polygons one list may hold (cap).
pub const MAX_POLYS: usize = 16_384;
/// Planes a list may clip against (cap; the callers use 6, an extruded box at most 24).
pub const MAX_PLANES: usize = 256;
/// Faces `ExtrudedPolyList::end` keeps for the best face: a fixed array of 30 (hits past it are not recorded).
pub const MAX_EXTRUDED_FACES: usize = 30;

/// `ExtrudedPolyList::EqualEpsilon`: the float 0.0001 widened.
pub const EQUAL_EPSILON: f64 = 0.0001f32 as f64;
/// `POINT_EPSILON`, the double 0.0001 (`normalizeSafe`'s length test).
pub const ZERO_EPSILON: f64 = 0.0001;

/// The data every list has (`AbstractPolyList`): the object being fed, the base, transform and combined matrices, the scale, the
/// plane transformer, and the interest normal.
#[derive(Debug, Clone, PartialEq)]
pub struct PolyBase {
    pub object: Option<ObjectId>,
    base: Matrix,
    transform: Matrix,
    matrix: Matrix,
    scale: F3,
    plane_transformer: PlaneTransformer,
    interest: Option<F3>,
    overflowed: bool,
}

impl Default for PolyBase {
    fn default() -> Self {
        PolyBase::new()
    }
}

impl PolyBase {
    /// `AbstractPolyList`'s constructor body: no object, identity matrices, unit scale, the identity transformer, no
    /// interest normal.
    pub fn new() -> PolyBase {
        PolyBase {
            object: None,
            base: IDENTITY,
            transform: IDENTITY,
            matrix: IDENTITY,
            scale: [1.0, 1.0, 1.0],
            plane_transformer: PlaneTransformer::default(),
            interest: None,
            overflowed: false,
        }
    }

    /// Again, as `findContact` calls it on its static list.
    pub fn reset(&mut self) {
        let overflowed = self.overflowed;
        *self = PolyBase::new();
        self.overflowed = overflowed;
    }

    /// `setTransform`: the transform kept, the matrix `base * transform` ([`matrix_product`]), the scale kept, and the plane
    /// transformer set from the matrix and the scale.
    pub fn set_transform(&mut self, transform: &Matrix, scale: F3) {
        self.transform = *transform;
        self.matrix = matrix_product(&self.base, transform);
        self.scale = scale;
        self.plane_transformer = PlaneTransformer::new(&self.matrix, scale);
    }

    /// `setBaseTransform`-style product in place: `base = base * m`.
    pub fn mul_base(&mut self, m: &Matrix) {
        self.base = matrix_product(&self.base, m);
    }

    pub fn set_object(&mut self, object: Option<ObjectId>) {
        self.object = object;
    }

    /// `setInterestNormal` / `clearInterestNormal`.
    pub fn set_interest_normal(&mut self, normal: Option<F3>) {
        self.interest = normal;
    }

    pub fn interest_normal(&self) -> Option<F3> {
        self.interest
    }

    pub fn matrix(&self) -> &Matrix {
        &self.matrix
    }

    /// The transform last given to [`set_transform`](Self::set_transform).
    pub fn transform(&self) -> Matrix {
        self.transform
    }

    pub fn scale(&self) -> F3 {
        self.scale
    }

    /// Whether a cap stopped this list since its last clear.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// A point in the object's space moved to the world: each component times the scale, then `mulP` in place.
    fn world_point(&self, p: F3) -> F3 {
        let s = [
            self.scale[0] * p[0],
            self.scale[1] * p[1],
            self.scale[2] * p[2],
        ];
        mul_p(&self.matrix, s)
    }

    /// `isInterestedInPlane(const PlaneF&)`: with an interest normal, the plane moved to the world and kept only when
    /// its normal points against the interest normal (the table's `interested`).
    pub fn interested_in(&self, plane: &Plane) -> bool {
        match self.interest {
            None => true,
            Some(n) => {
                let p = self.plane_transformer.transform(plane);
                (library::get().interested)(n, &p)
            }
        }
    }

    /// `isInterestedInPlane(U32)`: the same test on a plane the list already holds in world space.
    pub fn interested_in_world(&self, plane: &Plane) -> bool {
        match self.interest {
            None => true,
            Some(n) => (library::get().interested)(n, plane),
        }
    }
}

/// `isInterestedInPlane`'s test as `abstractPolyList.cpp` writes it: false when `mDot(plane, normal) >= 0` (a NaN is
/// interesting).
pub fn plain_interested(normal: F3, plane: &Plane) -> bool {
    !(crate::math::plain::dot(plane.normal(), normal) >= 0.0)
}

/// The interface a convex feeds (`AbstractPolyList`'s virtual methods).
pub trait PolyList {
    fn base(&self) -> &PolyBase;
    fn base_mut(&mut self) -> &mut PolyBase;
    fn is_empty(&self) -> bool;
    /// A point in the object's space; its index.
    fn add_point(&mut self, p: F3) -> u32;
    /// A plane in the object's space; its index among the list's polygon planes.
    fn add_plane(&mut self, plane: &Plane) -> u32;
    fn begin(&mut self, material: u32, surface_key: u32);
    /// The polygon's plane through three of its points (`plane(U32, U32, U32)`).
    fn plane_points(&mut self, v1: u32, v2: u32, v3: u32);
    /// The polygon's plane given in the object's space (`plane(const PlaneF&)`).
    fn plane(&mut self, plane: &Plane);
    /// The polygon's plane by index among the polygon planes (`plane(U32)`).
    fn plane_index(&mut self, index: u32);
    fn vertex(&mut self, index: u32);
    fn end(&mut self);
    /// `getIndexedPlane`: a polygon plane (world space); the zero plane past the end.
    fn indexed_plane(&self, index: u32) -> Plane;

    fn set_transform(&mut self, transform: &Matrix, scale: F3) {
        self.base_mut().set_transform(transform, scale);
    }

    fn set_object(&mut self, object: Option<ObjectId>) {
        self.base_mut().set_object(object);
    }

    /// `AbstractPolyList::addBox`: the eight corners from `min` by the box's sides (in this order), then six quads by the
    /// face table, each `begin(0, face)`, four vertices, the plane of the first three, `end`.
    fn add_box(&mut self, b: &Box3) {
        let dx = b.max[0] - b.min[0];
        let dy = b.max[1] - b.min[1];
        let dz = b.max[2] - b.min[2];
        let mut pos = b.min;
        let base = self.add_point(pos);
        pos[1] += dy;
        self.add_point(pos);
        pos[0] += dx;
        self.add_point(pos);
        pos[1] -= dy;
        self.add_point(pos);
        pos[2] += dz;
        self.add_point(pos);
        pos[0] -= dx;
        self.add_point(pos);
        pos[1] += dy;
        self.add_point(pos);
        pos[0] += dx;
        self.add_point(pos);
        for (i, face) in BOX_FACES.iter().enumerate() {
            self.begin(0, i as u32);
            let v = face.map(|k| base.wrapping_add(k));
            self.vertex(v[0]);
            self.vertex(v[1]);
            self.vertex(v[2]);
            self.vertex(v[3]);
            self.plane_points(v[0], v[1], v[2]);
            self.end();
        }
    }
}

/// `PolyFace`: the corners of each face of [`PolyList::add_box`]. The points are added as `addBox` walks them: min,
/// +y, +x+y, +x, then +x+z, +z, +y+z, +x+y+z; the faces are -z, +z, -y, -x, +y, +x, each clockwise seen from outside.
pub const BOX_FACES: [[u32; 4]; 6] = [
    [3, 2, 1, 0],
    [7, 4, 5, 6],
    [0, 5, 4, 3],
    [6, 5, 0, 1],
    [7, 6, 1, 2],
    [4, 7, 2, 3],
];

/// A vertex of a list: its world position and the planes it is in front of.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vertex {
    pub point: F3,
    pub mask: u32,
}

/// A polygon of a clipped or early-out list.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Poly {
    pub plane: Plane,
    pub object: Option<ObjectId>,
    pub material: u32,
    pub vertex_start: u32,
    pub vertex_count: u32,
    pub surface_key: u32,
}

/// The point where the edge `v1 -> v2` meets `plane` (the clip, through the table).
fn clip_point(v1: F3, v2: F3, plane: &Plane) -> F3 {
    (library::get().clip_point)(v1, v2, plane)
}

/// The clip as the polygon lists' `end` writes it: `vv = v2 - v1`, `t = -plane.distToPlane(v1) / mDot(plane, vv)`, the
/// point `v1 + vv * t`.
pub fn plain_clip_point(v1: F3, v2: F3, plane: &Plane) -> F3 {
    use crate::math::plain::{dot, plane_dist};
    let vv = [v2[0] - v1[0], v2[1] - v1[1], v2[2] - v1[2]];
    let t = -plane_dist(plane, v1) / dot(plane.normal(), vv);
    [v1[0] + vv[0] * t, v1[1] + vv[1] * t, v1[2] + vv[2] * t]
}

/// `1 << (n mod 32)`: the shift count wraps past 32 planes.
fn bit(n: usize) -> u32 {
    1u32 << (n & 31)
}

/// The shared clip of the clipped and early-out lists' `end`: the polygon `poly` (the last one) against
/// `planes`, the result compressed in place. Returns false when the polygon is dropped (facing away, outside, or degenerate).
fn clip_polygon(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    poly: &mut Poly,
    planes: &[Plane],
    overflowed: &mut bool,
) -> bool {
    let start = poly.vertex_start as usize;
    if start > indices.len() || indices[start..].iter().any(|&i| i as usize >= verts.len()) {
        indices.truncate(start.min(indices.len()));
        return false;
    }
    let mut front = 0u32;
    let mut back = 0u32;
    for &i in &indices[start..] {
        let m = verts[i as usize].mask;
        front |= m;
        back |= !m;
    }
    if front == 0 {
        poly.vertex_count = (indices.len() - start) as u32;
        return true;
    }
    let cross = front & back;
    if front & !cross != 0 {
        indices.truncate(start);
        return false;
    }
    let mut index_start = start;
    for (p, plane) in planes.iter().enumerate() {
        let pmask = bit(p);
        if cross & pmask == 0 {
            continue;
        }
        let index_end = indices.len();
        if index_end == index_start {
            break;
        }
        let mut i1 = index_end - 1;
        let mut mask1 = verts[indices[i1] as usize].mask;
        for i2 in index_start..index_end {
            let mask2 = verts[indices[i2] as usize].mask;
            if (mask1 ^ mask2) & pmask != 0 {
                if verts.len() >= MAX_POLY_VERTS {
                    *overflowed = true;
                    indices.truncate(start);
                    return false;
                }
                let v1 = verts[indices[i1] as usize].point;
                let v2 = verts[indices[i2] as usize].point;
                let point = clip_point(v1, v2, plane);
                let mut mask = 0u32;
                let mut rot = pmask;
                for later in &planes[p + 1..] {
                    rot = rot.rotate_left(1);
                    if later.dist(point) > 0.0 {
                        mask = rot;
                        break;
                    }
                }
                verts.push(Vertex { point, mask });
                indices.push((verts.len() - 1) as u32);
            }
            if mask2 & pmask == 0 {
                let idx = indices[i2];
                indices.push(idx);
            }
            mask1 = mask2;
            i1 = i2;
        }
        index_start = index_end;
        if indices.len() - index_start < 3 {
            indices.truncate(start);
            return false;
        }
    }
    let count = indices.len() - index_start;
    indices.copy_within(index_start.., start);
    indices.truncate(start + count);
    poly.vertex_count = count as u32;
    true
}

/// `ClippedPolyList`: polygons clipped to the inside of [`planes`](Self::planes).
#[derive(Debug, Clone, Default)]
pub struct ClippedPolyList {
    pub base: PolyBase,
    pub polys: Vec<Poly>,
    pub verts: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub poly_planes: Vec<Plane>,
    /// The planes to clip against (`mPlaneList`), each `n . p + d > 0` outside.
    pub planes: Vec<Plane>,
    /// `mNormal`: with [`normal_test`](Self::normal_test) on, a polygon whose plane has a positive dot with it is dropped.
    pub normal: F3,
    /// Gates the normal test (clear after construction).
    pub normal_test: bool,
}

impl ClippedPolyList {
    pub fn new() -> ClippedPolyList {
        ClippedPolyList::default()
    }

    /// The sizes `findContact` zeroes: polygons, vertices, indices and polygon planes (not the clip planes).
    pub fn clear(&mut self) {
        self.polys.clear();
        self.verts.clear();
        self.indices.clear();
        self.poly_planes.clear();
        self.base.overflowed = false;
    }

    /// The indices of polygon `p`.
    pub fn poly_indices(&self, p: &Poly) -> &[u32] {
        let s = (p.vertex_start as usize).min(self.indices.len());
        let e = (s + p.vertex_count as usize).min(self.indices.len());
        &self.indices[s..e]
    }

    fn last_poly(&mut self) -> Option<&mut Poly> {
        self.polys.last_mut()
    }
}

impl PolyList for ClippedPolyList {
    fn base(&self) -> &PolyBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PolyBase {
        &mut self.base
    }

    /// No polygon.
    fn is_empty(&self) -> bool {
        self.polys.is_empty()
    }

    /// The world point, and a bit for each plane it is strictly in front of (the bit doubles: none past 32 planes).
    fn add_point(&mut self, p: F3) -> u32 {
        if self.verts.len() >= MAX_POLY_VERTS {
            self.base.overflowed = true;
            return self.verts.len() as u32;
        }
        let point = self.base.world_point(p);
        let mut mask = 0u32;
        let mut b = 1u32;
        for plane in &self.planes {
            if plane.dist(point) > 0.0 {
                mask |= b;
            }
            b = b.wrapping_add(b);
        }
        self.verts.push(Vertex { point, mask });
        (self.verts.len() - 1) as u32
    }

    fn add_plane(&mut self, plane: &Plane) -> u32 {
        self.poly_planes
            .push(self.base.plane_transformer.transform(plane));
        (self.poly_planes.len() - 1) as u32
    }

    /// A new polygon for the current object, starting at the current index.
    fn begin(&mut self, material: u32, surface_key: u32) {
        if self.polys.len() >= MAX_POLYS {
            self.base.overflowed = true;
        }
        self.polys.push(Poly {
            plane: Plane::default(),
            object: self.base.object,
            material,
            vertex_start: self.indices.len() as u32,
            vertex_count: 0,
            surface_key,
        });
    }

    /// [`plane_from_points`] over the three vertices.
    fn plane_points(&mut self, v1: u32, v2: u32, v3: u32) {
        let get = |v: u32| {
            self.verts
                .get(v as usize)
                .map(|x| x.point)
                .unwrap_or([0.0; 3])
        };
        let plane = plane_from_points(get(v1), get(v2), get(v3));
        if let Some(p) = self.last_poly() {
            p.plane = plane;
        }
    }

    fn plane(&mut self, plane: &Plane) {
        let p = self.base.plane_transformer.transform(plane);
        if let Some(last) = self.last_poly() {
            last.plane = p;
        }
    }

    fn plane_index(&mut self, index: u32) {
        let p = self.indexed_plane(index);
        if let Some(last) = self.last_poly() {
            last.plane = p;
        }
    }

    fn vertex(&mut self, index: u32) {
        self.indices.push(index);
    }

    /// The normal test when on, then the clip (`clip_polygon`); a dropped polygon takes its indices with it.
    fn end(&mut self) {
        let Some(mut poly) = self.polys.last().copied() else {
            return;
        };
        let start = (poly.vertex_start as usize).min(self.indices.len());
        let drop = self.base.overflowed || self.polys.len() > MAX_POLYS || {
            self.normal_test && dot(self.normal, poly.plane.normal()) > 0.0
        };
        if drop {
            self.indices.truncate(start);
            self.polys.pop();
            return;
        }
        let kept = clip_polygon(
            &mut self.verts,
            &mut self.indices,
            &mut poly,
            &self.planes,
            &mut self.base.overflowed,
        );
        if kept {
            if let Some(last) = self.polys.last_mut() {
                *last = poly;
            }
        } else {
            self.polys.pop();
        }
    }

    fn indexed_plane(&self, index: u32) -> Plane {
        self.poly_planes
            .get(index as usize)
            .copied()
            .unwrap_or_default()
    }
}

/// `EarlyOutPolyList`: the clipped list that stops at the first point or polygon inside its planes (`isEmpty` is the
/// negation of its early-out flag).
#[derive(Debug, Clone, Default)]
pub struct EarlyOutPolyList {
    pub base: PolyBase,
    pub polys: Vec<Poly>,
    pub verts: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub early_out: bool,
    pub poly_planes: Vec<Plane>,
    pub planes: Vec<Plane>,
    /// `mNormal`: a polygon with a positive dot is dropped (always tested here).
    pub normal: F3,
}

impl EarlyOutPolyList {
    pub fn new() -> EarlyOutPolyList {
        EarlyOutPolyList::default()
    }

    /// `clear`: the lists emptied and the flag down (the planes stay).
    pub fn clear(&mut self) {
        self.polys.clear();
        self.verts.clear();
        self.indices.clear();
        self.poly_planes.clear();
        self.early_out = false;
        self.base.overflowed = false;
    }
}

impl PolyList for EarlyOutPolyList {
    fn base(&self) -> &PolyBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PolyBase {
        &mut self.base
    }

    fn is_empty(&self) -> bool {
        !self.early_out
    }

    /// Nothing once the flag is up; else the world point and its mask (`1 << (i mod 32)` for each plane it is strictly in
    /// front of); a point with no bit (or a list with no plane) raises the flag.
    fn add_point(&mut self, p: F3) -> u32 {
        if self.early_out {
            return 0;
        }
        if self.verts.len() >= MAX_POLY_VERTS {
            self.base.overflowed = true;
            return self.verts.len() as u32;
        }
        let point = self.base.world_point(p);
        let mut mask = 0u32;
        for (i, plane) in self.planes.iter().enumerate() {
            if plane.dist(point) > 0.0 {
                mask |= bit(i);
            }
        }
        if mask == 0 {
            self.early_out = true;
        }
        self.verts.push(Vertex { point, mask });
        (self.verts.len() - 1) as u32
    }

    /// Not gated by the flag.
    fn add_plane(&mut self, plane: &Plane) -> u32 {
        self.poly_planes
            .push(self.base.plane_transformer.transform(plane));
        (self.poly_planes.len() - 1) as u32
    }

    fn begin(&mut self, material: u32, surface_key: u32) {
        if self.early_out {
            return;
        }
        if self.polys.len() >= MAX_POLYS {
            self.base.overflowed = true;
        }
        self.polys.push(Poly {
            plane: Plane::default(),
            object: self.base.object,
            material,
            vertex_start: self.indices.len() as u32,
            vertex_count: 0,
            surface_key,
        });
    }

    fn plane_points(&mut self, v1: u32, v2: u32, v3: u32) {
        if self.early_out {
            return;
        }
        let get = |v: u32| {
            self.verts
                .get(v as usize)
                .map(|x| x.point)
                .unwrap_or([0.0; 3])
        };
        let plane = plane_from_points(get(v1), get(v2), get(v3));
        if let Some(p) = self.polys.last_mut() {
            p.plane = plane;
        }
    }

    fn plane(&mut self, plane: &Plane) {
        if self.early_out {
            return;
        }
        let p = self.base.plane_transformer.transform(plane);
        if let Some(last) = self.polys.last_mut() {
            last.plane = p;
        }
    }

    fn plane_index(&mut self, index: u32) {
        if self.early_out {
            return;
        }
        let p = self.indexed_plane(index);
        if let Some(last) = self.polys.last_mut() {
            last.plane = p;
        }
    }

    fn vertex(&mut self, index: u32) {
        if !self.early_out {
            self.indices.push(index);
        }
    }

    /// The clipped list's test; a polygon that survives (accepted whole or clipped) raises the flag.
    fn end(&mut self) {
        if self.early_out {
            return;
        }
        let Some(mut poly) = self.polys.last().copied() else {
            return;
        };
        let start = (poly.vertex_start as usize).min(self.indices.len());
        let facing = dot(self.normal, poly.plane.normal());
        if self.base.overflowed || self.polys.len() > MAX_POLYS || facing > 0.0 {
            self.indices.truncate(start);
            self.polys.pop();
            return;
        }
        if clip_polygon(
            &mut self.verts,
            &mut self.indices,
            &mut poly,
            &self.planes,
            &mut self.base.overflowed,
        ) {
            if let Some(last) = self.polys.last_mut() {
                *last = poly;
            }
            self.early_out = true;
        } else {
            self.polys.pop();
        }
    }

    fn indexed_plane(&self, index: u32) -> Plane {
        self.poly_planes
            .get(index as usize)
            .copied()
            .unwrap_or_default()
    }
}

/// An edge of a [`Polyhedron`]: the two faces it joins (the first sees the vertices clockwise) and its two vertices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edge {
    pub face: [u32; 2],
    pub vertex: [u32; 2],
}

/// `Polyhedron`: points, outward planes and edges.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Polyhedron {
    pub points: Vec<F3>,
    pub planes: Vec<Plane>,
    pub edges: Vec<Edge>,
}

/// The edges `Polyhedron::buildBox` writes: `(face0, face1, vertex0, vertex1)`.
const BOX_EDGES: [[u32; 4]; 12] = [
    [0, 4, 0, 1],
    [5, 0, 4, 5],
    [3, 0, 0, 4],
    [1, 4, 1, 2],
    [5, 1, 5, 6],
    [0, 1, 1, 5],
    [2, 4, 2, 3],
    [5, 2, 6, 7],
    [1, 2, 2, 6],
    [3, 4, 3, 0],
    [5, 3, 7, 4],
    [2, 3, 3, 7],
];

impl Polyhedron {
    /// `Polyhedron::buildBox`: the box's sides as the matrix's columns times the side lengths, its minimum moved by the
    /// matrix (`mulP`), the eight corners (`p1 = min + y`, `p2 = (min + x) + y`, `p3 = min + x`, then the four plus `z`), the
    /// six planes through a corner along a side (`PlaneF::set(point, normal)`, the first, fourth and fifth inverted), and the
    /// twelve edges.
    pub fn build_box(&mut self, m: &Matrix, b: &Box3) {
        let lx = b.max[0] - b.min[0];
        let ly = b.max[1] - b.min[1];
        let lz = b.max[2] - b.min[2];
        let xv = [m[0] * lx, m[4] * lx, m[8] * lx];
        let yv = [m[1] * ly, m[5] * ly, m[9] * ly];
        let zv = [m[2] * lz, m[6] * lz, m[10] * lz];
        let min = mul_p(m, b.min);
        let add = |a: F3, c: F3| [a[0] + c[0], a[1] + c[1], a[2] + c[2]];
        let p0 = min;
        let p1 = add(min, yv);
        let p3 = add(min, xv);
        let p2 = add(p3, yv);
        self.points.clear();
        self.points.extend_from_slice(&[
            p0,
            p1,
            p2,
            p3,
            add(p0, zv),
            add(p1, zv),
            add(p2, zv),
            add(p3, zv),
        ]);
        let plane = |p: F3, n: F3, invert: bool| {
            let pl = Plane::from_point_normal(p, n);
            if invert {
                Plane::new(-pl.x, -pl.y, -pl.z, -pl.d)
            } else {
                pl
            }
        };
        let p4 = self.points[4];
        self.planes.clear();
        self.planes.extend_from_slice(&[
            plane(p0, xv, true),
            plane(p2, yv, false),
            plane(p2, xv, false),
            plane(p0, yv, true),
            plane(p0, zv, true),
            plane(p4, zv, false),
        ]);
        self.edges.clear();
        self.edges.extend(BOX_EDGES.iter().map(|e| Edge {
            face: [e[0], e[1]],
            vertex: [e[2], e[3]],
        }));
    }
}

/// One face of the extruded volume.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ExtrudedFace {
    pub active: bool,
    pub plane: Plane,
    pub max_distance: f32,
    pub plane_mask: u32,
    pub face_dot: f32,
    pub face_shift: f32,
    pub time: f32,
    pub point: F3,
    pub height: f32,
}

/// The polygon being fed to an [`ExtrudedPolyList`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ExtrudedPoly {
    pub plane: Plane,
    pub object: Option<ObjectId>,
    pub material: u32,
}

/// `ExtrudedPolyList`: a [`Polyhedron`] swept along a vector; each polygon fed is tested against the faces that
/// move into it and the earliest contacts go to [`collisions`](Self::collisions).
#[derive(Debug, Clone, Default)]
pub struct ExtrudedPolyList {
    pub base: PolyBase,
    pub verts: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub faces: Vec<ExtrudedFace>,
    pub planes: Vec<Plane>,
    pub velocity: F3,
    pub normal_velocity: F3,
    pub poly: ExtrudedPoly,
    /// The list the extruded list reports into (Torque holds a pointer to it): owned here, reset by the caller ([`CollisionList::reset`]).
    pub collisions: CollisionList,
    pub poly_planes: Vec<Plane>,
}

impl ExtrudedPolyList {
    pub fn new() -> ExtrudedPolyList {
        ExtrudedPolyList::default()
    }

    /// `extrude`: the lists emptied; each face of `pt` whose dot with `vector` is above
    /// [`EQUAL_EPSILON`] is active, with that dot as its reach, its plane, and a mask bit, and adds itself inverted then itself to
    /// the clip planes; each edge next to an active face adds the plane through its far vertex, its near vertex and the near
    /// vertex moved by `vector`, then that plane inverted, and gives the first face the first bit and the second the
    /// next.
    pub fn extrude(&mut self, pt: &Polyhedron, vector: F3) {
        self.planes.clear();
        self.indices.clear();
        self.verts.clear();
        self.poly_planes.clear();
        self.base.overflowed = false;
        self.faces.clear();
        for face in &pt.planes {
            let dot = dot(face.normal(), vector);
            let mut ef = ExtrudedFace::default();
            if f64::from(dot) > EQUAL_EPSILON && self.planes.len() + 2 <= MAX_PLANES {
                ef.active = true;
                ef.max_distance = dot;
                ef.plane = *face;
                ef.plane_mask = bit(self.planes.len());
                self.planes
                    .push(Plane::new(-face.x, -face.y, -face.z, -face.d));
                self.planes.push(*face);
            }
            self.faces.push(ef);
        }
        for edge in &pt.edges {
            let (f0, f1) = (edge.face[0] as usize, edge.face[1] as usize);
            let active = |f: usize| self.faces.get(f).is_some_and(|x| x.active);
            if !(active(f0) || active(f1)) || self.planes.len() + 2 > MAX_PLANES {
                continue;
            }
            let get = |v: u32| pt.points.get(v as usize).copied().unwrap_or([0.0; 3]);
            let p1 = get(edge.vertex[1]);
            let p2 = get(edge.vertex[0]);
            let p3 = [p2[0] + vector[0], p2[1] + vector[1], p2[2] + vector[2]];
            let plane = plane_from_points(p3, p2, p1);
            let pmask = bit(self.planes.len());
            self.planes.push(plane);
            self.planes
                .push(Plane::new(-plane.x, -plane.y, -plane.z, -plane.d));
            if let Some(f) = self.faces.get_mut(f0) {
                f.plane_mask |= pmask;
            }
            if let Some(f) = self.faces.get_mut(f1) {
                f.plane_mask |= pmask.wrapping_add(pmask);
            }
        }
    }

    /// `setVelocity`: the velocity kept; when it is zero (`Point3F::isZero`) the normal velocity is zero and the interest
    /// normal off, else the normal velocity is the velocity normalised and the interest normal.
    pub fn set_velocity(&mut self, v: F3) {
        self.velocity = v;
        if is_zero(v) {
            self.normal_velocity = [0.0; 3];
            self.base.set_interest_normal(None);
        } else {
            self.normal_velocity = normalize(v);
            self.base.set_interest_normal(Some(self.normal_velocity));
        }
    }

    /// `testPoly` on face `fi`: the polygon in the index list clipped against the face's bounding planes, then the
    /// nearest vertex by the face plane (clamped at zero; highest vertex short of the reach is the height), then (the
    /// second pass) the same with each vertex pushed along the polygon's normal by `min(reach * 0.2, 0.01)`; the vertex and index
    /// lists are put back. A hit when the first pass's distance is below the reach: the time is the second distance over the
    /// reach, or the first's less 0.01 when the two times are more than 0.1 apart.
    fn test_poly(&mut self, fi: usize) -> bool {
        let face = self.faces[fi];
        let o_verts = self.verts.len();
        let o_indices = self.indices.len();
        let mut index_start = 0usize;
        let mut index_end = self.indices.len();
        if self.indices.iter().any(|&i| i as usize >= self.verts.len()) {
            return false;
        }
        let mut front = 0u32;
        let mut back = 0u32;
        for &i in &self.indices {
            let m = self.verts[i as usize].mask & face.plane_mask;
            front |= m;
            back |= !m;
        }
        if front != 0 {
            let cross = back & front;
            if front & !cross != 0 {
                return false;
            }
            for p in 0..self.planes.len() {
                let pmask = bit(p);
                if face.plane_mask & pmask & cross == 0 {
                    continue;
                }
                let new_start = self.indices.len();
                if index_end == index_start {
                    break;
                }
                let mut i1 = index_end - 1;
                let mut mask1 = self.verts[self.indices[i1] as usize].mask;
                for i2 in index_start..index_end {
                    let mask2 = self.verts[self.indices[i2] as usize].mask;
                    if (mask1 ^ mask2) & pmask != 0 {
                        if self.verts.len() >= MAX_POLY_VERTS {
                            self.base.overflowed = true;
                            self.verts.truncate(o_verts);
                            self.indices.truncate(o_indices);
                            return false;
                        }
                        let v1 = self.verts[self.indices[i1] as usize].point;
                        let v2 = self.verts[self.indices[i2] as usize].point;
                        let point = clip_point(v1, v2, &self.planes[p]);
                        let mut mask = 0u32;
                        for (later, plane) in self.planes.iter().enumerate().skip(p + 1) {
                            if plane.dist(point) > 0.0 {
                                mask |= bit(later);
                            }
                        }
                        self.verts.push(Vertex { point, mask });
                        self.indices.push((self.verts.len() - 1) as u32);
                    }
                    if mask2 & pmask == 0 {
                        let idx = self.indices[i2];
                        self.indices.push(idx);
                    }
                    mask1 = mask2;
                    i1 = i2;
                }
                index_start = new_start;
                index_end = self.indices.len();
                if index_end - index_start < 3 {
                    self.verts.truncate(o_verts);
                    self.indices.truncate(o_indices);
                    return false;
                }
            }
        }
        // The nearest vertex. With no vertex the point stays zero.
        let plane = face.plane;
        let mut bd = 1e30f32;
        let mut height = -1e30f32;
        let mut bp: F3 = [0.0; 3];
        for &i in &self.indices[index_start..index_end] {
            let v = self.verts[i as usize].point;
            let dist = plane.dist(v);
            // Taken only when `dist <= bd`: skipped when the best is below the distance or either is NaN.
            if dist <= bd {
                bd = if 0.0 > dist { 0.0 } else { dist };
                bp = v;
            }
            if v[2] > height && face.max_distance > dist {
                height = v[2];
            }
        }
        let bd1 = bd;
        if index_start < index_end {
            // The second pass: each vertex moved along the polygon's normal.
            let k = face.max_distance * 0.2f32;
            let t = if k > 0.01f32 { 0.01f32 } else { k };
            let n = self.poly.plane;
            for &i in &self.indices[index_start..index_end] {
                let v = self.verts[i as usize].point;
                let q = [v[0] + n.x * t, v[1] + n.y * t, v[2] + n.z * t];
                let dist = plane.dist(q);
                if dist <= bd {
                    bd = if 0.0 > dist { 0.0 } else { dist };
                    bp = v;
                }
            }
        }
        self.verts.truncate(o_verts);
        self.indices.truncate(o_indices);
        // The test is `bd1 < reach`, written negated so that a NaN takes the hit branch.
        if bd1 >= face.max_distance {
            return false;
        }
        let time = (library::get().extruded_time)(bd, bd1, face.max_distance);
        let f = &mut self.faces[fi];
        f.time = time;
        f.point = bp;
        f.height = height;
        true
    }
}

impl PolyList for ExtrudedPolyList {
    fn base(&self) -> &PolyBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PolyBase {
        &mut self.base
    }

    /// No collision recorded.
    fn is_empty(&self) -> bool {
        self.collisions.count == 0
    }

    /// The world point, and `1 << (i mod 32)` for each plane it is on or in front of (`>= 0`; a NaN sets nothing).
    fn add_point(&mut self, p: F3) -> u32 {
        if self.verts.len() >= MAX_POLY_VERTS {
            self.base.overflowed = true;
            return self.verts.len() as u32;
        }
        let point = self.base.world_point(p);
        let mut mask = 0u32;
        for (i, plane) in self.planes.iter().enumerate() {
            if plane.dist(point) >= 0.0 {
                mask |= bit(i);
            }
        }
        self.verts.push(Vertex { point, mask });
        (self.verts.len() - 1) as u32
    }

    fn add_plane(&mut self, plane: &Plane) -> u32 {
        self.poly_planes
            .push(self.base.plane_transformer.transform(plane));
        (self.poly_planes.len() - 1) as u32
    }

    /// The object and material kept, the index list emptied.
    fn begin(&mut self, material: u32, _surface_key: u32) {
        self.poly.object = self.base.object;
        self.poly.material = material;
        self.indices.clear();
    }

    /// [`plane_from_points`], then `PlaneF::normalizeSafe`: when [`point_len`] is above [`ZERO_EPSILON`], each component
    /// times its reciprocal; `d` unchanged.
    fn plane_points(&mut self, v1: u32, v2: u32, v3: u32) {
        let get = |v: u32| {
            self.verts
                .get(v as usize)
                .map(|x| x.point)
                .unwrap_or([0.0; 3])
        };
        let mut p = plane_from_points(get(v1), get(v2), get(v3));
        let len = point_len([p.x, p.y, p.z]);
        if f64::from(len) > ZERO_EPSILON {
            let inv = 1.0 / len;
            p.x *= inv;
            p.y *= inv;
            p.z *= inv;
        }
        self.poly.plane = p;
    }

    fn plane(&mut self, plane: &Plane) {
        self.poly.plane = self.base.plane_transformer.transform(plane);
    }

    fn plane_index(&mut self, index: u32) {
        self.poly.plane = self.indexed_plane(index);
    }

    fn vertex(&mut self, index: u32) {
        if self.indices.len() < MAX_POLY_VERTS {
            self.indices.push(index);
        } else {
            self.base.overflowed = true;
        }
    }

    /// A polygon facing along the motion (its dot with the normal velocity above zero) or a full list is
    /// skipped; the faces with a positive (or NaN) alignment `-(face . poly)` are tested, else (none hit) the rest; the
    /// best-aligned hit (a NaN alignment loses its place) goes in when its time is within [`EQUAL_EPSILON`] of the list's and
    /// below 1: an earlier one restarts the list.
    fn end(&mut self) {
        let nv = self.normal_velocity;
        let pp = self.poly.plane;
        let facing = dot(pp.normal(), nv);
        if facing > 0.0 || self.collisions.count >= MAX_COLLISIONS || self.base.overflowed {
            return;
        }
        let mut hits: Vec<usize> = Vec::new();
        for fi in 0..self.faces.len() {
            if !self.faces[fi].active {
                continue;
            }
            let f = self.faces[fi].plane;
            let face_dot = -dot(f.normal(), pp.normal());
            self.faces[fi].face_dot = face_dot;
            if !(0.0 >= face_dot) && self.test_poly(fi) && hits.len() < MAX_EXTRUDED_FACES {
                hits.push(fi);
            }
        }
        if hits.is_empty() {
            for fi in 0..self.faces.len() {
                let f = self.faces[fi];
                if f.active
                    && !(f.face_dot > 0.0)
                    && self.test_poly(fi)
                    && hits.len() < MAX_EXTRUDED_FACES
                {
                    hits.push(fi);
                }
            }
            if hits.is_empty() {
                return;
            }
        }
        let mut best = hits[0];
        for &h in &hits[1..] {
            let (b, o) = (self.faces[best].face_dot, self.faces[h].face_dot);
            if !(b >= o) {
                best = h;
            }
        }
        let face = self.faces[best];
        let time = face.time;
        let list = &mut self.collisions;
        let (later, earlier) = (library::get().time_against_list)(time, list.t);
        if later || !(time < 1.0) && !time.is_nan() {
            return;
        }
        if earlier {
            list.t = time;
            list.count = 0;
            list.max_height = face.height;
        } else if face.height > list.max_height {
            list.max_height = face.height;
        }
        list.push(Collision {
            object: self.poly.object,
            point: face.point,
            normal: [pp.x, pp.y, pp.z],
            material: self.poly.material,
            face: best as u32,
            face_dot: face.face_dot,
            distance: list.entries[list.count.min(MAX_COLLISIONS - 1)].distance,
        });
    }

    fn indexed_plane(&self, index: u32) -> Plane {
        self.poly_planes
            .get(index as usize)
            .copied()
            .unwrap_or_default()
    }
}

/// `Point3F::magnitudeSafe`: zero when the vector is zero (`Point3F::isZero`), else `mSqrt` of its dot with itself.
pub fn point_len(v: F3) -> f32 {
    if is_zero(v) {
        return 0.0;
    }
    dot(v, v).sqrt()
}

/// `testPoly`'s time as `extrudedPolyList.cpp` writes it: `oldT = oldBd / maxDistance`, `pushBackT = bd / maxDistance`;
/// `oldT - 0.01` when `oldT - pushBackT > 0.1`, else `pushBackT`.
pub fn plain_extruded_time(bd: f32, old_bd: f32, max_distance: f32) -> f32 {
    let old_t = old_bd / max_distance;
    let push_back_t = bd / max_distance;
    if f64::from(old_t - push_back_t) > 0.1 {
        old_t - 0.01
    } else {
        push_back_t
    }
}

/// `ExtrudedPolyList::end`'s comparisons with the list's time: `time > t + EqualEpsilon` and `time < t - EqualEpsilon`
/// (`EqualEpsilon` is an `F32`).
pub fn plain_time_against_list(time: f32, t: f32) -> (bool, bool) {
    let eps = EQUAL_EPSILON as f32;
    (time > t + eps, time < t - eps)
}

#[cfg(test)]
#[path = "poly_tests.rs"]
mod tests;
