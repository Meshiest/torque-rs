//! What the Player's step needs from the host: the world, the skeleton, the clocks, and the script callbacks at their call points.
//!
//! **The callback contract.** The step runs script in the middle of itself: the host's touch callback
//! ([`PlayerHost::on_contact_touch`]) inside `findContact` (between `updateMove`'s contact search and its run force, and again
//! inside `updatePos`' hit branch), `doDismount`/`onEnterLiquid`/
//! `onLeaveLiquid` at the end of `updateMove` (before `updatePos`), `onImpact` and the collision queue in the hit branch,
//! `onCollision` in `notifyCollision` at the end of `updatePos`, and `onTrigger` and the energy/damage
//! hooks in `ShapeBase::processTick`. A script there may call `setTransform`, `setVelocity`, `applyImpulse`, `mountObject` on this
//! same Player, and may delete objects. So:
//! - every callback receives the Player's [`PlayerState`] as `&mut`: the host makes it reachable to the natives the script calls,
//!   and the step owns no other copy of it. After the callback the step reads every field again from that state, exactly where
//!   `this` is re-read (the locals kept across the call, such as the wish vector and the gravity term of `updateMove`,
//!   stay as they were);
//! - deletes are deferred by the host to the end of the tick (the rule for objects deleted during a tick): no callback
//!   ends the step. An object that is gone answers `None` from [`torque_collide::World::object`] and is skipped where it
//!   would have been read;
//! - a callback runs at most once per call site and pass, so the host charges each to its script budget as an outermost run.
//!
//! The host implements [`PlayerHost`] over its own context; the defaults below are what a world with no scripts does.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::{Box3, F3, Matrix};
use torque_collide::{ObjectId, ObjectView, TypeMasks, World};

use std::rc::Rc;

use super::{PlayerClass, PlayerState};
use crate::Move;

/// The densest and the most viscous a water zone may be: a host holds a map's values to `0..=` these (a map is not trusted:
/// a density or viscosity near `f32::MAX` overflows the buoyancy and the drag to infinity).
pub const MAX_LIQUID_DENSITY: f32 = 1_000.0;
pub const MAX_LIQUID_VISCOSITY: f32 = 1_000.0;

/// A map's liquid value held to `0..=max` (a NaN to 0).
pub fn liquid_value(v: f32, max: f32) -> f32 {
    if v > 0.0 { v.min(max) } else { 0.0 }
}

/// A PhysicalZone that `updateContainer` found (the fields its callback reads).
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneView {
    pub id: ObjectId,
    /// The zone acts.
    pub active: bool,
    /// The world box.
    pub world_box: Box3,
    /// The zone's planes (its polyhedron moved to the world): `Player::buildPolyList` clips the Player's box against
    /// them, and an empty result skips the zone.
    pub planes: Vec<torque_collide::math::Plane>,
    /// The zone modifies velocities and forces.
    pub enabled: bool,
    /// Velocity multiplier (updatePos), gravity multiplier, drag (raw) and applied force.
    pub velocity_mod: f32,
    pub gravity_mod: f32,
    pub drag: f32,
    pub applied_force: F3,
    /// A water zone, with viscosity, density, colour and liquid type.
    pub water: bool,
    pub viscosity: f32,
    pub density: f32,
    pub color: [f32; 4],
    pub liquid_type: u32,
}

/// The statics `updateContainer` and its zone callback share across all objects (coverage, liquid type, liquid height,
/// viscosity, density and colour): host-owned, one per server.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiquidStatics {
    pub coverage: f32,
    pub liquid_type: u32,
    pub liquid_height: f32,
    pub viscosity: f32,
    pub density: f32,
    pub color: [f32; 4],
}

impl Default for LiquidStatics {
    /// The statics' initial values (viscosity 15.0, density 1.0).
    fn default() -> Self {
        LiquidStatics {
            coverage: 0.0,
            liquid_type: 0,
            liquid_height: 0.0,
            viscosity: 15.0,
            density: 1.0,
            color: [0.2, 0.6, 0.6, 0.3],
        }
    }
}

/// What the Player's control object is, for a subclass's driver path of `processTick` and its head code.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ControlledInfo {
    /// `VehicleObjectType`.
    pub vehicle: bool,
    /// `PlayerObjectType`.
    pub player: bool,
}

/// An object's energy link (`getEnergyLevel`): its energy, its datablock's inherit-energy flag, and its mount.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnergyLink {
    pub energy: f32,
    pub inherit: bool,
    pub mount: Option<ObjectId>,
}

/// The `$SB::DFDec` and `$SB::WODec` globals the flash decay reads.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SimGlobals {
    pub damage_flash_dec: f32,
    pub white_out_dec: f32,
}

/// The host's side of the step.
pub trait PlayerHost {
    /// The container and the convexes (`collide`).
    fn world(&mut self) -> &mut dyn World;
    /// The Player's shape instance and threads.
    fn skeleton(&mut self) -> &mut dyn Skeleton;
    /// The sim clock in milliseconds (the collision queue's 250 ms stamps).
    fn sim_time(&self) -> u32;
    /// The subclass of the Player, when this one is of one ([`PlayerClass`]): `None` is Torque's Player.
    fn player_class(&self) -> Option<Rc<dyn PlayerClass>> {
        None
    }
    fn globals(&self) -> SimGlobals {
        SimGlobals::default()
    }
    /// The masks the Player (and an AIPlayer's `getAIMove`) query with: Torque's unless the host claims bits of its own.
    fn type_masks(&self) -> &TypeMasks {
        &TypeMasks::TGE
    }

    /// An object's velocity (`getVelocity`); zero for an object that has none or is gone.
    fn object_velocity(&mut self, _id: ObjectId) -> F3 {
        [0.0; 3]
    }
    /// The mount's `getMountTransform(node)`, for a mounted `setPosition`.
    fn mount_transform(&mut self, _mount: ObjectId, _node: u32) -> Option<Matrix> {
        None
    }
    /// An object's energy link (for the mount chain).
    fn energy_link(&mut self, _id: ObjectId) -> Option<EnergyLink> {
        None
    }
    /// An Item's thrower: `findContact` does not queue a collision with an item this Player threw.
    fn item_thrower(&mut self, _id: ObjectId) -> Option<ObjectId> {
        None
    }
    /// `findContact`'s host touch: asked in working-set order for each object whose type meets `type_masks().player_touch`,
    /// after the trigger, corpse and item branches and before the object's polygons are fed (such an object is fed whatever
    /// the working mask). `view` is the object's view, `query` the contact search's slack box, and `queue_full` is true when
    /// [`MAX_CONTACT_TOUCHES`](super::MAX_CONTACT_TOUCHES) are queued already. True queues `obj` for
    /// [`on_contact_touch`](Self::on_contact_touch) (ignored when the queue is full); the host updates its own touch state
    /// here. The default queues nothing.
    fn contact_touch(
        &mut self,
        _s: &PlayerState,
        _obj: ObjectId,
        _view: &ObjectView,
        _query: &Box3,
        _queue_full: bool,
    ) -> bool {
        false
    }
    /// The PhysicalZones whose world box overlaps `query` (in container order).
    fn zones(&mut self, _query: &Box3, _out: &mut Vec<ZoneView>) {}
    /// The shared statics of `updateContainer`.
    fn liquid_statics(&mut self) -> &mut LiquidStatics;
    /// A zone's velocity multiplier when it is enabled, for `updatePos`' zone list.
    fn zone_velocity_mod(&mut self, _id: ObjectId) -> Option<f32> {
        None
    }
    /// The control object's kind.
    fn controlled_info(&mut self, _id: ObjectId) -> ControlledInfo {
        ControlledInfo::default()
    }
    /// `Container::castRay` from `start` to `end` with `mask`, this Player and its mounts excluded: the hit point.
    fn cast_ray(&mut self, _start: F3, _end: F3, _mask: u32) -> Option<F3> {
        None
    }

    // ---- what AIPlayer's `getAIMove` asks of the world ----

    /// The aim target the id names (a `GameBase`), or `None` when it is gone (a `SimObjectPtr` nulled by a delete).
    fn ai_target(&mut self, _id: ObjectId) -> Option<crate::ai::AiTarget> {
        None
    }
    /// The trigger flag of image slot `slot`, asked only when the slot has an image.
    fn image_trigger(&mut self, _s: &PlayerState, _slot: usize) -> bool {
        false
    }

    // ---- the callbacks, at their call points ----

    /// `ShapeBase::processTick` saw the energy change.
    fn energy_changed(&mut self, _s: &mut PlayerState) {}
    /// The damage step changed the damage (its `executef` on the datablock).
    fn damage_changed(&mut self, _s: &mut PlayerState) {}
    /// The image slot's state machine, called for slots 0-3 in order: the host runs
    /// [`crate::image::update_image_state`] with [`crate::image::IMAGE_DT`] (it does nothing for an empty slot) after moving the
    /// slot-0 wet flag in with [`crate::image::ImageSet::load_wet0`], and mirrors the occupancy back into `s.images`.
    fn image_tick(&mut self, _s: &mut PlayerState, _slot: usize) {}
    /// `ShapeBase::advanceThreads(0.032)`, the first step of the server branch of the tick: the four script threads run a
    /// tick (the host keeps them), each one that ends calling the datablock's `onEndSequence`.
    fn advance_threads(&mut self, _s: &mut PlayerState) {}
    /// The fade step of the tick (the host keeps the fade `startFade` sets), after the wet flag and before the image slots.
    fn fade_tick(&mut self, _s: &mut PlayerState) {}
    /// `setImageTriggerState` for slots 0 and 1 from the move.
    fn set_image_trigger(&mut self, _s: &mut PlayerState, _slot: usize, _down: bool) {}
    /// `onTrigger(datablock, obj, slot, state)`.
    fn on_trigger(&mut self, _s: &mut PlayerState, _slot: usize, _down: bool) {}
    /// `Trigger::potentialEnterObject`.
    fn potential_enter_trigger(&mut self, _s: &mut PlayerState, _trigger: ObjectId) {}
    /// The queued touches of [`contact_touch`](Self::contact_touch), in queue order, after a completed working-set walk.
    fn on_contact_touch(&mut self, _s: &mut PlayerState, _obj: ObjectId) {}
    /// An AIPlayer callback (`throwCallback(name)`): `%datablock.<name>(%obj)`, synchronous inside `getAIMove`.
    fn ai_callback(&mut self, _s: &mut PlayerState, _name: &'static str) {}
    /// A question a subclass ([`PlayerClass`]) asks its host that this trait has no method for: the subclass's own request
    /// type, which the host downcasts and answers in place. Torque's Player asks none; the default leaves it unanswered.
    fn class_query(&mut self, _query: &mut dyn std::any::Any) {}
    /// `%data.doDismount(%obj)`.
    fn do_dismount(&mut self, _s: &mut PlayerState) {}
    /// `%data.onEnterLiquid(%obj, %coverage, %type)`.
    fn on_enter_liquid(&mut self, _s: &mut PlayerState, _coverage: f32, _liquid_type: u32) {}
    /// `%data.onLeaveLiquid(%obj, %type)`.
    fn on_leave_liquid(&mut self, _s: &mut PlayerState, _liquid_type: u32) {}
    /// `Player::onImpact`: the hit object and the impact vector.
    fn on_impact(&mut self, _s: &mut PlayerState, _hit: Option<ObjectId>, _vec: F3) {}
    /// One entry of `notifyCollision`: `this->onCollision(other, vec)` and then, while both live,
    /// `other->onCollision(this, vec)`. False when this Player no longer exists (a safe pointer), which ends the walk.
    fn on_collision(&mut self, _s: &mut PlayerState, _other: ObjectId, _vec: F3) -> bool {
        true
    }
    /// The control object's own `processTick` with the driver's move, after the dirty-list queue.
    fn tick_controlled(&mut self, _s: &mut PlayerState, _control: ObjectId, _mv: Option<&Move>) {}
    /// `setPosition` moved the object: the host re-bins it (`Container::checkBins`) and updates its render transform.
    fn moved(&mut self, _s: &mut PlayerState) {}
}

/// The animated threads the step drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thread {
    /// The arm.
    Arm,
    /// "head".
    Head,
    /// "headside".
    HeadSide,
    /// A thread a subclass adds; the number is the subclass's own.
    Class(u32),
    /// The action animation.
    Action,
}

/// The Player's shape instance on the host (and its pose evaluation). This crate never evaluates a pose.
pub trait Skeleton {
    /// The shape instance exists.
    fn has_shape(&self) -> bool;
    /// `TSShapeInstance::animateNodes` for the current detail, as Torque3D's `Player::getEyeBaseTransform` calls it on a server.
    fn animate(&mut self);
    /// `mNodeTransforms[node]`.
    fn node_transform(&self, node: i32) -> Option<Matrix>;
    /// `ShapeBaseData::eyeNode` (-1 when none).
    fn eye_node(&self) -> i32;
    /// The node of mount point `point` (-1 when none).
    fn mount_node(&self, point: u32) -> i32;
    /// The thread exists.
    fn has_thread(&self, t: Thread) -> bool;
    /// The thread's position (the transition target's while the thread is in transition).
    fn thread_pos(&self, t: Thread) -> f32;
    /// The thread moved by `delta`.
    fn advance_thread(&mut self, t: Thread, delta: f32);
    /// The thread's time scale.
    fn thread_scale(&self, t: Thread) -> f32;
    fn set_thread_scale(&mut self, t: Thread, scale: f32);
    /// The thread put on a sequence.
    fn set_thread_sequence(&mut self, t: Thread, sequence: i32, pos: f32);
    /// Whether the thread is in transition: `updateDeathOffsets` skips the root displacement then.
    fn thread_in_transition(&self, t: Thread) -> bool;
    /// The root node's displacement along the action thread's sequence from `from` to `to`.
    fn root_displacement(&mut self, from: f32, to: f32) -> Matrix;
    /// `setActionThread(anim, forward 1, hold 0, wait 1, fsp 1, 0)` as the jump calls it: the action
    /// animation and its thread.
    fn set_action_thread(&mut self, s: &mut PlayerState, anim: u32);
    /// The pre-step advance, which a server runs before the step.
    fn advance_before(&mut self, s: &mut PlayerState, dt: f32);
    /// `updateAnimation` and `updateActionThread(true)` after the step; the latter may pick a new action
    /// animation from the state.
    fn advance_after(&mut self, s: &mut PlayerState);
}

/// A Player with no shape: no nodes, no threads.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSkeleton;

impl Skeleton for NoSkeleton {
    fn has_shape(&self) -> bool {
        false
    }
    fn animate(&mut self) {}
    fn node_transform(&self, _node: i32) -> Option<Matrix> {
        None
    }
    fn eye_node(&self) -> i32 {
        -1
    }
    fn mount_node(&self, _point: u32) -> i32 {
        -1
    }
    fn has_thread(&self, _t: Thread) -> bool {
        false
    }
    fn thread_pos(&self, _t: Thread) -> f32 {
        0.0
    }
    fn advance_thread(&mut self, _t: Thread, _delta: f32) {}
    fn thread_scale(&self, _t: Thread) -> f32 {
        0.0
    }
    fn set_thread_scale(&mut self, _t: Thread, _scale: f32) {}
    fn set_thread_sequence(&mut self, _t: Thread, _sequence: i32, _pos: f32) {}
    fn thread_in_transition(&self, _t: Thread) -> bool {
        false
    }
    fn root_displacement(&mut self, _from: f32, _to: f32) -> Matrix {
        torque_collide::math::IDENTITY
    }
    fn set_action_thread(&mut self, _s: &mut PlayerState, _anim: u32) {}
    fn advance_before(&mut self, _s: &mut PlayerState, _dt: f32) {}
    fn advance_after(&mut self, _s: &mut PlayerState) {}
}
