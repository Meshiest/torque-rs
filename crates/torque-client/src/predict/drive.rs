//! The client's prediction of a vehicle its Player drives: TGE's client runs the driven vehicle forward with the moves the
//! Player gets, as the server does (`Player::processTick` hands the vehicle its move; `WheeledVehicle::processTick`),
//! and reads the server's state of it from the Player's packet data (`Player::readPacketData` -> the control object's
//! `readPacketData`: `Vehicle`, `WheeledVehicle`). The step is torque-vehicle's (bit-exact with the server's); this
//! module gives it what the client knows:
//!
//! - the vehicle's datablock from the datablock the server sent (`WheeledVehicleData::unpackData`: the fields the server sends; the
//!   others keep the constructor's values), `preload`'s wheel records from the vehicle's shape in the install, and each wheel's
//!   tire and spring datablocks from the ghost's wheel block (`WheeledVehicle::unpackUpdate`);
//! - the shapes ([`ShapeCache`]: the install's `.dts`, read once a connection, its collision hulls made once and charged);
//! - the client's container for the step ([`super::ClientCollision`]: the vehicles there with their collision hulls, the game's
//!   static objects, the ground; `castRay` for the wheels' rays: the game's static objects (their own `castRay`, the nearest), then
//!   the ground
//!   plane, strictly nearer);
//! - [`ClientVehicleHost`]: no script runs on the client, so every callback is the default (nothing).
//!
//! **Engine-only, listed.** A FlyingVehicle is not predicted; another vehicle's shape is not cast by a wheel's ray (its LOS
//! meshes). A ghost is drawn where it is now ([`GhostVehicle::unpack`], [`GhostVehicle::tick`]): TGE holds the physics while
//! the drawn pose warps to the update's position, which is already the past when it comes (the network's delay), so the
//! vehicle was drawn that delay plus the warp behind where it is. Here the update's state coasts on through the warp at its
//! velocities (no physics, as TGE's warp), by the update's age as well (measured from how far ahead of each update the
//! vehicle is drawn, eased over updates, at most [`MAX_UPDATE_AGE`]: the whole ticks coasted, the rest a lead along its
//! velocity), and the drawn pose's offset from where it was going to that shrinks each tick ([`GHOST_OFFSET_KEEP`]; past
//! [`GHOST_SNAP`] it is drawn there at once). Values that are not finite are left out or undone ([`GhostVehicle::tick`]).
//!
//! Credit: follows TGE's vehicles and Player as Torque3D carries them (MIT: `T3D/vehicles/vehicle.cpp`,
//! `T3D/vehicles/wheeledVehicle.cpp`, each Copyright (c) 2012 GarageGames, LLC; `T3D/player.cpp`, Copyright (c) 2012 GarageGames,
//! LLC; Copyright (C) 2015 Faust Logic, Inc.).

use std::collections::HashMap;
use std::rc::Rc;

use collide::ObjectId;
use collide::convex::HullSlot;
use collide::math::{Box3, F3, Matrix};
use collide::mesh::{Hull, HullAccel};
use ghosts::Value;
use net::control::ControlData;
use shapes::preload::{Facts, SLOTS, SlotRule};
use shapes::shape::Shape;
use vehicle::data::{
    PreloadShape, ShapeBaseData, SpringData, TireData, VehicleData, WheeledVehicleData,
};
use vehicle::host::{RayHit, VehicleHost};
use vehicle::math::{quat_inverse, quat_mul};
use vehicle::wheeled::{self, Wheeled, WheeledClass};
use vehicle::words::VehicleWords;
use vehicle::{Vehicle, VehicleState};

use super::game::{PredictGame, PredictWorld, SteerRules};
use super::{ClientCollision, FileReader};
use crate::records::{EngineDatablock, EngineRecord, EngineSet, VehicleUpdate};

/// The most shapes a connection reads, and the largest file it reads (a vehicle's shape is far below both).
pub const MAX_SHAPES: usize = 256;
pub const MAX_SHAPE_BYTES: usize = 16 << 20;
/// The work a shape's read is charged per byte (its parse, facts and hulls are charged by their own units).
pub const SHAPE_WORK_PER_BYTE: u64 = 1;

/// A shape the client read from the install: the parsed model, what the preloads find, the node transforms at rest and the
/// collision hull of each slot (made once, as TGE computes the accelerators at `ShapeBaseData::preload`).
#[derive(Debug)]
pub struct ClientShape {
    pub shape: Shape,
    pub facts: Facts,
    pub hulls: [Option<Rc<Hull>>; SLOTS],
}

impl ClientShape {
    /// The eight collision slots as `collide` takes them.
    pub fn hull_slots(&self) -> Vec<HullSlot> {
        // A shape lists only the collision details it has: the slots past them are absent.
        (0..SLOTS)
            .map(|i| {
                let b = self.facts.bounds.get(i).copied().unwrap_or_default();
                HullSlot {
                    slot: i as u32,
                    present: self.facts.slots.collision.get(i).is_some_and(|&d| d >= 0),
                    bounds: Box3 {
                        min: [b[0], b[1], b[2]],
                        max: [b[3], b[4], b[5]],
                    },
                }
            })
            .collect()
    }

    /// The shape's bounds (`ShapeBase::onNewDataBlock`'s object box).
    pub fn bounds(&self) -> Box3 {
        let b = self.shape.bounds;
        Box3 {
            min: [b[0], b[1], b[2]],
            max: [b[3], b[4], b[5]],
        }
    }
}

/// The shapes a connection read, by path (lower case); `None` for one that is not there or does not parse. A static object's collision
/// shape is read in steps over several ticks ([`ShapeCache::get_staged`]); the reads begun and not done are in `loading`.
#[derive(Debug, Default, Clone)]
pub struct ShapeCache {
    shapes: HashMap<Vec<u8>, Option<Rc<ClientShape>>>,
    loading: HashMap<Vec<u8>, ShapeLoad>,
    /// The rule the collision and line-of-sight details are picked by (the game's, [`PredictGame::shape_slots`]; TGE's when
    /// none).
    slots: Option<SlotRule>,
}

/// What a staged read of a shape ([`ShapeCache::get_staged`]) has come to.
#[derive(Debug, Clone)]
pub enum Staged {
    Loaded(Rc<ClientShape>),
    /// Not there, not a shape the client reads, or more shapes than the cache keeps: for good.
    Failed,
    /// Begun and not done: a step of it was refused earlier this tick, so none is begun again before the next.
    Pending,
    /// Its next step did not fit in what was left (`room`: the units the step had when it began); tried again the next tick.
    Refused {
        room: u64,
    },
}

/// A shape read in steps, each charged as it runs and kept once done: the file and its parse (one step, so no file is held
/// between ticks), the preload's facts and rest pose, then the hull of each collision slot. A step that would pass the budget is
/// refused before the units it asks for are charged (what it did before is charged) and begins again from its start on a later
/// tick.
#[derive(Debug, Default, Clone)]
struct ShapeLoad {
    shape: Option<Shape>,
    posed: Option<(Facts, Vec<Matrix>)>,
    hulls: [Option<Rc<Hull>>; SLOTS],
    next_slot: usize,
    /// The tick a step was last refused at.
    refused: Option<u64>,
}

enum Step {
    More,
    Done(Box<ClientShape>),
    Bad,
    Refused,
}

impl ShapeLoad {
    /// Runs the next step, charged to `work` up to `budget`.
    fn step(
        &mut self,
        path: &[u8],
        reader: Option<&FileReader>,
        slots: SlotRule,
        work: &mut u64,
        budget: u64,
    ) -> Step {
        let room = budget.saturating_sub(*work);
        let mut refused = false;
        let mut charge = |units: u64| {
            let next = work.saturating_add(units);
            if next > budget {
                refused = true;
                return false;
            }
            *work = next;
            true
        };
        let Some(shape) = &self.shape else {
            // The read is charged once made: begun only with room for the largest file there can be.
            let most = SHAPE_WORK_PER_BYTE.saturating_mul(MAX_SHAPE_BYTES as u64);
            if room < most {
                return Step::Refused;
            }
            let Some(bytes) = reader.and_then(|r| r(path)) else {
                return Step::Bad;
            };
            let cost = SHAPE_WORK_PER_BYTE.saturating_mul(bytes.len() as u64);
            if bytes.len() > MAX_SHAPE_BYTES || !charge(cost) {
                return Step::Bad;
            }
            // A glTF's import and the parse of the `.dts` it makes can cost more than the room the read was begun with (the
            // largest file's): refused like any step, with its room, so the caller decides whether it can ever fit.
            let bytes = match torque_import::native::shape_file(path, bytes, &mut charge) {
                Ok(bytes) => bytes,
                Err(torque_import::ImportError::OutOfBudget) => return Step::Refused,
                Err(_) => return Step::Bad,
            };
            let parsed = shapes::dts::parse(&bytes, &mut charge);
            return match parsed {
                Ok(model) => {
                    self.shape = Some(Shape::for_server(model));
                    Step::More
                }
                Err(_) if refused => Step::Refused,
                Err(_) => Step::Bad,
            };
        };
        let Some((facts, nodes)) = &self.posed else {
            let posed = shapes::preload::facts_with(shape, slots, &mut charge).and_then(|facts| {
                let instance = shapes::instance::ShapeInstance::new(shape, &mut charge)?;
                Ok((facts, instance.node_transforms().to_vec()))
            });
            return match posed {
                Ok(p) => {
                    self.posed = Some(p);
                    Step::More
                }
                Err(_) if refused => Step::Refused,
                Err(_) => Step::Bad,
            };
        };
        let detail_of = |slot: usize| facts.slots.collision.get(slot).copied().unwrap_or(-1);
        while self.next_slot < SLOTS && detail_of(self.next_slot) < 0 {
            self.next_slot += 1;
        }
        if self.next_slot < SLOTS {
            let detail = detail_of(self.next_slot);
            let hull = slot_hull(shape, nodes, detail, &mut charge);
            return match hull {
                Some(hull) => {
                    self.hulls[self.next_slot] = Some(Rc::new(hull));
                    self.next_slot += 1;
                    Step::More
                }
                None if refused => Step::Refused,
                None => Step::Bad,
            };
        }
        match (self.shape.take(), self.posed.take()) {
            (Some(shape), Some((facts, _))) => Step::Done(Box::new(ClientShape {
                shape,
                facts,
                hulls: std::mem::take(&mut self.hulls),
            })),
            _ => Step::Bad,
        }
    }
}

impl ShapeCache {
    /// A cache whose shapes pick their collision details by `slots`.
    pub fn with_slots(slots: SlotRule) -> ShapeCache {
        ShapeCache {
            slots: Some(slots),
            ..ShapeCache::default()
        }
    }

    fn slot_rule(&self) -> SlotRule {
        self.slots.unwrap_or(shapes::preload::collision_slots)
    }

    /// The shape at `path` (read through `reader` the first time, everything charged to `work` up to `budget`; at most
    /// [`MAX_SHAPES`] shapes).
    pub fn get(
        &mut self,
        path: &[u8],
        reader: Option<&FileReader>,
        work: &mut u64,
        budget: u64,
    ) -> Option<Rc<ClientShape>> {
        let slots = self.slot_rule();
        let key = path.to_ascii_lowercase();
        if let Some(s) = self.shapes.get(&key) {
            return s.clone();
        }
        if self.shapes.len() >= MAX_SHAPES || *work >= budget {
            return None;
        }
        let loaded = reader.and_then(|r| r(path)).and_then(|bytes| {
            if bytes.len() > MAX_SHAPE_BYTES {
                return None;
            }
            *work = work.saturating_add(SHAPE_WORK_PER_BYTE.saturating_mul(bytes.len() as u64));
            if *work > budget {
                return None;
            }
            let mut charge = |units: u64| {
                *work = work.saturating_add(units);
                *work <= budget
            };
            let bytes = torque_import::native::shape_file(path, bytes, &mut charge).ok()?;
            load(&bytes, slots, work, budget).map(Rc::new)
        });
        // A read the budget cut short is tried again on a later tick; any other answer is kept.
        if loaded.is_none() && *work > budget {
            return None;
        }
        self.shapes.insert(key, loaded.clone());
        loaded
    }

    /// The shape at `path` read in steps (`ShapeLoad`): as many as fit in `budget` (charged to `work`) run now, the rest on
    /// later calls; once a step is refused at tick `tick`, none is begun again before the next. At most `max` shapes, read or
    /// begun.
    pub fn get_staged(
        &mut self,
        path: &[u8],
        reader: Option<&FileReader>,
        work: &mut u64,
        budget: u64,
        tick: u64,
        max: usize,
    ) -> Staged {
        let slots = self.slot_rule();
        let key = path.to_ascii_lowercase();
        if let Some(s) = self.shapes.get(&key) {
            return s.clone().map_or(Staged::Failed, Staged::Loaded);
        }
        if !self.loading.contains_key(&key) {
            if self.shapes.len() + self.loading.len() >= max {
                return Staged::Failed;
            }
            self.loading.insert(key.clone(), ShapeLoad::default());
        }
        let Some(load) = self.loading.get_mut(&key) else {
            return Staged::Failed;
        };
        if load.refused == Some(tick) {
            return Staged::Pending;
        }
        let done = loop {
            let room = budget.saturating_sub(*work);
            match load.step(path, reader, slots, work, budget) {
                Step::More => {}
                Step::Done(shape) => break Some(Rc::new(*shape)),
                Step::Bad => break None,
                Step::Refused => {
                    load.refused = Some(tick);
                    return Staged::Refused { room };
                }
            }
        };
        self.loading.remove(&key);
        self.shapes.insert(key, done.clone());
        done.map_or(Staged::Failed, Staged::Loaded)
    }

    /// Gives up the staged read of `path` for good: it answers [`Staged::Failed`] from now on.
    pub fn fail(&mut self, path: &[u8]) {
        let key = path.to_ascii_lowercase();
        self.loading.remove(&key);
        self.shapes.insert(key, None);
    }
}

/// The parse, the facts, the rest pose and the hulls of a `.dts`, charged to `work` (a refusal past `budget` loads nothing).
fn load(bytes: &[u8], slots: SlotRule, work: &mut u64, budget: u64) -> Option<ClientShape> {
    let mut charge = |units: u64| {
        *work = work.saturating_add(units);
        *work <= budget
    };
    let model = shapes::dts::parse(bytes, &mut charge).ok()?;
    let shape = Shape::for_server(model);
    let facts = shapes::preload::facts_with(&shape, slots, &mut charge).ok()?;
    let instance = shapes::instance::ShapeInstance::new(&shape, &mut charge).ok()?;
    let nodes = instance.node_transforms().to_vec();
    let mut hulls: [Option<Rc<Hull>>; SLOTS] = Default::default();
    for (slot, hull) in hulls.iter_mut().enumerate() {
        let detail = facts.slots.collision.get(slot).copied().unwrap_or(-1);
        if detail < 0 {
            continue;
        }
        *hull = Some(Rc::new(slot_hull(&shape, &nodes, detail, &mut charge)?));
    }
    Some(ClientShape {
        shape,
        facts,
        hulls,
    })
}

/// The hull of the collision detail `detail` with its accelerator (`computeAccelerator`), charged (None: refused or unreadable).
fn slot_hull(
    shape: &Shape,
    nodes: &[Matrix],
    detail: i32,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Option<Hull> {
    let parts = torque_game::scene::rays::detail_parts(shape, nodes, detail, charge).ok()?;
    let verts: u64 = parts.iter().map(|p| p.verts.len() as u64).sum();
    let tris: u64 = parts.iter().map(|p| p.tris.len() as u64).sum();
    let verts = verts.min(collide::mesh::MAX_HULL_VERTS as u64);
    let tris = tris.min(collide::mesh::MAX_HULL_TRIS as u64);
    // `computeAccelerator`, `O(V * F^2)` at worst (the server's charge).
    if !charge(
        collide::work::FEATURE
            .saturating_mul(verts.saturating_mul(tris).saturating_mul(tris.max(1))),
    ) {
        return None;
    }
    let accel = HullAccel::compute(&parts);
    Some(Hull {
        node: parts.first().map(|p| p.node),
        parts,
        accel: Some(accel),
    })
}

// ---- the datablocks the client has

fn value<'w, W: PredictWorld>(world: &'w W, db: u32, name: &str, index: u32) -> Option<&'w Value> {
    world.datablocks.get(&db)?.value(name, index)
}

pub fn float<W: PredictWorld>(world: &W, db: u32, name: &str, index: u32, default: f32) -> f32 {
    match value(world, db, name, index) {
        Some(Value::F32(x)) => *x,
        Some(Value::Int(n)) => *n as f32,
        _ => default,
    }
}

fn int<W: PredictWorld>(world: &W, db: u32, name: &str, default: i32) -> i32 {
    match value(world, db, name, 0) {
        Some(Value::Int(n)) => i32::try_from(*n).unwrap_or(default),
        _ => default,
    }
}

pub fn boolean<W: PredictWorld>(world: &W, db: u32, name: &str, default: bool) -> bool {
    match value(world, db, name, 0) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Int(n)) => *n != 0,
        _ => default,
    }
}

fn shape_base<W: PredictWorld>(world: &W, db: u32) -> ShapeBaseData {
    let mut d = ShapeBaseData::default();
    d.mass = float(world, db, "mass", 0, d.mass);
    d.drag = float(world, db, "drag", 0, d.drag);
    d.density = float(world, db, "density", 0, d.density);
    d.max_energy = float(world, db, "maxEnergy", 0, d.max_energy);
    d.max_damage = float(world, db, "maxDamage", 0, d.max_damage);
    d.repair_rate = float(world, db, "repairRate", 0, d.repair_rate);
    d.is_invincible = boolean(world, db, "isInvincible", d.is_invincible);
    d.inherit_energy = boolean(world, db, "inheritEnergyFromMount", d.inherit_energy);
    d
}

fn vehicle_base<W: PredictWorld>(world: &W, db: u32) -> VehicleData {
    let mut d = VehicleData {
        shape: shape_base(world, db),
        ..VehicleData::default()
    };
    d.body_restitution = float(world, db, "bodyRestitution", 0, d.body_restitution);
    d.body_friction = float(world, db, "bodyFriction", 0, d.body_friction);
    d.min_impact_speed = float(world, db, "minImpactSpeed", 0, d.min_impact_speed);
    d.max_steering_angle = float(world, db, "maxSteeringAngle", 0, d.max_steering_angle);
    d.min_drag = float(world, db, "minDrag", 0, d.min_drag);
    d.max_drag = float(world, db, "maxDrag", 0, d.max_drag);
    d.integration = int(world, db, "integration", d.integration);
    d.collision_tol = float(world, db, "collisionTol", 0, d.collision_tol);
    d.contact_tol = float(world, db, "contactTol", 0, d.contact_tol);
    for i in 0..3 {
        d.mass_center[i] = float(world, db, "massCenter", i as u32, d.mass_center[i]);
        d.mass_box[i] = float(world, db, "massBox", i as u32, d.mass_box[i]);
    }
    d.jet_force = float(world, db, "jetForce", 0, d.jet_force);
    d.jet_energy_drain = float(world, db, "jetEnergyDrain", 0, d.jet_energy_drain);
    d.min_jet_energy = float(world, db, "minJetEnergy", 0, d.min_jet_energy);
    d
}

/// The client's `WheeledVehicleData` of datablock `db`, `preload`'s wheel records from `shape`. `None` for another class.
pub fn wheeled_data<W: PredictWorld>(
    world: &W,
    db: u32,
    shape: &ClientShape,
) -> Option<WheeledVehicleData> {
    if world.datablocks.get(&db)?.class_name() != "WheeledVehicleData" {
        return None;
    }
    let mut d = WheeledVehicleData {
        base: vehicle_base(world, db),
        ..WheeledVehicleData::default()
    };
    macro_rules! f {
        ($field:ident, $name:literal) => {
            d.$field = float(world, db, $name, 0, d.$field);
        };
    }
    f!(max_wheel_speed, "maxWheelSpeed");
    f!(engine_torque, "engineTorque");
    f!(engine_brake, "engineBrake");
    f!(brake_torque, "brakeTorque");
    let mut pre = ClientPreload::new(&shape.shape);
    d.preload(&mut pre);
    Some(d)
}

/// A `WheeledVehicleTire` datablock (its `radius` is the server's preload's, sent).
pub fn tire_data<W: PredictWorld>(world: &W, db: u32) -> Option<TireData> {
    if world.datablocks.get(&db)?.class_name() != "WheeledVehicleTire" {
        return None;
    }
    let mut t = TireData::default();
    t.mass = float(world, db, "mass", 0, t.mass);
    t.kinetic_friction = float(world, db, "kineticFriction", 0, t.kinetic_friction);
    t.static_friction = float(world, db, "staticFriction", 0, t.static_friction);
    t.restitution = float(world, db, "restitution", 0, t.restitution);
    t.lateral_force = float(world, db, "lateralForce", 0, t.lateral_force);
    t.lateral_damping = float(world, db, "lateralDamping", 0, t.lateral_damping);
    t.lateral_relaxation = float(world, db, "lateralRelaxation", 0, t.lateral_relaxation);
    t.longitudinal_force = float(world, db, "longitudinalForce", 0, t.longitudinal_force);
    t.longitudinal_damping = float(world, db, "longitudinalDamping", 0, t.longitudinal_damping);
    t.longitudinal_relaxation = float(
        world,
        db,
        "logitudinalRelaxation",
        0,
        t.longitudinal_relaxation,
    );
    t.radius = float(world, db, "radius", 0, t.radius);
    Some(t)
}

/// A `WheeledVehicleSpring` datablock.
pub fn spring_data<W: PredictWorld>(world: &W, db: u32) -> Option<SpringData> {
    if world.datablocks.get(&db)?.class_name() != "WheeledVehicleSpring" {
        return None;
    }
    let mut s = SpringData::default();
    s.length = float(world, db, "length", 0, s.length);
    s.force = float(world, db, "force", 0, s.force);
    s.damping = float(world, db, "damping", 0, s.damping);
    s.anti_sway_force = float(world, db, "antiSwayForce", 0, s.anti_sway_force);
    Some(s)
}

/// `WheeledVehicleData::preload`'s temporary instance over the client's model (as the server's preload has it, uncharged: the shape was
/// charged when it was read and the instance's work is bounded by it).
struct ClientPreload<'a> {
    shape: &'a Shape,
    inst: Option<shapes::instance::ShapeInstance>,
    thread: Option<shapes::instance::ThreadId>,
}

impl<'a> ClientPreload<'a> {
    fn new(shape: &'a Shape) -> ClientPreload<'a> {
        let mut inst = shapes::instance::ShapeInstance::new(shape, &mut |_| true).ok();
        let thread = inst.as_mut().and_then(|i| i.create_thread(shape));
        ClientPreload {
            shape,
            inst,
            thread,
        }
    }
}

impl PreloadShape for ClientPreload<'_> {
    fn find_node(&self, name: &str) -> Option<i32> {
        let n = self.shape.find_name(name.as_bytes())?;
        let i = self.shape.nodes.iter().position(|node| node.name == n)?;
        i32::try_from(i).ok()
    }

    fn find_sequence(&self, name: &str) -> Option<i32> {
        shapes::instance::find_sequence(self.shape, name.as_bytes())
            .and_then(|i| i32::try_from(i).ok())
    }

    fn animate(&mut self, seq: i32, pos: f32) {
        let (Some(inst), Some(t)) = (self.inst.as_mut(), self.thread) else {
            return;
        };
        inst.set_sequence(self.shape, t, seq, pos);
        let _ = inst.animate(self.shape, 0, None, &mut |_| true);
    }

    fn node_translation(&self, node: i32) -> F3 {
        usize::try_from(node)
            .ok()
            .and_then(|n| self.inst.as_ref()?.node_transform(n))
            .map_or([0.0; 3], |m| [m[3], m[7], m[11]])
    }
}

// ---- the driven vehicle

/// The vehicle the predicted Player drives: its ghost index, datablock, the step's data and state, its shape.
#[derive(Debug, Clone)]
pub struct DrivenVehicle {
    pub index: u32,
    pub datablock: u32,
    pub data: Rc<WheeledVehicleData>,
    /// The game's subclass of the datablock and its steering rules ([`PredictGame::wheeled`]), the type masks its step uses.
    pub class: Option<Rc<dyn WheeledClass>>,
    pub steer: SteerRules,
    pub masks: &'static collide::TypeMasks,
    pub v: Vehicle<Wheeled>,
    pub shape: Rc<ClientShape>,
    /// The ticks its ghost had left to run its last move when the drive began, for the ghost it becomes again.
    pub ghost_prediction: i32,
}

/// The object id the client's collision world gives ghost `index`.
pub fn ghost_id(index: u32) -> ObjectId {
    ObjectId(index.wrapping_add(1))
}

/// The vehicle shape a datablock names (`shapeFile`).
pub fn shape_path<W: PredictWorld>(world: &W, db: u32) -> Option<&[u8]> {
    world
        .datablock_text(db, "shapeFile")
        .filter(|p| !p.is_empty())
}

impl DrivenVehicle {
    /// The client's object of WheeledVehicle ghost `index` as TGE's client has it when it first reads its state: the datablock's
    /// `onNewDataBlock` (the rigid from the datablock, the object box from the shape) at the identity, the wheels' tires and
    /// springs from the ghost's wheel block. `None` when the ghost is not a WheeledVehicle the client can build.
    pub fn new<W: PredictWorld>(
        world: &W,
        index: u32,
        shapes: &mut ShapeCache,
        reader: Option<&FileReader>,
        work: &mut u64,
        budget: u64,
        game: &dyn PredictGame<W>,
    ) -> Option<DrivenVehicle> {
        let g = world.ghosts.get(&index)?;
        let rec = g.record.as_ref()?.vehicle().filter(|v| !v.flying)?;
        let db = g.datablock()?;
        let shape = shapes.get(shape_path(world, db)?, reader, work, budget)?;
        let data = wheeled_data(world, db, &shape)?;
        let wheeling = game.wheeled(world, db);
        let m = collide::math::IDENTITY;
        let s = VehicleState::new(ghost_id(index), &data.base, shape.bounds(), &m, [1.0; 3]);
        let mut v = Vehicle::new(s, Wheeled::default());
        wheeled::on_new_data_block(&mut v.s, &mut v.k, &data);
        // `ShapeBase::onNewDataBlock`: the recharge rate is the datablock's (the energy step adds it every tick).
        v.s.recharge_rate = float(world, db, "rechargeRate", 0, 0.0);
        game.new_wheeled(&mut v.s);
        if let Some(wheels) = &rec.wheel_datablocks {
            for (w, b) in v.k.wheels.iter_mut().zip(wheels) {
                let Some(b) = b else { continue };
                w.tire = tire_data(world, b.tire).map(|t| (ObjectId(b.tire), t));
                w.spring = spring_data(world, b.spring).map(|s| (ObjectId(b.spring), s));
            }
        }
        Some(DrivenVehicle {
            index,
            datablock: db,
            data: Rc::new(data),
            class: wheeling.class,
            steer: wheeling.steer,
            masks: game.type_masks(),
            v,
            shape,
            ghost_prediction: 0,
        })
    }

    /// The wheels' tires and springs as the ghost's last wheel block set them (`WheeledVehicle::unpackUpdate`).
    pub fn refresh_wheels<W: PredictWorld>(&mut self, world: &W) {
        let Some(rec) = world
            .ghosts
            .get(&self.index)
            .and_then(|g| g.record.as_ref())
            .and_then(EngineRecord::vehicle)
            .filter(|v| !v.flying)
        else {
            return;
        };
        let Some(wheels) = &rec.wheel_datablocks else {
            return;
        };
        for (w, b) in self.v.k.wheels.iter_mut().zip(wheels) {
            match b {
                Some(b) => {
                    if w.tire.map(|t| t.0) != Some(ObjectId(b.tire)) {
                        w.tire = tire_data(world, b.tire).map(|t| (ObjectId(b.tire), t));
                    }
                    if w.spring.map(|t| t.0) != Some(ObjectId(b.spring)) {
                        w.spring = spring_data(world, b.spring).map(|s| (ObjectId(b.spring), s));
                    }
                }
                None => {
                    w.tire = None;
                    w.spring = None;
                }
            }
        }
    }

    /// `Vehicle::readPacketData` then `WheeledVehicle::readPacketData`; packet data whose state is not finite is ignored.
    pub fn read_packet_data(&mut self, p: &VehicleWords, world: &mut ClientCollision) {
        if !finite_state(&[
            &p.position,
            &p.rotation,
            &p.lin_mom,
            &p.ang_mom,
            &p.steering,
        ]) {
            return;
        }
        let mut host = ClientVehicleHost {
            world,
            sim_time: 0,
            class: self.class.clone(),
            masks: self.masks,
        };
        let s = &mut *self.v.s;
        s.set_energy_level(p.energy, &self.data.base.shape, &mut host);
        s.steering = p.steering;
        let r = &mut s.rigid;
        r.lin_pos = p.position;
        r.ang_pos = p.rotation;
        r.lin_mom = p.lin_mom;
        r.ang_mom = p.ang_mom;
        r.at_rest = p.at_rest;
        if p.contacts_zero {
            s.contacts_count = 0;
        }
        let r = &mut s.rigid;
        r.update_inertial_tensor();
        r.update_velocity();
        r.update_center_of_mass();
        s.disable_move = p.disable_move;
        if let vehicle::words::KindWords::Wheeled { brake, wheels } = &p.kind {
            self.v.k.brake = *brake;
            for (dst, src) in self.v.k.wheels.iter_mut().zip(wheels) {
                if finite_state(&[&[src.avel, src.dy, src.dx]]) {
                    dst.avel = src.avel;
                    dst.dy = src.dy;
                    dst.dx = src.dx;
                }
                dst.slipping = src.slipping;
            }
        }
        let s = &mut *self.v.s;
        let (pos, rot) = (s.rigid.lin_pos, s.rigid.ang_pos);
        s.set_position(pos, &rot, &mut host);
        s.delta_pos = pos;
        s.delta_rot[1] = rot;
    }

    /// The vehicle's `writePacketData` (a WheeledVehicle's) as set `S`'s packet data.
    pub fn packet_data<S: EngineSet>(&self, world: &mut ClientCollision) -> Box<dyn ControlData> {
        let mut host = ClientVehicleHost {
            world,
            sim_time: 0,
            class: self.class.clone(),
            masks: self.masks,
        };
        let w = wheeled::packet_words(&self.v, &self.data, &mut host);
        S::vehicle_data(&w, self.v.s.position())
    }

    /// One tick of the vehicle with the driver's move (`None`: the null move).
    pub fn tick(
        &mut self,
        mv: Option<&vehicle::Move>,
        world: &mut ClientCollision,
        sim_time: u32,
    ) -> bool {
        let mut host = ClientVehicleHost {
            world,
            sim_time,
            class: self.class.clone(),
            masks: self.masks,
        };
        vehicle::process_tick(&mut self.v, &self.data, mv, &mut host).is_ok()
    }
}

/// The vehicle step's host on the client: its container, the clock, rays; no script.
pub struct ClientVehicleHost<'a> {
    pub world: &'a mut ClientCollision,
    pub sim_time: u32,
    /// The vehicle's subclass (the game's), for its step, and the type masks the step uses.
    pub class: Option<Rc<dyn WheeledClass>>,
    pub masks: &'static collide::TypeMasks,
}

impl VehicleHost for ClientVehicleHost<'_> {
    fn type_masks(&self) -> &collide::TypeMasks {
        self.masks
    }

    fn wheeled_class(&self) -> Option<Rc<dyn vehicle::wheeled::WheeledClass>> {
        self.class.clone()
    }

    fn world(&mut self) -> &mut dyn collide::World {
        self.world
    }

    fn sim_time(&self) -> u32 {
        self.sim_time
    }

    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<RayHit> {
        self.world.cast_ray(start, end, mask).map(|h| RayHit {
            t: h.t,
            point: h.point,
            normal: h.normal,
            material: 0,
            object: Some(h.object),
        })
    }

    fn moved(&mut self, s: &mut VehicleState) {
        self.world.set_view(s.id, s.object_view());
    }
}

/// The vehicle's object view for the client's container (its last pose the server sent, or the predicted one).
pub fn vehicle_view(m: &Matrix, shape: &ClientShape) -> collide::ObjectView {
    super::view_of(
        *m,
        shape.bounds(),
        [1.0; 3],
        collide::types::VEHICLE | collide::types::SHAPE_BASE,
    )
}

// ---- the vehicles nobody on this client drives

/// A ghost's warp state (`mDelta` as the engine keeps it, module docs): the frame fraction, the warp's ticks and count, the
/// offset of the drawn pose from the state at the warp's first tick, the rotations it turns between, the ticks left to run the
/// last move; and the updates' age (seconds, eased over updates), the whole ticks of it run at the warp's first tick and the
/// rest the drawn pose leads the state by along its velocity.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Warp {
    pub dt: f32,
    pub ticks: i32,
    pub count: i32,
    pub offset: F3,
    pub rot: [[f32; 4]; 2],
    pub prediction: i32,
    pub age: Option<f32>,
    pub catch_up: i32,
    pub lead: f32,
}

/// `sMaxPredictionTicks`: a ghost runs its last move this many ticks after an update.
pub const MAX_PREDICTION_TICKS: i32 = 30;
/// The ticks a ghost coasts after an update before its physics runs again.
pub const GHOST_WARP_TICKS: i32 = 5;
/// The share of a ghost's drawn offset from its state left after each tick (a new update's offset, taken from where the
/// vehicle is drawn, goes on from there at the same pace).
pub const GHOST_OFFSET_KEEP: f32 = 0.7;
/// The farthest (units) a ghost's drawn pose eases to its state from; past it (a teleport, a hostile update) it is drawn there
/// at once.
pub const GHOST_SNAP: f32 = 10.0;
/// The oldest an update is taken to be (seconds): no ghost is run on further than this past its update, besides its prediction.
pub const MAX_UPDATE_AGE: f32 = 0.1;
/// The share of each update's measured age taken into the eased age.
pub const AGE_EASE: f32 = 0.2;
/// The least speed (units a second) an update's age is measured at.
pub const AGE_MIN_SPEED: f32 = 1.0;
/// The farthest (units) and the most turn (radians) the update's age runs a ghost on by.
pub const AGE_MAX_TRAVEL: f32 = 3.0;
pub const AGE_MAX_TURN: f32 = 0.5;
/// A tick (seconds).
const TICK_S: f32 = 0.032;

/// A vehicle the client runs on its own (a ghost of a vehicle it does not drive): its step's object, and the warp.
#[derive(Debug, Clone)]
pub struct GhostVehicle {
    pub body: DrivenVehicle,
    pub warp: Warp,
}

fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `Point3F::len`: `mSqrt(f32((x*x + y*y) + z*z))`.
fn len3(v: F3) -> f32 {
    collide::math::sqrt_f32(((d(v[0]) * d(v[0]) + d(v[1]) * d(v[1])) + d(v[2]) * d(v[2])) as f32)
}

/// The threshold (1e-5, a double): below it `1 - cos` takes the linear blend.
const SLERP_LINEAR: f64 = 1.0e-5;

/// `QuatF::interpolate(q1, q2, t)`: the slerp in double precision, each component stored as a float.
pub fn quat_slerp(q1: [f32; 4], q2: [f32; 4], t: f32) -> [f32; 4] {
    let mut cos =
        ((d(q1[1]) * d(q2[1]) + d(q1[0]) * d(q2[0])) + d(q1[2]) * d(q2[2])) + d(q1[3]) * d(q2[3]);
    let sign = if 0.0 > cos {
        cos = -cos;
        -1.0f64
    } else {
        1.0
    };
    let td = d(t);
    let (s1, s2) = if 1.0 - cos <= SLERP_LINEAR {
        (1.0 - td, sign * td)
    } else {
        let omega = collide::math::transcendental::acos(cos);
        let sin_omega = collide::math::transcendental::sin(omega);
        (
            collide::math::transcendental::sin(omega * (1.0 - td)) / sin_omega,
            collide::math::transcendental::sin(omega * td) * sign / sin_omega,
        )
    };
    std::array::from_fn(|i| (d(q1[i]) * s1 + d(q2[i]) * s2) as f32)
}

/// Whether every value the server sent is finite.
pub fn finite_state(values: &[&[f32]]) -> bool {
    values.iter().all(|v| v.iter().all(|x| x.is_finite()))
}

impl GhostVehicle {
    /// The client object of a WheeledVehicle ghost when it first sees it: its datablock vehicle at the pose its creating record
    /// carried (`unpackUpdate` before the object is added: the snap).
    pub fn new<W: PredictWorld>(
        world: &W,
        index: u32,
        shapes: &mut ShapeCache,
        reader: Option<&FileReader>,
        work: &mut u64,
        budget: u64,
        game: &dyn PredictGame<W>,
    ) -> Option<GhostVehicle> {
        let body = DrivenVehicle::new(world, index, shapes, reader, work, budget, game)?;
        let mut g = GhostVehicle {
            body,
            warp: Warp::default(),
        };
        let gh = world.ghosts.get(&index)?;
        if let Some(rec) = gh
            .record
            .as_ref()
            .and_then(EngineRecord::vehicle)
            .filter(|v| !v.flying)
        {
            g.unpack(&rec, gh.position, false, &mut ClientCollision::empty());
        }
        Some(g)
    }

    /// The vehicle the client drove, as the ordinary ghost it is again once its Player drives it no longer: the state the
    /// client predicted, its last tick's interpolation (what is drawn) and its position in the container (what is collided with)
    /// kept, no warp running, the prediction ticks its ghost had left; `dt` the frame fraction the next update's warp reads.
    pub fn released(body: DrivenVehicle, dt: f32) -> GhostVehicle {
        let warp = Warp {
            dt,
            rot: [body.v.s.rigid.ang_pos; 2],
            prediction: body.ghost_prediction,
            ..Warp::default()
        };
        GhostVehicle { body, warp }
    }

    /// `Vehicle::unpackUpdate` and the WheeledVehicle part of one update for a vehicle the client does not
    /// control. `added`: the object is in the client world; a first record snaps.
    pub fn unpack(
        &mut self,
        rec: &VehicleUpdate,
        position: Option<F3>,
        added: bool,
        world: &mut ClientCollision,
    ) {
        if rec.controlled {
            return;
        }
        let db = Rc::clone(&self.body.data);
        let s = &mut *self.body.v.s;
        // The steering: two 9-bit fractions of the datablock maximum, first.
        let max = db.base.max_steering_angle;
        for (k, v) in rec.steering.iter().enumerate() {
            let q = (f64::from((v & 0x1ff) as f32) / 511.0) as f32;
            s.steering[k] = ((d(q) + d(q)) * d(max) - d(max)) as f32;
        }
        let m = &rec.last_move;
        s.last_move = vehicle::Move {
            x: m.x,
            y: m.y,
            z: m.z,
            yaw: m.yaw,
            pitch: m.pitch,
            roll: m.roll,
            free_look: m.free_look,
            triggers: m.triggers,
        };
        // A pose the server sent with a value that is not finite is left out (the vehicle goes on as it was).
        let pose = rec
            .pose
            .as_ref()
            .zip(position)
            .filter(|(p, pos)| finite_state(&[pos, &p.rotation, &p.linear, &p.angular]))
            // A rotation of no length (or one whose length overflows) turns nothing into a rotation: left out too.
            .filter(|(p, _)| {
                let n: f32 = p.rotation.iter().map(|c| c * c).sum();
                n.is_finite() && n > 1e-12
            });
        if let Some((pose, pos)) = pose {
            self.warp.prediction = MAX_PREDICTION_TICKS;
            self.warp.rot[0] = s.rigid.ang_pos;
            let r = &mut s.rigid;
            r.lin_pos = pos;
            r.ang_pos = pose.rotation;
            r.lin_mom = pose.linear;
            r.ang_mom = pose.angular;
            r.at_rest = pose.at_rest;
            r.update_velocity();
            // Momenta the body's mass and inertia turn into velocities that are not finite: the body at rest.
            if !finite_state(&[&r.lin_vel, &r.ang_vel]) {
                r.lin_mom = [0.0; 3];
                r.ang_mom = [0.0; 3];
                r.update_velocity();
            }
            let mut host = ClientVehicleHost {
                world,
                sim_time: 0,
                class: self.body.class.clone(),
                masks: self.body.masks,
            };
            if !pose.warp && added {
                // Engine addition (module docs): the drawn pose goes on as it was until the next tick, which runs the update's
                // state on to where it is now and warps the drawn pose to it. The update's age: how far ahead of it along
                // its way the vehicle is drawn now, over its speed, eased over updates (the network's delay, not one update's
                // error), at most `MAX_UPDATE_AGE`.
                let w = &mut self.warp;
                let dt = w.dt;
                let cp: F3 = std::array::from_fn(|i| s.delta_pos[i] + s.delta_pos_vec[i] * dt);
                let v = s.rigid.lin_vel;
                let speed = len3(v);
                let turning = len3(s.rigid.ang_vel);
                let measured = (speed.is_finite() && turning.is_finite() && speed > AGE_MIN_SPEED)
                    .then(|| (0..3).map(|i| (cp[i] - pos[i]) * v[i]).sum::<f32>() / speed)
                    .map(|ahead| ahead / speed)
                    .filter(|age| age.is_finite());
                if let Some(age) = measured {
                    let age = age.clamp(0.0, MAX_UPDATE_AGE);
                    let eased = w.age.map_or(age, |a| a + (age - a) * AGE_EASE);
                    w.age = eased.is_finite().then_some(eased).or(w.age);
                }
                // At most `AGE_MAX_TRAVEL` along its way and `AGE_MAX_TURN` round (a hostile speed or spin runs it on by
                // nothing).
                let age = w.age.filter(|a| a.is_finite()).unwrap_or(0.0);
                let age = if speed.is_finite() && turning.is_finite() {
                    age.min(AGE_MAX_TRAVEL / speed.max(1e-6))
                        .min(AGE_MAX_TURN / turning.max(1e-6))
                } else {
                    0.0
                };
                w.catch_up = (age / TICK_S).floor() as i32;
                w.lead = age - w.catch_up as f32 * TICK_S;
                w.count = 0;
                w.ticks = GHOST_WARP_TICKS;
                w.rot = [s.delta_rot[1], s.rigid.ang_pos];
            } else {
                s.delta_pos = s.rigid.lin_pos;
                self.warp.dt = 0.0;
                s.delta_pos_vec = [0.0; 3];
                s.delta_rot = [s.rigid.ang_pos; 2];
                self.warp.ticks = 0;
                self.warp.count = 0;
                // Drawn at the update's pose: no offset left of an earlier update's warp.
                self.warp.offset = [0.0; 3];
                self.warp.rot[0] = [0.0, 0.0, 0.0, 1.0];
                let (p, q) = (s.rigid.lin_pos, s.rigid.ang_pos);
                s.set_position(p, &q, &mut host);
            }
            s.rigid.update_center_of_mass();
        }
        self.body.v.k.brake = rec.brake;
        if let Some(wheels) = &rec.wheels {
            for (dst, src) in self.body.v.k.wheels.iter_mut().zip(wheels) {
                if finite_state(&[&[src.avel, src.dy, src.dx]]) {
                    dst.avel = src.avel;
                    dst.dy = src.dy;
                    dst.dx = src.dx;
                }
                dst.slipping = src.slipping;
            }
        }
    }

    /// `WheeledVehicle::processTick(NULL)` of a ghost: `ShapeBase::processTick`, then the next step of the warp, else the
    /// last move run while the prediction lasts.
    pub fn tick(&mut self, world: &mut ClientCollision, sim_time: u32) {
        let s = &self.body.v.s;
        let kept = (s.rigid, s.delta_pos, s.delta_rot);
        self.tick_state(world, sim_time);
        let s = &mut *self.body.v.s;
        let r = &s.rigid;
        if finite_state(&[
            &r.lin_pos,
            &r.ang_pos,
            &r.lin_vel,
            &r.ang_vel,
            &s.delta_pos,
            &s.delta_pos_vec,
            &s.delta_rot[0],
            &s.delta_rot[1],
        ]) {
            return;
        }
        // A state that is not finite (hostile values the update's checks let through): the tick undone, the body at rest where
        // it was drawn, no warp and no prediction until the next update.
        s.rigid = kept.0;
        s.rigid.lin_mom = [0.0; 3];
        s.rigid.ang_mom = [0.0; 3];
        s.rigid.update_velocity();
        s.delta_pos = kept.1;
        s.delta_pos_vec = [0.0; 3];
        s.delta_rot = [kept.2[1]; 2];
        self.warp.count = self.warp.ticks;
        self.warp.prediction = 0;
        let mut host = ClientVehicleHost {
            world,
            sim_time,
            class: self.body.class.clone(),
            masks: self.body.masks,
        };
        let (p, q) = (s.rigid.lin_pos, s.rigid.ang_pos);
        if finite_state(&[&p, &q]) {
            s.set_position(p, &q, &mut host);
        }
    }

    /// [`GhostVehicle::tick`]'s step, before its check.
    fn tick_state(&mut self, world: &mut ClientCollision, sim_time: u32) {
        let db = Rc::clone(&self.body.data);
        let mut host = ClientVehicleHost {
            world,
            sim_time,
            class: self.body.class.clone(),
            masks: self.body.masks,
        };
        vehicle::state::shape_base_tick(&mut self.body.v.s, &db.base.shape, None, &mut host);
        // Engine addition (module docs). Through the warp no physics runs (TGE's), but the state coasts on at its velocities
        // (the warp's first tick also by the whole ticks of the update's age), so the physics after the warp goes on from where
        // the vehicle is; the drawn pose is the state `lead` seconds on along its velocity, plus what is left of the offset from
        // where it would have been drawn (position and turn), shrinking each tick.
        let (from_pos, from_rot) = (self.body.v.s.delta_pos, self.body.v.s.delta_rot[1]);
        let w = &mut self.warp;
        let s = &mut *self.body.v.s;
        if w.count < w.ticks {
            let steps = if w.count == 0 {
                1 + w.catch_up.clamp(0, 3)
            } else {
                1
            };
            w.count += 1;
            let mut before = s.rigid.ang_pos;
            for _ in 0..steps {
                before = s.rigid.ang_pos;
                s.rigid.force = [0.0; 3];
                s.rigid.torque = [0.0; 3];
                s.rigid.integrate(TICK_S);
            }
            let (p, q) = (s.rigid.lin_pos, s.rigid.ang_pos);
            s.set_position(p, &q, &mut host);
            let vel = s.rigid.lin_vel;
            let led: F3 = std::array::from_fn(|i| p[i] + vel[i] * w.lead);
            if w.count == 1 {
                // The offsets of where it would have been drawn this tick from the state: the position, and the turn that takes
                // the state's rotation a tick before to the drawn one (the state turns on by its own this tick). Past
                // `GHOST_SNAP`, none.
                w.offset = std::array::from_fn(|i| from_pos[i] + vel[i] * TICK_S - led[i]);
                w.rot[0] = quat_mul(&quat_inverse(&before), &from_rot);
                let far = len3(w.offset);
                if far.is_nan() || far > GHOST_SNAP {
                    w.offset = [0.0; 3];
                    w.rot[0] = [0.0, 0.0, 0.0, 1.0];
                }
            }
            let left = GHOST_OFFSET_KEEP.powi(w.count);
            s.delta_pos = std::array::from_fn(|i| led[i] + w.offset[i] * left);
            let turn = quat_slerp([0.0, 0.0, 0.0, 1.0], w.rot[0], left);
            s.delta_rot = [from_rot, quat_mul(&q, &turn)];
            s.delta_pos_vec = std::array::from_fn(|i| from_pos[i] - s.delta_pos[i]);
            return;
        }
        let left = w.prediction;
        w.prediction -= 1;
        if left < 1 {
            return;
        }
        // What is left of the offset goes on shrinking after the warp.
        w.count = w.count.saturating_add(1);
        let (lead, keep, offset, turn) = (
            w.lead,
            GHOST_OFFSET_KEEP.powi(w.count.min(64)),
            w.offset,
            w.rot[0],
        );
        let mv = s.last_move;
        let _ = vehicle::tick::process_tick_rest(&mut self.body.v, &db, Some(&mv), &mut host);
        let s = &mut *self.body.v.s;
        let (p, vel, q) = (s.rigid.lin_pos, s.rigid.lin_vel, s.rigid.ang_pos);
        s.delta_pos = std::array::from_fn(|i| p[i] + vel[i] * lead + offset[i] * keep);
        s.delta_rot = [
            from_rot,
            quat_mul(&q, &quat_slerp([0.0, 0.0, 0.0, 1.0], turn, keep)),
        ];
        s.delta_pos_vec = std::array::from_fn(|i| from_pos[i] - s.delta_pos[i]);
    }

    /// The frame after a tick (`interpolateTick`): the warp state keeps the fraction the next update reads.
    pub fn frame(&mut self, dt: f32) {
        self.warp.dt = dt;
    }
}
