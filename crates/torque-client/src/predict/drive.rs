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
//! meshes).
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
        (0..SLOTS)
            .map(|i| {
                let b = self.facts.bounds[i];
                HullSlot {
                    slot: i as u32,
                    present: self.facts.slots.collision[i] >= 0,
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
        while self.next_slot < SLOTS && facts.slots.collision[self.next_slot] < 0 {
            self.next_slot += 1;
        }
        if self.next_slot < SLOTS {
            let detail = facts.slots.collision[self.next_slot];
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
        let detail = facts.slots.collision[slot];
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

    /// `Vehicle::readPacketData` then `WheeledVehicle::readPacketData`.
    pub fn read_packet_data(&mut self, p: &VehicleWords, world: &mut ClientCollision) {
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
                dst.avel = src.avel;
                dst.dy = src.dy;
                dst.dx = src.dx;
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

/// `Vehicle::unpackUpdate`'s warp state of a ghost (`mDelta`): the frame fraction, the warp's ticks and count, its offset per
/// tick and rotations, the ticks left to run the last move.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Warp {
    pub dt: f32,
    pub ticks: i32,
    pub count: i32,
    pub offset: F3,
    pub rot: [[f32; 4]; 2],
    pub prediction: i32,
}

/// `sMaxPredictionTicks`: a ghost runs its last move this many ticks after an update.
pub const MAX_PREDICTION_TICKS: i32 = 30;
/// `sMaxWarpTicks` (compared as a double, stored as a float).
pub const MAX_WARP_TICKS: f32 = 3.0;

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

/// The warp's rotation extrapolation: `cr` the rotation at the frame, `r1` the new end; `om = acos(dot) / dt`.
fn quat_extrapolate(r1: [f32; 4], cr: [f32; 4], dt: f32) -> [f32; 4] {
    let mut dot =
        ((d(cr[1]) * d(r1[1]) + d(cr[0]) * d(r1[0])) + d(cr[2]) * d(r1[2])) + d(r1[3]) * d(cr[3]);
    let sign = if 0.0 > dot {
        dot = -dot;
        -1.0f64
    } else {
        1.0
    };
    let dtd = d(dt);
    let (a, b) = if 1.0 - dot <= SLERP_LINEAR {
        (sign / dtd, (1.0 - dtd) / dtd)
    } else {
        let om = collide::math::transcendental::acos(dot) / dtd;
        let inv = 1.0 / collide::math::transcendental::sin(om * dtd);
        (
            collide::math::transcendental::sin(om) * sign * inv,
            collide::math::transcendental::sin(om * (1.0 - dtd)) * inv,
        )
    };
    std::array::from_fn(|i| (d(cr[i]) * a - d(r1[i]) * b) as f32)
}

/// `vec *= f32(1 / f)` per component.
fn scale_inv(v: F3, f: f32) -> F3 {
    let inv = (1.0 / d(f)) as f32;
    v.map(|x| (d(x) * d(inv)) as f32)
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
        if let (Some(pose), Some(pos)) = (&rec.pose, position) {
            self.warp.prediction = MAX_PREDICTION_TICKS;
            let old_vel = s.rigid.lin_vel;
            self.warp.rot[0] = s.rigid.ang_pos;
            let r = &mut s.rigid;
            r.lin_pos = pos;
            r.ang_pos = pose.rotation;
            r.lin_mom = pose.linear;
            r.ang_mom = pose.angular;
            r.at_rest = pose.at_rest;
            r.update_velocity();
            let mut host = ClientVehicleHost {
                world,
                sim_time: 0,
                class: self.body.class.clone(),
                masks: self.body.masks,
            };
            if !pose.warp && added {
                let w = &mut self.warp;
                let dt = w.dt;
                let cp: F3 = std::array::from_fn(|i| {
                    (d((d(s.delta_pos_vec[i]) * d(dt)) as f32) + d(s.delta_pos[i])) as f32
                });
                w.offset = std::array::from_fn(|i| (d(s.rigid.lin_pos[i]) - d(cp[i])) as f32);
                let old_speed = len3(old_vel);
                let speed =
                    (((d(len3(s.rigid.lin_vel)) + d(old_speed)) * 0.5) * d(0.032f32)) as f32;
                let dtd = d(dt);
                let result: f64 = if speed == 0.0 {
                    dtd + 3.0
                } else {
                    let dtw = (d(len3(w.offset)) / d(speed)) as f32;
                    if dtw > MAX_WARP_TICKS {
                        dtd + 3.0
                    } else if dtd < d(dtw) {
                        let c = (d(dtw) - dtd) as f32;
                        let c = d(c).ceil() as f32;
                        d(c) + dtd
                    } else {
                        dtd
                    }
                };
                let result = result as f32;
                if dt != 0.0 {
                    let f = (dtd / d(result)) as f32;
                    let np: F3 = std::array::from_fn(|i| {
                        (d((d(w.offset[i]) * d(f)) as f32) + d(cp[i])) as f32
                    });
                    s.delta_pos = np;
                    let diff: F3 = std::array::from_fn(|i| (d(cp[i]) - d(np[i])) as f32);
                    s.delta_pos_vec = scale_inv(diff, dt);
                    let cr = quat_slerp(s.delta_rot[1], s.delta_rot[0], dt);
                    s.delta_rot[1] = quat_slerp(cr, s.rigid.ang_pos, f);
                    s.delta_rot[0] = quat_extrapolate(s.delta_rot[1], cr, dt);
                }
                w.count = 0;
                w.ticks = collide::math::float_to_int(d(d(result).floor() as f32));
                if w.ticks != 0 {
                    let off: F3 =
                        std::array::from_fn(|i| (d(s.rigid.lin_pos[i]) - d(s.delta_pos[i])) as f32);
                    w.offset = scale_inv(off, w.ticks as f32);
                    w.rot = [s.delta_rot[1], s.rigid.ang_pos];
                }
            } else {
                s.delta_pos = s.rigid.lin_pos;
                self.warp.dt = 0.0;
                s.delta_pos_vec = [0.0; 3];
                s.delta_rot = [s.rigid.ang_pos; 2];
                self.warp.ticks = 0;
                self.warp.count = 0;
                let (p, q) = (s.rigid.lin_pos, s.rigid.ang_pos);
                s.set_position(p, &q, &mut host);
            }
            s.rigid.update_center_of_mass();
        }
        self.body.v.k.brake = rec.brake;
        if let Some(wheels) = &rec.wheels {
            for (dst, src) in self.body.v.k.wheels.iter_mut().zip(wheels) {
                dst.avel = src.avel;
                dst.dy = src.dy;
                dst.dx = src.dx;
                dst.slipping = src.slipping;
            }
        }
    }

    /// `WheeledVehicle::processTick(NULL)` of a ghost: `ShapeBase::processTick`, then the next step of the warp, else the
    /// last move run while the prediction lasts.
    pub fn tick(&mut self, world: &mut ClientCollision, sim_time: u32) {
        let db = Rc::clone(&self.body.data);
        let mut host = ClientVehicleHost {
            world,
            sim_time,
            class: self.body.class.clone(),
            masks: self.body.masks,
        };
        vehicle::state::shape_base_tick(&mut self.body.v.s, &db.base.shape, None, &mut host);
        let w = &mut self.warp;
        let s = &mut *self.body.v.s;
        if w.count < w.ticks {
            w.count += 1;
            let t = s.obj_to_world;
            s.delta_pos = [
                (d(t[3]) + d(w.offset[0])) as f32,
                (d(t[7]) + d(w.offset[1])) as f32,
                (d(t[11]) + d(w.offset[2])) as f32,
            ];
            s.delta_rot[0] = s.delta_rot[1];
            let frac = (f64::from(w.count) / f64::from(w.ticks)) as f32;
            s.delta_rot[1] = quat_slerp(w.rot[0], w.rot[1], frac);
            let (p, q) = (s.delta_pos, s.delta_rot[1]);
            s.set_position(p, &q, &mut host);
            s.delta_pos_vec = w.offset.map(|v| -v);
            return;
        }
        let left = w.prediction;
        w.prediction -= 1;
        if left < 1 {
            return;
        }
        let mv = s.last_move;
        let _ = vehicle::tick::process_tick_rest(&mut self.body.v, &db, Some(&mv), &mut host);
    }

    /// The frame after a tick (`interpolateTick`): the warp state keeps the fraction the next update reads.
    pub fn frame(&mut self, dt: f32) {
        self.warp.dt = dt;
    }
}
