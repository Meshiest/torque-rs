//! `SceneObject` for every scene object: the transform, the scale, the boxes and the type mask, and the
//! ten console methods that read and write them.
//!
//! **Where the state is.** The transform is the matrix the `position` and `rotation`
//! fields share, and the scale is the field `scale`: both are the VM's static field slots and nowhere else. The boxes
//! ([`SceneState`]) and the type mask are Rust state in the object's [`ObjectExt`](super::ext::ObjectExt): nothing in a script
//! names them. A script's write to `position`, `rotation` or `scale` as a field changes the slot and nothing else (no hook
//! runs and the world box stays what the last `setTransform` made it); the console methods go through
//! [`set_transform`], which is what the virtual `setTransform` does.
//!
//! **What a class decides.** A scene object's `setTransform` is a virtual function: a class the host registered decides first
//! ([`super::classes::SceneClass::set_transform`]: it may change the matrix it is given), the others store the matrix and
//! recompute the world box
//! ([`set_plain_transform`]). Players, vehicles and items override it for their own reasons. The object box is the
//! class's: the empty box at the origin for a plain scene object.
//!
//! **What is left out.** The console methods refuse to answer for objects of the player, vehicle, item and
//! projectile types on a client or without a control object; the engine has no such objects yet and
//! the mask test is not made. The inverse transform, the render transform and the world sphere are not script
//! visible and are not kept.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `scene/sceneObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this
//! crate's own.

use std::rc::Rc;

use crate::geometry::Geometry;
use torque_scene::transform::{self as grid, Matrix};
use ts_vm::classes::EngineClass;
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::text::{format_g, scan_floats, to_f64};
use ts_vm::vm::{Native, Vm, VmError};

use super::access::{matrix_of, rows_of, scale_of, set_field, set_matrix};
use super::camera;
use super::container::Container;
use super::ext::{self, SceneState};
use crate::methods::register_method;
use crate::scene::ghost as ghosts;

/// The class every scene object is below.
pub const SCENE_OBJECT_CLASS: &str = "SceneObject";

/// A float as `%g` prints it (promoted to a double).
fn g(v: f32) -> String {
    format_g(to_f64(v))
}

/// `%g %g %g` of three floats.
fn text3(v: [f32; 3]) -> Vec<u8> {
    v.map(g).join(" ").into_bytes()
}

/// `%g` of six floats.
fn text6(low: [f32; 3], high: [f32; 3]) -> Vec<u8> {
    low.iter()
        .chain(&high)
        .copied()
        .map(g)
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes()
}

fn position_of(matrix: &Matrix) -> [f32; 3] {
    [matrix[3], matrix[7], matrix[11]]
}

/// The rotation of a transform as the console prints it: `AngAxisF::set(QuatF::set(matrix))`. The axis is not normalised
/// (`1 0 0` for no rotation), the angle is in radians.
pub fn axis_angle_of(geo: &Geometry, matrix: &Matrix) -> ([f32; 3], f32) {
    (geo.axis_angle_from_quat)((geo.quat_from_matrix)(&rows_of(matrix)))
}

/// `SceneObject::resetWorldBox`: the world box from the object box, the scale and the transform as the slots hold them now.
pub fn reset_world_box(vm: &mut Vm, id: ObjId) {
    let Some(matrix) = matrix_of(vm, id) else {
        return;
    };
    let scale = scale_of(vm, id);
    let Some(state) = ext::get_mut(vm.objects_mut(), id).and_then(|ext| ext.scene.as_mut()) else {
        return;
    };
    let (low, high) = grid::world_box(&matrix, scale, state.object_min, state.object_max);
    state.world_min = low;
    state.world_max = high;
    (state.world_center, state.world_radius) = grid::world_sphere(low, high);
    if let Some(answer) = super::classes::of(vm, id) {
        answer.world_box_changed(vm, id);
    }
}

/// Sets the object box (the class's) and recomputes the world box.
pub fn set_object_box(vm: &mut Vm, id: ObjId, low: [f32; 3], high: [f32; 3]) {
    if let Some(state) = ext::get_mut(vm.objects_mut(), id).and_then(|ext| ext.scene.as_mut()) {
        state.object_min = low;
        state.object_max = high;
    }
    reset_world_box(vm, id);
}

/// Sets the object box and leaves the world box where it is (`Player::onNewDataBlock` writes its box fields and does not reset
/// it).
pub fn set_object_box_only(vm: &mut Vm, id: ObjId, low: [f32; 3], high: [f32; 3]) {
    if let Some(state) = ext::get_mut(vm.objects_mut(), id).and_then(|ext| ext.scene.as_mut()) {
        state.object_min = low;
        state.object_max = high;
    }
}

/// `SceneObject::setTransform`: the matrix becomes the transform, the world box follows.
pub fn set_plain_transform(vm: &mut Vm, id: ObjId, matrix: &Matrix) {
    set_matrix(vm, id, matrix);
    reset_world_box(vm, id);
}

/// The virtual `setTransform`: the class of the object decides what is made of the matrix.
pub fn set_transform(vm: &mut Vm, id: ObjId, matrix: Matrix) {
    if let Some(answer) = super::classes::of(vm, id)
        && answer.set_transform(vm, id, matrix)
    {
        return;
    }
    if camera::is_camera(vm, id) {
        // `Camera::setTransform` rebuilds the matrix from a pitch and a yaw and keeps those.
        camera::set_transform(vm, id, &matrix);
        ghosts::mark(vm, id, camera::MOVE_MASK);
    } else if crate::sim::item::is_item(vm, id) {
        crate::sim::item::set_transform(vm, id, &matrix);
    } else if crate::sim::vehicle::is_vehicle(vm, id) {
        crate::sim::vehicle::set_transform(vm, id, &matrix);
    } else if super::shapebase::is_item(vm, id) {
        super::shapebase::item_set_transform(vm, id, &matrix);
    } else if super::shapebase::is_player(vm, id) {
        crate::sim::player::set_transform(vm, id, &matrix);
    } else {
        set_plain_transform(vm, id, &matrix);
        // The classes whose own `setTransform` marks the object: the ghosting layer knows each ghost class's mask (none for the
        // base `SceneObject::setTransform`).
        let mask = ghosts::transform_mask(vm, id);
        if mask != 0 {
            ghosts::mark(vm, id, mask);
        }
    }
}

/// `SceneObject::onAdd`: the object's boxes start empty and the world box is made from the transform the
/// fields gave it.
struct SceneBehaviour {
    container: Rc<Container>,
}

impl EngineClass for SceneBehaviour {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        if let Some(ext) = ext::get_mut(vm.objects_mut(), id) {
            ext.scene = Some(SceneState::default());
        }
        reset_world_box(vm, id);
        Ok(true)
    }

    /// `SceneObject::onRemove` takes the object out of the container (the bins), charged (always carried out).
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        let vm: &Vm = vm;
        self.container
            .bins_mut()
            .remove_charged(id, &mut |u| vm.charge_work(u));
        Ok(())
    }
}

/// `SceneObject::addToScene` for a server object: the object enters the container's bins by its world box. Each class
/// calls it at the end of its own `onAdd`, after it set its object box (`set_object_box`). Each cell is charged to the run (the
/// insertion is made whole: a refusal ends the run at its next instruction).
pub fn add_to_scene(vm: &Vm, container: &Container, id: ObjId) {
    if let Some(state) = world_box_of(vm, id) {
        container
            .bins_mut()
            .insert_charged(id, &(state.world_min, state.world_max), &mut |u| {
                vm.charge_work(u)
            });
    }
}

/// `Container::checkBins`, run by `SceneObject::setTransform` and `setScale` for an object that is in the container.
/// Charged to the run, or to a tick's host budget (`torque_scene::bins::Bins::check_charged`): a move the budget refuses leaves
/// the object in its old cells (engine-only), and the run ends at its next instruction.
pub fn check_bins(vm: &Vm, container: &Container, id: ObjId) {
    let in_bins = container.bins().contains(id);
    if in_bins && let Some(state) = world_box_of(vm, id) {
        container
            .bins_mut()
            .check_charged(id, &(state.world_min, state.world_max), &mut |u| {
                vm.charge_work(u)
            });
    }
}

/// What a mover's tick does after it stores a new transform (`Player::setPosition`, `SceneObject::setTransform` and the
/// other movers' setters): `resetWorldBox` from the transform the slots now hold, then `checkBins`, which moves the
/// object in the bins only when its four cell numbers changed. For the tickers of the simulation's Player and vehicle glue.
pub fn moved(vm: &mut Vm, container: &Container, id: ObjId) {
    reset_world_box(vm, id);
    check_bins(vm, container, id);
}

/// The text of a transform as `getTransform` prints it: the position, then the axis and angle.
pub fn transform_text(geo: &Geometry, matrix: &Matrix) -> Vec<u8> {
    let (axis, angle) = axis_angle_of(geo, matrix);
    let [x, y, z] = position_of(matrix);
    [x, y, z, axis[0], axis[1], axis[2], angle]
        .map(g)
        .join(" ")
        .into_bytes()
}

/// `SceneObject::setTransform`'s text: seven floats (position, axis, angle in radians), those the text does not give keep
/// what the object has now (`sscanf` into the current values), made into a matrix the way `MatrixCreate`
/// makes one. Public for a host that replays a spawn (the angle that gives a yaw).
pub fn matrix_from_text(geo: &Geometry, current: &Matrix, text: &[u8]) -> Matrix {
    let (axis, angle) = axis_angle_of(geo, current);
    let [x, y, z] = position_of(current);
    let mut values = [x, y, z, axis[0], axis[1], axis[2], angle];
    scan_floats(text, &mut values);
    let rows = (geo.matrix_from_quat)((geo.quat_from_axis_angle)(
        [values[3], values[4], values[5]],
        values[6],
    ));
    let mut matrix: Matrix = std::array::from_fn(|i| rows[i / 4][i % 4]);
    matrix[3] = values[0];
    matrix[7] = values[1];
    matrix[11] = values[2];
    matrix
}

fn world_box_of(vm: &Vm, id: ObjId) -> Option<SceneState> {
    ext::get(vm.objects(), id)?.scene
}

/// Registers the `SceneObject` behaviour and the ten console methods, and `getType`.
pub fn register(vm: &mut Vm, container: &Rc<Container>) {
    vm.set_class_behaviour(
        SCENE_OBJECT_CLASS,
        Rc::new(SceneBehaviour {
            container: Rc::clone(container),
        }),
    );
    // `GameBase::getDataBlock`: the datablock's id, 0 for none.
    register_method(vm, "GameBase", "getDataBlock", |vm, c| {
        let db = c
            .this_object
            .and_then(|id| crate::scene::access::object_field(vm, id, b"dataBlock"));
        Native::Int(db.map_or(0, |d| i32::try_from(d).unwrap_or(0)))
    });
    // The type mask lives in the object's ext, for every object: 0 until a class sets it.
    vm.register_in("SimObject", "getType", 2, 2, "obj.getType()", |vm, c| {
        let mask = c
            .this_object
            .and_then(|id| ext::get(vm.objects(), id))
            .map_or(0, |ext| ext.type_mask);
        Native::Int(i32::from_ne_bytes(mask.to_ne_bytes()))
    });
    register_method(vm, SCENE_OBJECT_CLASS, "getTransform", |vm, c| {
        let matrix = c.this_object.and_then(|id| matrix_of(vm, id));
        Native::Str(matrix.map_or_else(Vec::new, |m| transform_text(crate::geometry::of(vm), &m)))
    });
    register_method(vm, SCENE_OBJECT_CLASS, "getPosition", |vm, c| {
        let matrix = c.this_object.and_then(|id| matrix_of(vm, id));
        Native::Str(matrix.map_or_else(Vec::new, |m| text3(position_of(&m))))
    });
    // Columns 1 and 2 of the rotation: where the object's y and z axes point.
    register_method(vm, SCENE_OBJECT_CLASS, "getForwardVector", |vm, c| {
        let matrix = c.this_object.and_then(|id| matrix_of(vm, id));
        Native::Str(matrix.map_or_else(Vec::new, |m| text3([m[1], m[5], m[9]])))
    });
    register_method(vm, SCENE_OBJECT_CLASS, "getUpVector", |vm, c| {
        let matrix = c.this_object.and_then(|id| matrix_of(vm, id));
        Native::Str(matrix.map_or_else(Vec::new, |m| text3([m[2], m[6], m[10]])))
    });
    let w = Rc::clone(container);
    register_method(vm, SCENE_OBJECT_CLASS, "setTransform", move |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Void;
        };
        let Some(current) = matrix_of(vm, id) else {
            return Native::Void;
        };
        let text = c.argv.get(2).map_or(&b""[..], Vec::as_slice);
        let matrix = matrix_from_text(crate::geometry::of(vm), &current, text);
        set_transform(vm, id, matrix);
        check_bins(vm, &w, id);
        Native::Void
    });
    register_method(vm, SCENE_OBJECT_CLASS, "getScale", |vm, c| {
        Native::Str(text3(c.this_object.map_or([1.0; 3], |id| scale_of(vm, id))))
    });
    let w = Rc::clone(container);
    register_method(vm, SCENE_OBJECT_CLASS, "setScale", move |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Void;
        };
        let mut scale = [0.0f32; 3];
        scan_floats(c.argv.get(2).map_or(&b""[..], Vec::as_slice), &mut scale);
        set_field(vm, id, b"scale", Value::Floats3(grid::clamp_scale(scale)));
        // The transform is set again with the matrix it has (a registered class sees it again), and the world box
        // follows the new scale through it.
        if let Some(matrix) = matrix_of(vm, id) {
            set_transform(vm, id, matrix);
        }
        check_bins(vm, &w, id);
        // `SceneObject::setScale` sets bit 1 on every object after its `setTransform`.
        ghosts::mark(vm, id, 1);
        Native::Void
    });
    register_method(vm, SCENE_OBJECT_CLASS, "getWorldBox", |vm, c| {
        let state = c.this_object.and_then(|id| world_box_of(vm, id));
        Native::Str(state.map_or_else(Vec::new, |s| text6(s.world_min, s.world_max)))
    });
    register_method(vm, SCENE_OBJECT_CLASS, "getWorldBoxCenter", |vm, c| {
        let state = c.this_object.and_then(|id| world_box_of(vm, id));
        Native::Str(state.map_or_else(Vec::new, |s| {
            text3(grid::box_center(s.world_min, s.world_max))
        }))
    });
    register_method(vm, SCENE_OBJECT_CLASS, "getObjectBox", |vm, c| {
        let state = c.this_object.and_then(|id| world_box_of(vm, id));
        Native::Str(state.map_or_else(Vec::new, |s| text6(s.object_min, s.object_max)))
    });
}
