//! `Camera::processTick(move)` and the `ShapeBase::processTick(move)` it starts with: the server runs it for a connection's
//! control object, once per move, in the 32 ms tick (the loop is `sim::pass::advance_objects`).
//!
//! TGE's camera has five modes: stationary (0), free rotate (1), fly (2), orbit an object (3) and orbit a point (4). The move
//! turns every mode; the orbit modes then put the camera at their centre and back the eye off it along the camera's forward
//! axis, and every other mode flies along the camera's axes. A host whose camera has more modes installs its own class
//! ([`super::camera::CameraClass`]).
//!
//! Credit: this module follows Torque3D's MIT-licensed `T3D/camera.cpp` (`Copyright (c) 2012 GarageGames, LLC`) where it has
//! TGE's rules; the code here is this crate's own.

use collide::types::{INTERIOR, ITEM, PLAYER, STATIC_SHAPE, TERRAIN, VEHICLE, WATER};
use net::moves::Move;
use torque_scene::transform::{self as grid, Matrix};
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::text::{atof, to_f32};
use ts_vm::vm::{Vm, VmError};

use super::access::{matrix_of, object_field, set_matrix, value_named};
use super::camera::{
    MOVE_MASK, ORBIT_OBJECT_MODE, ORBIT_POINT_MODE, is_camera, set_pose, state, state_mut,
};
use super::ext::{self, CameraExt};
use crate::scene::ghost as ghosts;

/// `$Camera::movementSpeed`: the fly speed in units per second.
pub const MOVEMENT_SPEED: &str = "$Camera::movementSpeed";
/// Its value when the script has not set it.
pub const MOVEMENT_SPEED_DEFAULT: f32 = 40.0;
/// `MaxPitch`: how far the camera pitches up or down.
pub const MAX_PITCH: f32 = 1.3962;
/// `TickSec`: one tick in seconds.
const TICK_SEC: f32 = 0.032;
/// `CameraRadius`: how far the orbit eye stays off a surface in its way.
pub const CAMERA_RADIUS: f32 = 0.05;
/// The types `validateEyePoint`'s cast stops at.
pub const EYE_MASK: u32 = TERRAIN | INTERIOR | WATER | STATIC_SHAPE | PLAYER | ITEM | VEHICLE;

/// How `onTrigger` reaches the script: the host charges and limits these calls (`net::pump`); a test calls the VM directly.
pub trait TriggerCalls {
    /// `Con::executef(datablock, 4, "onTrigger", obj, slot, state)`, the three arguments as decimal strings.
    fn on_trigger(
        &mut self,
        vm: &mut Vm,
        datablock: ObjId,
        obj: ObjId,
        slot: usize,
        state: bool,
    ) -> Result<(), VmError>;
}

/// [`TriggerCalls`] that calls the script at once and never refuses.
pub struct DirectCalls;

impl TriggerCalls for DirectCalls {
    fn on_trigger(
        &mut self,
        vm: &mut Vm,
        datablock: ObjId,
        obj: ObjId,
        slot: usize,
        state: bool,
    ) -> Result<(), VmError> {
        call_on_trigger(vm, datablock, obj, slot, state)
    }
}

/// The script call itself.
pub fn call_on_trigger(
    vm: &mut Vm,
    datablock: ObjId,
    obj: ObjId,
    slot: usize,
    state: bool,
) -> Result<(), VmError> {
    let args = [
        obj.to_string().into_bytes(),
        slot.to_string().into_bytes(),
        u8::from(state).to_string().into_bytes(),
    ];
    vm.call_method(datablock, b"onTrigger", &args).map(|_| ())
}

// ---- ShapeBase
// ------------------------------------------------------------------------------------------------------------------

/// `$Camera::movementSpeed`, or its default when the script has not set it.
pub fn movement_speed(vm: &Vm) -> f32 {
    vm.global(MOVEMENT_SPEED)
        .map_or(MOVEMENT_SPEED_DEFAULT, |v| {
            let end = v.iter().position(|b| *b == 0).unwrap_or(v.len());
            to_f32(atof(&v[..end]))
        })
}

fn float_field(vm: &Vm, id: ObjId, name: &[u8]) -> f32 {
    match value_named(vm, id, name) {
        Some(Value::Float(f)) => f,
        _ => 0.0,
    }
}

fn global_float(vm: &Vm, name: &str) -> f32 {
    vm.global(name).map_or(0.0, |v| {
        let end = v.iter().position(|b| *b == 0).unwrap_or(v.len());
        to_f32(atof(&v[..end]))
    })
}

/// `ShapeBase::processTick(move)` for a camera: the energy step, the fade, `onTrigger` for each trigger that changed,
/// and the decay of the damage flash and the white-out. With no move (`processTick(NULL)`, an object that is nobody's control
/// object) the triggers are not looked at.
pub fn shape_base_tick(
    vm: &mut Vm,
    id: ObjId,
    input: Option<(&Move, &mut dyn TriggerCalls)>,
) -> Result<(), VmError> {
    let datablock = object_field(vm, id, b"dataBlock");
    if let Some(db) = datablock {
        let (recharge, max) = (
            float_field(vm, db, b"rechargeRate"),
            float_field(vm, db, b"maxEnergy"),
        );
        if let Some(s) = state_mut(vm, id) {
            let e = s.energy + recharge;
            s.energy = if e <= max {
                if e < 0.0 { 0.0 } else { e }
            } else {
                max
            };
        }
    }
    // The script threads, then the fade, before the image slots and the triggers.
    super::shapebase::advance_threads(vm, id)?;
    super::shapebase::fade_tick(vm, id);
    super::shapebase::sound_timeout_tick(vm, id);
    // The Ghostable, non-ghost objects with a datablock call the script for each trigger that changed.
    if let Some(db) = datablock
        && let Some((mv, calls)) = input
        && ghosts::is_ghosted(vm, id)
    {
        for slot in 0..5 {
            let changed = state(vm, id).is_some_and(|s| s.triggers[slot] != mv.triggers[slot]);
            if changed {
                if let Some(s) = state_mut(vm, id) {
                    s.triggers[slot] = mv.triggers[slot];
                }
                calls.on_trigger(vm, db, id, slot, mv.triggers[slot])?;
            }
        }
    }
    let (flash_decay, white_decay) = (
        global_float(vm, "$SB::DFDec"),
        global_float(vm, "$SB::WODec"),
    );
    if let Some(s) = state_mut(vm, id) {
        if 0.0 < s.flash {
            s.flash -= flash_decay;
            if s.flash <= 0.0 {
                s.flash = 0.0;
            }
        }
        if 0.0 < s.white_out {
            s.white_out -= white_decay;
            if s.white_out <= 0.0 {
                s.white_out = 0.0;
            }
        }
    }
    Ok(())
}

fn alive(vm: &Vm, id: ObjId) -> bool {
    vm.objects().contains(id)
}

// ---- the eye ----------------------------------------------------------------------------------------------------------------

/// What an eye cast met: the point and the normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeHit {
    pub point: [f32; 3],
    pub normal: [f32; 3],
}

/// The cast `validate_eye` makes: `Container::castRay` with [`EYE_MASK`] from the first point to the second.
pub type EyeCast<'a> = dyn FnMut([f32; 3], [f32; 3]) -> Result<Option<EyeHit>, VmError> + 'a;

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn back(start: [f32; 3], dir: [f32; 3], distance: f32) -> [f32; 3] {
    std::array::from_fn(|i| start[i] - dir[i] * distance)
}

/// `validateEyePoint(1.0, matrix)`: the eye goes back from `start` (the camera's position) along `dir` (the matrix's forward
/// column) by `max - min`, unless the cast from `start` to 2.5 times that distance back meets something on the way: a surface
/// that faces the eye (`dot(dir, normal)` above 0.01) stops it [`CAMERA_RADIUS`] off the surface, clamped to `0..=max - min`.
pub fn validate_eye(
    start: [f32; 3],
    dir: [f32; 3],
    min: f32,
    max: f32,
    cast: &mut EyeCast<'_>,
) -> Result<[f32; 3], VmError> {
    let pos = max - min;
    if pos == 0.0 {
        return Ok(start);
    }
    let Some(hit) = cast(start, back(start, dir, 2.5 * pos))? else {
        return Ok(back(start, dir, pos));
    };
    let facing = dot(dir, hit.normal);
    if facing <= 0.01 {
        return Ok(back(start, dir, pos));
    }
    let to_start = std::array::from_fn(|i| start[i] - hit.point[i]);
    let mut distance = dot(to_start, dir) - (1.0 / facing) * CAMERA_RADIUS;
    if distance > pos {
        distance = pos;
    }
    if distance < 0.0 {
        distance = 0.0;
    }
    Ok(back(start, dir, distance))
}

// ---- the tick
// -------------------------------------------------------------------------------------------------------------------

/// `Camera::processTick(move)` for the camera `id`: `ShapeBase::processTick` (the energy, `onTrigger`, the decays), then the
/// move turns the camera (the pitch held within [`MAX_PITCH`]) and moves it by mode: the orbit modes put it at the orbit centre
/// (an orbited `ShapeBase`'s eye, another object's world-box centre, else the orbit point) and back the eye off it
/// ([`validate_eye`]); every other mode flies it along its old matrix's axes at `$Camera::movementSpeed` per second, twice that
/// with trigger 0 or 1 down. The move mask is set. `calls` carries `onTrigger` to the script; an error from it ends the tick.
pub fn process_tick(
    vm: &mut Vm,
    id: ObjId,
    mv: &Move,
    calls: &mut dyn TriggerCalls,
) -> Result<(), VmError> {
    if !alive(vm, id) || !is_camera(vm, id) {
        return Ok(());
    }
    shape_base_tick(vm, id, Some((mv, calls)))?;
    // A script that ran in `onTrigger` may have removed the camera.
    let (Some(s), Some(matrix)) = (state(vm, id).cloned(), matrix_of(vm, id)) else {
        return Ok(());
    };
    let pitch = (s.pitch + mv.pitch).clamp(-MAX_PITCH, MAX_PITCH);
    let yaw = s.yaw + mv.yaw;
    if s.mode == ORBIT_OBJECT_MODE || s.mode == ORBIT_POINT_MODE {
        orbit_step(vm, id, &s, pitch, yaw)?;
    } else {
        let faster = mv.triggers[0] || mv.triggers[1];
        let scale = movement_speed(vm) * if faster { 2.0 } else { 1.0 };
        let mut pos = translation(&matrix);
        for (axis, amount) in [mv.x, mv.y, mv.z].into_iter().enumerate() {
            let column = [matrix[axis], matrix[4 + axis], matrix[8 + axis]];
            for k in 0..3 {
                pos[k] += column[k] * amount * TICK_SEC * scale;
            }
        }
        set_pose(vm, id, pos, pitch, yaw);
    }
    ghosts::check_bins(vm, id);
    ghosts::mark(vm, id, MOVE_MASK);
    Ok(())
}

/// `Camera::processTick(NULL)`: what the process list runs for a camera that is nobody's control object, `ShapeBase`'s tick
/// with no move (the energy step, the decay of the damage flash and the white-out; no `onTrigger`).
pub fn process_tick_idle(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    if !alive(vm, id) || !is_camera(vm, id) {
        return Ok(());
    }
    shape_base_tick(vm, id, None)
}

fn orbit_step(vm: &mut Vm, id: ObjId, s: &CameraExt, pitch: f32, yaw: f32) -> Result<(), VmError> {
    let target = s.orbit.filter(|t| alive(vm, *t));
    let mut centre = s.orbit_point;
    if s.mode == ORBIT_OBJECT_MODE
        && let Some(target) = target
    {
        centre = if is_shape_base(vm, target) {
            translation(&crate::sim::player::methods::eye_transform(vm, target))
        } else {
            ext::get(vm.objects(), target)
                .and_then(|e| e.scene)
                .map_or(centre, |sc| grid::box_center(sc.world_min, sc.world_max))
        };
        if let Some(s) = state_mut(vm, id) {
            s.orbit_point = centre;
        }
    }
    set_pose(vm, id, centre, pitch, yaw);
    let Some(m) = matrix_of(vm, id) else {
        return Ok(());
    };
    let dir = [m[1], m[5], m[9]];
    let eye = cast_eye(vm, id, target, translation(&m), dir, s.orbit_distances)?;
    let mut moved = m;
    (moved[3], moved[7], moved[11]) = (eye[0], eye[1], eye[2]);
    set_matrix(vm, id, &moved);
    Ok(())
}

/// `validateEyePoint` over the server's container, with the orbit object and the camera out of collision for the cast. A camera
/// outside a process list with a world (a bare test VM) casts into nothing.
fn cast_eye(
    vm: &mut Vm,
    id: ObjId,
    target: Option<ObjId>,
    start: [f32; 3],
    dir: [f32; 3],
    distances: [f32; 3],
) -> Result<[f32; 3], VmError> {
    let Some(env) = crate::sim::env_of(vm, id) else {
        return validate_eye(start, dir, distances[0], distances[1], &mut |_, _| Ok(None));
    };
    if let Some(t) = target {
        super::container::disable_collision(vm, t);
    }
    super::container::disable_collision(vm, id);
    let eye = validate_eye(start, dir, distances[0], distances[1], &mut |from, to| {
        let hit = super::container::container_ray(
            vm,
            &env.container,
            Some(&env.shapes),
            from,
            to,
            EYE_MASK,
        )?;
        Ok(hit.map(|h| EyeHit {
            point: h.point,
            normal: h.normal,
        }))
    });
    super::container::enable_collision(vm, id);
    if let Some(t) = target {
        super::container::enable_collision(vm, t);
    }
    eye
}

fn translation(m: &Matrix) -> [f32; 3] {
    [m[3], m[7], m[11]]
}

fn is_shape_base(vm: &Vm, id: ObjId) -> bool {
    let objects = vm.objects();
    match (objects.get(id), objects.classes.find(b"ShapeBase")) {
        (Some(o), Some(c)) => objects.classes.is_a(o.class(), c),
        _ => false,
    }
}

#[cfg(test)]
#[path = "camera_tick_tests.rs"]
mod tests;
