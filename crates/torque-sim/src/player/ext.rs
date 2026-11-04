//! A subclass of the Player, Torque's way of adding to a class: the fields it adds to each object ([`PlayerFields`], held in
//! [`PlayerState::ext`](super::PlayerState::ext)) and the overrides of the step its class makes ([`PlayerClass`], which the
//! host hands the step through [`PlayerHost::player_class`]).
//!
//! Every [`PlayerClass`] method is called at the one point of the step it replaces, and its default is what Torque's Player does
//! there; a method with nothing to do by default is an extension point.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use std::any::Any;
use std::fmt::Debug;

use torque_collide::Exhausted;
use torque_collide::math::{Box3, F3};

use super::{Player, PlayerData, PlayerHost, PlayerState};
use crate::Move;

/// The fields a subclass adds to each Player. They travel with the state: cloned, compared and printed with it. Any
/// `Clone + PartialEq + Debug` type is one; the subclass reads its own back with [`PlayerState::fields`] and
/// [`PlayerState::fields_mut`].
pub trait PlayerFields: Any + Debug {
    fn clone_box(&self) -> Box<dyn PlayerFields>;
    fn eq_fields(&self, other: &dyn PlayerFields) -> bool;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any + Debug + Clone + PartialEq> PlayerFields for T {
    fn clone_box(&self) -> Box<dyn PlayerFields> {
        Box::new(self.clone())
    }
    fn eq_fields(&self, other: &dyn PlayerFields) -> bool {
        other.as_any().downcast_ref::<T>() == Some(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Clone for Box<dyn PlayerFields> {
    fn clone(&self) -> Self {
        (**self).clone_box()
    }
}

impl PartialEq for Box<dyn PlayerFields> {
    fn eq(&self, other: &Self) -> bool {
        (**self).eq_fields(&**other)
    }
}

impl PlayerState {
    /// The subclass's fields of type `T`, if this Player has them.
    pub fn fields<T: Any>(&self) -> Option<&T> {
        self.ext.as_deref()?.as_any().downcast_ref()
    }

    /// The subclass's fields of type `T`, mutably.
    pub fn fields_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.ext.as_deref_mut()?.as_any_mut().downcast_mut()
    }
}

/// A subclass's overrides of the Player's step, one method per point of the step. The class's own data (datablock fields
/// beyond `PlayerData`'s) lives in the implementing value; its per-object fields in [`PlayerState::ext`](super::PlayerState::ext).
pub trait PlayerClass: Debug {
    /// `processTick` with no move given: the move an AI of the class builds (`getAIMove`), `None` for none. Torque's is
    /// AIPlayer's ([`crate::ai::get_move`], for a Player with an [`AiState`](crate::ai::AiState)).
    fn ai_move(&self, s: &mut PlayerState, host: &mut dyn PlayerHost) -> Option<Move> {
        crate::ai::get_move(s, host)
    }

    /// `processTick` with a control object: the move split into the Player's own (first) and the control object's (second).
    /// Torque's: the Player keeps only the jump trigger when mounted (the control object loses it) and, with free look, the
    /// look angles (the control object gets none and no free look).
    fn split_control_move(
        &self,
        s: &mut PlayerState,
        _ctrl: torque_collide::ObjectId,
        mv: &Move,
        _host: &mut dyn PlayerHost,
    ) -> (Move, Move) {
        let mut p = Move::default();
        let mut c = *mv;
        if s.mount.is_some() {
            p.triggers[2] = mv.triggers[2];
            c.triggers[2] = false;
        }
        if mv.free_look {
            p.yaw = mv.yaw;
            p.pitch = mv.pitch;
            p.roll = mv.roll;
            p.free_look = true;
            c.free_look = false;
            c.yaw = 0.0;
            c.pitch = 0.0;
            c.roll = 0.0;
        }
        (p, c)
    }

    /// `processTick`, before `ShapeBase::processTick`, with the move the Player runs this tick.
    fn on_move(&self, _s: &mut PlayerState, _mv: &Move) {}

    /// `updateMove`, after the image triggers and before the head and rotation: a pose change of the class's own (its box,
    /// its threads).
    fn update_pose(
        &self,
        _p: &mut Player,
        _db: &PlayerData,
        _mv: &Move,
        _host: &mut dyn PlayerHost,
    ) -> Result<(), Exhausted> {
        Ok(())
    }

    /// `updateMove`'s head and rotation (run while the Player is enabled): the look angles, the yaw, their wrap and the deltas.
    /// TGE's: the pitch clamped to the look angles; free look (mounted on node 0, or a third-person controlling client) turns
    /// the head within `maxFreelookAngle`, else the body turns and the head eases back to the front.
    fn head_and_rotation(
        &self,
        s: &mut PlayerState,
        db: &PlayerData,
        mv: &Move,
        _host: &mut dyn PlayerHost,
    ) {
        super::update_move::head_and_rotation(s, db, mv);
    }

    /// `updateMove`'s speed cap of the wish. TGE's: the larger of the forward (or backward) speed times `|y|` and the side
    /// speed times `|x|`, from the datablock, the underwater speeds at a water coverage of 0.9 or more.
    fn move_speed(&self, s: &PlayerState, db: &PlayerData, mv: &Move) -> f32 {
        super::update_move::move_speed(s, db, mv)
    }

    /// `updateMove` on a run surface (`n` the contact normal; `w` the wish, `cap` the speed cap, `acc` holds the gravity term):
    /// the contact timer, the acceleration toward the wish along the surface. TGE's: the gravity into the surface removed, the
    /// energy gate, the wish made parallel to the surface and the run force toward it.
    fn run_surface(
        &self,
        s: &mut PlayerState,
        db: &PlayerData,
        w: &F3,
        cap: f32,
        acc: &mut F3,
        n: F3,
    ) {
        super::update_move::run_surface(s, db, w, cap, acc, n);
    }

    /// `updateMove` off a run surface, after the contact timer: acceleration the class adds in the air. `w` is the wish (the
    /// jump and the swim read it after), `acc` holds the gravity term `gz`. None by default.
    #[allow(clippy::too_many_arguments)]
    fn air_move(
        &self,
        _s: &mut PlayerState,
        _db: &PlayerData,
        _w: &mut F3,
        _cap: f32,
        _acc: &mut F3,
        _gz: f32,
    ) {
    }

    /// `updateMove` after the jump and before swimming: a thrust of the class's own. None by default.
    fn thrust(&self, _s: &mut PlayerState, _db: &PlayerData, _mv: &Move, _w: &F3, _acc: &mut F3) {}

    /// `updateMove` after the thrust: the acceleration a class adds in water. None by default (the Player in water takes the
    /// underwater speeds, [`PlayerClass::move_speed`], and the container's drag and buoyancy).
    fn swim(&self, _s: &mut PlayerState, _db: &PlayerData, _mv: &Move, _w: &mut F3, _acc: &mut F3) {
    }

    /// `updateMove`'s integration: the applied force and `acc` into the velocity, the speed limits, buoyancy, drag and the
    /// falling flag (`run`: on a run surface). TGE's: the horizontal and upward resistances, buoyancy unless standing still on
    /// a run surface, the container drag `velocity * drag * dt`.
    fn integrate(&self, s: &mut PlayerState, db: &PlayerData, _mv: &Move, acc: &F3, run: bool) {
        super::update_move::integrate(s, db, acc, run);
    }

    /// The trigger that dismounts a mounted Player (`doDismount`). TGE's: 2, the jump trigger.
    fn dismount_trigger(&self) -> usize {
        2
    }

    /// `Player::canJump`, which the jump asks. TGE's: [`super::update_move::can_jump`].
    fn can_jump(&self, s: &PlayerState, db: &PlayerData) -> bool {
        super::update_move::can_jump(s, db)
    }

    /// `findContact(&run, &jump, &normal)`: `run` and `jump` come cleared; `normal` keeps the caller's value when nothing is
    /// found. TGE's: [`super::contact::find_contact`].
    fn find_contact(
        &self,
        p: &mut Player,
        db: &PlayerData,
        run: &mut bool,
        jump: &mut bool,
        normal: &mut F3,
        host: &mut dyn PlayerHost,
    ) -> Result<(), Exhausted> {
        super::contact::find_contact(p, db, run, jump, normal, host)
    }

    /// `updateWorkingCollisionSet`. TGE's: [`super::contact::update_working_collision_set`].
    fn update_working_set(
        &self,
        p: &mut Player,
        db: &PlayerData,
        host: &mut dyn PlayerHost,
    ) -> Result<(), Exhausted> {
        super::contact::update_working_collision_set(p, db, host)
    }

    /// `updatePos`: whether the step `delta` is small enough to try the early out (no polygons near the end). TGE's: each
    /// axis of `delta` is shorter than the object box's side.
    fn early_out_fits(&self, s: &PlayerState, delta: F3) -> bool {
        let b = s.obj_box;
        (0..3).all(|k| delta[k].abs() < b.max[k] - b.min[k])
    }

    /// `updateWorkingCollisionSet`: the object-space box of the Player's convex, before the transform and the growth by the
    /// step. TGE's: the object box times the scale.
    fn working_box(&self, s: &PlayerState) -> Box3 {
        scaled(s.obj_box, s.scale)
    }

    /// `step`: the box at `pos` moved by `step`, whose top the ledge search starts from. TGE's: the object box.
    fn step_box(&self, s: &PlayerState, pos: F3, step: F3) -> Box3 {
        let b = s.obj_box;
        Box3::new(
            std::array::from_fn(|k| b.min[k] + step[k] + pos[k]),
            std::array::from_fn(|k| b.max[k] + step[k] + pos[k]),
        )
    }

    /// `updateContainer`: the box the PhysicalZones are searched in. TGE's: the world box.
    fn container_box(&self, s: &PlayerState) -> Box3 {
        s.world_box
    }

    /// `Player::buildPolyList` for a PhysicalZone's test: the box (in object space, at the position, unrotated and unscaled).
    /// TGE's: the object box.
    fn zone_box(&self, s: &PlayerState) -> Box3 {
        s.obj_box
    }

    /// The vertical extent a water zone's coverage is measured over: the bottom and top z. TGE's: the world box's.
    fn liquid_extent(&self, s: &PlayerState) -> (f32, f32) {
        (s.world_box.min[2], s.world_box.max[2])
    }

    /// `updateLookAnimation`: the arm thread's position `pos` from the pitch, before it is clamped to 0..1. TGE's keeps it.
    fn arm_pos(&self, s: &PlayerState, pos: f32) -> f32 {
        let _ = s;
        pos
    }
}

/// `box * scale` per corner.
pub fn scaled(b: Box3, k: F3) -> Box3 {
    Box3::new(
        std::array::from_fn(|i| b.min[i] * k[i]),
        std::array::from_fn(|i| b.max[i] * k[i]),
    )
}

/// Torque's Player: every override at its default.
#[derive(Debug)]
pub struct TorquePlayer;

impl PlayerClass for TorquePlayer {}
