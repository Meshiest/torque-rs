//! AIPlayer: `getAIMove`, the `updateMove` override and the console methods' state effects, over the Player of [`crate::player`].
//!
//! **What an AIPlayer is here.** A [`Player`](crate::player::Player) whose [`PlayerState::ai`](crate::player::PlayerState) holds an
//! [`AiState`]. AIPlayer has no `processTick` of its own; `Player::processTick` asks its class for a move when it was given none
//! ([`PlayerClass::ai_move`](crate::player::PlayerClass::ai_move), whose default is [`get_move`]), then everything else is the
//! Player's.
//!
//! **What `getAIMove` does.** It turns toward the aim point (the aim object's position plus its offset, else the aim location,
//! else the move destination) and pitches toward it when something is aimed at; it walks toward the move destination, the
//! larger axis at full stick and the other in proportion, rotated into the Player's frame, slowed within 5 of the destination
//! when slowdown is on; it watches the aim object's line of sight; and it holds the image triggers it has.
//!
//! **The callback contract** is the Player's (see [`crate::player::host`]): the callbacks (`onReachDestination`, `onMoveStuck`,
//! `onTargetEnterLOS`, `onTargetExitLOS`) are synchronous on the host through
//! [`PlayerHost::ai_callback`](crate::player::PlayerHost::ai_callback) and receive the Player's state as `&mut`; a script may
//! change the AI's fields there, so [`get_move`] re-reads them after every call.
//!
//! **Arithmetic** is plain `f32`.
//!
//! **Differences from Torque:** [`MAX_AI_YAW_WRAPS`] bounds the two yaw-wrap loops; a target id the host no longer knows is
//! dropped like a `SimObjectPtr` whose object was deleted.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/aiPlayer.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

// Comparisons are written so that a NaN takes a fixed side: a negated comparison is the intended form.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

mod console;
pub mod driver;
mod get_move;

#[cfg(test)]
mod tests;

use torque_collide::ObjectId;
use torque_collide::math::{Box3, F3};

pub use console::AiOp;
pub use get_move::get_move;

/// The steps of each of `getAIMove`'s two yaw-wrap loops. Looping until the yaw is within `[-2pi, 2pi]` never ends for a yaw of
/// 2^25 or more (adding 2pi no longer changes the float) or an infinity; past the cap the yaw is left as the last step made it.
pub const MAX_AI_YAW_WRAPS: u32 = 1 << 18;
/// The least tolerance `setMoveTolerance` stores.
pub const MIN_MOVE_TOLERANCE: f32 = 0.1;
/// Within this distance of the destination a slowing AIPlayer scales its speed by `distance / SLOWDOWN_DISTANCE`.
pub const SLOWDOWN_DISTANCE: f32 = 5.0;

/// `mMoveState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoveState {
    /// `ModeStop`: standing.
    #[default]
    Stop,
    /// `ModeMove`: walking to the destination.
    Move,
}

/// What `getAIMove` reads of an aim target (a `GameBase` that the host resolved from the id): its position and its world box
/// (whose centre the line-of-sight ray aims at).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiTarget {
    pub position: F3,
    pub world_box: Box3,
}

/// The AIPlayer's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct AiState {
    /// `mMoveState`, `mMoveSpeed` ([0, 1]), `mMoveTolerance` and `mMoveDestination`.
    pub move_state: MoveState,
    pub move_speed: f32,
    pub move_tolerance: f32,
    pub move_destination: F3,
    /// `mMoveSlowdown`.
    pub move_slowdown: bool,
    /// `mAimObject` (a `SimObjectPtr`: dropped when its object is gone) and `mAimOffset`, added to its position.
    pub aim_object: Option<ObjectId>,
    pub aim_offset: F3,
    /// `mAimLocationSet` and `mAimLocation` (the aim point of the last move).
    pub aim_location_set: bool,
    pub aim_location: F3,
    /// `mTargetInLOS`.
    pub target_in_los: bool,
    /// `mLastLocation`, the stuck test's: the eye position a walk that does not move compares with. Torque never writes it after
    /// the constructor, so the test fires only for an AIPlayer whose eye is exactly there; this crate keeps that.
    pub last_location: F3,
}

impl Default for AiState {
    fn default() -> Self {
        AiState::new()
    }
}

impl AiState {
    /// The constructor's values: speed 1, tolerance 0.25, slowdown on, stopped, nothing aimed at.
    pub fn new() -> AiState {
        AiState {
            move_state: MoveState::Stop,
            move_speed: 1.0,
            move_tolerance: 0.25,
            move_destination: [0.0; 3],
            move_slowdown: true,
            aim_object: None,
            aim_offset: [0.0; 3],
            aim_location_set: false,
            aim_location: [0.0; 3],
            target_in_los: false,
            last_location: [0.0; 3],
        }
    }
}

/// `AIPlayer::updateMove`: `if (no client && isGhost) return; else Player::updateMove`. The Player of this crate is a
/// server's (never a ghost), so this is always true there; it is the rule for a host that also steps ghosts.
pub fn update_move_runs(is_ghost: bool, has_client: bool) -> bool {
    has_client || !is_ghost
}
