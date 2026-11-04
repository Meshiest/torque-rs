//! Synced poses: what the server of a hitbox Player ([`crate::scene::hitboxes`]) and a client that asks for it both do with the
//! action thread, so the pose the client draws is the pose the server casts hits at.
//!
//! TGE's two sides differ: its server never advances a Player's action thread (unless the first-person flag says so), picks
//! `root` after `sContactTickTime` ticks off the ground and plays a jump only for a move's jump trigger, and pins the spine bones;
//! its client plays each action from when it saw it, blends it in, plays a jump when it sees the Player rise, and steps a moving
//! action at the velocity's rate. Synced, both sides follow the client's rules:
//!
//! - **The pick**: falling plays `fall`; otherwise the moving action whose ground direction best fits the velocity (over 0.1,
//!   `side` also backwards), else `root`. No contact timer.
//! - **The jump**: an upward speed passing [`JUMP_RISE_SPEED`] from below plays `jump` (`standjump` under [`STAND_JUMP_SPEED`]
//!   along the ground, when the shape has it) to its end. The server keeps the jump its move's trigger starts (the same tick
//!   for any jump that clears the rise speed).
//! - **The blend**: a new action blends in over [`TRANSITION`] seconds from the pose the threads give then
//!   (`transitionToSequence`), on both sides.
//! - **The phase**: a moving action (`run`, `back`, `side`) steps by the ground the Player covered along its direction (the change
//!   of its position over the step, turned into its own frame) over the action's speed, at most `maxTimeScale` times the step's
//!   time ([`step_scale`]); every other action by time. A new action starts at its start on both sides at the same change of
//!   state, so the two phases differ only by the ground covered between the server's tick and the update that showed the client
//!   that change (one update's worth: a tick at the arena's packet rate), and they do not drift apart over a long run: both
//!   follow the same path, not two clocks.
//! - **The bones**: the server lets the spine bones go, as a client draws another Player.
//!
//! The server reads it from the datablock's `hitboxes` field; the client from its host's option (a client cannot read a
//! datablock's dynamic fields). With neither, both sides are TGE's.

use collide::math::F3;

/// `sAnimationTransitionTime`: the seconds a new action blends in over.
pub const TRANSITION: f32 = 0.25;
/// An upward speed past this (units a second), from below it, is a jump leaving the ground.
pub const JUMP_RISE_SPEED: f32 = 3.0;
/// Below this speed along the ground a jump is a standing jump.
pub const STAND_JUMP_SPEED: f32 = 0.5;

/// Whether the upward speed `vz` passed [`JUMP_RISE_SPEED`] from `last` (the step before).
pub fn rose(last: f32, vz: f32) -> bool {
    vz > JUMP_RISE_SPEED && last <= JUMP_RISE_SPEED
}

/// Whether a jump at `velocity` is a standing one.
pub fn standing_jump(velocity: F3) -> bool {
    velocity[0].hypot(velocity[1]) < STAND_JUMP_SPEED
}

/// The rate of a moving action of ground direction `dir` and speed `speed` over a step of `dt` seconds in which the Player moved
/// `step` (its own frame): the ground covered along `dir` over the speed's, at most `max`; 1 for no time or no speed.
pub fn step_scale(step: F3, dt: f32, dir: F3, speed: f32, max: f32) -> f32 {
    if dt <= 0.0 || speed == 0.0 {
        return 1.0;
    }
    let along = step[0] * dir[0] + step[1] * dir[1] + step[2] * dir[2];
    (along / dt / speed).abs().min(max)
}
