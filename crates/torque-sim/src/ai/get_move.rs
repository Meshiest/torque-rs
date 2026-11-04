//! `AIPlayer::getAIMove`: the Move an AIPlayer feeds its own `processTick`.
//!
//! The sections, in order: the setup (the move starts as the null move, the location is the eye's), the aim point with the yaw
//! and pitch toward it, the walk toward the destination (reach, direction, the rotation into the Player's frame, the speed, the
//! stuck test), the aim object's line of sight, and the image triggers.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/aiPlayer.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::{F3, set_euler};

use super::{AiState, MAX_AI_YAW_WRAPS, MoveState, SLOWDOWN_DISTANCE};
use crate::Move;
use crate::player::{PlayerHost, PlayerState, eye_transform};

/// `isZero`'s epsilon.
const EPS: f32 = 0.000_001;
const PI: f32 = std::f32::consts::PI;
const TWO_PI: f32 = std::f32::consts::TAU;
/// The pitch dead zone.
const PITCH_DEAD: f32 = 0.01;

fn is_zero(x: f32) -> bool {
    x.abs() < EPS
}

/// `mAtan(x, y)`: `atan2(x, y)`.
fn atan(x: f32, y: f32) -> f32 {
    x.atan2(y)
}

fn ai(s: &mut PlayerState) -> Option<&mut AiState> {
    s.ai.as_deref_mut()
}

/// The `AIPlayer` hook: `None` when this is not an AIPlayer (no [`AiState`]); `Some(move)` otherwise (it always answers
/// true).
pub fn get_move(s: &mut PlayerState, host: &mut dyn PlayerHost) -> Option<Move> {
    s.ai.as_ref()?;
    let mut mv = Move::default();
    let eye = eye_transform(s, host.skeleton());
    let loc: F3 = [eye[3], eye[7], eye[11]];
    let rot_z = s.rot[2];
    // A deleted aim object is a null pointer.
    if let Some(id) = ai(s)?.aim_object
        && host.ai_target(id).is_none()
    {
        ai(s)?.aim_object = None;
    }
    aim(s, host, &mut mv, loc, rot_z)?;
    walk(s, host, &mut mv, loc, rot_z)?;
    line_of_sight(s, host)?;
    for slot in 0..mv.triggers.len() {
        mv.triggers[slot] =
            s.images.get(slot).copied().unwrap_or(false) && host.image_trigger(s, slot);
    }
    Some(mv)
}

/// The aim point, and the yaw and pitch toward it.
fn aim(
    s: &mut PlayerState,
    host: &mut dyn PlayerHost,
    mv: &mut Move,
    loc: F3,
    rot_z: f32,
) -> Option<()> {
    let head_x = s.head[0];
    let a = s.ai.as_deref()?;
    if !(a.aim_object.is_some() || a.aim_location_set || a.move_state == MoveState::Move) {
        mv.pitch = -head_x;
        return Some(());
    }
    let target = match a.aim_object {
        Some(id) => host.ai_target(id),
        None => None,
    };
    let a = ai(s)?;
    if let Some(t) = target {
        a.aim_location = std::array::from_fn(|i| t.position[i] + a.aim_offset[i]);
    } else if !a.aim_location_set {
        a.aim_location = a.move_destination;
    }
    let x_diff = a.aim_location[0] - loc[0];
    let y_diff = a.aim_location[1] - loc[1];
    if is_zero(x_diff) && is_zero(y_diff) {
        return Some(());
    }
    let mut cur = rot_z;
    for _ in 0..MAX_AI_YAW_WRAPS {
        if !(cur > TWO_PI) {
            break;
        }
        cur -= TWO_PI;
    }
    for _ in 0..MAX_AI_YAW_WRAPS {
        if !(cur < -TWO_PI) {
            break;
        }
        cur += TWO_PI;
    }
    let new_yaw = atan(x_diff, y_diff);
    let mut diff = new_yaw - cur;
    if diff < 0.0 {
        diff += TWO_PI;
    } else if diff >= TWO_PI {
        diff -= TWO_PI;
    }
    if diff > PI {
        diff -= TWO_PI;
    } else if diff < -PI {
        diff += TWO_PI;
    }
    mv.yaw = diff;
    if a.aim_object.is_none() && !a.aim_location_set {
        mv.pitch = -head_x;
    } else {
        let vert = a.aim_location[2] - loc[2];
        let horz = (x_diff * x_diff + y_diff * y_diff).sqrt();
        let new_pitch = atan(horz, vert) - PI / 2.0;
        if new_pitch.abs() > PITCH_DEAD {
            mv.pitch = new_pitch - head_x;
        }
    }
    Some(())
}

/// The walk toward the destination: reached within the tolerance on both axes (`onReachDestination`), else the larger axis at
/// full stick and the other in proportion, turned into the Player's frame by the yaw it is about to make, scaled by the speed (and
/// by `distance / 5` within 5 when slowing), and the stuck test.
fn walk(
    s: &mut PlayerState,
    host: &mut dyn PlayerHost,
    mv: &mut Move,
    loc: F3,
    rot_z: f32,
) -> Option<()> {
    let a = s.ai.as_deref()?;
    if a.move_state != MoveState::Move {
        return Some(());
    }
    let dest = a.move_destination;
    let x_diff = dest[0] - loc[0];
    let y_diff = dest[1] - loc[1];
    if x_diff.abs() < a.move_tolerance && y_diff.abs() < a.move_tolerance {
        ai(s)?.move_state = MoveState::Stop;
        host.ai_callback(s, "onReachDestination");
        return Some(());
    }
    let toward = |here: f32, there: f32, v: f32| if here > there { -v } else { v };
    if is_zero(x_diff) {
        mv.y = toward(loc[1], dest[1], 1.0);
    } else if is_zero(y_diff) {
        mv.x = toward(loc[0], dest[0], 1.0);
    } else if x_diff.abs() > y_diff.abs() {
        let v = (y_diff / x_diff).abs();
        mv.y = toward(loc[1], dest[1], v);
        mv.x = toward(loc[0], dest[0], 1.0);
    } else {
        let v = (x_diff / y_diff).abs();
        mv.x = toward(loc[0], dest[0], v);
        mv.y = toward(loc[1], dest[1], 1.0);
    }
    // Into the Player's frame: the inverse of the yaw it is about to have.
    let m = set_euler([0.0, 0.0, -(rot_z + mv.yaw)]);
    let (x, y) = (mv.x, mv.y);
    mv.x = m[0] * x + m[1] * y;
    mv.y = m[4] * x + m[5] * y;
    let a = s.ai.as_deref()?;
    let mut speed = a.move_speed;
    if a.move_slowdown {
        let dist = (x_diff * x_diff + y_diff * y_diff).sqrt();
        if dist < SLOWDOWN_DISTANCE {
            speed *= dist / SLOWDOWN_DISTANCE;
        }
    }
    mv.x *= speed;
    mv.y *= speed;
    if loc == a.last_location {
        host.ai_callback(s, "onMoveStuck");
        ai(s)?.move_state = MoveState::Stop;
    }
    Some(())
}

/// The aim object's line of sight from the eye to its box centre, against the static world (`ai_los`): entering and leaving it
/// call `onTargetEnterLOS` and `onTargetExitLOS`.
fn line_of_sight(s: &mut PlayerState, host: &mut dyn PlayerHost) -> Option<()> {
    let Some(id) = s.ai.as_deref()?.aim_object else {
        return Some(());
    };
    let Some(t) = host.ai_target(id) else {
        return Some(());
    };
    let eye = eye_transform(s, host.skeleton());
    let start: F3 = [eye[3], eye[7], eye[11]];
    let b = t.world_box;
    let centre: F3 = std::array::from_fn(|i| (b.min[i] + b.max[i]) * 0.5);
    let mask = host.type_masks().ai_los;
    let blocked = host.cast_ray(start, centre, mask).is_some();
    let in_los = s.ai.as_deref()?.target_in_los;
    if blocked {
        if in_los {
            host.ai_callback(s, "onTargetExitLOS");
            ai(s)?.target_in_los = false;
        }
    } else if !in_los {
        host.ai_callback(s, "onTargetEnterLOS");
        ai(s)?.target_in_los = true;
    }
    Some(())
}
