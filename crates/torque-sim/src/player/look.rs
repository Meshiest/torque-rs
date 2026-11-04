//! `updateLookAnimation` and `updateDeathOffsets`.
//!
//! The look animation writes thread positions only (no packet word reads them; the eye and muzzle chains do). TGE's: the arm
//! and the head at the pitch's place between `minLookAngle` and `maxLookAngle`, the arm at 0.5 while the Player controls
//! another object, the headside at the yaw's place between `-maxFreelookAngle` and `maxFreelookAngle`, each clamped to 0..1.
//! A host replaces the whole step through the [`library`](crate::library) table.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::Matrix;

use super::PlayerState;
use super::data::PlayerData;
use super::ext::{PlayerClass, TorquePlayer};
use super::host::{PlayerHost, Thread};

/// `mClampF(v, 0, 1)`: a NaN is 0.
fn clamp01(v: f32) -> f32 {
    let v = if v > 1.0 { 1.0 } else { v };
    if v > 0.0 { v } else { 0.0 }
}

/// Moves the thread to `target` (by `target - pos`).
fn drive(host: &mut dyn PlayerHost, t: Thread, target: f32) {
    let sk = host.skeleton();
    if !sk.has_thread(t) {
        return;
    }
    let pos = sk.thread_pos(t);
    sk.advance_thread(t, target - pos);
}

/// `updateLookAnimation`, through the [`library`](crate::library) table.
pub(super) fn update_look_animation(
    s: &mut PlayerState,
    db: &PlayerData,
    host: &mut dyn PlayerHost,
) {
    (crate::library::get().player_look_animation)(s, db, host);
}

/// `updateLookAnimation` in plain arithmetic: [`PLAIN`](crate::library::PLAIN)'s entry.
pub fn plain_update_look_animation(
    s: &mut PlayerState,
    db: &PlayerData,
    host: &mut dyn PlayerHost,
) {
    let look = (s.head[0] - db.min_look_angle) / (db.max_look_angle - db.min_look_angle);
    if host.skeleton().has_thread(Thread::Arm) {
        let t = if s.control_object.is_some() {
            0.5
        } else {
            let class = host.player_class();
            let class: &dyn PlayerClass = class.as_deref().unwrap_or(&TorquePlayer);
            clamp01(class.arm_pos(s, look))
        };
        drive(host, Thread::Arm, t);
    }
    if host.skeleton().has_thread(Thread::Head) {
        drive(host, Thread::Head, clamp01(look));
    }
    if host.skeleton().has_thread(Thread::HeadSide) {
        let m = db.max_freelook_angle;
        drive(host, Thread::HeadSide, clamp01((s.head[2] + m) / (2.0 * m)));
    }
}

/// A death action that moves the root hands its displacement to `updatePos` and keeps the thread position; anything else resets
/// both.
pub(super) fn update_death_offsets(
    s: &mut PlayerState,
    db: &PlayerData,
    host: &mut dyn PlayerHost,
) {
    let sk = host.skeleton();
    if sk.has_shape() && sk.has_thread(Thread::Action) && db.action_is_death(s.action_anim) {
        let cur = sk.thread_pos(Thread::Action);
        let m: Matrix = if sk.thread_in_transition(Thread::Action) {
            torque_collide::math::IDENTITY
        } else {
            sk.root_displacement(s.death_pos, cur)
        };
        s.pending_offset = [m[3], m[7], m[11]];
        s.death_pos = cur;
        return;
    }
    s.death_pos = 0.0;
    s.pending_offset = [0.0; 3];
}
