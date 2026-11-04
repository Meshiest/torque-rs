//! Rays and boxes against the meshes of a shape: what a shape-based object's `castRay` and its polygon list (`buildPolyList`,
//! which `containerBoxEmpty` clips against its box) ask of `TSShapeInstance` and `TSMesh`.
//! Nothing here knows a script VM or an object type: the caller hands over a ray or a box in the object's space and the
//! collision and LOS details ([`Slots`]).
//!
//! **A ray is not tested against triangles.** `TSMesh::castRay` clips the segment against the
//! *convex hull* of the mesh: `buildConvexHull` (run the first time a ray reaches the mesh) turns every triangle that
//! is not degenerate into a plane (the normal of its cross product; a plane the hull already has is dropped), and the
//! cast keeps an entering time and a leaving time as fractions while it goes plane by plane (rule: [`Hull::cast`]). A concave
//! mesh answers as its hull does, a ray that starts inside the hull hits nothing, and one that ends exactly on a face hits it
//! (at t = 1). `TSShapeInstance::castRay` moves the ray into each object's node
//! space with the inverse of the node's transform, keeps the nearest hit (a later object wins a tie) and turns the hit
//! normal back with the node's rotation; the object casts once for each of its LOS details ([`detail_for_ray`]) and keeps
//! the nearest (an earlier entry wins a tie), and the caller turns the normal by the object's transform.
//!
//! **A box is tested against the polygon list.** `TSShapeInstance::buildPolyList` gives an `EarlyOutPolyList`
//! the world positions of every vertex of each object's mesh in each collision detail ([`detail_for_polylist`]) and then
//! its triangles (a strip's degenerate triangles, those with a repeated index, are left out; the triangle lists are not
//! filtered). The list stops at the first thing inside the six planes of the box: a vertex that no plane puts strictly outside
//! (a vertex *on* a plane is inside, so a box a mesh only touches is not empty), or a triangle that survives clipping against the
//! planes it crosses ([`Collision::box_hit`]). The object's scale is not applied: the polygon list is built with unit scale.
//!
//! **The pose.** A shape-based object's `TSShapeInstance` is made once at preload and animated once with no thread (`animateNodes`,
//! at rest): every node is its default rotation (the shorts over 32767, `QuatF::setMatrix`) and translation, times its
//! parent's. Every mesh object is visible, at frame 0. [`Collision::build`] makes those transforms and their inverses and the
//! hulls, for the collision and LOS details only.
//!
//! **Cost.** The hull of a mesh is built once and paid for by its triangles ([`TRI_WORK`], and [`PLANE_WORK`] for each plane a
//! triangle is compared with); a ray pays [`RAY_PLANE_WORK`] for each plane of each object it is clipped against; a box pays
//! [`VERTEX_WORK`] for each vertex moved and [`TRI_WORK`] for each triangle, and [`PART_WORK`] for each object's mesh it goes
//! through. The walks over the objects of a detail ([`OBJECT_WORK`] each) and over the primitives of a mesh ([`PRIMITIVE_WORK`]
//! each, empty or not) are paid for before they are made. A shape whose collision and LOS details hold more than
//! [`MAX_COLLISION_TRIANGLES`] triangles is refused at load ([`slot_triangles`]).
//!
//! **Not modelled**: a mesh with more than one frame (the hull of the first frame answers), an object state with a frame or a
//! hidden object (every object is visible), an object with no node (Torque would read a null pointer; this crate uses identity), a
//! node whose parent comes later (Torque multiplies by a transform not made yet; this crate by the parent's own local one). Torque's
//! initial bounds of the two fractions, -0.01 and 1.01, are kept ([`Hull::cast`]); a crossing's own fraction is at most 1, so they
//! never decide a hit that the plane tests do not.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::library;
use crate::math::{self, IDENTITY, Matrix};
use crate::preload::Slots;
use crate::shape::{Mesh, Shape};
use crate::{Charge, ShapeError, pay};

/// Work (units; a step is 64) for each triangle tested or turned into a plane.
pub const TRI_WORK: u64 = 32;
/// Work for each plane a new triangle's plane is compared with.
pub const PLANE_WORK: u64 = 2;
/// Work for each plane a ray is clipped against.
pub const RAY_PLANE_WORK: u64 = 1;
/// Work for each vertex a box test moves into the world and masks.
pub const VERTEX_WORK: u64 = 4;
/// Work for each object of a detail's sub-shape the walks visit (a mesh index lookup), paid before the visit.
pub const OBJECT_WORK: u64 = 1;
/// Work for each primitive a walk over a mesh visits, counted or turned into triangles, whatever its element count (a primitive
/// of no elements adds no triangle and still costs a visit), paid before the walk.
pub const PRIMITIVE_WORK: u64 = 1;
/// Work for each part (one object's mesh of a detail) a box test goes through, over what its vertices and triangles cost.
pub const PART_WORK: u64 = 1;
/// The most triangles the collision and LOS details of one shape may hold.
pub const MAX_COLLISION_TRIANGLES: usize = 16_384;

/// What `TSMesh::castRay` starts the entering time at (-0.01, a float) and the leaving time at (1.01).
const START_NUM: f32 = -0.01;
const END_NUM: f32 = 1.01;
/// A plane the hull already has: a normal closer than this (0.99) and a constant within 0.01.
const SAME_NORMAL: f32 = 0.99;
const SAME_CONSTANT: f32 = 0.01;
/// A triangle whose cross product is shorter (squared) than this (0.001) is degenerate from that vertex.
const TINY_AREA: f32 = 0.001;

/// The detail LOS entry `slot` casts a ray against (`ShapeBase::castRay`; the fallback to the collision detail is made when
/// the entries are, [`crate::preload::collision_slots`]), or -1.
pub fn detail_for_ray(slots: &Slots, slot: usize) -> i32 {
    slots.los.get(slot).copied().unwrap_or(-1)
}

/// The detail collision entry `slot`'s polygon list takes (`ShapeBase::buildPolyList`), or -1.
pub fn detail_for_polylist(slots: &Slots, slot: usize) -> i32 {
    slots.collision.get(slot).copied().unwrap_or(-1)
}

/// A hit: the fraction of the segment (`RayInfo::t`) and the normal of the plane the segment entered through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub t: f32,
    pub normal: [f32; 3],
}

/// The planes of a convex hull (`TSMesh::planeNormals` and `planeConstants`, first frame).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hull {
    normals: Vec<[f32; 3]>,
    constants: Vec<f32>,
}

/// One triangle's plane: the normal of the first attempt, from vertex 1, then vertex 2, then vertex 3, whose cross product
/// is long enough, normalised; the constant `n . p1`. None for a triangle degenerate from all three.
fn triangle_plane(p1: &[f32; 3], p2: &[f32; 3], p3: &[f32; 3]) -> Option<([f32; 3], f32)> {
    let (mut cross, mut squared) = math::triangle_cross(p1, p2, p3);
    if squared < TINY_AREA {
        (cross, squared) = math::triangle_cross(p2, p3, p1);
        if squared < TINY_AREA {
            (cross, squared) = math::triangle_cross(p3, p1, p2);
            if squared < TINY_AREA {
                return None;
            }
        }
    }
    let n = math::normalize(cross);
    Some((n, math::dot(*p1, n)))
}

/// The triangles of a mesh's primitives, first frame (`TSMesh::buildConvexHull` and `TSMesh::buildPolyList`).
/// A primitive with the type bits clear is a list of triangles; any other is read as a strip (a fan too: they cannot be told
/// apart here), whose triangles alternate (`v0 v1 v2`, `v2 v1 v3`, `v2 v3 v4`, ...). `skip_repeats` leaves out a strip
/// triangle with a repeated index (the polygon list does; the hull's own area test finds those). The numbers of a primitive are
/// signed shorts; an index past the vertices ends the triangle (Torque would read past its array).
fn triangles(mesh: &Mesh, verts: usize, skip_repeats: bool) -> Vec<[u32; 3]> {
    let mut out = Vec::new();
    let index = |at: usize| -> Option<u32> {
        let i = u32::from(*mesh.indices.get(at)?);
        ((i as usize) < verts).then_some(i)
    };
    for prim in &mesh.primitives {
        let start = usize::try_from(prim.start as i16).ok();
        let count = prim.num_elements as i16;
        let (Some(start), true) = (start, count > 0) else {
            continue;
        };
        let count = count as usize;
        if prim.material & 0xc000_0000 == 0 {
            let mut k = 0;
            while k < count {
                if let (Some(a), Some(b), Some(c)) =
                    (index(start + k), index(start + k + 1), index(start + k + 2))
                {
                    out.push([a, b, c]);
                }
                k += 3;
            }
        } else if count > 2 {
            // Two slots hold the last two vertices; the new one is `c`.
            let (Some(mut a), Some(mut b)) = (index(start), index(start + 1)) else {
                continue;
            };
            let mut into_a = false;
            for k in 2..count {
                // The slot written first is the one that was `b` on the first round (Torque's XOR toggle).
                let Some(c) = index(start + k) else {
                    // Torque would read a garbage vertex; stop this strip.
                    break;
                };
                if !(skip_repeats && (a == b || a == c || b == c)) {
                    out.push([a, b, c]);
                }
                // The next round stores `c` where the toggle points: first `a`, then `b`, alternately.
                if into_a {
                    b = c;
                } else {
                    a = c;
                }
                into_a = !into_a;
            }
        }
    }
    out
}

/// The triangles of a mesh (what the hull and the list see), counted without making them: a primitive's elements are signed
/// shorts.
pub fn mesh_triangles(mesh: &Mesh) -> u64 {
    let mut total = 0u64;
    for prim in &mesh.primitives {
        let count = i64::from(prim.num_elements as i16);
        if count <= 0 {
            continue;
        }
        total += if prim.material & 0xc000_0000 == 0 {
            ((count + 2) / 3) as u64
        } else {
            (count - 2).max(0) as u64
        };
    }
    total
}

/// The objects of `detail` that have a mesh to show: `(object index, mesh)` for each object of the detail's sub-shape that has a
/// mesh at the detail's object detail (`TSShapeInstance::castRay` and `buildPolyList` skip the others). Every object of the
/// sub-shape is paid for ([`OBJECT_WORK`]) before the walk: nothing stops many objects from pointing at one range of meshes.
fn detail_meshes<'a>(
    shape: &'a Shape,
    detail: i32,
    charge: Charge,
) -> Result<Vec<(usize, &'a Mesh)>, ShapeError> {
    let mut out = Vec::new();
    let Some(d) = usize::try_from(detail)
        .ok()
        .and_then(|i| shape.details.get(i))
    else {
        return Ok(out);
    };
    let Some(ss) = usize::try_from(d.sub_shape)
        .ok()
        .and_then(|i| shape.sub_shapes.get(i))
    else {
        return Ok(out);
    };
    let (Ok(first), Ok(count)) = (
        usize::try_from(ss.first_object),
        usize::try_from(ss.num_objects),
    ) else {
        return Ok(out);
    };
    let end = first.saturating_add(count).min(shape.objects.len());
    pay(
        charge,
        OBJECT_WORK.saturating_mul(end.saturating_sub(first) as u64),
    )?;
    for oi in first..end {
        let object = &shape.objects[oi];
        let od = d.object_detail;
        if od < 0 || od >= object.num_meshes {
            continue;
        }
        let Some(at) = object
            .start_mesh_index
            .checked_add(od)
            .and_then(|i| usize::try_from(i).ok())
        else {
            continue;
        };
        if let Some(Some(mesh)) = shape.meshes.get(at) {
            out.push((oi, mesh));
        }
    }
    Ok(out)
}

/// The triangles the collision and LOS details of a shape hold, a detail several entries share once, an object that shares a mesh with
/// another counted for it: the number [`MAX_COLLISION_TRIANGLES`] bounds. The walk is paid for before it is done: each object
/// of a detail's sub-shape ([`OBJECT_WORK`]), and the primitives of each mesh the first time it is reached ([`PRIMITIVE_WORK`];
/// the count of a mesh is kept, so many objects over one mesh walk it once), so a file of empty primitives that add nothing to
/// the count still cannot make the walk free.
pub fn slot_triangles(shape: &Shape, slots: &Slots, charge: Charge) -> Result<u64, ShapeError> {
    let mut details: Vec<i32> = slots
        .collision
        .iter()
        .chain(&slots.los)
        .copied()
        .filter(|&d| d >= 0)
        .collect();
    details.sort_unstable();
    details.dedup();
    let mut counted: BTreeMap<*const Mesh, u64> = BTreeMap::new();
    let mut total = 0u64;
    for detail in details {
        for (_, mesh) in detail_meshes(shape, detail, charge)? {
            let key: *const Mesh = mesh;
            let count = match counted.get(&key) {
                Some(&c) => c,
                None => {
                    pay(
                        charge,
                        PRIMITIVE_WORK.saturating_mul(mesh.primitives.len() as u64),
                    )?;
                    let c = mesh_triangles(mesh);
                    counted.insert(key, c);
                    c
                }
            };
            total = total.saturating_add(count);
        }
    }
    Ok(total)
}

impl Hull {
    /// The hull of `mesh` (`buildConvexHull`, first frame): the planes of its triangles in primitive order, each unless the hull
    /// already has a plane whose normal is within 0.99 (a dot above it) and whose constant is within 0.01. Charged by the
    /// triangle and by the planes each is compared with, and the primitives walked ([`PRIMITIVE_WORK`]), before the work.
    pub fn build(mesh: &Mesh, charge: Charge) -> Result<Hull, ShapeError> {
        let mut hull = Hull::default();
        if mesh.num_frames <= 0 {
            return Ok(hull);
        }
        // Torque reads the vertex array by the primitives' indices, whatever the frame's size: all the positions are there.
        let verts = mesh.positions.as_ref();
        pay(
            charge,
            TRI_WORK.saturating_add(PRIMITIVE_WORK.saturating_mul(mesh.primitives.len() as u64)),
        )?;
        for tri in triangles(mesh, verts.len(), false) {
            pay(
                charge,
                TRI_WORK.saturating_add(PLANE_WORK.saturating_mul(hull.normals.len() as u64)),
            )?;
            let [a, b, c] = tri.map(|i| verts[i as usize]);
            let Some((n, constant)) = triangle_plane(&a, &b, &c) else {
                continue;
            };
            let same = hull.normals.iter().zip(&hull.constants).any(|(e, ec)| {
                math::dot(*e, n) > SAME_NORMAL && (constant - *ec).abs() < SAME_CONSTANT
            });
            if !same {
                hull.normals.push(n);
                hull.constants.push(constant);
            }
        }
        Ok(hull)
    }

    /// How many planes the hull has.
    pub fn planes(&self) -> usize {
        self.normals.len()
    }

    /// `TSMesh::castRay` for the segment `a` to `b`, in the mesh's own space: the entering fraction and the normal of
    /// the plane it entered through, or none (a ray starting inside, or missing, or lying in no plane, hits nothing). Torque
    /// keeps two fractions, one narrowing from the start (entering) and one from the end (leaving), each as a numerator
    /// and a denominator it compares by cross-multiplication; `side` is -1 while the ray is entering a plane (the start
    /// outside) and +1 while it is leaving one, and a plane with the start exactly on it flips the side as a leaving one does.
    pub fn cast(
        &self,
        a: [f32; 3],
        b: [f32; 3],
        charge: Charge,
    ) -> Result<Option<RayHit>, ShapeError> {
        let planes = self.normals.len();
        pay(charge, RAY_PLANE_WORK.saturating_mul(planes as u64 + 1))?;
        let (mut xn, mut xd) = (START_NUM, 1.0f32);
        let (mut yn, mut yd) = (END_NUM, 1.0f32);
        let mut x_plane = 0usize;
        let mut x_found = false;
        let mut side = -1.0f32;
        let mut on_y = false;
        let lib = library::get();
        for (i, (n, &c)) in self.normals.iter().zip(&self.constants).enumerate() {
            // The distances of the two ends: `mDot(normal, end) - constant`.
            let sd = math::dot(*n, a) - c;
            let ed = math::dot(*n, b) - c;
            // Both ends exactly on the plane: nothing to learn from it.
            if sd == 0.0 && ed == 0.0 {
                continue;
            }
            if (lib.same_side)(sd, ed) {
                // Both on one side: outside it ends the cast, inside it changes nothing.
                if sd > 0.0 {
                    return Ok(None);
                }
                continue;
            }
            let nt = sd.abs();
            let dt = (sd - ed).abs();
            // The first side flip: entering planes (start outside) accumulate in x, leaving ones in y.
            if side * sd >= 0.0 {
                side = -side;
                on_y = !on_y;
            }
            // `num*endDen*sgn < endNum*den*sgn && num*startDen*sgn < startNum*den*sgn`: no collision.
            let before = |cn: f32, cd: f32| (lib.ray_time_less)(nt, cd, cn, dt, side);
            if before(yn, yd) && before(xn, xd) {
                return Ok(None);
            }
            // Keep the plane when it narrows the current fraction: `num * *pden * sgn < *pnum * den * sgn`.
            let (cn, cd) = if on_y { (yn, yd) } else { (xn, xd) };
            if (lib.ray_time_less)(nt, cd, cn, dt, side) {
                if on_y {
                    yn = nt;
                    yd = dt;
                } else {
                    xn = nt;
                    xd = dt;
                    x_plane = i;
                    x_found = true;
                }
            }
        }
        if !x_found {
            return Ok(None);
        }
        let t = xn / xd;
        Ok(Some(RayHit {
            t,
            normal: self.normals[x_plane],
        }))
    }
}

/// A mesh as the queries use it: the hull, and the first frame's vertices and the triangles of the polygon list.
#[derive(Debug, Clone, PartialEq)]
struct MeshGeom {
    hull: Hull,
    verts: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
}

/// One object's mesh of a detail with its node's pose.
#[derive(Debug, Clone, PartialEq)]
struct Part {
    node: Matrix,
    node_inverse: Matrix,
    geom: Arc<MeshGeom>,
}

/// The prepared queries of a shape: the parts of each collision and LOS detail.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Collision {
    details: BTreeMap<i32, Vec<Part>>,
}

/// The six planes of a box `min` to `max` as `containerBoxEmpty` makes them, each `(nx, ny, nz, d)` with a point
/// outside when `n . p + d > 0`: -x at the minimum, +y at the maximum, +x at the maximum, -y at the minimum, -z, +z.
pub fn box_planes(min: [f32; 3], max: [f32; 3]) -> [[f32; 4]; 6] {
    [
        [-1.0, 0.0, 0.0, min[0]],
        [0.0, 1.0, 0.0, -max[1]],
        [1.0, 0.0, 0.0, -max[0]],
        [0.0, -1.0, 0.0, min[1]],
        [0.0, 0.0, -1.0, min[2]],
        [0.0, 0.0, 1.0, -max[2]],
    ]
}

/// A vertex of the polygon list: its position and the planes it is outside of.
#[derive(Debug, Clone, Copy)]
struct ListVertex {
    p: [f32; 3],
    mask: u32,
}

/// `EarlyOutPolyList::addPoint`'s test: the planes `p` is strictly in front of (the table's `in_front`).
fn outside_mask(planes: &[[f32; 4]; 6], p: [f32; 3]) -> u32 {
    let in_front = library::get().in_front;
    let mut mask = 0;
    for (i, pl) in planes.iter().enumerate() {
        if in_front(pl, p) {
            mask |= 1 << i;
        }
    }
    mask
}

/// `EarlyOutPolyList::end`: whether the polygon `poly` (indices into `verts`) is inside the box or survives clipping
/// against the planes it crosses. A vertex made by a clip is added to `verts`.
fn polygon_in_box(planes: &[[f32; 4]; 6], verts: &mut Vec<ListVertex>, poly: &[usize; 3]) -> bool {
    let mut front = 0u32;
    let mut back = 0u32;
    for &i in poly {
        front |= verts[i].mask;
        back |= !verts[i].mask;
    }
    if front == 0 {
        return true;
    }
    let cross = front & back;
    // Some plane that the polygon does not cross has all of it in front.
    if !cross & front != 0 {
        return false;
    }
    let mut current: Vec<usize> = poly.to_vec();
    for p in 0..planes.len() {
        let bit = 1u32 << p;
        if cross & bit == 0 {
            continue;
        }
        let mut next = Vec::with_capacity(current.len() + 2);
        let mut previous = current[current.len() - 1];
        let mut mask1 = verts[previous].mask;
        for &index in &current {
            let mask2 = verts[index].mask;
            if (mask1 ^ mask2) & bit != 0 {
                let made = clip_vertex(planes, p, &verts[previous], &verts[index]);
                verts.push(made);
                next.push(verts.len() - 1);
            }
            if mask2 & bit == 0 {
                next.push(index);
            }
            mask1 = mask2;
            previous = index;
        }
        if next.len() < 3 {
            return false;
        }
        current = next;
    }
    true
}

/// The vertex where the edge `from` to `to` meets plane `p`: `t = -distToPlane(start) / mDot(plane, diff)`, the point
/// `start + diff * t`. Its mask names the first later plane it is outside of, nothing else.
fn clip_vertex(planes: &[[f32; 4]; 6], p: usize, from: &ListVertex, to: &ListVertex) -> ListVertex {
    let lib = library::get();
    let pl = &planes[p];
    let (dx, dy, dz) = (
        to.p[0] - from.p[0],
        to.p[1] - from.p[1],
        to.p[2] - from.p[2],
    );
    let start = (lib.plane_dist)(pl, from.p);
    let rate = math::dot([pl[0], pl[1], pl[2]], [dx, dy, dz]);
    let t = -start / rate;
    let point = [
        (lib.mul_add)(dx, t, from.p[0]),
        (lib.mul_add)(dy, t, from.p[1]),
        (lib.mul_add)(dz, t, from.p[2]),
    ];
    let mut mask = 0;
    for (later, other) in planes.iter().enumerate().skip(p + 1) {
        let dist = (lib.plane_dist)(other, point);
        if dist > 0.0 {
            mask = 1 << later;
            break;
        }
    }
    ListVertex { p: point, mask }
}

/// The transforms of the nodes of one sub-shape at rest (`animateNodes`): each node's local matrix from its default rotation
/// and translation, then each node, in order, as its parent's final matrix times its own. A parent outside the range, or one not
/// made yet, leaves the node as its local matrix.
fn pose(
    shape: &Shape,
    first: usize,
    count: usize,
    charge: Charge,
) -> Result<BTreeMap<usize, Matrix>, ShapeError> {
    // The reader does not check a sub-shape's node range, so the range is cut to the nodes there are before it is paid for.
    let end = first.saturating_add(count).min(shape.nodes.len());
    pay(charge, 8 + 64 * end.saturating_sub(first) as u64)?;
    let mut out = BTreeMap::new();
    let mut local = BTreeMap::new();
    for i in first..end {
        let (Some(q), Some(t)) = (
            shape.default_rotations.get(i),
            shape.default_translations.get(i),
        ) else {
            return Err(ShapeError::Bad("default transform"));
        };
        local.insert(i, math::node_matrix(*q, *t));
    }
    out.extend(local.iter().map(|(i, m)| (*i, *m)));
    for i in first..end {
        let parent = shape.nodes[i].parent;
        if let Ok(parent) = usize::try_from(parent)
            && let (Some(p), Some(l)) = (out.get(&parent).copied(), local.get(&i))
        {
            out.insert(i, math::mat_mul(&p, l));
        }
    }
    Ok(out)
}

impl Collision {
    /// Prepares the queries for the details of `slots`: the pose of each sub-shape they use, and the hull, vertices and triangles
    /// of each mesh. Charged by the triangles ([`TRI_WORK`], [`PLANE_WORK`]), vertices and nodes, before the work; a shape past
    /// [`MAX_COLLISION_TRIANGLES`] triangles in its slots is refused.
    pub fn build(shape: &Shape, slots: &Slots, charge: Charge) -> Result<Collision, ShapeError> {
        let total = slot_triangles(shape, slots, charge)?;
        if total > MAX_COLLISION_TRIANGLES as u64 {
            return Err(ShapeError::Cap {
                what: "collision triangles",
                value: total,
                cap: MAX_COLLISION_TRIANGLES as u64,
            });
        }
        let mut wanted: Vec<i32> = slots
            .collision
            .iter()
            .chain(&slots.los)
            .copied()
            .filter(|&d| d >= 0)
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        let mut geoms: BTreeMap<*const Mesh, Arc<MeshGeom>> = BTreeMap::new();
        let mut poses: BTreeMap<usize, BTreeMap<usize, Matrix>> = BTreeMap::new();
        let mut details = BTreeMap::new();
        for detail in wanted {
            let Some(d) = usize::try_from(detail)
                .ok()
                .and_then(|i| shape.details.get(i))
            else {
                continue;
            };
            let Some(ss_index) = usize::try_from(d.sub_shape).ok() else {
                continue;
            };
            let Some(ss) = shape.sub_shapes.get(ss_index) else {
                continue;
            };
            if let std::collections::btree_map::Entry::Vacant(e) = poses.entry(ss_index) {
                let first = usize::try_from(ss.first_node).unwrap_or(0);
                let count = usize::try_from(ss.num_nodes).unwrap_or(0);
                e.insert(pose(shape, first, count, charge)?);
            }
            let mut parts = Vec::new();
            for (oi, mesh) in detail_meshes(shape, detail, charge)? {
                let node = usize::try_from(shape.objects[oi].node)
                    .ok()
                    .and_then(|n| poses[&ss_index].get(&n).copied())
                    .unwrap_or(IDENTITY);
                let key: *const Mesh = mesh;
                let geom = match geoms.get(&key) {
                    Some(g) => Arc::clone(g),
                    None => {
                        // `Hull::build` paid for its own walk of the primitives; the polygon list's is a second one.
                        pay(
                            charge,
                            PRIMITIVE_WORK.saturating_mul(mesh.primitives.len() as u64),
                        )?;
                        let per_frame = usize::try_from(mesh.verts_per_frame).unwrap_or(0);
                        let all = mesh.positions.as_ref();
                        let verts = all.get(..per_frame.min(all.len())).unwrap_or(all).to_vec();
                        let g = Arc::new(MeshGeom {
                            hull: Hull::build(mesh, charge)?,
                            tris: triangles(mesh, verts.len(), true),
                            verts,
                        });
                        geoms.insert(key, Arc::clone(&g));
                        g
                    }
                };
                parts.push(Part {
                    node,
                    node_inverse: math::mat_inverse(&node),
                    geom,
                });
            }
            details.insert(detail, parts);
        }
        Ok(Collision { details })
    }

    /// `TSShapeInstance::castRay` for one detail: the ray moved into each object's node space, cast at its hull, the nearest
    /// hit kept (a later object wins a tie; the fraction must be at most 1), its normal turned by the node's rotation.
    pub fn cast_detail(
        &self,
        detail: i32,
        a: [f32; 3],
        b: [f32; 3],
        charge: Charge,
    ) -> Result<Option<RayHit>, ShapeError> {
        let Some(parts) = self.details.get(&detail) else {
            return Ok(None);
        };
        let mut best: Option<(RayHit, &Matrix)> = None;
        let mut best_t = 1.0f32;
        for part in parts {
            let a2 = math::mul_p(&part.node_inverse, a);
            let b2 = math::mul_p(&part.node_inverse, b);
            if let Some(hit) = part.geom.hull.cast(a2, b2, charge)?
                && hit.t <= best_t
            {
                best_t = hit.t;
                best = Some((hit, &part.node));
            }
        }
        Ok(best.map(|(hit, node)| RayHit {
            t: hit.t,
            normal: math::mul_v(node, hit.normal),
        }))
    }

    /// A shape-based object's `castRay` up to its own turn of the normal: the segment cast against each LOS detail
    /// ([`detail_for_ray`]), the nearest hit kept (the earlier entry on a tie). The normal is in the object's space; the
    /// caller turns it by the object's transform.
    pub fn cast_ray(
        &self,
        slots: &Slots,
        a: [f32; 3],
        b: [f32; 3],
        charge: Charge,
    ) -> Result<Option<RayHit>, ShapeError> {
        let mut best: Option<RayHit> = None;
        for slot in 0..slots.los.len() {
            let detail = detail_for_ray(slots, slot);
            if detail < 0 {
                continue;
            }
            if let Some(hit) = self.cast_detail(detail, a, b, charge)?
                && best.is_none_or(|b| hit.t < b.t)
            {
                best = Some(hit);
            }
        }
        Ok(best)
    }

    /// `ShapeBase::buildPolyList` into an `EarlyOutPolyList` of the six `planes`: whether anything of the collision
    /// details lies inside them (a vertex on a plane is inside). `world` is the object's transform; each object's
    /// matrix is that times its node's, with the unit scale of the list.
    pub fn box_hit(
        &self,
        slots: &Slots,
        world: &Matrix,
        planes: &[[f32; 4]; 6],
        charge: Charge,
    ) -> Result<bool, ShapeError> {
        for slot in 0..slots.collision.len() {
            let detail = detail_for_polylist(slots, slot);
            if detail < 0 {
                continue;
            }
            let Some(parts) = self.details.get(&detail) else {
                continue;
            };
            for part in parts {
                pay(
                    charge,
                    PART_WORK
                        .saturating_add(VERTEX_WORK.saturating_mul(part.geom.verts.len() as u64))
                        .saturating_add(TRI_WORK.saturating_mul(part.geom.tris.len() as u64)),
                )?;
                let scaled = math::mat_mul(world, &IDENTITY);
                let matrix = math::mat_mul(&scaled, &part.node);
                let mut verts: Vec<ListVertex> = part
                    .geom
                    .verts
                    .iter()
                    .map(|&v| {
                        let p = math::mul_p(&matrix, v);
                        ListVertex {
                            p,
                            mask: outside_mask(planes, p),
                        }
                    })
                    .collect();
                if verts.iter().any(|v| v.mask == 0) {
                    return Ok(true);
                }
                let base = verts.len();
                for tri in &part.geom.tris {
                    verts.truncate(base);
                    let poly = tri.map(|i| i as usize);
                    if polygon_in_box(planes, &mut verts, &poly) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// How many details the prepared queries cover.
    pub fn details(&self) -> usize {
        self.details.len()
    }
}

#[cfg(test)]
#[path = "collide_tests.rs"]
mod tests;
