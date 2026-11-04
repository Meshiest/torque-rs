//! What a scene object in the container's bins answers a query with: its `castRay` and its
//! `buildPolyList`, by class, and the mesh pieces of a `ShapeBase` at its current pose that both of them and `collide`'s
//! convexes read.
//!
//! - a class the host registered ([`super::classes`]): `castRay` its own; `buildPolyList` its own
//! - `Player`, `AIPlayer`: `castRay` no hit when the damage state is not 0, else a slab test of the object box
//!   ([`RayRules::player_cast`]), or its LOS meshes at its pose when its datablock asks for hitboxes ([`super::hitboxes`]);
//!   `buildPolyList` that box at the position, no rotation
//! - `Item`: `castRay` the LOS slots as `ShapeBase`'s ([`RayRules::item_without_los`] when no slot is set); `buildPolyList` the
//!   raw box at the position, the object's scale
//! - the other `ShapeBase` classes (StaticShape, the vehicles, markers, Camera): `castRay` each LOS slot at the current pose
//!   ([`RayRules::los_pick`]); `buildPolyList` the collision slots at the current pose
//! - everything else (Projectile, zones, lights, emitters, the sun): `castRay` the `SceneObject` default: no hit;
//!   `buildPolyList` none
//!
//! **The segment in the object's space** is `Container::castRay`'s own transform ([`torque_scene::bins::object_point`]: the
//! world-to-object matrix, the affine inverse of the transform, and the scale); the normal comes back by the rotation of
//! the transform (`mulV`, no scale). The container recomputes the point from the world segment and `t`.
//!
//! **The pose.** `ShapeBase::castRay` animates the instance for each LOS detail first (`TSShapeInstance::animate`,
//! [`shapebase::animate`]), then casts against each mesh object of the detail moved by its node's transform at that pose
//! (`TSShapeInstance::castRay`: the nearest per detail, a later object winning a tie). The convex hull of a mesh
//! (`TSMesh::buildConvexHull`) is made once per kept shape and mesh ([`crate::shapes::KeptShape`]'s cache) and paid for when it
//! is made.
//!
//! **Edge cases.** A Player whose segment starts inside its box is hit at `t = -0.0` with the normal `(0, 0, 1)` (the container
//! then puts the point at the start).
//!
//! **Guard.** A `ShapeBase` whose every hit has a NaN `t` answers nothing.
//!
//! **A Player that is not enabled** (the damage state of `sim::vitals`: disabled or destroyed) answers no ray, as
//! `Player::castRay` tests it first.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/shapeBase.cpp`
//! and `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); `T3D/item.cpp`,
//! `ts/tsCollision.cpp` and `ts/tsMesh.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::rc::Rc;

use collide::ObjectId;
use collide::math::Box3;
use collide::mesh::MeshPart;
use collide::poly::PolyList;
use shapes::ShapeError;
use shapes::collide::{Hull as MeshHull, RayHit};
use shapes::math as fpu;
use shapes::preload::Slots;
use shapes::shape::{Mesh, Shape};
use torque_scene::bins::object_point;
use torque_scene::ray_point;
use torque_scene::transform::{self as grid, Matrix};
use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError};

use super::access::{matrix_of, scale_of};
use super::camera::is_class;
use super::classes::SceneClass;
use super::ext;
use super::shapebase;
use crate::shapes::KeptShape;

/// What an object's `castRay` answered: the fraction of the segment and the normal in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectHit {
    pub t: f32,
    pub normal: [f32; 3],
}

/// Which `castRay` and `buildPolyList` an object has.
#[derive(Clone)]
pub enum RayClass {
    /// A class the host registered ([`super::classes`]): it answers for itself.
    Custom(Rc<dyn SceneClass>),
    Player,
    Item,
    Shape,
    None,
}

/// The object's class as the container's two virtual calls see it: a registered class first, then the engine's.
pub fn ray_class(vm: &Vm, id: ObjId) -> RayClass {
    if let Some(answer) = super::classes::of(vm, id) {
        RayClass::Custom(answer)
    } else if is_class(vm, id, "Player") {
        RayClass::Player
    } else if is_class(vm, id, "Item") {
        RayClass::Item
    } else if is_class(vm, id, "ShapeBase") {
        RayClass::Shape
    } else {
        RayClass::None
    }
}

fn budget(e: ShapeError) -> Option<VmError> {
    matches!(e, ShapeError::OutOfBudget).then_some(VmError::StepLimit)
}

/// The object's box, the empty box when it has no scene state.
pub fn object_box(vm: &Vm, id: ObjId) -> ([f32; 3], [f32; 3]) {
    ext::get(vm.objects(), id)
        .and_then(|e| e.scene)
        .map_or(([0.0; 3], [0.0; 3]), |s| (s.object_min, s.object_max))
}

/// The ray rules of the engine's `Player`, `Item` and `ShapeBase` classes that a host may replace ([`set_ray_rules`]); every
/// method has TGE's answer by default ([`TorqueRays`]). Leaves: no script runs in them.
pub trait RayRules {
    /// `Player::castRay` in the object's space against its object box `min`..`max`: the fraction and the normal in the object's
    /// space. TGE's: the box's slab test (as `Box3F::collideLine`), the normal `start - end` normalised.
    fn player_cast(
        &self,
        min: [f32; 3],
        max: [f32; 3],
        a: [f32; 3],
        b: [f32; 3],
    ) -> Option<(f32, [f32; 3])> {
        torque_player_slab(min, max, a, b)
    }

    /// The box `Player::buildPolyList` adds (at the Player's position, no rotation): TGE's is the object box.
    fn player_poly_box(&self, min: [f32; 3], max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
        (min, max)
    }

    /// `Item::castRay` when its datablock names no LOS detail: TGE's Item has `ShapeBase`'s, which answers nothing then.
    fn item_without_los(
        &self,
        min: [f32; 3],
        max: [f32; 3],
        a: [f32; 3],
        b: [f32; 3],
    ) -> Option<(f32, [f32; 3])> {
        let _ = (min, max, a, b);
        None
    }

    /// `ShapeBase::castRay`'s fold of one LOS detail's hit into the answer so far: TGE keeps the nearest.
    fn los_pick(&self, answer: Option<RayHit>, hit: Option<RayHit>) -> Option<RayHit> {
        match (answer, hit) {
            (Some(a), Some(h)) if h.t < a.t => Some(h),
            (None, Some(h)) if h.t < 1e8 => Some(h),
            (answer, _) => answer,
        }
    }
}

/// TGE's ray rules.
#[derive(Debug, Default, Clone, Copy)]
pub struct TorqueRays;

impl RayRules for TorqueRays {}

struct InstalledRays(Rc<dyn RayRules>);

/// The host's ray rules for `vm` from now on.
pub fn set_ray_rules(vm: &mut Vm, rules: Rc<dyn RayRules>) {
    vm.set_extension(Rc::new(InstalledRays(rules)));
}

/// The ray rules `vm`'s game installed, else TGE's.
pub fn ray_rules(vm: &Vm) -> Rc<dyn RayRules> {
    vm.extension::<InstalledRays>().map_or_else(
        || Rc::new(TorqueRays) as Rc<dyn RayRules>,
        |i| Rc::clone(&i.0),
    )
}

/// TGE's `Player::castRay` slab test in the object's space: the entry fraction of the segment into the box, and the normal
/// `a - b` normalised (`normalizeSafe`); `None` when the segment misses the box.
pub fn torque_player_slab(
    min: [f32; 3],
    max: [f32; 3],
    a: [f32; 3],
    b: [f32; 3],
) -> Option<(f32, [f32; 3])> {
    let mut first = 0.0f32;
    let mut last = 1.0f32;
    for i in 0..3 {
        let (st, et);
        if a[i] < b[i] {
            if a[i] > max[i] || b[i] < min[i] {
                return None;
            }
            let di = b[i] - a[i];
            st = if a[i] < min[i] {
                (min[i] - a[i]) / di
            } else {
                0.0
            };
            et = if b[i] > max[i] {
                (max[i] - a[i]) / di
            } else {
                1.0
            };
        } else {
            if b[i] > max[i] || a[i] < min[i] {
                return None;
            }
            let di = b[i] - a[i];
            st = if a[i] > max[i] {
                (max[i] - a[i]) / di
            } else {
                0.0
            };
            et = if b[i] < min[i] {
                (min[i] - a[i]) / di
            } else {
                1.0
            };
        }
        if st > first {
            first = st;
        }
        if et < last {
            last = et;
        }
        if last < first {
            return None;
        }
    }
    let n = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let n = if len > POINT_EPSILON {
        n.map(|c| c / len)
    } else {
        n
    };
    Some((first, n))
}

/// `POINT_EPSILON`: the smallest length `normalizeSafe` divides by.
const POINT_EPSILON: f32 = 1e-4;

/// The `animate(detail)` calls of the object's `castRay` (for each LOS detail, in slot order), made before [`object_ray`]: the
/// casts do not change the instance, so animating every detail first and casting after gives what an interleaved loop gives,
/// and the cast can read the pose with a shared borrow. Nothing for a class whose `castRay` has no shape.
pub fn animate_for_ray(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    if matches!(ray_class(vm, id), RayClass::Player) {
        return super::hitboxes::prepare(vm, id);
    }
    animate_slots(vm, id, |s| &s.los)
}

/// The `animate(detail)` calls of the object's `buildPolyList` (each collision detail in slot order), made before
/// [`build_poly_list`].
pub fn animate_for_poly(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    animate_slots(vm, id, |s| &s.collision)
}

fn animate_slots(vm: &mut Vm, id: ObjId, pick: fn(&Slots) -> &[i32]) -> Result<(), VmError> {
    if !matches!(ray_class(vm, id), RayClass::Item | RayClass::Shape) {
        return Ok(());
    }
    let Some(slots) = los_slots(vm, id) else {
        return Ok(());
    };
    for &detail in pick(&slots) {
        if let Ok(at) = usize::try_from(detail) {
            shapebase::animate(vm, id, at)?;
        }
    }
    Ok(())
}

/// `castRay` of the object for the world segment `start` to `end`: `None` for a miss or a class that answers nothing. A
/// `ShapeBase` is cast at its pose as it stands ([`animate_for_ray`] first animates it). Charged; a refusal of the budget is
/// [`VmError::StepLimit`].
pub fn object_ray(
    vm: &Vm,
    id: ObjId,
    start: [f32; 3],
    end: [f32; 3],
) -> Result<Option<ObjectHit>, VmError> {
    // A map object (a terrain, an interior): its terrain or hulls in the world's space.
    if let Some(m) = super::ext::map_of(vm, id) {
        let Some(shape) = &m.shape else {
            return Ok(None);
        };
        return match shape.cast_ray(start, end, &mut |u| vm.charge_work(u)) {
            Ok(hit) => Ok(hit.map(|(t, normal)| ObjectHit { t, normal })),
            Err(_) => Err(VmError::StepLimit),
        };
    }
    let class = ray_class(vm, id);
    match &class {
        RayClass::None => return Ok(None),
        RayClass::Custom(answer) => return answer.cast_ray(vm, id, start, end),
        _ => {}
    }
    let Some(matrix) = matrix_of(vm, id) else {
        return Ok(None);
    };
    let scale = scale_of(vm, id);
    let w2o = grid::affine_inverse(&matrix);
    let a = object_point(&w2o, scale, start);
    let b = object_point(&w2o, scale, end);
    let (min, max) = object_box(vm, id);
    let turned = |(t, n): (f32, [f32; 3])| ObjectHit {
        t,
        normal: grid::rotate(&matrix, n),
    };
    Ok(match class {
        RayClass::Player => {
            // A Player that is not enabled (disabled or destroyed) is not hit.
            if crate::sim::vitals::get(vm, id).damage_state != 0 {
                return Ok(None);
            }
            if super::hitboxes::wanted(vm, id)
                && let Some(parts) = super::hitboxes::cast_live(vm, id, a, b)?
            {
                return Ok(parts.map(|p| {
                    super::hitboxes::note(vm, id, ray_point(start, end, p.t), p.object);
                    turned((p.t, p.normal))
                }));
            }
            ray_rules(vm).player_cast(min, max, a, b).map(turned)
        }
        RayClass::Item => match los_slots(vm, id) {
            Some(slots) if slots.los.iter().any(|&l| l >= 0) => {
                shape_los(vm, id, &slots, a, b)?.map(turned)
            }
            _ => ray_rules(vm).item_without_los(min, max, a, b).map(turned),
        },
        RayClass::Shape => match los_slots(vm, id) {
            Some(slots) if body_of(vm, id).is_some() => {
                shape_los(vm, id, &slots, a, b)?.map(turned)
            }
            _ => None,
        },
        RayClass::None | RayClass::Custom(_) => None,
    })
}

/// The world segment in an object's space as `Container::castRay` moves it ([`torque_scene::bins::object_point`]), with the
/// transform its normal turns back by: for a registered class's own `castRay`. `None` for an object with no transform.
pub fn object_segment(
    vm: &Vm,
    id: ObjId,
    start: [f32; 3],
    end: [f32; 3],
) -> Option<(Matrix, [f32; 3], [f32; 3])> {
    let matrix = matrix_of(vm, id)?;
    let scale = scale_of(vm, id);
    let w2o = grid::affine_inverse(&matrix);
    Some((
        matrix,
        object_point(&w2o, scale, start),
        object_point(&w2o, scale, end),
    ))
}

/// The shape body of a `ShapeBase`, if it has an instance.
pub(crate) fn body_of(vm: &Vm, id: ObjId) -> Option<&shapebase::ShapeBody> {
    shapebase::state(vm, id)?.body.as_ref()
}

/// The eight slots of the shape the object's datablock loaded (resolved at its preload).
pub(crate) fn los_slots(vm: &Vm, id: ObjId) -> Option<Slots> {
    body_of(vm, id)?
        .kept
        .facts
        .as_ref()
        .map(|f| f.slots.clone())
}

/// The LOS loop of `ShapeBase::castRay` and `Item::castRay` in the object's space: per slot with a detail, the
/// cast at the current pose; every success marks the object and, when its `t` is below 1e8, becomes THE answer (the last one
/// wins).
fn shape_los(
    vm: &Vm,
    id: ObjId,
    slots: &Slots,
    a: [f32; 3],
    b: [f32; 3],
) -> Result<Option<(f32, [f32; 3])>, VmError> {
    let rules = ray_rules(vm);
    let mut local: Option<RayHit> = None;
    for &detail in &slots.los {
        if detail < 0 {
            continue;
        }
        local = rules.los_pick(local, posed_cast(vm, id, detail, a, b)?);
    }
    Ok(local.map(|h| (h.t, h.normal)))
}

/// The objects of `detail` that have a mesh at the detail's object detail: `(object index, mesh index)` (as
/// `TSShapeInstance::castRay` and `buildPolyList` walk them). Each object of the sub-shape is paid for first.
pub fn detail_objects(
    shape: &Shape,
    detail: i32,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Vec<(usize, usize)>, VmError> {
    let mut out = Vec::new();
    let Some(dl) = usize::try_from(detail)
        .ok()
        .and_then(|i| shape.details.get(i))
    else {
        return Ok(out);
    };
    let Some(ss) = usize::try_from(dl.sub_shape)
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
    if !charge(shapes::collide::OBJECT_WORK.saturating_mul(end.saturating_sub(first) as u64)) {
        return Err(VmError::StepLimit);
    }
    for oi in first..end {
        let object = &shape.objects[oi];
        let od = dl.object_detail;
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
        if let Some(Some(_)) = shape.meshes.get(at) {
            out.push((oi, at));
        }
    }
    Ok(out)
}

/// The convex hull of mesh `index` of the kept shape, built the first time (`TSMesh::buildConvexHull`) and paid for then.
pub(crate) fn mesh_hull(
    kept: &KeptShape,
    index: usize,
    mesh: &Mesh,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Option<Rc<MeshHull>>, VmError> {
    if let Some(h) = kept.hulls.meshes.borrow().get(&index) {
        return Ok(Some(Rc::clone(h)));
    }
    match MeshHull::build(mesh, charge) {
        Ok(h) => {
            let h = Rc::new(h);
            kept.hulls.meshes.borrow_mut().insert(index, Rc::clone(&h));
            Ok(Some(h))
        }
        Err(e) => match budget(e) {
            Some(err) => Err(err),
            None => Ok(None),
        },
    }
}

/// A node's transform at the instance's current pose (the identity for an object with no node, as `shapes::collide` takes it).
pub(crate) fn node_of(nodes: &[Matrix], shape: &Shape, object: usize) -> Matrix {
    usize::try_from(shape.objects[object].node)
        .ok()
        .and_then(|n| nodes.get(n).copied())
        .unwrap_or(fpu::IDENTITY)
}

/// `TSShapeInstance::castRay` for one detail at the instance's current node transforms: per object, the segment moved by
/// the inverse of its node's transform, cast at its mesh's hull; the nearest kept (a later object wins a tie, the fraction at
/// most 1), its normal turned by the node.
fn posed_cast(
    vm: &Vm,
    id: ObjId,
    detail: i32,
    a: [f32; 3],
    b: [f32; 3],
) -> Result<Option<RayHit>, VmError> {
    let Some(body) = body_of(vm, id) else {
        return Ok(None);
    };
    let shape = body.shape();
    let nodes = body.instance.node_transforms();
    let mut charge = |u: u64| vm.charge_work(u);
    let objects = detail_objects(shape, detail, &mut charge)?;
    let mut best: Option<(RayHit, Matrix)> = None;
    let mut best_t = 1.0f32;
    for (oi, mi) in objects {
        let Some(Some(mesh)) = shape.meshes.get(mi) else {
            continue;
        };
        let Some(hull) = mesh_hull(&body.kept, mi, mesh, &mut charge)? else {
            continue;
        };
        let node = node_of(nodes, shape, oi);
        let inverse = fpu::mat_inverse(&node);
        let a2 = fpu::mul_p(&inverse, a);
        let b2 = fpu::mul_p(&inverse, b);
        match hull.cast(a2, b2, &mut charge) {
            Ok(Some(hit)) if hit.t <= best_t => {
                best_t = hit.t;
                best = Some((hit, node));
            }
            Ok(_) => {}
            Err(e) => {
                if let Some(err) = budget(e) {
                    return Err(err);
                }
            }
        }
    }
    Ok(best.map(|(hit, node)| RayHit {
        t: hit.t,
        normal: fpu::mul_v(&node, hit.normal),
    }))
}

// --------------------------------------------------------------------------------------------------------------- polygon lists

/// The triangles of a mesh's primitives, first frame, with their material (`matIndex & MaterialMask`), in the order
/// `TSMesh::buildPolyList` emits them: a list's as they are, a strip's alternating, its repeated-index ones left out
/// (`shapes::collide`'s reading of the same code). An index past the vertices ends the triangle.
fn mesh_triangles(mesh: &Mesh, verts: usize) -> (Vec<[u32; 3]>, Vec<u32>) {
    let (mut tris, mut materials) = (Vec::new(), Vec::new());
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
        let material = prim.material & 0x0fff_ffff;
        if prim.material & 0xc000_0000 == 0 {
            let mut k = 0;
            while k < count {
                if let (Some(a), Some(b), Some(c)) =
                    (index(start + k), index(start + k + 1), index(start + k + 2))
                {
                    tris.push([a, b, c]);
                    materials.push(material);
                }
                k += 3;
            }
        } else if count > 2 {
            let (Some(mut a), Some(mut b)) = (index(start), index(start + 1)) else {
                continue;
            };
            let mut into_a = false;
            for k in 2..count {
                let Some(c) = index(start + k) else {
                    break;
                };
                if !(a == b || a == c || b == c) {
                    tris.push([a, b, c]);
                    materials.push(material);
                }
                if into_a {
                    b = c;
                } else {
                    a = c;
                }
                into_a = !into_a;
            }
        }
    }
    (tris, materials)
}

/// The mesh parts of `detail` at the node transforms `nodes` (a `collide` hull's parts): per object with a mesh, its node, the
/// first frame's vertices and the triangles. Paid for by the vertices and triangles.
pub fn detail_parts(
    shape: &Shape,
    nodes: &[Matrix],
    detail: i32,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Vec<MeshPart>, VmError> {
    let mut parts = Vec::new();
    for (oi, mi) in detail_objects(shape, detail, charge)? {
        let Some(Some(mesh)) = shape.meshes.get(mi) else {
            continue;
        };
        let per_frame = usize::try_from(mesh.verts_per_frame).unwrap_or(0);
        let all = mesh.positions.as_ref();
        let verts = all.get(..per_frame.min(all.len())).unwrap_or(all).to_vec();
        let cost = collide::work::VERTEX
            .saturating_mul(verts.len() as u64)
            .saturating_add(collide::work::POLY.saturating_mul(mesh.primitives.len() as u64));
        if !charge(cost) {
            return Err(VmError::StepLimit);
        }
        let (tris, materials) = mesh_triangles(mesh, verts.len());
        parts.push(MeshPart {
            node: node_of(nodes, shape, oi),
            verts,
            tris,
            materials,
        });
    }
    Ok(parts)
}

/// `TSShapeInstance::buildPolyList` for one detail into `list`, as `collide::mesh::feed_hull` does it: the list's transform
/// times its scale times each part's node becomes the list's transform at unit scale, every vertex, then each triangle; the
/// list's transform and scale are put back.
fn feed_parts(
    list: &mut dyn PolyList,
    parts: &[MeshPart],
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<(), VmError> {
    let saved = list.base().transform();
    let saved_scale = list.base().scale();
    let mut s = collide::math::IDENTITY;
    s[0] = saved_scale[0];
    s[5] = saved_scale[1];
    s[10] = saved_scale[2];
    let scaled = collide::math::matrix_product(&saved, &s);
    let mut key = 0u32;
    let mut result = Ok(());
    for part in parts {
        let cost = collide::work::VERTEX * part.verts.len() as u64
            + collide::work::POLY * part.tris.len() as u64;
        if !charge(cost) {
            result = Err(VmError::StepLimit);
            break;
        }
        let m = collide::math::matrix_product(&scaled, &part.node);
        list.set_transform(&m, [1.0, 1.0, 1.0]);
        let mut base = 0u32;
        for (i, v) in part.verts.iter().enumerate() {
            let idx = list.add_point(*v);
            if i == 0 {
                base = idx;
            }
        }
        for (t, tri) in part.tris.iter().enumerate() {
            list.begin(part.materials.get(t).copied().unwrap_or(0), key);
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

/// The translation of the transform in an otherwise identity matrix (the static matrices whose translation
/// slots `Player::buildPolyList` and `Item::buildPolyList` overwrite).
fn at_position(matrix: &Matrix) -> Matrix {
    let mut m = collide::math::IDENTITY;
    m[3] = matrix[3];
    m[7] = matrix[7];
    m[11] = matrix[11];
    m
}

/// `buildPolyList` of the object into `list`: true when the object added its geometry (a `ShapeBase` with at least one
/// collision slot, a Player, an Item). A `ShapeBase` feeds its pose as it stands ([`animate_for_poly`] first animates it).
/// Charged.
pub fn build_poly_list(vm: &Vm, id: ObjId, list: &mut dyn PolyList) -> Result<bool, VmError> {
    let class = ray_class(vm, id);
    if let RayClass::Custom(answer) = &class {
        return answer.build_poly_list(vm, id, list);
    }
    let Some(matrix) = matrix_of(vm, id) else {
        return Ok(false);
    };
    let (min, max) = object_box(vm, id);
    match class {
        RayClass::Player => {
            list.set_transform(&at_position(&matrix), [1.0; 3]);
            list.set_object(Some(ObjectId(id)));
            let (min, max) = ray_rules(vm).player_poly_box(min, max);
            list.add_box(&Box3 { min, max });
            if !vm.charge_work(collide::work::POLY * 6) {
                return Err(VmError::StepLimit);
            }
            Ok(true)
        }
        RayClass::Item => {
            list.set_transform(&at_position(&matrix), scale_of(vm, id));
            list.set_object(Some(ObjectId(id)));
            list.add_box(&Box3 { min, max });
            if !vm.charge_work(collide::work::POLY * 6) {
                return Err(VmError::StepLimit);
            }
            Ok(true)
        }
        RayClass::Shape => {
            let Some(slots) = los_slots(vm, id).filter(|_| body_of(vm, id).is_some()) else {
                return Ok(false);
            };
            list.set_transform(&matrix, scale_of(vm, id));
            list.set_object(Some(ObjectId(id)));
            let mut any = false;
            for &detail in &slots.collision {
                if detail < 0 {
                    continue;
                }
                any = true;
                let parts = {
                    let Some(body) = body_of(vm, id) else {
                        break;
                    };
                    let mut charge = |u: u64| vm.charge_work(u);
                    detail_parts(
                        body.shape(),
                        body.instance.node_transforms(),
                        detail,
                        &mut charge,
                    )?
                };
                let mut charge = |u: u64| vm.charge_work(u);
                feed_parts(list, &parts, &mut charge)?;
            }
            Ok(any)
        }
        RayClass::Custom(_) | RayClass::None => Ok(false),
    }
}

#[cfg(test)]
#[path = "rays_plain_tests.rs"]
mod tests;
