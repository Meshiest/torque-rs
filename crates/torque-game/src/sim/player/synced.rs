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
//! - **The walk** (the engine's own, on both sides): a shape with a `walk` sequence that moves it forward walks at walking
//!   pace instead of playing `run` or `back` slowly ([`walk_or`]); its thread steps by the ground covered like a moving
//!   action's, held to [`WALK_MAX_TIME_SCALE`] rather than `maxTimeScale` ([`walk_limit`] keeps it near 1). An action that
//!   only turns around (a walk forward into one backward) is set again on both sides.
//!
//! The server reads it from the datablock's `hitboxes` field; the client from its host's option (a client cannot read a
//! datablock's dynamic fields). With neither, both sides are TGE's.

use collide::math::F3;

use super::ground::ActionMotion;

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

/// The name of the engine's walk action.
pub const WALK: &[u8] = b"walk";
/// The share past [`walk_limit`] a Player must go to change from walking to the faster action (and under it to change back).
pub const WALK_MARGIN: f32 = 0.05;
/// The fastest the walk plays: past any rate its limit allows (about 1.6 for a walk and a run 2.4 times its speed), so only a
/// jump in position (a teleport) reaches it.
pub const WALK_MAX_TIME_SCALE: f32 = 2.5;
/// The table's run and back actions, which the walk stands in for at walking pace.
const RUN: usize = 1;
const BACK: usize = 2;

/// The speed under which a Player walks instead of playing a faster moving action: the geometric mean of the two actions'
/// own speeds, where each plays the same share off its pace (the walk faster than its own, the other slower).
pub fn walk_limit(walk_speed: f32, faster_speed: f32) -> f32 {
    (walk_speed * faster_speed).sqrt()
}

/// The walk action of an action list (`name`, whether it has a sequence, and its ground motion, in the list's order; the
/// first `fixed` are the table's): `walk`, after the table, with a sequence whose ground motion goes forward.
pub fn walk_action<'a>(
    actions: impl IntoIterator<Item = (&'a [u8], bool, &'a ActionMotion)>,
    fixed: usize,
) -> Option<usize> {
    actions
        .into_iter()
        .enumerate()
        .skip(fixed)
        .find_map(|(i, (name, has, m))| {
            (name.eq_ignore_ascii_case(WALK) && has && m.speed > 0.0 && m.dir[1] > 0.5).then_some(i)
        })
}

/// The walk in place of the pick `(chosen, forward)` at `speed` along the chosen action's direction, with the action now
/// playing `now` (and whether forward) and `speed_of` each action's ground speed: `run` under its limit walks forward,
/// `back` under its limit walks backward, past the limit by [`WALK_MARGIN`] from a walk and under it by as much from
/// anything else.
pub fn walk_or(
    chosen: usize,
    forward: bool,
    speed: f32,
    walk: usize,
    speed_of: impl Fn(usize) -> Option<f32>,
    now: (usize, bool),
) -> (usize, bool) {
    if !matches!(chosen, RUN | BACK) {
        return (chosen, forward);
    }
    let (Some(w), Some(c)) = (speed_of(walk), speed_of(chosen)) else {
        return (chosen, forward);
    };
    let ahead = chosen == RUN;
    let walking = now == (walk, ahead);
    let limit = walk_limit(w, c)
        * if walking {
            1.0 + WALK_MARGIN
        } else {
            1.0 - WALK_MARGIN
        };
    if speed < limit {
        (walk, ahead)
    } else {
        (chosen, forward)
    }
}

/// Whether a change from action `from` to `to` (each with whether it plays forward) keeps the thread's place in the loop:
/// between the walk and `run`, both forward (their feet land at the same places in the loop).
pub fn keeps_phase(from: (usize, bool), to: (usize, bool), walk: Option<usize>) -> bool {
    let gait = |(a, f): (usize, bool)| f && (a == RUN || Some(a) == walk);
    gait(from) && gait(to)
}
