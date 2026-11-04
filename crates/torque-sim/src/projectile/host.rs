//! What the Projectile's step needs from the host: the ray cast, the collision switch of the source, the clock and the script
//! callbacks at their call points.
//!
//! **The callback contract** is the Player's (`torque_sim::player::host`): every callback receives the Projectile's
//! [`ProjectileState`] as `&mut` and the step reads every field again afterwards; deletes are deferred by the host to the end of
//! the tick (the rule for objects deleted during a tick); each callback is one outermost run charged to the script
//! budget. Script runs in the middle of the step at two points: `onCollision` and `onExplode` (from `explode`).
//!
//! A `SimObjectPtr` that is nulled when its object is deleted (the source) is the host's to null in
//! [`ProjectileState::source`](super::ProjectileState::source).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/projectile.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::F3;
use torque_collide::{ObjectId, TypeMasks};

use super::ProjectileState;
use crate::scene::RayHit;

pub trait ProjectileHost {
    /// The sim clock in milliseconds (the delete event's time).
    fn sim_time(&self) -> u32;
    /// The masks the Projectile casts with: Torque's unless the host claims bits of its own. The ray uses
    /// `projectile_ray_players` (the dynamic and static collision masks); `projectile_solid` is the static part.
    fn type_masks(&self) -> &TypeMasks {
        &TypeMasks::TGE
    }
    /// `Container::castRay` on the projectile's own container from `start` to `end` with `mask`: the nearest hit. The
    /// host applies the container's exemptions (objects whose collision is disabled are skipped).
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<RayHit>;
    /// The source's collision switch: `disableCollision` (true) or `enableCollision` (false) on the source.
    fn set_source_collision(&mut self, _source: ObjectId, _disabled: bool) {}
    /// `SceneObject::setTransform` moved the object: the host re-bins it and updates its render transform.
    fn moved(&mut self, _s: &mut ProjectileState) {}
    /// `%data.onCollision(%obj, %col, %fade, %pos, %normal)`: the fade is `s.fade`.
    fn on_collision(&mut self, _s: &mut ProjectileState, _col: ObjectId, _point: F3, _normal: F3) {}
    /// `%data.onExplode(%obj, %pos, %fade)`: `pos` is `s.explosion_pos`.
    fn on_explode(&mut self, _s: &mut ProjectileState, _pos: F3, _fade: f32) {}
    /// `new ObjectDeleteEvent` at `at` ms on the sim clock.
    fn schedule_delete(&mut self, _id: ObjectId, _at: u32) {}
    /// `SimObject::deleteObject` from inside the tick (the lifetime end): deferred to the end of the tick.
    fn delete_object(&mut self, _id: ObjectId) {}
}
