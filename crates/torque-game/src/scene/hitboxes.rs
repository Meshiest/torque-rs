//! Posed hitboxes: a Player whose datablock says `hitboxes = true` answers a ray with the meshes of its shape's LOS details at
//! its pose instead of its upright object box. Movement collision stays the box.
//!
//! - **The parts** are what `ShapeBase::castRay` casts against: each mesh object of the LOS details (`LOS-N`, or a collision
//!   detail that has none of its own), on its node, as its convex hull. A part's name is its object's name (`head`, `torso`,
//!   ...). A shape with no such mesh falls back to the box.
//! - **The pose** is the server's own threads as they stand (the action, arm, head, headside and class threads, and the
//!   script threads); each LOS detail is animated before the cast, as `ShapeBase::castRay` does.
//! - **The answer**: the nearest part hit with `t` at most 1 (a later object wins a tie), its normal turned by its node. The
//!   container's bins still reach a Player only through its world box, so a part that pokes out of the box is not reached
//!   outside it.
//! - **The part hit** is kept per Player with the point ([`note`]); `getDamageLocation(%pos)` answers that part for that
//!   point, else the part nearest `%pos` at the pose now, and "" for a Player without hitboxes (the method's stub answer).
//!
//! The opt-in field is a dynamic field (`dAtob`), read at each cast: a datablock without it is TGE's box, unchanged.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/shapeBase.cpp`
//! and `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); `ts/tsCollision.cpp`
//! (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use shapes::ShapeError;
use shapes::math as fpu;
use torque_scene::bins::object_point;
use torque_scene::transform::{self as grid, Matrix};
use ts_vm::fields::atob;
use ts_vm::sim::ObjId;
use ts_vm::text::scan_floats;
use ts_vm::vm::{Native, Vm, VmError};

use super::access::{matrix_of, object_field, scale_of};
use super::rays::{body_of, detail_objects, los_slots, mesh_hull, node_of};
use super::shapebase;
use crate::methods::register_method;
use crate::shapes::KeptShape;

/// The `PlayerData` field that turns the hitboxes on.
pub const FIELD: &[u8] = b"hitboxes";

/// The most Players whose last part hit is kept; past it the record starts again.
const MAX_NOTES: usize = 4096;

/// How far (units) `getDamageLocation`'s point may be from the last hit's point and still be that hit.
const SAME_POINT: f32 = 0.01;

/// Whether the Player's datablock asks for hitboxes.
pub fn wanted(vm: &Vm, id: ObjId) -> bool {
    let Some(db) = object_field(vm, id, b"dataBlock") else {
        return false;
    };
    let objects = vm.objects();
    objects
        .get(db)
        .is_some_and(|o| atob(&o.data_field(&objects.classes, FIELD, b"")))
}

/// A part hit in the object's space: the fraction, the normal and the shape's object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartHit {
    pub t: f32,
    pub normal: [f32; 3],
    pub object: usize,
}

/// A Player's hitboxes at one pose: the shape, the node transforms and the LOS details.
#[derive(Debug, Clone)]
pub struct Pose {
    pub kept: Rc<KeptShape>,
    pub nodes: Rc<[Matrix]>,
    pub los: Rc<[i32]>,
}

/// The cast of the segment `a` to `b` (the object's space) at the parts of `los` posed by `nodes`: `None` when the details have
/// no mesh (the box answers), else the nearest part hit. Charged; a refusal is [`VmError::StepLimit`].
pub fn cast(
    kept: &KeptShape,
    nodes: &[Matrix],
    los: &[i32],
    a: [f32; 3],
    b: [f32; 3],
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Option<Option<PartHit>>, VmError> {
    let Some(shape) = kept.shape.as_ref() else {
        return Ok(None);
    };
    let mut any = false;
    let mut best: Option<PartHit> = None;
    let mut best_t = 1.0f32;
    for &detail in los {
        if detail < 0 {
            continue;
        }
        for (oi, mi) in detail_objects(shape, detail, charge)? {
            let Some(Some(mesh)) = shape.meshes.get(mi) else {
                continue;
            };
            any = true;
            let Some(hull) = mesh_hull(kept, mi, mesh, charge)? else {
                continue;
            };
            let node = node_of(nodes, shape, oi);
            let inverse = fpu::mat_inverse(&node);
            match hull.cast(fpu::mul_p(&inverse, a), fpu::mul_p(&inverse, b), charge) {
                Ok(Some(hit)) if hit.t <= best_t => {
                    best_t = hit.t;
                    best = Some(PartHit {
                        t: hit.t,
                        normal: fpu::mul_v(&node, hit.normal),
                        object: oi,
                    });
                }
                Ok(_) => {}
                Err(ShapeError::OutOfBudget) => return Err(VmError::StepLimit),
                Err(_) => {}
            }
        }
    }
    Ok(any.then_some(best))
}

/// [`cast`] for the Player at its pose now (after [`prepare`]).
pub fn cast_live(
    vm: &Vm,
    id: ObjId,
    a: [f32; 3],
    b: [f32; 3],
) -> Result<Option<Option<PartHit>>, VmError> {
    let (Some(body), Some(slots)) = (body_of(vm, id), los_slots(vm, id)) else {
        return Ok(None);
    };
    let mut charge = |u: u64| vm.charge_work(u);
    cast(
        &body.kept,
        body.instance.node_transforms(),
        &slots.los,
        a,
        b,
        &mut charge,
    )
}

/// What a Player's cast does first: for one with hitboxes, each LOS detail animated (and the part record made).
pub fn prepare(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    if !wanted(vm, id) {
        return Ok(());
    }
    if vm.extension::<Notes>().is_none() {
        vm.set_extension(Rc::new(Notes::default()));
    }
    let Some(slots) = los_slots(vm, id) else {
        return Ok(());
    };
    for &detail in &slots.los {
        if let Ok(at) = usize::try_from(detail) {
            shapebase::animate(vm, id, at)?;
        }
    }
    Ok(())
}

/// The Player's hitboxes at its pose now ([`prepare`] first): `None` without hitboxes or a shape.
pub fn pose_now(vm: &mut Vm, id: ObjId) -> Result<Option<Pose>, VmError> {
    prepare(vm, id)?;
    let (Some(body), Some(slots)) = (body_of(vm, id), los_slots(vm, id)) else {
        return Ok(None);
    };
    Ok(Some(Pose {
        kept: Rc::clone(&body.kept),
        nodes: body.instance.node_transforms().into(),
        los: slots.los.into(),
    }))
}

/// A part hit kept: the world point and the part's name.
pub type Note = ([f32; 3], Vec<u8>);

/// The last part hit of each Player.
#[derive(Debug, Default)]
struct Notes(RefCell<HashMap<ObjId, Note>>);

/// The name of the shape's object `object`.
fn part_name(kept: &KeptShape, object: usize) -> Vec<u8> {
    kept.shape
        .as_ref()
        .and_then(|s| {
            let name = s.objects.get(object)?.name;
            s.names.get(usize::try_from(name).ok()?).cloned()
        })
        .unwrap_or_default()
}

/// Keeps `object` of the Player's shape as its last part hit, at the world point `point`.
pub fn note(vm: &Vm, id: ObjId, point: [f32; 3], object: usize) {
    let Some(body) = body_of(vm, id) else {
        return;
    };
    note_part(vm, id, point, part_name(&body.kept, object));
}

/// [`note`] for a part of a shape the Player had then (a rewound pose).
pub fn note_pose(vm: &Vm, id: ObjId, point: [f32; 3], pose: &Pose, object: usize) {
    note_part(vm, id, point, part_name(&pose.kept, object));
}

fn note_part(vm: &Vm, id: ObjId, point: [f32; 3], name: Vec<u8>) {
    let Some(notes) = vm.extension::<Notes>() else {
        return;
    };
    let mut map = notes.0.borrow_mut();
    if map.len() >= MAX_NOTES && !map.contains_key(&id) {
        map.clear();
    }
    map.insert(id, (point, name));
}

/// The Player's last part hit, with its world point.
pub fn last_hit(vm: &Vm, id: ObjId) -> Option<Note> {
    vm.extension::<Notes>()?.0.borrow().get(&id).cloned()
}

fn distance2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum()
}

/// `getDamageLocation(pos)` of a Player with hitboxes: the last part hit when `pos` is its point, else the part whose posed box
/// is nearest `pos` now (the first on a tie); "" without hitboxes.
pub fn damage_location(vm: &mut Vm, id: ObjId, pos: [f32; 3]) -> Result<Vec<u8>, VmError> {
    if !wanted(vm, id) {
        return Ok(Vec::new());
    }
    if let Some((point, name)) = last_hit(vm, id)
        && distance2(point, pos) <= SAME_POINT * SAME_POINT
    {
        return Ok(name);
    }
    prepare(vm, id)?;
    let (Some(body), Some(slots), Some(matrix)) =
        (body_of(vm, id), los_slots(vm, id), matrix_of(vm, id))
    else {
        return Ok(Vec::new());
    };
    let Some(shape) = body.kept.shape.as_ref() else {
        return Ok(Vec::new());
    };
    let p = object_point(&grid::affine_inverse(&matrix), scale_of(vm, id), pos);
    let mut charge = |u: u64| vm.charge_work(u);
    let mut best: Option<(f32, usize)> = None;
    for &detail in &slots.los {
        for (oi, mi) in detail_objects(shape, detail, &mut charge)? {
            let Some(Some(mesh)) = shape.meshes.get(mi) else {
                continue;
            };
            let node = node_of(body.instance.node_transforms(), shape, oi);
            let q = fpu::mul_p(&fpu::mat_inverse(&node), p);
            let b = mesh.bounds;
            let d: f32 = (0..3)
                .map(|i| {
                    let out = (b[i] - q[i]).max(q[i] - b[i + 3]).max(0.0);
                    out * out
                })
                .sum();
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, oi));
            }
        }
    }
    Ok(best.map_or_else(Vec::new, |(_, oi)| part_name(&body.kept, oi)))
}

/// Registers `Player::getDamageLocation` (over the class table's stub, when it declares it).
pub fn register(vm: &mut Vm) {
    register_method(vm, "Player", "getDamageLocation", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Str(Vec::new());
        };
        let mut pos = [0.0f32; 3];
        scan_floats(c.argv.get(2).map_or(&b""[..], Vec::as_slice), &mut pos);
        match damage_location(vm, id, pos) {
            Ok(name) => Native::Str(name),
            Err(e) => Native::Fail(e),
        }
    });
}
