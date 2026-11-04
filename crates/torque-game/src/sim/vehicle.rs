//! The vehicle host glue: `WheeledVehicle` and `FlyingVehicle` as objects of the server, over the `vehicle` crate.
//!
//! **Where the state lives** (as the Player's and the Item's). `ObjectExt::vehicle` holds a [`VehicleExt`]: the
//! `vehicle` crate's [`Vehicle`] of the object's class. Its [`VehicleState`] is the truth for every field the step reads, the
//! transform included; the VM's `position` matrix and the scene's boxes are copies written at every placement ([`mirror`]),
//! then the bins are checked (`scene::check_bins`, the `checkBins` half of `scene::moved`). The damage and energy words are
//! [`super::vitals`]' (the console methods read them there): the step takes them in before it runs, gives them back before
//! every callback and after it. While the vehicle's own tick runs its body is out of the ext and, around each callback, its
//! state is *parked* in [`VehicleExt::parked`], so a callback's script can call `setTransform`, `setVelocity`, `applyImpulse`
//! on it.
//!
//! **The datablock** is read live (the members are read through the datablock every tick); `WheeledVehicleData::preload`'s
//! products (the wheel records from the shape's `hub<i>` nodes and `spring<i>` sequences, the `steering` and `brakelight`
//! sequences) are made once per vehicle from the asset format model ([`HostPreload`], `vehicle`'s [`PreloadShape`]) when its
//! datablock is set.
//!
//! - `onAdd` (both classes) (Torque: `Vehicle::onAdd`, `addToScene`, `%db.onAdd(%obj)`): `VehicleClass`
//! - `onNewDataBlock` (Torque: Vehicle, WheeledVehicle): [`new_datablock`] and the class's `on_add`
//! - `processTick` (Torque: Flying, Wheeled): [`super::vehicle_tick`]
//! - `setTransform`, `setVelocity`, `applyImpulse` (Torque: the vehicle's): [`set_transform`], [`set_velocity`],
//!   [`apply_impulse`]; a host's angular velocity setter: [`set_angular_velocity`]
//! - the console methods, the driven tail, the ghost view (Torque: the wheel methods, the packet data):
//!   [`super::vehicle_methods`]
//!
//! **Listed (engine-only).** `updateContainer` for a vehicle is the no-zone form (no PhysicalZone or water on a vehicle yet);
//! the image slots of a vehicle are not stepped; `Trigger::potentialEnterObject` from the vehicle's scan is not called; a wheel
//! method called from a callback of the vehicle's own tick (its wheels are out with the step) does nothing and answers false.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `T3D/vehicles/vehicle.cpp`, `T3D/vehicles/wheeledVehicle.cpp` and `T3D/vehicles/flyingVehicle.cpp` (Copyright (c) 2012
//! GarageGames, LLC). The code here is this crate's own.

use std::rc::Rc;

use collide::ObjectId;
use collide::math::{Box3, F3, Matrix};
use shapes::instance::{ShapeInstance, ThreadId, find_sequence};
use ts_vm::classes::EngineClass;
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError};
use vehicle::data::{
    FlyingVehicleData, PreloadShape, ShapeBaseData, SpringData, TireData, VehicleData, WheelRecord,
    WheeledVehicleData,
};
use vehicle::flying::Flying;
use vehicle::host::{EmptyWorld, VehicleHost};
use vehicle::wheeled::{self, Wheeled};
use vehicle::{Vehicle, VehicleState};

use super::player::tick::SimEnv;
use super::vitals::{self, Vitals};
use crate::scene::access::{matrix_of, object_field, scale_of, set_matrix, value_named, value_of};
use crate::scene::camera::is_class;
use crate::scene::container::Container;
use crate::scene::ext;
use crate::scene::ghost as ghosts;
use crate::scene::object as scene;

/// A vehicle of either class.
#[derive(Debug, Clone)]
pub enum Body {
    Wheeled(Box<Vehicle<Wheeled>>),
    Flying(Vehicle<Flying>),
}

impl Body {
    pub fn s(&self) -> &VehicleState {
        match self {
            Body::Wheeled(v) => &v.s,
            Body::Flying(v) => &v.s,
        }
    }

    pub fn s_mut(&mut self) -> &mut VehicleState {
        match self {
            Body::Wheeled(v) => &mut v.s,
            Body::Flying(v) => &mut v.s,
        }
    }
}

/// `WheeledVehicleData::preload`'s products for the vehicle's datablock (empty for a FlyingVehicle).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Preload {
    pub db: Option<ObjId>,
    pub wheels: Vec<WheelRecord>,
    pub steering_seq: i32,
    pub brakelight_seq: i32,
}

/// A vehicle's host state.
#[derive(Debug, Clone)]
pub struct VehicleExt {
    /// The step's vehicle; `None` while its tick runs.
    pub body: Option<Body>,
    /// The state while a callback of its own tick runs script.
    pub parked: Option<Box<VehicleState>>,
    pub preload: Rc<Preload>,
}

impl PartialEq for VehicleExt {
    fn eq(&self, other: &Self) -> bool {
        self.state() == other.state()
    }
}

impl VehicleExt {
    pub fn state(&self) -> Option<&VehicleState> {
        self.body.as_ref().map(Body::s).or(self.parked.as_deref())
    }

    pub fn state_mut(&mut self) -> Option<&mut VehicleState> {
        match self.body.as_mut() {
            Some(b) => Some(b.s_mut()),
            None => self.parked.as_deref_mut(),
        }
    }
}

/// The datablock of a vehicle as the step reads it.
#[derive(Debug, Clone)]
pub enum Data {
    /// The datablock and the host's subclass of it (none: TGE's), [`crate::sim_classes`].
    Wheeled(
        Box<WheeledVehicleData>,
        Option<Rc<dyn vehicle::wheeled::WheeledClass>>,
    ),
    Flying(Box<FlyingVehicleData>),
}

impl Data {
    pub fn base(&self) -> &VehicleData {
        match self {
            Data::Wheeled(d, _) => &d.base,
            Data::Flying(d) => &d.base,
        }
    }
}

pub fn ext_of(vm: &Vm, id: ObjId) -> Option<&VehicleExt> {
    ext::get(vm.objects(), id)?.vehicle.as_deref()
}

pub fn ext_mut(vm: &mut Vm, id: ObjId) -> Option<&mut VehicleExt> {
    ext::get_mut(vm.objects_mut(), id)?.vehicle.as_deref_mut()
}

/// The vehicle's state (its own tick's parked copy while a callback runs).
pub fn state(vm: &Vm, id: ObjId) -> Option<&VehicleState> {
    ext_of(vm, id)?.state()
}

pub fn state_mut(vm: &mut Vm, id: ObjId) -> Option<&mut VehicleState> {
    ext_mut(vm, id)?.state_mut()
}

/// Whether the object is a vehicle with its state.
pub fn is_vehicle(vm: &Vm, id: ObjId) -> bool {
    state(vm, id).is_some()
}

// --------------------------------------------------------------------------------------------------------------- the datablock

pub fn float(vm: &Vm, db: ObjId, name: &[u8], default: f32) -> f32 {
    match value_named(vm, db, name) {
        Some(Value::Float(x)) => x,
        Some(Value::Int(n)) => n as f32,
        _ => default,
    }
}

fn int(vm: &Vm, db: ObjId, name: &[u8], default: i32) -> i32 {
    match value_named(vm, db, name) {
        Some(Value::Int(n)) => n,
        Some(Value::Float(x)) => collide::math::float_to_int(f64::from(x)),
        _ => default,
    }
}

pub fn boolean(vm: &Vm, db: ObjId, name: &[u8], default: bool) -> bool {
    match value_named(vm, db, name) {
        Some(Value::Bool(b)) => b,
        Some(Value::Int(n)) => n != 0,
        _ => default,
    }
}

fn floats3(vm: &Vm, db: ObjId, name: &[u8], default: F3) -> F3 {
    match value_named(vm, db, name) {
        Some(Value::Floats3(v)) => v,
        _ => default,
    }
}

fn shape_data(vm: &Vm, db: ObjId) -> ShapeBaseData {
    let mut d = ShapeBaseData::default();
    d.mass = float(vm, db, b"mass", d.mass);
    d.drag = float(vm, db, b"drag", d.drag);
    d.density = float(vm, db, b"density", d.density);
    d.max_energy = float(vm, db, b"maxEnergy", d.max_energy);
    d.max_damage = float(vm, db, b"maxDamage", d.max_damage);
    d.repair_rate = float(vm, db, b"repairRate", d.repair_rate);
    d.is_invincible = boolean(vm, db, b"isInvincible", d.is_invincible);
    d.inherit_energy = boolean(vm, db, b"inheritEnergyFromMount", d.inherit_energy);
    d
}

fn base_data(vm: &Vm, db: ObjId, mut d: VehicleData) -> VehicleData {
    d.shape = shape_data(vm, db);
    d.body_restitution = float(vm, db, b"bodyRestitution", d.body_restitution);
    d.body_friction = float(vm, db, b"bodyFriction", d.body_friction);
    d.min_impact_speed = float(vm, db, b"minImpactSpeed", d.min_impact_speed);
    d.max_steering_angle = float(vm, db, b"maxSteeringAngle", d.max_steering_angle);
    d.min_drag = float(vm, db, b"minDrag", d.min_drag);
    d.max_drag = float(vm, db, b"maxDrag", d.max_drag);
    d.integration = int(vm, db, b"integration", d.integration);
    d.collision_tol = float(vm, db, b"collisionTol", d.collision_tol);
    d.contact_tol = float(vm, db, b"contactTol", d.contact_tol);
    d.mass_center = floats3(vm, db, b"massCenter", d.mass_center);
    d.mass_box = floats3(vm, db, b"massBox", d.mass_box);
    d.jet_force = float(vm, db, b"jetForce", d.jet_force);
    d.jet_energy_drain = float(vm, db, b"jetEnergyDrain", d.jet_energy_drain);
    d.min_jet_energy = float(vm, db, b"minJetEnergy", d.min_jet_energy);
    d
}

/// `WheeledVehicleData` from datablock `db` now, with the preload's wheel records.
pub fn wheeled_data(vm: &Vm, db: ObjId, pre: &Preload) -> WheeledVehicleData {
    let mut d = WheeledVehicleData::default();
    d.base = base_data(vm, db, d.base);
    macro_rules! f {
        ($field:ident, $name:literal) => {
            d.$field = float(vm, db, $name, d.$field);
        };
    }
    f!(max_wheel_speed, b"maxWheelSpeed");
    f!(engine_torque, b"engineTorque");
    f!(engine_brake, b"engineBrake");
    f!(brake_torque, b"brakeTorque");
    d.wheels = pre.wheels.clone();
    d.steering_seq = pre.steering_seq;
    d.brakelight_seq = pre.brakelight_seq;
    d
}

/// `FlyingVehicleData` from datablock `db` now (`preload`'s `maxSpeed` included).
pub fn flying_data(vm: &Vm, db: ObjId) -> FlyingVehicleData {
    let mut d = FlyingVehicleData::default();
    d.base = base_data(vm, db, d.base);
    macro_rules! f {
        ($field:ident, $name:literal) => {
            d.$field = float(vm, db, $name, d.$field);
        };
    }
    f!(min_trail_speed, b"minTrailSpeed");
    f!(maneuvering_force, b"maneuveringForce");
    f!(horizontal_surface_force, b"horizontalSurfaceForce");
    f!(vertical_surface_force, b"verticalSurfaceForce");
    f!(auto_input_damping, b"autoInputDamping");
    f!(steering_force, b"steeringForce");
    f!(steering_roll_force, b"steeringRollForce");
    f!(roll_force, b"rollForce");
    f!(auto_angular_force, b"autoAngularForce");
    f!(rotational_drag, b"rotationalDrag");
    f!(max_auto_speed, b"maxAutoSpeed");
    f!(auto_linear_force, b"autoLinearForce");
    f!(hover_height, b"hoverHeight");
    f!(create_hover_height, b"createHoverHeight");
    f!(vert_thrust_multiple, b"vertThrustMultiple");
    d.preload();
    d
}

/// A `WheeledVehicleTire` datablock's fields (its preload set `radius` from the tire shape).
pub fn tire_data(vm: &Vm, db: ObjId) -> TireData {
    let mut t = TireData::default();
    t.mass = float(vm, db, b"mass", t.mass);
    t.kinetic_friction = float(vm, db, b"kineticFriction", t.kinetic_friction);
    t.static_friction = float(vm, db, b"staticFriction", t.static_friction);
    t.restitution = float(vm, db, b"restitution", t.restitution);
    t.lateral_force = float(vm, db, b"lateralForce", t.lateral_force);
    t.lateral_damping = float(vm, db, b"lateralDamping", t.lateral_damping);
    t.lateral_relaxation = float(vm, db, b"lateralRelaxation", t.lateral_relaxation);
    t.longitudinal_force = float(vm, db, b"longitudinalForce", t.longitudinal_force);
    t.longitudinal_damping = float(vm, db, b"longitudinalDamping", t.longitudinal_damping);
    // The field name is spelt this way.
    t.longitudinal_relaxation = float(vm, db, b"logitudinalRelaxation", t.longitudinal_relaxation);
    t.radius = float(vm, db, b"radius", t.radius);
    t
}

/// A `WheeledVehicleSpring` datablock's fields.
pub fn spring_data(vm: &Vm, db: ObjId) -> SpringData {
    let mut s = SpringData::default();
    s.length = float(vm, db, b"length", s.length);
    s.force = float(vm, db, b"force", s.force);
    s.damping = float(vm, db, b"damping", s.damping);
    s.anti_sway_force = float(vm, db, b"antiSwayForce", s.anti_sway_force);
    s
}

/// The vehicle's datablock now, by its class.
pub fn data(vm: &Vm, id: ObjId) -> Option<Data> {
    let db = object_field(vm, id, b"dataBlock")?;
    let e = ext_of(vm, id)?;
    match e.body.as_ref().map(|b| matches!(b, Body::Flying(_))) {
        Some(true) => Some(Data::Flying(Box::new(flying_data(vm, db)))),
        Some(false) => Some(Data::Wheeled(
            Box::new(wheeled_data(vm, db, &e.preload)),
            crate::sim_classes::sim_classes(vm).wheeled_class(vm, db),
        )),
        None => {
            if is_class(vm, id, "FlyingVehicle") {
                Some(Data::Flying(Box::new(flying_data(vm, db))))
            } else {
                Some(Data::Wheeled(
                    Box::new(wheeled_data(vm, db, &e.preload)),
                    crate::sim_classes::sim_classes(vm).wheeled_class(vm, db),
                ))
            }
        }
    }
}

/// The datablock's shape, when the engine holds its body.
fn shape_of(vm: &Vm, db: ObjId) -> Option<Rc<crate::shapes::KeptShape>> {
    ext::get(vm.objects(), db)?
        .shape_resource
        .as_ref()
        .map(crate::shapes::ShapeResource::current)
        .filter(|k| k.shape.is_some())
}

/// The temporary shape instance of `WheeledVehicleData::preload` over the asset format model: one thread, animated at detail 0
/// (the instance's current detail). Charged by the instance; a refused budget leaves the default pose (the run ends with the
/// step limit).
pub struct HostPreload<'a> {
    vm: &'a Vm,
    shape: &'a shapes::Shape,
    inst: Option<ShapeInstance>,
    thread: Option<ThreadId>,
}

impl<'a> HostPreload<'a> {
    pub fn new(vm: &'a Vm, shape: &'a shapes::Shape) -> HostPreload<'a> {
        let inst = ShapeInstance::new(shape, &mut |u| vm.charge_work(u)).ok();
        let mut p = HostPreload {
            vm,
            shape,
            inst,
            thread: None,
        };
        // `addThread` before the hub loop.
        if let Some(i) = p.inst.as_mut() {
            p.thread = i.create_thread(shape);
        }
        p
    }
}

impl PreloadShape for HostPreload<'_> {
    fn find_node(&self, name: &str) -> Option<i32> {
        let n = self.shape.find_name(name.as_bytes())?;
        let i = self.shape.nodes.iter().position(|node| node.name == n)?;
        i32::try_from(i).ok()
    }

    fn find_sequence(&self, name: &str) -> Option<i32> {
        find_sequence(self.shape, name.as_bytes()).and_then(|i| i32::try_from(i).ok())
    }

    fn animate(&mut self, seq: i32, pos: f32) {
        let (Some(inst), Some(t)) = (self.inst.as_mut(), self.thread) else {
            return;
        };
        let vm = self.vm;
        inst.set_sequence(self.shape, t, seq, pos);
        let _ = inst.animate(self.shape, 0, None, &mut |u| vm.charge_work(u));
    }

    fn node_translation(&self, node: i32) -> F3 {
        usize::try_from(node)
            .ok()
            .and_then(|n| self.inst.as_ref()?.node_transform(n))
            .map_or([0.0; 3], |m| [m[3], m[7], m[11]])
    }
}

/// `WheeledVehicleData::preload`'s wheel records and sequences for datablock `db` (none without a shape body).
pub fn preload(vm: &Vm, db: ObjId) -> Preload {
    let mut d = WheeledVehicleData::default();
    if let Some(kept) = shape_of(vm, db)
        && let Some(shape) = kept.shape.as_ref()
    {
        let mut p = HostPreload::new(vm, shape);
        d.preload(&mut p);
    }
    Preload {
        db: Some(db),
        wheels: d.wheels,
        steering_seq: d.steering_seq,
        brakelight_seq: d.brakelight_seq,
    }
}

// ------------------------------------------------------------------------------------------------------------------- placement

/// The object box the scene holds for `id` (the shape's bounds, `ShapeBase::onNewDataBlock`).
fn scene_box(vm: &Vm, id: ObjId) -> Option<Box3> {
    let s = ext::get(vm.objects(), id)?.scene?;
    Some(Box3::new(s.object_min, s.object_max))
}

/// The damage and energy words into the state.
pub fn take_vitals(vm: &Vm, id: ObjId, s: &mut VehicleState) {
    let v = vitals::get(vm, id);
    s.energy = v.energy;
    s.recharge_rate = v.recharge_rate;
    s.damage = v.damage;
    s.repair_rate = v.repair_rate;
    s.repair_reserve = v.repair_reserve;
    s.damage_state = v.damage_state;
}

/// The state's damage and energy words back to where the methods read them.
pub fn put_vitals(vm: &mut Vm, id: ObjId, s: &VehicleState) {
    vitals::put(
        vm,
        id,
        &Vitals {
            energy: s.energy,
            recharge_rate: s.recharge_rate,
            damage: s.damage,
            repair_rate: s.repair_rate,
            repair_reserve: s.repair_reserve,
            damage_state: s.damage_state,
        },
    );
}

/// What the host keeps elsewhere, into the state: the type mask, the scale (the world box follows a change), the mount, the
/// controlling client (`Player::setControlObject` hands the driver's on), the damage and energy words.
pub fn take_fields(vm: &Vm, id: ObjId, s: &mut VehicleState) {
    s.type_mask = ext::get(vm.objects(), id).map_or(s.type_mask, |e| e.type_mask);
    let scale = scale_of(vm, id);
    if s.scale != scale {
        s.scale = scale;
        s.reset_world_box();
    }
    s.mount = super::mount::mount_of(vm, id).map(ObjectId);
    // `disableMove` is a persist field.
    s.disable_move = matches!(value_of(vm, id, b"disableMove"), Some(Value::Bool(true)));
    // What the class reads of the controlling client.
    let client = ghosts::controlling_client(vm, id);
    crate::sim_classes::sim_classes(vm).before_vehicle_step(vm, id, client, s);
    take_vitals(vm, id, s);
}

/// Writes the state's transform and world box into the VM's matrix and the scene (the copies), without the bins.
pub fn write_copies(vm: &mut Vm, id: ObjId, s: &VehicleState) {
    set_matrix(vm, id, &s.obj_to_world);
    if let Some(sc) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.scene.as_mut()) {
        sc.world_min = s.world_box.min;
        sc.world_max = s.world_box.max;
        sc.world_center = s.world_center;
        sc.world_radius = s.world_radius;
    }
}

/// The copies of the state now, then `checkBins`.
pub fn mirror(vm: &mut Vm, container: &Rc<Container>, id: ObjId) {
    if let Some(s) = state(vm, id).cloned() {
        write_copies(vm, id, &s);
    }
    scene::check_bins(vm, container, id);
}

/// The mask bits the state set, into the ghost layer's dirty masks (`setMaskBits`).
pub fn flush_mask(vm: &mut Vm, id: ObjId) {
    let bits = state_mut(vm, id).map_or(0, |s| std::mem::take(&mut s.mask_bits));
    if bits != 0 {
        ghosts::mark(vm, id, bits);
    }
}

/// The world a placement outside the tick re-bins in (the process list's environment).
pub fn env_of(vm: &Vm, id: ObjId) -> Option<Rc<SimEnv>> {
    ext::get(vm.objects(), id)?.process.as_ref()?.env()
}

/// The host of a placement outside the tick: no world, `moved` noted.
struct PlaceHost {
    world: EmptyWorld,
    masks: &'static collide::TypeMasks,
}

impl VehicleHost for PlaceHost {
    fn type_masks(&self) -> &collide::TypeMasks {
        self.masks
    }

    fn world(&mut self) -> &mut dyn collide::World {
        &mut self.world
    }

    fn sim_time(&self) -> u32 {
        0
    }
}

/// Runs `f` on the state of vehicle `id` outside its tick (a script's call; also on the parked state of a callback), then the
/// copies, the bins and the mask bits.
fn with_state<R>(
    vm: &mut Vm,
    id: ObjId,
    f: impl FnOnce(&mut VehicleState, &mut dyn VehicleHost) -> R,
) -> Option<R> {
    let mut host = PlaceHost {
        world: EmptyWorld,
        masks: crate::masks::type_masks(vm),
    };
    // `setScale` stores the scale and then calls `setTransform`: the state takes the scale the VM holds first.
    let scale = scale_of(vm, id);
    let s = state_mut(vm, id)?;
    if s.scale != scale {
        s.scale = scale;
        s.reset_world_box();
    }
    let r = f(s, &mut host);
    after_placement(vm, id);
    Some(r)
}

fn after_placement(vm: &mut Vm, id: ObjId) {
    match env_of(vm, id) {
        Some(env) => mirror(vm, &env.container, id),
        None => {
            if let Some(s) = state(vm, id).cloned() {
                write_copies(vm, id, &s);
            }
        }
    }
    flush_mask(vm, id);
}

/// Runs `f` on the body of vehicle `id` with its datablock, outside its tick (`None` while its tick has it).
pub fn with_body<R>(vm: &mut Vm, id: ObjId, f: impl FnOnce(&mut Body, &Data) -> R) -> Option<R> {
    let d = data(vm, id)?;
    let body = ext_mut(vm, id)?.body.as_mut()?;
    let r = f(body, &d);
    after_placement(vm, id);
    Some(r)
}

/// `Vehicle::setTransform`, for a script's `setTransform`.
pub fn set_transform(vm: &mut Vm, id: ObjId, m: &Matrix) {
    let m = *m;
    with_state(vm, id, |s, h| s.set_transform(&m, h));
}

/// `Vehicle::setVelocity`.
pub fn set_velocity(vm: &mut Vm, id: ObjId, v: F3) {
    with_state(vm, id, |s, _| s.set_velocity(v));
}

/// `Vehicle::applyImpulse(pos, J)`.
pub fn apply_impulse(vm: &mut Vm, id: ObjId, pos: F3, j: F3) {
    with_state(vm, id, |s, _| s.apply_impulse(pos, j));
}

/// Sets a vehicle's angular velocity (and its momentum), for a host's console method.
pub fn set_angular_velocity(vm: &mut Vm, id: ObjId, v: F3) {
    with_state(vm, id, |s, _| s.set_angular_velocity(v));
}

/// `Vehicle::getVelocity`: the rigid body's linear velocity.
pub fn velocity(vm: &Vm, id: ObjId) -> Option<F3> {
    state(vm, id).map(|s| s.rigid.lin_vel)
}

// ------------------------------------------------------------------------------------------------------------------- the class

/// The body of a new vehicle of datablock `db` at the object's transform (`onNewDataBlock` then `Vehicle::onAdd`'s rigid and
/// deltas).
fn make_body(vm: &Vm, id: ObjId, db: ObjId, flying: bool, pre: &Preload) -> Body {
    let m = matrix_of(vm, id).unwrap_or(collide::math::IDENTITY);
    let obj_box = scene_box(vm, id).unwrap_or_default();
    let scale = scale_of(vm, id);
    let oid = ObjectId(id);
    if flying {
        let d = flying_data(vm, db);
        let mut s = VehicleState::new(oid, &d.base, obj_box, &m, scale);
        s.set_mask(u32::MAX);
        Body::Flying(Vehicle::new(s, Flying::default()))
    } else {
        let d = wheeled_data(vm, db, pre);
        let s = VehicleState::new(oid, &d.base, obj_box, &m, scale);
        let mut v = Vehicle::new(s, Wheeled::default());
        wheeled::on_new_data_block(&mut v.s, &mut v.k, &d);
        // `Vehicle::onAdd` after the datablock: the rigid from the transform, the deltas.
        let s = &mut v.s;
        s.rigid.set_transform(&m);
        s.delta_rot = [s.rigid.ang_pos; 2];
        s.delta_pos = s.rigid.lin_pos;
        Body::Wheeled(Box::new(v))
    }
}

/// `onNewDataBlock` again for a vehicle that has its state (`setDataBlock` after the add): the preload's products for the new
/// datablock and the class's `onNewDataBlock` on the state. The first datablock's work is the class's `on_add`.
pub fn new_datablock(vm: &mut Vm, id: ObjId, db: ObjId) {
    if !is_vehicle(vm, id)
        || !(is_class(vm, db, "WheeledVehicleData") || is_class(vm, db, "FlyingVehicleData"))
    {
        return;
    }
    let flying = is_class(vm, id, "FlyingVehicle");
    let pre = Rc::new(if flying {
        Preload::default()
    } else {
        preload(vm, db)
    });
    let d = if flying {
        Data::Flying(Box::new(flying_data(vm, db)))
    } else {
        Data::Wheeled(
            Box::new(wheeled_data(vm, db, &pre)),
            crate::sim_classes::sim_classes(vm).wheeled_class(vm, db),
        )
    };
    let Some(e) = ext_mut(vm, id) else {
        return;
    };
    e.preload = pre;
    if let Some(body) = e.body.as_mut() {
        match (body, &d) {
            (Body::Wheeled(v), Data::Wheeled(d, _)) => {
                wheeled::on_new_data_block(&mut v.s, &mut v.k, d)
            }
            (Body::Flying(v), Data::Flying(d)) => {
                v.s.on_new_data_block(&d.base);
                v.s.set_mask(u32::MAX);
            }
            _ => {}
        }
    }
    flush_mask(vm, id);
}

/// The collision hulls of vehicle `id`'s shape, made once here so that their accelerators are cached with the shape. Torque
/// computes a shape's accelerators at `ShapeBaseData::preload` (`TSShape::computeAccelerator` over the collision details); the
/// engine makes them on the first ask, and a tick's object budget ([`super::pass::TICK_OBJECT_WORK`]) is below the charge of a
/// large vehicle's (a large body is `O(V * F^2)`, above 2^20 units): asked first by a tick, the charge was refused, nothing was
/// cached, and every tick stopped after its first sub-step. Here the creating script's run pays. Nothing without the process
/// list's environment.
fn warm_hulls(vm: &mut Vm, id: ObjId) {
    let Some(env) = env_of(vm, id) else {
        return;
    };
    let world = crate::scene::container_world::ContainerWorld::new(vm, &env.container, &env.shapes);
    for slot in 0..shapes::preload::SLOTS as u32 {
        let _ = collide::World::hull(&world, ObjectId(id), slot);
    }
}

/// `onAdd` of WheeledVehicle and FlyingVehicle (`Vehicle::onAdd`, then `addToScene`, then
/// `%db.onAdd(%obj)` for a server object): with a vehicle datablock the state is made (the class's `onNewDataBlock` and the
/// rigid at the transform), the copies written, the object enters the container and the script runs. Without one, only the
/// container.
struct VehicleClass {
    container: Rc<Container>,
    flying: bool,
}

impl EngineClass for VehicleClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let want = if self.flying {
            "FlyingVehicleData"
        } else {
            "WheeledVehicleData"
        };
        let Some(db) = object_field(vm, id, b"dataBlock").filter(|d| is_class(vm, *d, want)) else {
            scene::add_to_scene(vm, &self.container, id);
            return Ok(true);
        };
        let pre = Rc::new(if self.flying {
            Preload::default()
        } else {
            preload(vm, db)
        });
        let mut body = make_body(vm, id, db, self.flying, &pre);
        take_fields(vm, id, body.s_mut());
        let s = body.s().clone();
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.vehicle = Some(Box::new(VehicleExt {
                body: Some(body),
                parked: None,
                preload: pre,
            }));
        }
        write_copies(vm, id, &s);
        flush_mask(vm, id);
        scene::add_to_scene(vm, &self.container, id);
        warm_hulls(vm, id);
        vm.call_method(db, b"onAdd", &[id.to_string().into_bytes()])?;
        Ok(true)
    }

    /// `onRemove` (Wheeled, Flying): `%db.onRemove(%obj)` for a server object with a datablock (`scriptOnRemove`), before
    /// `Vehicle::onRemove` unmounts and leaves the
    /// container (the ShapeBase and scene behaviours, after this one).
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        if let Some(db) = object_field(vm, id, b"dataBlock") {
            vm.call_method(db, b"onRemove", &[id.to_string().into_bytes()])?;
        }
        Ok(())
    }
}

/// Gives WheeledVehicle and FlyingVehicle their behaviour (replacing the scene entry) and their console methods. After
/// `shapebase::register`.
pub fn register(vm: &mut Vm, container: &Rc<Container>) {
    for (class, flying) in [("WheeledVehicle", false), ("FlyingVehicle", true)] {
        vm.set_class_behaviour(
            class,
            Rc::new(VehicleClass {
                container: Rc::clone(container),
                flying,
            }),
        );
    }
    super::vehicle_methods::register(vm);
}
