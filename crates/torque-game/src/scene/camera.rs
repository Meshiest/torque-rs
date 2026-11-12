//! The `Camera` object of a server: its state ([`CameraExt`]), `setTransform`, `setPosition`, the console methods and the class
//! a game may replace ([`CameraClass`]).
//!
//! TGE's camera has five modes ([`STATIONARY_MODE`] to [`ORBIT_POINT_MODE`]); `setFlyMode` and `setOrbitMode` choose them and
//! `processTick` ([`camera_tick`]) moves the camera by its mode. `setPosition` rebuilds the transform as `Rz(yaw) * Rx(pitch)`
//! and keeps the two angles; `setTransform` reads them back from the matrix's forward column. `getPosition` prints `"%g %g
//! %g"`.
//!
//! **Control chain.** An object may control another (`getControlObject`: a Player's vehicle in TGE): the connection's chain
//! walks those links from its control object, its field of view goes to the last object, and the client that controls the
//! chain's first object controls all of it (TGE's `GameConnection::getControlCameraFov`). A host whose objects link otherwise
//! (a camera's own control object, kept in [`CameraExt`]) or that sends the field of view elsewhere installs a [`ControlChain`].
//! The chain is bounded by [`CONTROL_CHAIN`] and cut at a repeat.
//!
//! **A host's camera.** A host whose camera has more modes or other setters installs a [`CameraClass`] (its `processTick` and
//! its `setPosition`) and registers its own console methods after [`register`].
//!
//! Credit: this module follows Torque3D's MIT-licensed `T3D/camera.cpp` (`Copyright (c) 2012 GarageGames, LLC`) where it has
//! TGE's rules, and its `T3D/gameBase/gameConnection.cpp` and `T3D/shapeBase.cpp` (each `Copyright (c) 2012 GarageGames, LLC`,
//! `Copyright (C) 2015 Faust Logic, Inc.`) for the control chain's walk and `ShapeBase::setCameraFov`; the code here is this
//! crate's own.

use std::rc::Rc;

use crate::geometry::Geometry;
use net::control::{FovTarget, clamp_fov as clamp_camera_fov};
use net::moves::Move;
use torque_scene::transform::{self as grid, Matrix};
use ts_vm::sim::ObjId;
use ts_vm::text::{format_g, scan_floats, to_f32, to_f64};
use ts_vm::vm::{Native, Vm, VmError};

use super::access::matrix_of;
use super::camera_tick::{self, TriggerCalls};
use super::ext::{self, CameraExt};
use super::object as scene;
use crate::methods::register_method;
use crate::scene::ghost as ghosts;

/// The mask bit a `Camera` sets when its pose changes (`MoveMask`).
pub const MOVE_MASK: u32 = 0x0100_0000;

/// How far the control chain is followed (a camera holding a camera holding ...): the network's camera tail refuses a deeper
/// one.
pub const CONTROL_CHAIN: usize = 4;

/// Whether the object is a `Camera`.
pub fn is_camera(vm: &Vm, id: ObjId) -> bool {
    let objects = vm.objects();
    match (objects.get(id), objects.classes.find(b"Camera")) {
        (Some(o), Some(c)) => objects.classes.is_a(o.class(), c),
        _ => false,
    }
}

/// The camera's state, if the object is a camera that has any.
pub fn state(vm: &Vm, id: ObjId) -> Option<&CameraExt> {
    ext::get(vm.objects(), id)?.camera.as_ref()
}

/// The camera's state, made from the constructor's values first when the object is a camera that has none.
pub fn state_mut(vm: &mut Vm, id: ObjId) -> Option<&mut CameraExt> {
    if !is_camera(vm, id) {
        return None;
    }
    Some(
        ext::get_mut(vm.objects_mut(), id)?
            .camera
            .get_or_insert_with(CameraExt::default),
    )
}

pub fn translation(m: &Matrix) -> [f32; 3] {
    [m[3], m[7], m[11]]
}

/// TGE's camera modes: still, turning in place, flying, orbiting an object, orbiting a point.
pub const STATIONARY_MODE: u32 = 0;
pub const FREE_ROTATE_MODE: u32 = 1;
pub const FLY_MODE: u32 = 2;
pub const ORBIT_OBJECT_MODE: u32 = 3;
pub const ORBIT_POINT_MODE: u32 = 4;

/// A host's `Camera` class ([`set_camera_class`]): its `processTick` and its `setPosition`. Without one, TGE's
/// ([`TorqueCamera`]).
pub trait CameraClass {
    /// `Camera::processTick(move)`.
    fn process_tick(
        &self,
        vm: &mut Vm,
        id: ObjId,
        mv: &Move,
        calls: &mut dyn TriggerCalls,
    ) -> Result<(), VmError> {
        camera_tick::process_tick(vm, id, mv, calls)
    }

    /// `Camera::processTick(NULL)`: a camera that is nobody's control object.
    fn process_tick_idle(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        camera_tick::process_tick_idle(vm, id)
    }

    /// `Camera::setPosition(position, rotation)`.
    fn set_position(&self, vm: &mut Vm, id: ObjId, position: [f32; 3], pitch: f32, yaw: f32) {
        set_pose(vm, id, position, pitch, yaw);
    }
}

/// TGE's camera.
#[derive(Debug, Default, Clone, Copy)]
pub struct TorqueCamera;

impl CameraClass for TorqueCamera {}

struct InstalledCamera(Rc<dyn CameraClass>);

/// The host's camera class for `vm` from now on.
pub fn set_camera_class(vm: &mut Vm, class: Rc<dyn CameraClass>) {
    vm.set_extension(Rc::new(InstalledCamera(class)));
}

/// The camera class `vm`'s game installed, else TGE's.
pub fn camera_class(vm: &Vm) -> Rc<dyn CameraClass> {
    vm.extension::<InstalledCamera>().map_or_else(
        || Rc::new(TorqueCamera) as Rc<dyn CameraClass>,
        |i| Rc::clone(&i.0),
    )
}

/// The matrix `Camera::setPosition` builds: `Rz(yaw) * Rx(pitch)` with the translation `position` (the library's
/// [`camera_pose`](crate::library::GameLibrary::camera_pose)).
pub fn pose_matrix(position: [f32; 3], pitch: f32, yaw: f32) -> Matrix {
    (crate::library::get().camera_pose)(position, pitch, yaw)
}

/// The rotation `Camera::setTransform` reads from a matrix, `(pitch, yaw)` (the library's
/// [`camera_rotation`](crate::library::GameLibrary::camera_rotation)).
pub fn rotation_of(m: &Matrix) -> (f32, f32) {
    (crate::library::get().camera_rotation)(m)
}

/// TGE's `Camera::setPosition(position, rotation)`: rebuilds the transform and the world box and keeps the rotation. The
/// container's `checkBins` is the caller's (every `setTransform` native does it after).
pub fn set_pose(vm: &mut Vm, id: ObjId, position: [f32; 3], pitch: f32, yaw: f32) {
    scene::set_plain_transform(vm, id, &pose_matrix(position, pitch, yaw));
    if let Some(s) = state_mut(vm, id) {
        s.pitch = pitch;
        s.yaw = yaw;
    }
}

/// `Camera::setPosition(position, rotation)` of the camera's class.
pub fn set_position(vm: &mut Vm, id: ObjId, position: [f32; 3], pitch: f32, yaw: f32) {
    camera_class(vm).set_position(vm, id, position, pitch, yaw);
}

/// `Camera::setTransform`: the rotation is read from the matrix and `setPosition` does the rest.
pub fn set_transform(vm: &mut Vm, id: ObjId, matrix: &Matrix) {
    let (pitch, yaw) = rotation_of(matrix);
    set_position(vm, id, translation(matrix), pitch, yaw);
}

/// How an object leads on to the one it controls in turn, and which object of a connection's chain its field of view goes to
/// (a `Vm` extension, [`set_control_chain`]). Without a host's, TGE's ([`TorqueChain`]): `GameConnection::getControlCameraFov`
/// and `setControlCameraFov` walk `getControlObject` from the control object to the last object, whatever its class.
pub trait ControlChain {
    /// `getControlObject()` of `id`: TGE's `Player::getControlObject` (the object the Player controls, a vehicle it drives);
    /// every other class has none (`ShapeBase`'s; TGE's `Camera` controls nothing).
    ///
    /// A host that overrides this must override [`Self::controller`] to match: the server finds a connection by walking up
    /// through `controller` to a control object, so a chain whose links `controller` does not mirror is not found (the lookup
    /// returns `None`; it never names another connection).
    fn control_object(&self, vm: &Vm, id: ObjId) -> Option<ObjId> {
        crate::sim::player::state(vm, id)
            .and_then(|s| s.control_object)
            .map(|o| o.0)
            .filter(|o| vm.objects().contains(*o))
    }

    /// The object whose control object `id` is, the inverse of [`Self::control_object`]: what a connection's chain is walked
    /// back through to find the control object it starts at. The default is the object's controlling link
    /// ([`controlling_object`](crate::sim::mount::controlling_object)), which every class that holds a control object sets; a
    /// chain whose links are stored some other way answers for itself.
    fn controller(&self, vm: &Vm, id: ObjId) -> Option<ObjId> {
        crate::sim::mount::controlling_object(vm, id)
    }

    /// The object of `chain` (a control object's, [`chain`]) that `setControlCameraFov` and `getControlCameraFov` talk to:
    /// TGE's is the last one.
    fn fov_object(&self, vm: &Vm, chain: &[ObjId]) -> Option<ObjId> {
        let _ = vm;
        chain.last().copied()
    }
}

/// TGE's chain.
#[derive(Debug, Default, Clone, Copy)]
pub struct TorqueChain;

impl ControlChain for TorqueChain {}

struct InstalledChain(Rc<dyn ControlChain>);

/// The host's control chain for `vm` from now on.
pub fn set_control_chain(vm: &mut Vm, rules: Rc<dyn ControlChain>) {
    vm.set_extension(Rc::new(InstalledChain(rules)));
}

/// The control chain `vm`'s game installed, else TGE's.
pub fn control_chain(vm: &Vm) -> Rc<dyn ControlChain> {
    vm.extension::<InstalledChain>().map_or_else(
        || Rc::new(TorqueChain) as Rc<dyn ControlChain>,
        |i| Rc::clone(&i.0),
    )
}

/// The control chain from `start` (a connection's control object): each object leads to the one it controls
/// ([`ControlChain::control_object`]) while that one still exists. Bounded by [`CONTROL_CHAIN`] and cut at a repeat.
pub fn chain(vm: &Vm, start: ObjId) -> Chain {
    let installed = vm.extension::<InstalledChain>();
    let rules: &dyn ControlChain = match &installed {
        Some(i) => &*i.0,
        None => &TorqueChain,
    };
    let mut out = Chain {
        ids: [start; CONTROL_CHAIN + 1],
        len: 1,
    };
    let mut at = start;
    while out.len < out.ids.len() {
        match rules.control_object(vm, at) {
            Some(n) if !out.contains(&n) => {
                out.ids[out.len] = n;
                out.len += 1;
                at = n;
            }
            _ => break,
        }
    }
    out
}

/// A control chain ([`chain`]): at most [`CONTROL_CHAIN`] links after its start, held without an allocation (the server
/// walks one per connection for every Player's tick). Reads as a slice.
#[derive(Debug, Clone, Copy)]
pub struct Chain {
    ids: [ObjId; CONTROL_CHAIN + 1],
    len: usize,
}

impl std::ops::Deref for Chain {
    type Target = [ObjId];

    fn deref(&self) -> &[ObjId] {
        &self.ids[..self.len]
    }
}

/// The object of the chain from `start` that `setControlCameraFov` and `getControlCameraFov` talk to
/// ([`ControlChain::fov_object`]).
pub fn fov_object(vm: &Vm, start: ObjId) -> Option<ObjId> {
    control_chain(vm).fov_object(vm, &chain(vm, start))
}

/// `Camera::setControlObject` (the virtual): lets go of the object held, then takes `new`: another camera that controlled it
/// and the connection that controlled it lose it first. The mark is `setControllingClient`'s (`CloakMask`, which TGE's
/// controlling-client change piggybacks on). The controlling client of a chained object is found by walking the chain, so only
/// the camera links are stored.
pub fn set_control_object(vm: &mut Vm, id: ObjId, new: Option<ObjId>) {
    let old = state(vm, id).and_then(|s| s.controlled);
    if let Some(old) = old {
        // The object's controller link (a Camera's in its state, any other's in its ext) and the mark.
        crate::sim::player::methods::set_controlling(vm, old, None);
        if let Some(s) = state_mut(vm, id) {
            s.controlled = None;
        }
    }
    let Some(new) = new.filter(|n| *n != id) else {
        if let Some(s) = state_mut(vm, id) {
            s.controlled = None;
        }
        return;
    };
    // The camera that held `new` lets it go, and so does the client that controlled it.
    let holder = crate::sim::mount::controlling_object(vm, new);
    if let Some(holder) = holder.filter(|h| *h != id) {
        crate::sim::player::methods::release_holder(vm, holder);
    }
    ghosts::release_controlling_connection(vm, new);
    if let Some(s) = state_mut(vm, id) {
        s.controlled = Some(new);
    }
    crate::sim::player::methods::set_controlling(vm, new, Some(id));
}

/// `Camera::onRemove`: an orbit object hears `onCameraLeaveOrbit`.
/// The object `setControlCameraFov` talks to (`setCameraFov`, `getCameraFov`): a camera keeps its field of view in its state;
/// any other `ShapeBase` keeps `ShapeBase::mCameraFov` (90 from the constructor), which `setCameraFov` clamps to its
/// datablock's `cameraMinFov` .. `cameraMaxFov` as TGE's does.
pub struct CameraFov<'a> {
    pub vm: &'a mut Vm,
    pub id: ObjId,
}

impl FovTarget for CameraFov<'_> {
    fn set_camera_fov(&mut self, fov: f32) {
        if let Some(s) = state_mut(self.vm, self.id) {
            s.fov = clamp_camera_fov(fov);
            return;
        }
        let (min, max) = shape_fov_range(self.vm, self.id);
        if let Some(e) = ext::get_mut(self.vm.objects_mut(), self.id) {
            e.camera_fov = Some(fov.min(max).max(min));
        }
    }

    fn camera_fov(&self) -> f32 {
        match state(self.vm, self.id) {
            Some(s) => s.fov,
            None => ext::get(self.vm.objects(), self.id)
                .and_then(|e| e.camera_fov)
                .unwrap_or(90.0),
        }
    }
}

/// A `ShapeBase`'s datablock's `cameraMinFov` and `cameraMaxFov` (TGE's constructor gives 5 and 120; a datablock without the
/// fields keeps those).
fn shape_fov_range(vm: &Vm, id: ObjId) -> (f32, f32) {
    let db = super::access::object_field(vm, id, b"dataBlock");
    let read = |name: &[u8], default: f32| match db
        .and_then(|d| super::access::value_named(vm, d, name))
    {
        Some(ts_vm::fields::Value::Float(x)) => x,
        _ => default,
    };
    (read(b"cameraMinFov", 5.0), read(b"cameraMaxFov", 120.0))
}

/// `ShapeBase::onNewDataBlock`'s state for a camera (the part that needs no shape): the first datablock zeroes the energy
/// words, and every datablock sets the field of view to `getDefaultCameraFov` (the constant 90.0).
pub fn new_datablock(vm: &mut Vm, id: ObjId) {
    if let Some(s) = state_mut(vm, id) {
        if !s.had_datablock {
            s.energy = 0.0;
            s.had_datablock = true;
        }
        s.fov = 90.0;
    }
}

fn id_value(id: Option<ObjId>) -> i32 {
    id.map_or(0, |id| i32::try_from(id).unwrap_or(0))
}

pub fn is_class(vm: &Vm, id: ObjId, class: &str) -> bool {
    let objects = vm.objects();
    match (objects.get(id), objects.classes.find(class.as_bytes())) {
        (Some(o), Some(c)) => objects.classes.is_a(o.class(), c),
        _ => false,
    }
}

/// The rotation of `setOrbitMode`'s transform, as a pure function: `AngAxisF(axis, angle)` -> `QuatF` (half angle stored as a
/// float, `sin`/`cos` of it each stored as a float, the axis times the sine) -> `QuatF::setMatrix` (the plain version) -> the
/// Euler angles `(x, y, z)` that `Camera::setPosition` receives as its rotation.
///
/// `x = asin(m[1][2])` (stored as a float); when `cos(x)` is exactly zero `y = 0` and `z = atan2(m[0][1], m[0][0])`, else
/// `y = atan2(-m[0][2], m[2][2])` and `z = atan2(-m[1][0], m[1][1])` (each stored as a float). `setPosition` keeps `x` as the
/// pitch and `z` as the yaw. `sin`, `cos`, `asin` and `atan2` are the double-precision library's.
pub fn orbit_rotation(geo: &Geometry, axis: [f32; 3], angle: f32) -> [f32; 3] {
    let m = (geo.matrix_from_quat)((geo.quat_from_axis_angle)(axis, angle));
    let pitch = to_f32(to_f64(m[1][2]).asin());
    let cos_pitch = to_f32(to_f64(pitch).cos());
    if cos_pitch == 0.0 {
        let yaw = to_f32(to_f64(m[0][1]).atan2(to_f64(m[0][0])));
        [pitch, 0.0, yaw]
    } else {
        let roll = to_f32((-to_f64(m[0][2])).atan2(to_f64(m[2][2])));
        let yaw = to_f32((-to_f64(m[1][0])).atan2(to_f64(m[1][1])));
        [pitch, roll, yaw]
    }
}

/// `setOrbitMode`'s transform argument: `sscanf("%f %f %f %f %f %f %f")` into the position, the axis and the angle; fewer
/// than seven numbers (the call counts them) leave `0 0 0  0 0 1  0`, whatever was parsed.
pub fn orbit_transform(text: &[u8]) -> ([f32; 3], [f32; 3], f32) {
    let mut v = [0.0f32; 7];
    if scan_floats(text, &mut v) < 7 {
        v = [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    }
    ([v[0], v[1], v[2]], [v[3], v[4], v[5]], v[6])
}

/// `deleteNotify(target)` of the orbit core: the camera is put on the target's list (once; a camera that orbits another
/// object by now is dropped from it, as `clearNotify` takes it off when the orbit object changes).
pub fn delete_notify(vm: &mut Vm, camera: ObjId, target: ObjId) {
    let mut list = ext::get(vm.objects(), target)
        .map(|e| e.orbit_notify.clone())
        .unwrap_or_default();
    list.retain(|c| *c != camera && state(vm, *c).is_some_and(|s| s.orbit == Some(target)));
    list.push(camera);
    if let Some(e) = ext::get_mut(vm.objects_mut(), target) {
        e.orbit_notify = list;
    }
}

/// `Camera::onDeleteNotify` of each camera that orbits `id`, which is being removed: the orbit object is let go and an orbit
/// mode (3) becomes the orbit-point mode (4) around the point the camera last held, which `writePacketData` then sends.
pub fn orbit_removed(vm: &mut Vm, id: ObjId) {
    let list = match ext::get_mut(vm.objects_mut(), id) {
        Some(e) => std::mem::take(&mut e.orbit_notify),
        None => return,
    };
    for camera in list {
        if let Some(s) = ext::get_mut(vm.objects_mut(), camera).and_then(|e| e.camera.as_mut())
            && s.orbit == Some(id)
        {
            s.orbit = None;
            if s.mode == 3 {
                s.mode = 4;
            }
        }
    }
}

/// `Camera::setFlyMode`: the fly mode; an orbit object is let go (its process-after link and the delete notify).
pub fn fly_mode(vm: &mut Vm, id: ObjId) {
    let previous = state_mut(vm, id).and_then(|s| {
        s.mode = FLY_MODE;
        s.orbit.take()
    });
    if let Some(previous) = previous {
        crate::sim::clear_after(vm, id);
        forget_notify(vm, id, previous);
    }
}

/// `Camera::setOrbitMode(obj, transform, min, max, cur, ownClientObject)`: the previous orbit object is let go, `target` is
/// orbited (processed after it, a delete notify on it) around the centre of its world box, the camera turned to the rotation of
/// the transform's angle-axis ([`orbit_rotation`]), and the three distances kept.
pub fn orbit_mode(
    vm: &mut Vm,
    id: ObjId,
    target: ObjId,
    axis: [f32; 3],
    angle: f32,
    distances: [f32; 3],
    own: bool,
) {
    let Some(previous) = state(vm, id).map(|s| s.orbit) else {
        return;
    };
    if let Some(previous) = previous {
        crate::sim::clear_after(vm, id);
        forget_notify(vm, id, previous);
    }
    let point = ext::get(vm.objects(), target)
        .and_then(|e| e.scene)
        .map_or([0.0; 3], |sc| grid::box_center(sc.world_min, sc.world_max));
    if let Some(s) = state_mut(vm, id) {
        s.orbit_flag = own;
        s.orbit = Some(target);
        s.mode = ORBIT_OBJECT_MODE;
        s.orbit_point = point;
    }
    crate::sim::process_after(vm, id, Some(target));
    delete_notify(vm, id, target);
    let rotation = orbit_rotation(crate::geometry::of(vm), axis, angle);
    set_position(vm, id, point, rotation[0], rotation[2]);
    if let Some(s) = state_mut(vm, id) {
        s.orbit_distances = distances;
    }
}

/// `clearNotify(target)`: the camera leaves the target's list.
fn forget_notify(vm: &mut Vm, camera: ObjId, target: ObjId) {
    if let Some(e) = ext::get_mut(vm.objects_mut(), target) {
        e.orbit_notify.retain(|c| *c != camera);
    }
}

/// Registers TGE's camera methods (`getPosition`, `setFlyMode`, `setOrbitMode`) and `ShapeBase`'s controlling-client and
/// controlling-object getters. The `Camera` class behaviour (type mask, box, container) is the host's.
pub fn register(vm: &mut Vm) {
    register_method(vm, "Camera", "getPosition", |vm, c| {
        let m = c.this_object.and_then(|id| matrix_of(vm, id));
        Native::Str(m.map_or_else(Vec::new, |m| {
            translation(&m)
                .map(|v| format_g(to_f64(v)))
                .join(" ")
                .into_bytes()
        }))
    });
    register_method(vm, "Camera", "setFlyMode", |vm, c| {
        if let Some(id) = c.this_object {
            fly_mode(vm, id);
        }
        Native::Void
    });
    register_method(vm, "Camera", "setOrbitMode", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Void;
        };
        let target = vm
            .objects()
            .find(c.arg(2))
            .filter(|t| is_class(vm, *t, "GameBase"));
        let Some(target) = target else {
            vm.warn("Cannot orbit non-existing object.".to_string());
            fly_mode(vm, id);
            return Native::Void;
        };
        let (_, axis, angle) = orbit_transform(c.arg(3));
        let distances = [c.float(4), c.float(5), c.float(6)];
        let own = c.argc() > 7 && c.boolean(7);
        orbit_mode(vm, id, target, axis, angle, distances, own);
        Native::Void
    });
    // `ShapeBase::getControllingClient` / `getControllingObject`: the connection whose control object this is
    // (the hub knows it), and the object that controls this one through a mount.
    register_method(vm, "ShapeBase", "getControllingClient", |vm, c| {
        let client = c
            .this_object
            .and_then(|id| ghosts::controlling_client(vm, id));
        Native::Int(id_value(client))
    });
    register_method(vm, "ShapeBase", "getControllingObject", |vm, c| {
        let holder = c
            .this_object
            .and_then(|id| crate::sim::mount::controlling_object(vm, id));
        Native::Int(id_value(holder))
    });
}

/// The camera's transform (the `position` field's matrix), for the tests that read the rebuilt matrix.
pub fn matrix(vm: &Vm, id: ObjId) -> Option<Matrix> {
    matrix_of(vm, id)
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    /// A chain where each object leads to the one its `next` field names.
    struct ByField;

    impl ControlChain for ByField {
        fn control_object(&self, vm: &Vm, id: ObjId) -> Option<ObjId> {
            let next = vm.objects().field(id, b"next", b"")?;
            vm.objects().find(&next)
        }
    }

    fn vm_with_pair() -> (Vm, ObjId, ObjId) {
        let mut vm = crate::testing::vm();
        vm.eval(b"new ScriptObject(A) { next = \"B\"; };\nnew ScriptObject(B) { next = \"A\"; };")
            .unwrap();
        let a = vm.objects().find(b"A").unwrap();
        let b = vm.objects().find(b"B").unwrap();
        (vm, a, b)
    }

    #[test]
    fn tges_chain_stops_at_an_object_that_controls_nothing_and_the_fov_goes_to_its_last_object() {
        let (mut vm, a, b) = vm_with_pair();
        assert_eq!(*chain(&vm, a), [a]);
        assert_eq!(fov_object(&vm, a), Some(a), "whatever its class");
        set_control_chain(&mut vm, Rc::new(ByField));
        assert_eq!(*chain(&vm, a), [a, b], "cut at the repeat");
        assert_eq!(fov_object(&vm, a), Some(b));
    }

    #[test]
    fn an_object_that_is_not_a_camera_keeps_its_fov_within_its_datablocks_range() {
        let (mut vm, a, _) = vm_with_pair();
        let mut target = CameraFov { vm: &mut vm, id: a };
        assert_eq!(target.camera_fov(), 90.0);
        target.set_camera_fov(150.0);
        assert_eq!(target.camera_fov(), 120.0);
        target.set_camera_fov(1.0);
        assert_eq!(target.camera_fov(), 5.0);
        target.set_camera_fov(70.0);
        assert_eq!(target.camera_fov(), 70.0);
    }
}
