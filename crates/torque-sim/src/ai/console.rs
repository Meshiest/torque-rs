//! The AIPlayer console methods' effects on [`AiState`]: `stop`, `clearAim`, `setMoveSpeed`, `setMoveTolerance`,
//! `setMoveDestination`, `getMoveDestination`, `setAimLocation`, `getAimLocation`, `setAimObject` and `getAimObject`. The host
//! parses the arguments (and resolves an object name or id) and calls these.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/aiPlayer.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::ObjectId;
use torque_collide::math::F3;

use super::{AiState, MIN_MOVE_TOLERANCE, MoveState};

/// `getMax(a, b)`: `a > b ? a : b` (a NaN `b` passes).
fn get_max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// `getMin(a, b)`: `a < b ? a : b` (a NaN `b` passes).
fn get_min(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

impl AiState {
    /// `stop()`: the move state goes to stop.
    pub fn stop(&mut self) {
        self.move_state = MoveState::Stop;
    }

    /// `clearAim()`: no aim object, no aim location, no offset (the AIPlayer looks where it walks).
    pub fn clear_aim(&mut self) {
        self.aim_object = None;
        self.aim_location_set = false;
        self.aim_offset = [0.0; 3];
    }

    /// `setMoveSpeed(speed)`: `getMax(0, getMin(1, speed))`.
    pub fn set_move_speed(&mut self, v: f32) {
        self.move_speed = get_max(0.0, get_min(1.0, v));
    }

    /// `setMoveTolerance(tolerance)`: `getMax(0.1, tolerance)`.
    pub fn set_move_tolerance(&mut self, v: f32) {
        self.move_tolerance = get_max(MIN_MOVE_TOLERANCE, v);
    }

    /// `setMoveDestination(goal, slowdown = true)`: walk there.
    pub fn set_move_destination(&mut self, dest: F3, slowdown: bool) {
        self.move_destination = dest;
        self.move_state = MoveState::Move;
        self.move_slowdown = slowdown;
    }

    /// `getMoveDestination()`.
    pub fn move_destination(&self) -> F3 {
        self.move_destination
    }

    /// `setAimLocation(target)`: aim there, at no object.
    pub fn set_aim_location(&mut self, loc: F3) {
        self.aim_object = None;
        self.aim_location_set = true;
        self.aim_location = loc;
        self.aim_offset = [0.0; 3];
    }

    /// `getAimLocation()`: the aim point of the last move.
    pub fn aim_location(&self) -> F3 {
        self.aim_location
    }

    /// `setAimObject(obj [, offset])`: aim at the object (`None` when the name finds none), out of sight until the next look.
    pub fn set_aim_object(&mut self, obj: Option<ObjectId>, offset: F3) {
        self.aim_object = obj;
        self.target_in_los = false;
        self.aim_offset = offset;
    }

    /// `getAimObject()`: the id, `None` for none (the script sees -1).
    pub fn aim_object_id(&self) -> Option<ObjectId> {
        self.aim_object
    }
}

/// One console call on an AIPlayer, for the driver.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AiOp {
    Stop,
    ClearAim,
    SetMoveSpeed(f32),
    SetMoveTolerance(f32),
    SetMoveDestination(F3, bool),
    SetAimLocation(F3),
    SetAimObject(Option<ObjectId>, F3),
}

impl AiOp {
    /// Applies the call to `ai`.
    pub fn apply(&self, ai: &mut AiState) {
        match *self {
            AiOp::Stop => ai.stop(),
            AiOp::ClearAim => ai.clear_aim(),
            AiOp::SetMoveSpeed(v) => ai.set_move_speed(v),
            AiOp::SetMoveTolerance(v) => ai.set_move_tolerance(v),
            AiOp::SetMoveDestination(p, slow) => ai.set_move_destination(p, slow),
            AiOp::SetAimLocation(p) => ai.set_aim_location(p),
            AiOp::SetAimObject(o, off) => ai.set_aim_object(o, off),
        }
    }
}
