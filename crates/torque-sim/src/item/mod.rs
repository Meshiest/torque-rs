//! The Item's server step: `Item::processTick` -> `updateVelocity` -> `updateWorkingCollisionSet` -> `updatePos`,
//! `setTransform`, `setVelocity`, `applyImpulse`, `setCollisionTimeout`, and the rest/wake/undo state machine.
//!
//! **This module is the server's Item**: `netFlags` bit 1 (ghost) clear and bit 8 (ghostable) set. The client branches (the warp,
//! the render interpolation, the ghost render delta) are not here: the warp counter is only written by `unpackUpdate`, so it is 0
//! on a server and the first branch of `processTick` never runs.
//!
//! **Caps:** [`MAX_ITEM_SUBSTEPS`] is the sweep's own limit; [`MAX_COLLISION_QUEUE`](crate::MAX_COLLISION_QUEUE) bounds
//! the collision queue; `collide`'s caps bound the polygon lists, the working set and the collision list; the host's work budget
//! stops a step that would run long (the step returns `Err(Exhausted)` with the state as far as it got).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/item.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

// Comparisons are written so that a NaN takes a fixed side: a negated comparison (`!(a >= b)`, "below, or unordered") is the
// intended form, as in `collide`.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

mod data;
pub mod driver;
mod host;
mod tick;
mod update_pos;

#[cfg(test)]
mod tests;

use torque_collide::math::{Box3, F3, IDENTITY, Matrix, set_euler, transcendental};
use torque_collide::{
    EarlyOutPolyList, ExtrudedPolyList, ObjectId, Polyhedron, TypeMasks, WorkingSet, types,
};

pub use data::ItemData;
pub use host::{ItemHost, no_zones};

use crate::scene::{QueuedCollision, SceneObject};

/// `updatePos`' sweep sub-steps (the counter in the sweep loop); the third that still hits zeroes the
/// velocity.
pub const MAX_ITEM_SUBSTEPS: u32 = 3;

/// Net mask bits the step sets (`setMaskBits`).
pub mod net_mask {
    /// Position, velocity and the at-rest flag (`setVelocity`, `updatePos`).
    pub const POSITION: u32 = 0x0400_0000;
    /// The collision object (`setCollisionTimeout`).
    pub const COLLISION_OBJECT: u32 = 0x0200_0000;
    /// The rotation (`setTransform`, when `rotate` is clear).
    pub const ROTATION: u32 = 0x0800_0000;
    /// The warp flag (`setTransform`).
    pub const WARP: u32 = 0x40;
}

/// The cached box value meaning "no working set yet" (`-1e9` as a float).
pub const CACHE_SENTINEL: f32 = -1.0e9;

/// The Item's state: every field the step reads or writes. Plain data, so a run can start from any state.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemState {
    pub id: ObjectId,
    /// The transform, its inverse, the scale and the boxes.
    pub scene: SceneObject,
    /// `mTypeMask`.
    pub type_mask: u32,
    /// Mask bits set (the host ORs them into its ghost masks; `clearMaskBits` is a `&= !`).
    pub mask_bits: u32,
    /// Drag, buoyancy, liquid type, water coverage and zone gravity factor.
    pub drag: f32,
    pub buoyancy: f32,
    pub liquid_type: u32,
    pub water_coverage: f32,
    pub zone_gravity: f32,
    /// Hidden (`ShapeBase::setHidden`): a hidden item is not simulated.
    pub hidden: bool,
    /// The collision queue (newest first).
    pub collisions: Vec<QueuedCollision>,
    /// The render delta (a server writes the old position at the start of `updatePos`, zero when the tick skips it) and
    /// the render base (the position after the tick, or the undo's). Neither is read on a server.
    pub render_delta: F3,
    pub render_base: F3,
    /// `collideable`, `static` and `rotate` (persist fields).
    pub collideable: bool,
    pub is_static: bool,
    pub rotate: bool,
    /// The velocity.
    pub velocity: F3,
    /// `mAtRest` and its counter.
    pub at_rest: bool,
    pub rest_ticks: i32,
    /// The liquid callbacks have fired for the current liquid.
    pub in_liquid: bool,
    /// The collision-exempt object (the thrower) and its timeout in ticks.
    pub exempt: Option<ObjectId>,
    pub exempt_ticks: i32,
    /// The working set's cached box (six floats, `-1e9` = unset).
    pub working_box: Box3,
}

impl ItemState {
    /// An Item as `Item::Item`, `onNewDataBlock` and `onAdd` leave it: at rest (a new non-static item sleeps until a
    /// `setTransform`, `setVelocity` or `applyImpulse` wakes it), velocity zero, the datablock's drag, the
    /// cache unset, at `transform` with `obj_box` from the shape. `masks` are the host's ([`ItemHost::type_masks`]).
    pub fn new(
        id: ObjectId,
        db: &ItemData,
        obj_box: Box3,
        transform: Matrix,
        masks: &TypeMasks,
    ) -> ItemState {
        let mut scene = SceneObject::new(obj_box);
        scene.set_matrix(&transform);
        ItemState {
            id,
            scene,
            type_mask: types::ITEM | (db.dynamic_type & masks.item_dynamic_allowed),
            mask_bits: 0,
            drag: db.drag,
            buoyancy: 0.0,
            liquid_type: 0,
            water_coverage: 0.0,
            zone_gravity: 1.0,
            hidden: false,
            collisions: Vec::new(),
            render_delta: [0.0; 3],
            render_base: scene_pos(&transform),
            collideable: false,
            is_static: false,
            rotate: false,
            velocity: [0.0; 3],
            at_rest: true,
            rest_ticks: 0,
            in_liquid: false,
            exempt: None,
            exempt_ticks: 0,
            working_box: Box3::new([CACHE_SENTINEL; 3], [CACHE_SENTINEL; 3]),
        }
    }

    /// The translation (`getPosition`).
    pub fn position(&self) -> F3 {
        self.scene.position()
    }

    /// The view `collide` reads of this object (for the host's [`torque_collide::World`]).
    pub fn object_view(&self) -> torque_collide::ObjectView {
        self.scene.view(self.type_mask)
    }

    pub fn set_mask(&mut self, bits: u32) {
        self.mask_bits |= bits;
    }

    /// `NetObject::clearMaskBits`.
    pub fn clear_mask(&mut self, bits: u32) {
        self.mask_bits &= !bits;
    }
}

fn scene_pos(m: &Matrix) -> F3 {
    [m[3], m[7], m[11]]
}

/// The scratch lists and the polyhedron the step reuses, on the heap.
#[derive(Debug, Default)]
pub struct Scratch {
    /// The extruded list.
    pub ext: ExtrudedPolyList,
    /// The early-out list.
    pub early: EarlyOutPolyList,
    /// The swept box.
    pub hedron: Polyhedron,
}

/// An Item: its state, its working set and the scratch lists.
#[derive(Debug)]
pub struct Item {
    pub s: Box<ItemState>,
    pub working: WorkingSet,
    pub scratch: Box<Scratch>,
}

impl Clone for Item {
    /// The state and the working set; the scratch lists start empty (they are rebuilt before every read).
    fn clone(&self) -> Self {
        Item {
            s: self.s.clone(),
            working: self.working.clone(),
            scratch: Box::default(),
        }
    }
}

impl Item {
    pub fn new(state: ItemState) -> Item {
        Item {
            s: Box::new(state),
            working: WorkingSet::new(),
            scratch: Box::default(),
        }
    }

    pub fn id(&self) -> ObjectId {
        self.s.id
    }

    /// `Item::onAdd` after `ShapeBase::onAdd`: the datablock's `dynamicType` bit joins the type mask; a static item is at
    /// rest; the render base is the position; the cache is unset. `masks` are the host's ([`ItemHost::type_masks`]).
    pub fn on_add(&mut self, db: &ItemData, masks: &TypeMasks) {
        let s = &mut *self.s;
        s.type_mask |= db.dynamic_type & masks.item_dynamic_allowed;
        if s.is_static {
            s.at_rest = true;
        }
        s.render_base = s.position();
        s.working_box = Box3::new([CACHE_SENTINEL; 3], [CACHE_SENTINEL; 3]);
    }

    /// `Item::onRemove`: the cache is unset and the item leaves every other working set.
    pub fn on_remove(&mut self) {
        self.s.working_box = Box3::new([CACHE_SENTINEL; 3], [CACHE_SENTINEL; 3]);
        self.working.clear();
    }

    /// `Item::setTransform`: the position from elements 3, 7, 11; with `rotate` clear the yaw is `atan2(-m[1], m[5])` (the
    /// [`library`](crate::library) table's `item_yaw`) and the matrix `set_euler(0, 0, -yaw)` (roll and pitch are dropped), with
    /// `rotate` set the identity; the translation is written in raw; the inverse, the world box, the bins (host); a non-static item wakes
    /// (`mAtRest = 0`, counter 0); mask `0xc000040` (rotation, position, warp).
    pub fn set_transform(&mut self, m: &Matrix, host: &mut dyn ItemHost) {
        let pos = [m[3], m[7], m[11]];
        let mut r = if self.s.rotate {
            IDENTITY
        } else {
            let ang = (crate::library::get().item_yaw)(m);
            set_euler([0.0, 0.0, -ang])
        };
        r[3] = pos[0];
        r[7] = pos[1];
        r[11] = pos[2];
        let s = &mut *self.s;
        s.scene.set_matrix(&r);
        host.moved(s);
        if !s.is_static {
            s.at_rest = false;
            s.rest_ticks = 0;
        }
        s.set_mask(net_mask::ROTATION | net_mask::POSITION | net_mask::WARP);
    }

    /// `Item::setVelocity`: the velocity (the [`library`](crate::library) table's `item_set_velocity`; TGE stores it as
    /// given); mask `0x4000000`; the item wakes.
    pub fn set_velocity(&mut self, v: F3) {
        let s = &mut *self.s;
        (crate::library::get().item_set_velocity)(s, v);
        s.set_mask(net_mask::POSITION);
        s.at_rest = false;
        s.rest_ticks = 0;
    }

    /// `Item::applyImpulse`: `v = impulse / mass` per component, then `setVelocity(v)`: it sets the velocity.
    pub fn apply_impulse(&mut self, db: &ItemData, impulse: F3) {
        let v = std::array::from_fn(|i| impulse[i] / db.mass);
        self.set_velocity(v);
    }

    /// `setCollisionTimeout(obj)`: the exempt object, 15 ticks, mask `0x2000000`.
    pub fn set_collision_timeout(&mut self, other: ObjectId) {
        let s = &mut *self.s;
        s.exempt = Some(other);
        s.exempt_ticks = 15;
        s.set_mask(net_mask::COLLISION_OBJECT);
    }
}

/// `Item::setVelocity`'s velocity, TGE's: stored as given. [`PLAIN`](crate::library::PLAIN)'s `item_set_velocity`.
pub fn plain_set_velocity(s: &mut ItemState, v: F3) {
    s.velocity = v;
}

/// The yaw `setTransform` keeps of a matrix: `atan2(-m[1], m[5])`. [`PLAIN`](crate::library::PLAIN)'s `item_yaw`.
pub fn plain_yaw(m: &Matrix) -> f32 {
    transcendental::atan2_f32(-m[1], m[5])
}
