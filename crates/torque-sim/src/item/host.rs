//! What the Item's step needs from the host: the world, the ray cast, the collision switches, the clocks and the script callbacks at
//! their call points.
//!
//! **The callback contract** is the Player's (`torque_sim::player::host`): every callback receives the Item's [`ItemState`] as `&mut`
//! and the step reads every field again afterwards; deletes are deferred by the host to the end of the tick (the rule for
//! objects deleted during a tick); an object that is gone answers `None` from [`torque_collide::World::object`];
//! each callback is one outermost run charged to the script budget. Script runs here at three points, all at the end of
//! `updatePos`: `notifyCollision` (`onCollision` on both parties), `onEnterLiquid` and `onLeaveLiquid`.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/item.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use torque_collide::math::F3;
use torque_collide::{TypeMasks, World};

use super::{ItemData, ItemState};
use crate::scene::RayHit;

pub trait ItemHost {
    /// The container and the convexes (`collide`).
    fn world(&mut self) -> &mut dyn World;
    /// The sim clock in milliseconds (the collision queue's 250 ms stamps).
    fn sim_time(&self) -> u32;
    /// The masks the Item queries and casts with: Torque's unless the host claims bits of its own.
    fn type_masks(&self) -> &TypeMasks {
        &TypeMasks::TGE
    }
    /// `Container::castRay` on the Item's own container from `start` to `end` with `mask`: the nearest
    /// hit. The host applies the container's exemptions (objects whose collision is disabled are skipped).
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<RayHit>;
    /// `SceneObject::disableCollision`: the object's collision count goes up. The Item switches itself and its
    /// collision-exempt object (the thrower) off around the working-set update, and the exempt object around the first ray.
    fn disable_collision(&mut self, _id: torque_collide::ObjectId) {}
    /// `SceneObject::enableCollision`: the count goes down if it is not zero.
    fn enable_collision(&mut self, _id: torque_collide::ObjectId) {}
    /// An object's velocity (`getVelocity`); zero for an object that has none or is gone.
    fn object_velocity(&mut self, _id: torque_collide::ObjectId) -> F3 {
        [0.0; 3]
    }
    /// `ShapeBase::updateContainer` for an Item, called at the end of `updatePos`: it sets the drag, buoyancy, liquid
    /// type, coverage and zone gravity factor the NEXT tick's `updateVelocity` reads. The default is a world with no
    /// PhysicalZone ([`no_zones`]); the host's zone-aware version replaces it.
    fn update_container(&mut self, s: &mut ItemState, db: &ItemData) {
        no_zones(s, db);
    }
    /// The transform changed: the host re-bins the object (`Container::checkBins`) and updates its render transform.
    fn moved(&mut self, _s: &mut ItemState) {}
    /// One entry of `notifyCollision`: `this->onCollision(other, vec)` and then, while both live,
    /// `other->onCollision(this, vec)`. False when this Item no longer exists, which ends the walk and the liquid callbacks.
    fn on_collision(
        &mut self,
        _s: &mut ItemState,
        _other: torque_collide::ObjectId,
        _vec: F3,
    ) -> bool {
        true
    }
    /// `%data.onEnterLiquid(%obj, %coverage, %type)`.
    fn on_enter_liquid(&mut self, _s: &mut ItemState, _coverage: f32, _liquid_type: u32) {}
    /// `%data.onLeaveLiquid(%obj, %type)`.
    fn on_leave_liquid(&mut self, _s: &mut ItemState, _liquid_type: u32) {}
}

/// `ShapeBase::updateContainer` when no PhysicalZone overlaps the Item: `drag = db.drag`, `buoyancy = 0`,
/// the zone gravity factor `1.0`; a mounted Item gets coverage 0 and nothing else; otherwise the coverage clamps to 0 (and, with
/// the statics of an earlier zone call still in `liquid_type`, nothing else changes).
pub fn no_zones(s: &mut ItemState, db: &ItemData) {
    s.drag = db.drag;
    s.buoyancy = 0.0;
    s.zone_gravity = 1.0;
    s.water_coverage = 0.0;
}
