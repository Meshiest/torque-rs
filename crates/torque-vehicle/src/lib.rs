//! The vehicles of the server step: the rigid body and the
//! Wheeled and Flying vehicles, as pure state and steps over `collide`. Std only.
//!
//! - [`rigid`]: `Rigid` and [`math`]: the quaternion and matrix helpers it calls.
//! - [`state`]: the vehicle's state and the base `Vehicle` code both classes share.
//! - [`tick`]: `processTick`, written once and generic over the class ([`tick::Kind`]).
//! - [`wheeled`]: `WheeledVehicle` (wheels, tires, springs, `updateForces`, `extendWheels`), and the
//!   subclass seam ([`wheeled::WheeledClass`]); [`ext`]: the per-object fields a subclass adds.
//! - [`flying`]: `FlyingVehicle` (`updateForces`, `updateMove`, `useCreateHeight`).
//! - [`words`]: `writePacketData` (both classes) as plain words.
//! - [`host`]: what the host provides; [`run`]: running a vehicle over a sequence of moves.
//! - [`library`]: the table of routines whose arithmetic a host may replace ([`library::VehicleLibrary`]); this crate's own is
//!   plain `f32` arithmetic in the order Torque's source writes it.
//!
//! The rigid body and the vehicles follow TGE 1.4's `game/rigid.cc` and `game/vehicles`; each module names the Torque3D file it
//! follows where Torque3D has the same rules, with that file's copyright lines.
//!
//! **The driver.** `torque_sim::player`'s `processTick` builds the vehicle's move and hands it to its host's `tick_controlled`,
//! which calls [`tick::process_tick`] with `Some(&move)`; a disabled driver hands `None`, the null move, as does the process
//! list for a vehicle nobody drives. A driven vehicle is not ticked by the process list, only by its driver.
//!
//! **Caps.** [`tick::MAX_SUBSTEPS`] bounds `integration`, which the datablock leaves unclamped;
//! [`state::MAX_COLLISION_QUEUE`] bounds the collision list; fixed limits are 200 entries and 50 passes in
//! `resolveCollision`, 64 collisions, 8 wheels and 16 GJK iterations; `collide`'s caps bound the sets.

// Comparisons are written so that a NaN takes a fixed side: a negated one is the intended form (`!(a >= b)` is "below, or
// unordered"), as in `collide` and `sim`.
#![allow(clippy::neg_cmp_op_on_partial_ord)]
// The per-component loops index several vectors with one axis, as the x, y, z stores do.
#![allow(clippy::needless_range_loop)]

pub mod data;
pub mod ext;
pub mod flying;
pub mod host;
pub mod library;
pub mod math;
pub mod rigid;
pub mod run;
pub mod state;
pub mod tick;
pub mod wheeled;
pub mod words;

pub use data::{FlyingVehicleData, VehicleData, WheeledVehicleData};
pub use host::VehicleHost;
pub use rigid::Rigid;
pub use state::VehicleState;
pub use tick::{MAX_SUBSTEPS, Vehicle, process_tick};

/// The triggers a move carries (`MaxTriggerKeys`).
pub const MAX_TRIGGERS: usize = 6;

/// One move as `processTick` reads it (the same fields as `torque_sim::Move`; `vehicle` does not depend on `sim`, so the host
/// converts). The null move is `None` where a move is optional, as `processTick` compares the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Move {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub free_look: bool,
    pub triggers: [bool; MAX_TRIGGERS],
}
