//! The Player's server step: `Player::processTick` -> `ShapeBase::processTick` -> `updateWorkingCollisionSet` -> `updateMove`
//! -> `updateLookAnimation` -> `updateDeathOffsets` -> `updatePos`, and `writePacketData`, the words the control-object checksum covers.
//!
//! The helpers are findContact, the working set, step, setPosition, updateContainer, the PlayerData products, the eye, mount and
//! muzzle transforms, and updateDeathOffsets. The steps whose arithmetic a host may replace go through the
//! [`library`](crate::library) table; this crate's own are plain `f32` arithmetic in the order Torque's source writes it.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` and `T3D/shapeBase.cpp`
//! (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.
//!
//! **This module is the server's Player**: `netFlags` bit 1 (ghost) clear and bit 8 (ghostable) set. The ghost-only branches (the
//! warp, the jump sound, the client camera shake) are not here.
//!
//! **Caps:** [`MAX_POS_ITERATIONS`] is the sweep's own limit; [`MAX_YAW_WRAPS`] and [`MAX_MOUNT_DEPTH`] bound loops that would
//! otherwise be open; [`MAX_COLLISION_QUEUE`] bounds the collision queue; `collide`'s caps bound the lists and sets.

// Comparisons are written so that a NaN takes a fixed side: a negated comparison (`!(a >= b)`, "below, or unordered") is the
// intended form, as in `collide`.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod contact;
mod container;
pub mod data;
pub mod driver;
pub mod ext;
pub mod host;
pub mod look;
pub mod pose;
pub mod shape_base;
pub mod tether;
pub mod transform;
pub mod update_move;
mod update_pos;

#[cfg(test)]
pub(crate) mod tests;

use torque_collide::math::{Box3, F3, IDENTITY, Matrix};
use torque_collide::{
    ClippedPolyList, EarlyOutPolyList, ExtrudedPolyList, ObjectId, Polyhedron, WorkingSet,
};

pub use data::{ActionNumbers, PlayerData, TGE_ACTION_NUMBERS};
pub use ext::{PlayerClass, PlayerFields, TorquePlayer};
pub use host::{NoSkeleton, PlayerHost, Skeleton, Thread};
pub use pose::{Pose, PoseData, PoseSpeeds, PoseTriggers};
pub use transform::{
    ImageMuzzle, affine_inverse, eye_transform, mount_transform, muzzle_transform,
};
pub use update_move::{JUMP_SKIP_CONTACTS_MAX, can_jump};

use crate::Move;

/// `updatePos`' sweep iterations (the counter; the fifth restores the start and zeroes
/// the velocity).
pub const MAX_POS_ITERATIONS: u32 = 5;
/// The steps of each of `updateMove`'s two yaw-wrap loops. Looping until the yaw is in `[0, 2pi]` never ends for a yaw of 2^25 or
/// more (adding 2pi no longer changes the float) or an infinity; 64 steps cover every yaw a script can reach with `setTransform`
/// in practice (|yaw| < 402), and past the cap the yaw is left as the last step made it.
pub const MAX_YAW_WRAPS: u32 = 64;
/// The longest mount chain `getEnergyLevel` walks.
pub const MAX_MOUNT_DEPTH: usize = 32;
/// The most entries the collision queue holds (it could otherwise grow by one entry per object for 250 ms).
pub const MAX_COLLISION_QUEUE: usize = 64;
/// The touches `findContact` queues for the host in one search ([`PlayerHost::contact_touch`]); later ones are offered as full.
pub const MAX_CONTACT_TOUCHES: usize = 10;

/// The tick (`TickSec`), in seconds.
pub const DT: f32 = 0.032;

/// Net mask bits the step sets (`setMaskBits`).
pub mod net_mask {
    /// `MoveMask`.
    pub const MOVE: u32 = 0x0200_0000;
    /// The impact bit.
    pub const IMPACT: u32 = 0x0400_0000;
    /// The jump bit.
    pub const JUMP: u32 = 0x1000_0000;
    /// Image wet.
    pub const WET: u32 = 0x0010_0000;
}

/// One entry of the collision queue: the object, its id stamp, the expiry on the sim clock, the relative velocity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueuedCollision {
    pub object: Option<ObjectId>,
    pub id: ObjectId,
    pub expiry: u32,
    pub vec: F3,
}

/// The controlling client.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClientView {
    /// The connection's camera position: 0 in the first person, 1 in the third.
    pub freelook_value: f32,
}

impl ClientView {
    /// The connection views in the first person: its camera position is 0.
    pub fn is_first_person(&self) -> bool {
        self.freelook_value == 0.0
    }
}

/// The Player's state: every field `processTick` reads or writes. Plain data, so a run can start from any state.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub id: ObjectId,
    // ---- SceneObject ----
    /// `mObjToWorld` (row-major, translation at 3, 7, 11) and `mWorldToObj`.
    pub obj_to_world: Matrix,
    pub world_to_obj: Matrix,
    /// `mObjScale`.
    pub scale: F3,
    /// `mObjBox` (its max z is rewritten by the crouch switch).
    pub obj_box: Box3,
    /// `mWorldBox`, the sphere centre and its radius.
    pub world_box: Box3,
    pub world_center: F3,
    pub world_radius: f32,
    /// `mTypeMask`.
    pub type_mask: u32,
    /// Mask bits set this tick (the host ORs them into its ghost masks).
    pub mask_bits: u32,
    // ---- ShapeBase ----
    /// Energy and recharge rate.
    pub energy: f32,
    pub recharge_rate: f32,
    /// Mass, drag and buoyancy.
    pub mass: f32,
    pub drag: f32,
    pub buoyancy: f32,
    /// Liquid type, liquid height, water coverage and liquid colour.
    pub liquid_type: u32,
    pub liquid_height: f32,
    pub water_coverage: f32,
    pub liquid_color: [f32; 4],
    /// Applied force, gravity multiplier and second drag.
    pub applied_force: F3,
    pub gravity_mod: f32,
    pub drag2: f32,
    /// Damage flash and white-out.
    pub damage_flash: f32,
    pub white_out: f32,
    /// Damage, repair rate and repair reserve.
    pub damage: f32,
    pub repair_rate: f32,
    pub repair_reserve: f32,
    /// Damage state (0 = enabled).
    pub damage_state: u32,
    /// The object this one is mounted on, and the mount node.
    pub mount: Option<ObjectId>,
    pub mount_node: u32,
    /// The trigger states `onTrigger` compares against.
    pub trigger_state: [bool; crate::MAX_TRIGGERS],
    /// Whether each of the mount slots 0-3 has an image (the image step runs for these).
    pub images: [bool; 4],
    /// The image slot 0 wet flag.
    pub image_wet: bool,
    /// The collision queue (newest first).
    pub collisions: Vec<QueuedCollision>,
    /// The controlling client.
    pub client: Option<ClientView>,
    // ---- Player ----
    /// The last move.
    pub last_move: Move,
    /// A flag the update tests (set by the host when a control-object change is pending).
    pub pending_update: bool,
    /// Delta rot, delta head, delta pos, delta rot vec and delta head vec.
    pub delta_rot: F3,
    pub delta_head: F3,
    pub delta_pos: F3,
    pub delta_rot_vec: F3,
    pub delta_head_vec: F3,
    /// Head (pitch, unused, free-look yaw), rot (x, y, yaw) and velocity.
    pub head: F3,
    pub rot: F3,
    pub velocity: F3,
    /// The yaw in its parent's frame of a Player that moves in one (a scene-hierarchy child), turned by its moves as `rot`'s
    /// yaw is and kept in one turn; `None` for any other Player (Torque's have none).
    pub frame_yaw: Option<f32>,
    /// The impact flag, and the flag that blocks `onImpact` when set.
    pub impact_flag: u32,
    pub impact_block: u32,
    /// The per-object speeds: forward, side, back; underwater forward, side, back.
    pub speeds: [f32; 6],
    /// Action state (1 move, 2 recover).
    pub action_state: u32,
    /// Falling.
    pub falling: bool,
    /// Jump delay and jump sound spacing.
    pub jump_delay: i32,
    pub jump_sound: i32,
    /// The contact timer.
    pub contact_timer: u32,
    /// The jump-surface normal.
    pub jump_normal: F3,
    /// Ticks since the jump surface (the words carry `min(this, 15)`).
    pub jump_surface_last: u32,
    /// The control object.
    pub control_object: Option<ObjectId>,
    /// The action animation (the datablock's [`ActionNumbers::none`](data::ActionNumbers::none) for none).
    pub action_anim: u32,
    /// A ceiling contact (a subclass's), in liquid, and out of water once.
    pub ceiling: bool,
    pub in_liquid: bool,
    pub dry: bool,
    /// `findContact`'s snapshot: found, jump, run, the normal.
    pub contact_found: bool,
    pub contact_jump: bool,
    pub contact_run: bool,
    pub contact_normal: F3,
    /// The action thread's last position, and the pending root offset `updatePos` consumes.
    pub death_pos: f32,
    pub pending_offset: F3,
    /// Raw-axes mode: the wish is the move's `x`, `y` and `z` as given, not turned by the yaw.
    pub raw_axes: bool,
    /// The working set's cached box.
    pub working_box: Box3,
    /// The accumulators the tail adds into (cosmetic).
    pub accum: F3,
    pub accum_delta: F3,
    pub accum_hold: bool,
    /// The scaled collision box (`mScaledBox`: the object box times the scale).
    pub scaled_box: Box3,
    /// Moved.
    pub moved: bool,
    /// Whether the working set held a convex when the step ended: what `pickActionAnimation` and the ghost's
    /// move block test, copied for the host's skeleton (which sees only the state).
    pub working_nonempty: bool,
    // ---- AIPlayer ----
    /// An AIPlayer's fields; `None` for a plain Player, whose `getAIMove` answers false.
    pub ai: Option<Box<crate::ai::AiState>>,
    // ---- a subclass ----
    /// The fields a subclass adds ([`PlayerFields`]); `None` for Torque's Player.
    pub ext: Option<Box<dyn PlayerFields>>,
    /// The pose (the engine's own after Torque3D's, [`pose`]): standing unless the datablock allows others; the one before it
    /// and the ticks since it changed.
    pub pose: Pose,
    pub pose_from: Pose,
    pub pose_ticks: u32,
    /// The boxes of the poses the body was still moving from at the last change ([`pose::reach_box`]).
    pub pose_reach: Box3,
}

impl PlayerState {
    /// A Player at `pos` with yaw `yaw`, its box and speeds from `db` (the parts of `onNewDataBlock` the step reads): the
    /// object box spans half the bounding box's x and y on each side and z from 0 to its height.
    pub fn new(id: ObjectId, db: &PlayerData, pos: F3, yaw: f32) -> PlayerState {
        let hx = db.bounding_box[0] * 0.5;
        let hy = db.bounding_box[1] * 0.5;
        let obj_box = Box3::new([-hx, -hy, 0.0], [hx, hy, db.bounding_box[2]]);
        let mut s = PlayerState {
            id,
            obj_to_world: IDENTITY,
            world_to_obj: IDENTITY,
            scale: [1.0, 1.0, 1.0],
            obj_box,
            world_box: Box3::default(),
            world_center: [0.0; 3],
            world_radius: 0.0,
            type_mask: torque_collide::types::PLAYER | torque_collide::types::SHAPE_BASE,
            mask_bits: 0,
            energy: 0.0,
            recharge_rate: 0.0,
            mass: db.mass,
            drag: db.drag,
            buoyancy: 0.0,
            liquid_type: 0,
            liquid_height: 0.0,
            water_coverage: 0.0,
            liquid_color: [0.0; 4],
            applied_force: [0.0; 3],
            gravity_mod: 1.0,
            drag2: 0.0,
            damage_flash: 0.0,
            white_out: 0.0,
            damage: 0.0,
            repair_rate: 0.0,
            repair_reserve: 0.0,
            damage_state: 0,
            mount: None,
            mount_node: 0,
            trigger_state: [false; crate::MAX_TRIGGERS],
            images: [false; 4],
            image_wet: false,
            collisions: Vec::new(),
            client: None,
            last_move: Move::default(),
            pending_update: false,
            delta_rot: [0.0; 3],
            delta_head: [0.0; 3],
            delta_pos: [0.0; 3],
            delta_rot_vec: [0.0; 3],
            delta_head_vec: [0.0; 3],
            head: [0.0; 3],
            rot: [0.0, 0.0, yaw],
            velocity: [0.0; 3],
            frame_yaw: None,
            impact_flag: 0,
            impact_block: 0,
            speeds: [
                db.max_forward_speed,
                db.max_side_speed,
                db.max_backward_speed,
                db.max_underwater_forward_speed,
                db.max_underwater_side_speed,
                db.max_underwater_backward_speed,
            ],
            action_state: 1,
            falling: false,
            jump_delay: 0,
            jump_sound: 0,
            contact_timer: 0,
            jump_normal: [0.0; 3],
            jump_surface_last: 0,
            control_object: None,
            action_anim: db.actions.none,
            ceiling: false,
            in_liquid: false,
            dry: false,
            contact_found: false,
            contact_jump: false,
            contact_run: false,
            contact_normal: [1.0; 3],
            death_pos: 0.0,
            pending_offset: [0.0; 3],
            raw_axes: false,
            working_box: Box3::new([contact::CACHE_SENTINEL; 3], [contact::CACHE_SENTINEL; 3]),
            accum: [0.0; 3],
            accum_delta: [0.0; 3],
            accum_hold: false,
            scaled_box: Box3::default(),
            moved: false,
            working_nonempty: false,
            ai: None,
            ext: None,
            pose: Pose::Stand,
            pose_from: Pose::Stand,
            // Long settled: a new Player is not in the middle of a change.
            pose_ticks: u32::MAX,
            pose_reach: obj_box,
        };
        s.update_scaled_box();
        s.set_transform_raw(pos, s.rot);
        s
    }

    /// The translation (`getPosition`).
    pub fn position(&self) -> F3 {
        [
            self.obj_to_world[3],
            self.obj_to_world[7],
            self.obj_to_world[11],
        ]
    }

    /// The view `collide` reads of this object (for the host's [`torque_collide::World`]).
    pub fn object_view(&self) -> torque_collide::ObjectView {
        torque_collide::ObjectView {
            obj_to_world: self.obj_to_world,
            world_to_obj: self.world_to_obj,
            scale: self.scale,
            obj_box: self.obj_box,
            world_box: self.world_box,
            world_radius: self.world_radius,
            type_mask: self.type_mask,
            collision_disabled: false,
        }
    }

    /// Sets mask bits (`setMaskBits`).
    pub fn set_mask(&mut self, bits: u32) {
        self.mask_bits |= bits;
    }
}

/// The scratch lists and the polyhedron the step reuses, on the heap.
#[derive(Debug, Default)]
pub struct Scratch {
    /// The ground and zone lists.
    pub ground: ExtrudedPolyList,
    pub zone: ExtrudedPolyList,
    /// The early out.
    pub early: EarlyOutPolyList,
    /// `findContact`'s list.
    pub contact: ClippedPolyList,
    /// `stepUp`'s list.
    pub step: ClippedPolyList,
    /// The swept box.
    pub hedron: Polyhedron,
    pub zones: Vec<host::ZoneView>,
}

/// A Player: its state, its working set and the scratch lists.
#[derive(Debug)]
pub struct Player {
    pub s: Box<PlayerState>,
    pub working: WorkingSet,
    pub scratch: Box<Scratch>,
}

impl Clone for Player {
    /// The state and the working set; the scratch lists start empty (they are rebuilt before every read).
    fn clone(&self) -> Self {
        Player {
            s: self.s.clone(),
            working: self.working.clone(),
            scratch: Box::default(),
        }
    }
}

/// The words of `Player::writePacketData`: the energy, the not-mounted flag and when free the position, the velocity and
/// `min(ticks since the jump surface, 15)` in 4 bits, then the head x, the head z and **the rot z**, then the control object's
/// flag and ghost index. With the flags that is 13 words. The host maps the control object to its ghost index and adds its own
/// tail before taking the checksum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerWords {
    pub energy: f32,
    /// `None` when mounted.
    pub free: Option<FreeWords>,
    pub head_x: f32,
    pub head_z: f32,
    pub rot_z: f32,
    pub control_object: Option<ObjectId>,
}

/// The free Player's words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FreeWords {
    pub position: F3,
    pub velocity: F3,
    /// `min(ticks since the jump surface, 15)` (an unsigned compare).
    pub contact: u32,
}

impl PlayerWords {
    /// The words as bit patterns in write order (flags as 0/1), for comparisons.
    pub fn bits(&self) -> Vec<u32> {
        let mut v = vec![self.energy.to_bits(), u32::from(self.free.is_some())];
        if let Some(f) = &self.free {
            v.extend(f.position.iter().map(|x| x.to_bits()));
            v.extend(f.velocity.iter().map(|x| x.to_bits()));
            v.push(f.contact);
        }
        v.extend([
            self.head_x.to_bits(),
            self.head_z.to_bits(),
            self.rot_z.to_bits(),
        ]);
        v.push(u32::from(self.control_object.is_some()));
        v
    }
}

impl Player {
    pub fn new(state: PlayerState) -> Player {
        Player {
            s: Box::new(state),
            working: WorkingSet::new(),
            scratch: Box::default(),
        }
    }

    pub fn id(&self) -> ObjectId {
        self.s.id
    }

    /// `writePacketData`.
    pub fn packet_words(&self, db: &PlayerData, host: &mut dyn PlayerHost) -> PlayerWords {
        PlayerWords {
            energy: shape_base::energy_level(&self.s, db.inherit_energy, host),
            free: self.s.mount.is_none().then(|| FreeWords {
                position: self.s.position(),
                velocity: self.s.velocity,
                contact: self.s.jump_surface_last.min(15),
            }),
            head_x: self.s.head[0],
            head_z: self.s.head[2],
            rot_z: self.s.rot[2],
            control_object: self.s.control_object,
        }
    }

    /// `Player::processTick(move)` for a server object. A spent work budget stops the step where it was (deterministic;
    /// the state is as far as it got).
    pub fn process_tick(
        &mut self,
        db: &PlayerData,
        mv: Option<&Move>,
        host: &mut dyn PlayerHost,
    ) -> Result<(), torque_collide::Exhausted> {
        // The AI hook for a tick with no move: an AIPlayer builds one, a Player answers false.
        let class = host.player_class();
        let class: &dyn PlayerClass = class.as_deref().unwrap_or(&TorquePlayer);
        let mut own: Option<Move> = mv.copied();
        if own.is_none() {
            own = class.ai_move(&mut self.s, host);
        }
        // The control object gets the driver's move first; the Player continues with what is left.
        if let Some(ctrl) = self.s.control_object {
            own = self.drive_control_object(ctrl, own, class, host);
        }
        if let Some(m) = &own {
            class.on_move(&mut self.s, m);
        }
        shape_base::process_tick(&mut self.s, db, own.as_ref(), host);
        // The normal path (warp ticks are a ghost's).
        let m = own.unwrap_or_default();
        host.skeleton().advance_before(&mut self.s, DT);
        let mut result = Ok(());
        if self.s.mount.is_none() {
            result = class.update_working_set(self, db, host);
        }
        // `AIPlayer::updateMove` skips a ghost with no client; a server's object is never one.
        let runs = self
            .s
            .ai
            .as_ref()
            .is_none_or(|_| crate::ai::update_move_runs(false, self.s.client.is_some()));
        if result.is_ok() && runs {
            result = self.update_move(db, &m, host);
        }
        // The host's tether (a grapple), when it gives one: between the velocity `updateMove` made and the sweep along it. None
        // (the default) leaves Torque's step as it is.
        if result.is_ok()
            && let Some(t) = host.tether(&self.s)
        {
            let change = tether::apply(&mut self.s, &t, DT);
            if change != [0.0; 3] {
                host.tethered(&self.s, change);
            }
        }
        if result.is_ok() {
            look::update_look_animation(&mut self.s, db, host);
            look::update_death_offsets(&mut self.s, db, host);
            result = self.update_pos(db, DT, host);
        }
        self.s.working_nonempty = !self.working.is_empty();
        host.skeleton().advance_after(&mut self.s);
        // The accumulators.
        let s = &mut self.s;
        if !s.accum_hold {
            for i in 0..3 {
                s.accum[i] += s.accum_delta[i];
            }
            s.accum_delta = [0.0; 3];
        } else {
            s.accum_hold = false;
        }
        result
    }

    /// The driver path of the Player tick: the control object is ticked with the class's control move (the null move when this
    /// Player is disabled), and the Player continues with the class's own move ([`PlayerClass::split_control_move`]).
    fn drive_control_object(
        &mut self,
        ctrl: ObjectId,
        mv: Option<Move>,
        class: &dyn PlayerClass,
        host: &mut dyn PlayerHost,
    ) -> Option<Move> {
        let Some(mv) = mv else {
            host.tick_controlled(&mut self.s, ctrl, None);
            return None;
        };
        let (n, c) = class.split_control_move(&mut self.s, ctrl, &mv, host);
        let vehicle_move = if self.s.damage_state != 0 {
            Move::default()
        } else {
            c
        };
        host.tick_controlled(&mut self.s, ctrl, Some(&vehicle_move));
        Some(n)
    }
}
