//! The Projectile's server step: `Projectile::processTick`, `onCollision` and `explode`.
//!
//! Each tick a projectile moves from its position along its velocity (gravity first, for a ballistic one), casting one ray
//! over the step against the dynamic and static collision masks. A hit past the arming delay explodes it at the hit point; a
//! hit before it bounces a ballistic projectile (reflected velocity, friction along the surface, elasticity) and lets any other
//! one pass. A projectile that reaches its lifetime is deleted.
//!
//! **This module is the server's Projectile**: the client branches (the particle emitters, the sound, the fade, the explosion
//! objects and decals) are not here.
//!
//! **Caps:** none of its own; the step casts one ray a tick; `Container::castRay`'s caps are the host's.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/projectile.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

// Comparisons are written so that a NaN takes a fixed side: a negated comparison is the intended form.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

mod data;
pub mod driver;
mod host;
mod tick;

#[cfg(test)]
mod tests;

use torque_collide::ObjectId;
use torque_collide::math::{Box3, F3};

pub use data::{ProjectileData, validate};
pub use host::ProjectileHost;

use crate::scene::SceneObject;

/// The Projectile's `mTypeMask` bit (`Projectile::Projectile` sets `|= ProjectileObjectType`).
pub const PROJECTILE_TYPE: u32 = torque_collide::types::PROJECTILE;

/// The ticks the source stays exempt from the ray (`SourceIdTimeoutTicks`, 231 ms).
pub const SOURCE_TIMEOUT_TICKS: u32 = 7;
/// The delay of the delete event after `explode`, ms (`DeleteWaitTime`: time for the explosion to reach the clients).
pub const DELETE_WAIT_MS: u32 = 500;

/// Net mask bits the step sets (`setMaskBits`).
pub mod net_mask {
    /// `BounceMask`: a hit on something that is not static world (the position and velocity are re-sent).
    pub const BOUNCE: u32 = 0x20;
    /// `ExplosionMask`: the explosion record.
    pub const EXPLOSION: u32 = 0x40;
}

/// The Projectile's state: every field the step reads or writes. Plain data, so a run can start from any state.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectileState {
    pub id: ObjectId,
    /// The transform (a translation only), its inverse, the scale and the boxes.
    pub scene: SceneObject,
    /// `mTypeMask`.
    pub type_mask: u32,
    /// Mask bits set this tick (the host ORs them into its ghost masks).
    pub mask_bits: u32,
    /// `mCurrPosition` and `mCurrVelocity` (the persist fields `initialPosition` and `initialVelocity`).
    pub position: F3,
    pub velocity: F3,
    /// `sourceObject` (an id the host resolves at `onAdd`, constructor -1) and `sourceSlot` (constructor -1): the packer's.
    pub source_id: i32,
    pub source_slot: i32,
    /// `mCurrTick`: the age in ticks.
    pub age: u32,
    /// The source object (a SimObjectPtr): exempt from the ray while `age <= SOURCE_TIMEOUT_TICKS`.
    pub source: Option<ObjectId>,
    /// `mHidden`: exploded (a hidden projectile is no longer simulated).
    pub exploded: bool,
    /// `mFadeValue` (the server never changes it).
    pub fade: f32,
    /// `mCurrDeltaBase` and `mCurrBackDelta`: the render interpolation of the last step.
    pub render_base: F3,
    pub render_delta: F3,
    /// The explosion position, its normal and the type mask of what was hit.
    pub explosion_pos: F3,
    pub explosion_normal: F3,
    pub explosion_type: u32,
}

impl ProjectileState {
    /// A Projectile as `Projectile::Projectile` leaves it: position zero, velocity (0, 0, 1), `source_id` and `source_slot` -1,
    /// the fade 1. `obj_box` is the shape's bounds (or zero).
    pub fn new(id: ObjectId, obj_box: Box3) -> ProjectileState {
        ProjectileState {
            id,
            scene: SceneObject::new(obj_box),
            type_mask: PROJECTILE_TYPE,
            mask_bits: 0,
            position: [0.0; 3],
            velocity: [0.0, 0.0, 1.0],
            source_id: -1,
            source_slot: -1,
            age: 0,
            source: None,
            exploded: false,
            fade: 1.0,
            render_base: [0.0; 3],
            render_delta: [0.0; 3],
            explosion_pos: [0.0; 3],
            explosion_normal: [0.0; 3],
            explosion_type: 0,
        }
    }

    pub(crate) fn set_mask(&mut self, bits: u32) {
        self.mask_bits |= bits;
    }

    /// The view `collide` reads of this object (for the host's [`torque_collide::World`]).
    pub fn object_view(&self) -> torque_collide::ObjectView {
        self.scene.view(self.type_mask)
    }
}

/// A Projectile: its state.
#[derive(Debug, Clone)]
pub struct Projectile {
    pub s: Box<ProjectileState>,
}

impl Projectile {
    pub fn new(state: ProjectileState) -> Projectile {
        Projectile { s: Box::new(state) }
    }

    pub fn id(&self) -> ObjectId {
        self.s.id
    }

    /// `Projectile::onAdd`, the numeric part after `GameBase::onAdd` and the source lookup: the age is zero and the world box
    /// is reset for the container. The source is the host's: it sets `state.source`, and the process list orders this object
    /// after it.
    pub fn on_add(&mut self) {
        self.s.age = 0;
        self.s.scene.reset_world_box();
    }
}
