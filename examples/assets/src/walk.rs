//! The figure's `walk`, worked out from where its feet stand rather than keyed by hand, so that a foot on the ground stays
//! put while the body passes over it.
//!
//! Each leg spends [`STANCE`] of the loop on the ground and the rest swinging. On the ground, the foot lands on its heel with
//! the toe up ([`STRIKE_PITCH`]), rolls flat, stays flat while the body passes over it, then rolls up onto its toe
//! ([`PUSH_PITCH`]) and leaves: the heel's or the toe's bottom edge, whichever the foot stands on, is pinned to the ground, and
//! the ground slides back under the hips at [`WALK_SPEED`]. The hips ride a little lower as both feet are down and higher as
//! one passes under them ([`HIP_HEIGHT`], [`HIP_BOB`]). The thigh and the shin reach from the hip to the ankle with the knee
//! bent forward (a two-bone reach), and the ankle turns the foot to its pitch. Off the ground the ankle travels from where the
//! foot left to where it lands, lifted [`LIFT`] at the middle, the foot turning with the shin and a little up to clear the
//! ground.
//!
//! The left foot lands at a quarter of the loop and the right at three quarters, where `run` has each leg at its furthest
//! forward, so the two can blend at the same position.

use std::f32::consts::{PI, TAU};

use crate::shapes::{FOOT_HEEL, FOOT_TOE, SHIN, SOLE, THIGH, WALK_SPEED};

/// The loop's length (seconds): two steps.
pub const WALK_DURATION: f32 = 0.65;
/// The share of the loop each foot is on the ground (both are down for a tenth around each landing).
pub const STANCE: f32 = 0.6;
/// How far ahead of the hips the heel lands (units).
pub const STRIKE_AHEAD: f32 = 0.24;
/// The foot's pitch as its heel lands and as its toe leaves (degrees, toe up positive).
pub const STRIKE_PITCH: f32 = 16.0;
pub const PUSH_PITCH: f32 = -34.0;
/// The shares of the time on the ground the foot takes to roll flat from its heel, and stays flat until.
const HEEL_ROLL: f32 = 0.15;
const FLAT_UNTIL: f32 = 0.5;
/// The hips' mean height and how far they rise and fall about it (standing, they are at `THIGH + SHIN`).
pub const HIP_HEIGHT: f32 = 0.92;
pub const HIP_BOB: f32 = 0.018;
/// How high the ankle lifts at the middle of the swing, and how far the foot turns up there (degrees).
pub const LIFT: f32 = 0.07;
const SWING_TOE_UP: f32 = 10.0;
/// The loop position at which the left heel lands (the right's half a loop later).
pub const LEFT_LANDS: f32 = 0.25;

/// The ground covered over one loop.
pub const STRIDE: f32 = WALK_SPEED * WALK_DURATION;
/// The ankle above the sole, and the shin's length to the ankle.
const ANKLE_UP: f32 = SOLE;
const LOWER: f32 = SHIN - SOLE;

/// One leg's joints at a moment of the walk (degrees about x: the hip forward, the knee bent back negative, the ankle toe up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leg {
    pub hip: f32,
    pub knee: f32,
    pub ankle: f32,
}

/// The hips' height over the ground at `pos` of the loop (twice a loop: lowest as each foot lands, highest as each passes
/// under them).
pub fn hip_height(pos: f32) -> f32 {
    HIP_HEIGHT + HIP_BOB * (2.0 * TAU * (pos - LEFT_LANDS - 0.5 * STANCE)).cos()
}

/// The left leg (`side` 0) or the right (1) at `pos` of the loop.
pub fn leg(side: usize, pos: f32) -> Leg {
    let phase = (pos - LEFT_LANDS - 0.5 * side as f32).rem_euclid(1.0);
    let h = hip_height(pos);
    let (ankle, pitch, ankle_turn) = if phase < STANCE {
        let (a, p) = stance_ankle(phase);
        (a, p, None)
    } else {
        swing_ankle(phase)
    };
    let (hip, knee) = reach(ankle, h);
    let ankle = ankle_turn.unwrap_or(pitch - hip - knee);
    Leg {
        hip: hip.to_degrees(),
        knee: knee.to_degrees(),
        ankle: ankle.to_degrees(),
    }
}

/// `p` turned by `c` radians in the side plane (y forward, z up; positive turns +y toward +z).
fn rot(c: f32, p: [f32; 2]) -> [f32; 2] {
    let (s, k) = c.sin_cos();
    [p[0] * k - p[1] * s, p[0] * s + p[1] * k]
}

fn smooth(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// The foot's pitch (radians) a share `s` of the way through its time on the ground.
fn stance_pitch(s: f32) -> f32 {
    if s < HEEL_ROLL {
        STRIKE_PITCH.to_radians() * (1.0 - smooth(s / HEEL_ROLL))
    } else {
        PUSH_PITCH.to_radians() * smooth((s - FLAT_UNTIL) / (1.0 - FLAT_UNTIL))
    }
}

/// Where the ankle is (along the walk from the hips, and over the ground) and the foot's pitch, `phase` of the loop after
/// the heel landed: the heel's bottom edge pinned while the toe is up, the toe's while the heel is.
fn stance_ankle(phase: f32) -> ([f32; 2], f32) {
    let pitch = stance_pitch(phase / STANCE);
    let heel = STRIKE_AHEAD - STRIDE * phase;
    let (pinned, at) = if pitch >= 0.0 {
        ([FOOT_HEEL, -ANKLE_UP], heel)
    } else {
        ([FOOT_TOE, -ANKLE_UP], heel + FOOT_TOE - FOOT_HEEL)
    };
    let q = rot(pitch, pinned);
    ([at - q[0], -q[1]], pitch)
}

/// The rate of change of [`stance_ankle`] over the loop at `phase`.
fn stance_rate(phase: f32) -> ([f32; 2], f32) {
    let d = 1e-3;
    let (a, p) = stance_ankle(phase + d);
    let (b, q) = stance_ankle(phase - d);
    (
        [(a[0] - b[0]) / (2.0 * d), (a[1] - b[1]) / (2.0 * d)],
        (p - q) / (2.0 * d),
    )
}

/// The ankle's place in the swing (`phase` from [`STANCE`] to 1), with the ankle's own turn: from the foot's last pose on the
/// ground to its first, leaving and landing at the ground's speed (so the foot neither scuffs forward as it leaves nor skids
/// as it lands), lifted straight off and set straight down; the ankle turning on as it turned on the ground at either end,
/// the toe lifted at the middle.
fn swing_ankle(phase: f32) -> ([f32; 2], f32, Option<f32>) {
    let span = 1.0 - STANCE;
    let s = (phase - STANCE) / span;
    let leave = STANCE - 2e-3;
    let (from, _) = stance_ankle(leave);
    let (to, _) = stance_ankle(0.0);
    let (v_from, _) = stance_rate(leave - 1e-3);
    let (v_to, _) = stance_rate(1e-3);
    let hermite = |p0: f32, p1: f32, m0: f32, m1: f32| {
        let (s2, s3) = (s * s, s * s * s);
        (2.0 * s3 - 3.0 * s2 + 1.0) * p0
            + (s3 - 2.0 * s2 + s) * m0
            + (-2.0 * s3 + 3.0 * s2) * p1
            + (s3 - s2) * m1
    };
    let y = hermite(from[0], to[0], v_from[0] * span, v_to[0] * span);
    let z = hermite(from[1], to[1], v_from[1] * span, v_to[1] * span) + LIFT * (PI * s).sin();
    let turn_at = |p: f32| {
        let (a, pitch) = stance_ankle(p);
        let (hip, knee) = reach(a, hip_height(p + LEFT_LANDS));
        pitch - hip - knee
    };
    let d = 1e-3;
    let rate = |p: f32| (turn_at(p + d) - turn_at(p - d)) / (2.0 * d) * span;
    let turn = hermite(turn_at(leave), turn_at(0.0), rate(leave - d), rate(d))
        + SWING_TOE_UP.to_radians() * (PI * s).sin().powi(2);
    ([y, z], 0.0, Some(turn))
}

/// The hip and knee turns (radians) that put the ankle at `ankle` (along the walk from the hips, and over the ground) with
/// the hips `h` over the ground, the knee bent forward; a point out of reach gets the leg straight toward it.
fn reach(ankle: [f32; 2], h: f32) -> (f32, f32) {
    let d = [ankle[0], ankle[1] - h];
    let len = d[0].hypot(d[1]).clamp(1e-4, THIGH + LOWER);
    let inner =
        ((THIGH * THIGH + LOWER * LOWER - len * len) / (2.0 * THIGH * LOWER)).clamp(-1.0, 1.0);
    let knee = -(PI - inner.acos());
    let at_hip =
        ((THIGH * THIGH + len * len - LOWER * LOWER) / (2.0 * THIGH * len)).clamp(-1.0, 1.0);
    let hip = d[1].atan2(d[0]) + at_hip.acos() + 0.5 * PI;
    (hip, knee)
}
