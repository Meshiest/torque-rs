//! What a vehicle's step needs from the host: the world, the clock, ray casts, the trigger/item scan, other objects, and the
//! script callbacks at their call points.
//!
//! **The callback contract** (as `torque_sim::player::host`'s). Script runs mid-step: `onTrigger` and the
//! energy/damage hooks in `ShapeBase::processTick`, `onCollision` from `notifyCollision` once per sub-step,
//! `onImpact` and the liquid callbacks at the end of each sub-step,
//! `onWreck`. A script there may call `setTransform`, `setVelocity`, `applyImpulse` on this vehicle and may delete
//! objects. So every callback receives the vehicle's [`VehicleState`] as `&mut` (the host makes it reachable to the natives the
//! script calls) and the step re-reads every field after it; the locals it keeps across a call
//! (the old velocity of the impact test, the sub-step's `dt`) stay as they were. Deletes are deferred by the host to the end of the
//! tick; a gone object answers `None` from [`torque_collide::World::object`] and is skipped where it would be read.
//! Each callback runs at most once per call site and pass; the host charges it to its script budget as an outermost run.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/vehicle.cpp` (Copyright (c)
//! 2012 GarageGames, LLC); the code here is this crate's own.

use torque_collide::math::{Box3, F3};
use torque_collide::{ObjectId, TypeMasks, World};

use crate::state::VehicleState;

/// A ray hit (`RayInfo`): the fraction along the ray, the point, the normal, the material and the object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub t: f32,
    pub point: F3,
    pub normal: F3,
    pub material: u32,
    pub object: Option<ObjectId>,
}

/// What `findCallback` found for one object of the scan: a corpse, or an item this vehicle did not throw. Triggers are
/// the host's own (`Trigger::potentialEnterObject` is called there).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanHit {
    pub object: ObjectId,
    /// The object's velocity (`getVelocity`).
    pub velocity: F3,
}

/// The host's side of the step.
pub trait VehicleHost {
    /// The container and the convexes (`collide`).
    fn world(&mut self) -> &mut dyn World;
    /// The sim clock in milliseconds (the collision queue's 250 ms stamps; it does not move inside a tick).
    fn sim_time(&self) -> u32;
    /// The masks the vehicle queries and casts with (`vehicle_working`, `vehicle_ray`, and `vehicle_scan` for
    /// [`scan`](Self::scan)): Torque's unless the host claims bits of its own.
    fn type_masks(&self) -> &TypeMasks {
        &TypeMasks::TGE
    }

    /// `disableCollision` / `enableCollision`: the counter of this vehicle and,
    /// recursively, of what is mounted on it. Around the working-set query and each wheel or hover ray.
    fn disable_collision(&mut self, _s: &VehicleState) {}
    fn enable_collision(&mut self, _s: &VehicleState) {}
    /// `Container::castRay` from `start` to `end` with `mask`, on the container of the vehicle.
    fn cast_ray(&mut self, _start: F3, _end: F3, _mask: u32) -> Option<RayHit> {
        None
    }
    /// `Container::findObjects(box, vehicle_scan, findCallback)` for this vehicle: the host calls
    /// `Trigger::potentialEnterObject` for each trigger itself and pushes the corpses and the items whose thrower is not
    /// this vehicle into `out`, in the container's order.
    fn scan(&mut self, _s: &mut VehicleState, _query: &Box3, _out: &mut Vec<ScanHit>) {}
    /// Another object's velocity (`getVelocity`); zero for one that has none or is gone.
    fn object_velocity(&mut self, _id: ObjectId) -> F3 {
        [0.0; 3]
    }
    /// `getEnergyLevel` when the datablock inherits the mount's energy: the first mount up the chain that does not.
    fn mount_energy(&mut self, _s: &VehicleState) -> f32 {
        0.0
    }
    /// `setEnergyLevel` forwarded to the mount when the datablock inherits (the mount's setter).
    fn set_mount_energy(&mut self, _s: &VehicleState, _e: f32) {}

    // ---- the callbacks, at their call points ----

    /// The damage step changed the damage.
    fn damage_changed(&mut self, _s: &mut VehicleState) {}
    /// The image slot's state machine, slots 0-3 with an image.
    fn image_tick(&mut self, _s: &mut VehicleState, _slot: usize) {}
    /// `ShapeBase::advanceThreads(0.032)`, the first step of the server branch: the script threads run a tick.
    fn advance_threads(&mut self, _s: &mut VehicleState) {}
    /// The fade step (the host keeps the fade `startFade` sets), after the wet byte and before the image slots.
    fn fade_tick(&mut self, _s: &mut VehicleState) {}
    /// `setImageTriggerState` for slots 0 and 1 from the move (`Vehicle::updateMove`, damage state 0).
    fn set_image_trigger(&mut self, _s: &mut VehicleState, _slot: usize, _down: bool) {}
    /// `onTrigger(datablock, obj, slot, state)`.
    fn on_trigger(&mut self, _s: &mut VehicleState, _slot: usize, _down: bool) {}
    /// One entry of `notifyCollision`: `this->onCollision(other, vec)` (`Vehicle::onCollision`:
    /// the script call, then [`crate::state::collision_push`] on the other when it is a vehicle) and, while both live,
    /// `other->onCollision(this, vec)` (when the other is a vehicle, its push lands on this one: `collision_push(other_pos, &mut
    /// s.rigid, vec)`). False when this vehicle no longer exists, which ends the walk.
    fn on_collision(&mut self, _s: &mut VehicleState, _other: ObjectId, _vec: F3) -> bool {
        true
    }
    /// `ShapeBase::onImpact(vec)`: `%db.onImpact(%obj, "0", vec, speed)`.
    fn on_impact(&mut self, _s: &mut VehicleState, _vec: F3) {}
    /// `%db.onEnterLiquid(%obj, %coverage, %type)` / `onLeaveLiquid(%obj, %type)`.
    fn on_enter_liquid(&mut self, _s: &mut VehicleState, _coverage: f32, _liquid_type: u32) {}
    fn on_leave_liquid(&mut self, _s: &mut VehicleState, _liquid_type: u32) {}
    /// A WheeledVehicle's contact prep found the body touching something (after `resolveCollision`, before
    /// `resolveContacts`): an extension point with the wheels as they are; nothing by default. Script may run.
    fn wheeled_body_contact(&mut self, _s: &mut VehicleState, _wheels: &crate::wheeled::Wheeled) {}
    /// The subclass of the WheeledVehicle, when this one is of one ([`crate::wheeled::WheeledClass`]): `None` is
    /// Torque's WheeledVehicle.
    fn wheeled_class(&self) -> Option<std::rc::Rc<dyn crate::wheeled::WheeledClass>> {
        None
    }
    /// The solver this vehicle's datablock picks for the body (made at the start of a tick, dropped at its end); `None` is TGE's,
    /// the vehicle's own [`crate::state::Body`].
    fn body_solver(&mut self) -> Option<Box<dyn crate::solver::BodySolver>> {
        None
    }
    /// `setPosition` moved the object: the host re-bins it (`Container::checkBins`) and sets its render
    /// transform.
    fn moved(&mut self, _s: &mut VehicleState) {}
    /// `ShapeBase::updateContainer` at the end of `processTick`: the PhysicalZones and water the vehicle is in set its
    /// drag, buoyancy, gravity multiplier, applied force, water coverage and liquid type (the arithmetic is `torque_sim::player`'s; the
    /// query box and the clip against the vehicle's own polygons need the host's shapes). Default: no zones,
    /// [`VehicleState::clear_container`].
    fn update_container(&mut self, s: &mut VehicleState, db: &crate::data::ShapeBaseData) {
        s.clear_container(db);
    }
}

/// A host with an empty world for tests and runs without one: no objects, no rays, an unlimited budget.
#[derive(Debug, Default)]
pub struct EmptyWorld;

impl World for EmptyWorld {
    fn object(&self, _id: ObjectId) -> Option<torque_collide::ObjectView> {
        None
    }
    fn find_objects(
        &mut self,
        _query: &Box3,
        _mask: u32,
        _exclude: Option<ObjectId>,
        _limit: usize,
        _out: &mut Vec<ObjectId>,
    ) -> Result<(), torque_collide::Exhausted> {
        Ok(())
    }
    fn convex_source(&self, _id: ObjectId) -> torque_collide::ConvexSource {
        torque_collide::ConvexSource::None
    }
    fn hull(&self, _id: ObjectId, _slot: u32) -> Option<std::rc::Rc<torque_collide::mesh::Hull>> {
        None
    }
    fn charge(&mut self, _units: u64) -> bool {
        true
    }
}
