//! `processTick`, written once: `Vehicle::processTick` (the FlyingVehicle's) and `WheeledVehicle::processTick`
//! are the same but for the sub-step they call and where they read the datablock; both
//! read `integration` from the same datablock object. [`Kind`] supplies `updateMove` and the sub-step.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/vehicle.cpp` (Copyright (c)
//! 2012 GarageGames, LLC); the code here is this crate's own.

use torque_collide::{Exhausted, ObjectId};

use crate::Move;
use crate::data::VehicleData;
use crate::host::{ScanHit, VehicleHost};
use crate::math::quat_rotated;
use crate::state::{Body, VehicleState, net_mask, shape_base_tick};

/// The cap on the sub-steps one tick runs. `integration` is otherwise unclamped (only 0 skips): a negative count
/// would run about four billion sub-steps. The step size is still `0.032 / N` (negative for a negative N); only the
/// count is capped. Typical vehicles use 1..4.
pub const MAX_SUBSTEPS: u32 = 64;
/// Work units charged per sub-step (the collision queries charge their own).
pub const SUBSTEP_WORK: u64 = 64;

/// The tick (`TickSec`), in seconds.
pub const DT: f32 = 0.032;

/// A vehicle class: its datablock, its `updateMove` and its sub-step.
pub trait Kind: Sized {
    type Data;
    /// The base `VehicleData` of the datablock.
    fn base(db: &Self::Data) -> &VehicleData;
    /// `updateMove(mv)`; `None` is the null move.
    fn update_move(
        v: &mut Vehicle<Self>,
        db: &Self::Data,
        mv: Option<&Move>,
        host: &mut dyn VehicleHost,
    );
    /// One integration sub-step of `dt`.
    fn step(
        v: &mut Vehicle<Self>,
        db: &Self::Data,
        dt: f32,
        host: &mut dyn VehicleHost,
    ) -> Result<(), Exhausted>;
}

/// A vehicle: the shared state, the class's part, the body's collision state and the scan's scratch.
#[derive(Debug, Clone)]
pub struct Vehicle<K> {
    pub s: Box<VehicleState>,
    pub k: K,
    pub body: Box<Body>,
    /// Scratch for the end-of-step scan (`findObjects`' hits, reused tick to tick).
    pub scan: Vec<ScanHit>,
}

impl<K> Vehicle<K> {
    pub fn new(s: VehicleState, k: K) -> Vehicle<K> {
        Vehicle {
            s: Box::new(s),
            k,
            body: Box::default(),
            scan: Vec::new(),
        }
    }

    pub fn id(&self) -> ObjectId {
        self.s.id
    }
}

/// The sub-step count and size of `processTick`: `None` for 0 (no sub-step); else the count (capped at
/// [`MAX_SUBSTEPS`]; a negative N counts as unsigned, capped) and `dt = 0.032 / N`.
pub fn substeps(n: i32) -> Option<(u32, f32)> {
    if n == 0 {
        return None;
    }
    let a = n as f32;
    let dt = DT / a;
    Some(((n as u32).min(MAX_SUBSTEPS), dt))
}

/// `processTick(mv)` for a server vehicle; `None` is a tick with no move (the null move). A spent work budget
/// stops the tick where it was (deterministic; the state is as far as it got).
pub fn process_tick<K: Kind>(
    v: &mut Vehicle<K>,
    db: &K::Data,
    mv: Option<&Move>,
    host: &mut dyn VehicleHost,
) -> Result<(), Exhausted> {
    let base = K::base(db);
    shape_base_tick(&mut v.s, &base.shape, mv, host);
    process_tick_rest(v, db, mv, host)
}

/// [`process_tick`] after `ShapeBase::processTick`: the move, the integration and the deltas (a client's ghost runs its warp between
/// the two).
pub fn process_tick_rest<K: Kind>(
    v: &mut Vehicle<K>,
    db: &K::Data,
    mv: Option<&Move>,
    host: &mut dyn VehicleHost,
) -> Result<(), Exhausted> {
    K::update_move(v, db, mv, host);
    v.s.delta_pos_vec = v.s.rigid.lin_pos;
    v.s.delta_rot[0] = v.s.rigid.ang_pos;
    let n = K::base(db).integration;
    let mut result = v.body.update_working_set(&v.s, host);
    if result.is_ok()
        && let Some((count, dt)) = substeps(n)
    {
        for _ in 0..count {
            if !host.world().charge(SUBSTEP_WORK) {
                result = Err(Exhausted);
                break;
            }
            if let Err(e) = K::step(v, db, dt, host) {
                result = Err(e);
                break;
            }
        }
    }
    let s = &mut v.s;
    let p = s.rigid.lin_pos;
    s.delta_pos = p;
    for i in 0..3 {
        s.delta_pos_vec[i] -= p[i];
    }
    s.delta_rot[1] = s.rigid.ang_pos;
    let q = s.rigid.ang_pos;
    s.set_position(p, &q, host);
    let pv = s.delta_pos_vec;
    let len = crate::math::len_sq(pv).sqrt();
    if len > 0.001 || quat_rotated(&s.delta_rot[0], &s.delta_rot[1]) {
        s.set_mask(net_mask::POSE);
    }
    host.update_container(s, &K::base(db).shape);
    result
}
