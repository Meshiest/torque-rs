//! The figure's `walk` and `run`, worked out from where its feet stand rather than keyed by hand, so that a foot on the
//! ground stays put while the body passes over it. A [`Gait`] holds one's numbers: [`WALK`] and [`RUN`].
//!
//! Each leg spends [`Gait::stance`] of the loop on the ground and the rest swinging. On the ground, the foot lands on its heel
//! with the toe up ([`Gait::strike_pitch`]), rolls flat, stays flat while the body passes over it, then rolls up onto its toe
//! ([`Gait::push_pitch`]) and leaves: the heel's or the toe's bottom edge, whichever the foot stands on, is pinned to the
//! ground, and the ground slides back under the hips at the gait's speed. The hips rise and fall twice a loop
//! ([`Gait::hip_height`], [`Gait::hip_bob`]). The thigh and the shin reach from the hip to the ankle with the knee bent
//! forward (a two-bone reach), and the ankle turns the foot to its pitch. Off the ground the ankle travels from where the foot
//! left to where it lands, lifted [`Gait::lift`] at the middle, the foot turning with the shin and a little up to clear the
//! ground.
//!
//! The walk keeps a foot down at every moment (both for a tenth around each landing), its hips lowest as each foot lands and
//! highest as it passes under them. The run is on the ground for less than half the loop, so it is airborne between steps;
//! its hips are lowest as the body passes over the planted foot (it gives on landing) and highest in the air, and its knees
//! come up high through the swing.
//!
//! In both the left foot lands at a quarter of the loop and the right at three quarters, so the two blend at the same
//! position.

use std::f32::consts::{PI, TAU};

use crate::shapes::{FOOT_HEEL, FOOT_TOE, RUN_SPEED, SHIN, SOLE, THIGH, WALK_SPEED};

/// One gait's numbers (lengths in units, angles in degrees, shares of the loop or of the time on the ground).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gait {
    /// The ground speed and the loop's length (seconds: two steps).
    pub speed: f32,
    pub duration: f32,
    /// The share of the loop each foot is on the ground.
    pub stance: f32,
    /// How far ahead of the hips the heel lands.
    pub strike_ahead: f32,
    /// The foot's pitch as its heel lands and as its toe leaves (toe up positive).
    pub strike_pitch: f32,
    pub push_pitch: f32,
    /// The shares of the time on the ground the foot takes to roll flat from its heel, and stays flat until.
    pub heel_roll: f32,
    pub flat_until: f32,
    /// The hips' mean height (standing, they are at `THIGH + SHIN`) and how far they rise and fall about it: positive
    /// highest as a foot passes under them, negative lowest then.
    pub hip_height: f32,
    pub hip_bob: f32,
    /// How high the ankle lifts in the swing, and how far the foot turns up at its middle.
    pub lift: f32,
    pub swing_toe_up: f32,
    /// How early in the swing the lift peaks: 1 at its middle, less sooner (the heel kicked up behind, then the knee driven
    /// forward).
    pub lift_early: f32,
}

/// The walk: a relaxed stride at a held-Shift pace, a foot always down.
pub const WALK: Gait = Gait {
    speed: WALK_SPEED,
    duration: 0.65,
    stance: 0.6,
    strike_ahead: 0.24,
    strike_pitch: 16.0,
    push_pitch: -34.0,
    heel_roll: 0.15,
    flat_until: 0.5,
    hip_height: 0.92,
    hip_bob: 0.018,
    lift: 0.07,
    swing_toe_up: 10.0,
    lift_early: 1.0,
};

/// The run: a short flight between steps, the body bobbing and giving on each landing, the knees lifted high.
pub const RUN: Gait = Gait {
    speed: RUN_SPEED,
    duration: 0.55,
    stance: 0.3,
    strike_ahead: 0.18,
    strike_pitch: 8.0,
    push_pitch: -40.0,
    heel_roll: 0.2,
    flat_until: 0.35,
    hip_height: 0.89,
    hip_bob: -0.035,
    lift: 0.3,
    swing_toe_up: 10.0,
    lift_early: 0.6,
};

/// The loop position at which the left heel lands (the right's half a loop later).
pub const LEFT_LANDS: f32 = 0.25;

/// The ankle above the sole, and the shin's length to the ankle.
const ANKLE_UP: f32 = SOLE;
const LOWER: f32 = SHIN - SOLE;

/// One leg's joints at a moment of a gait (degrees about x: the hip forward, the knee bent back negative, the ankle toe up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leg {
    pub hip: f32,
    pub knee: f32,
    pub ankle: f32,
}

impl Gait {
    /// The ground covered over one loop.
    pub fn stride(&self) -> f32 {
        self.speed * self.duration
    }

    /// The hips' height over the ground at `pos` of the loop (twice a loop: furthest from the mean as each foot passes
    /// under them and between).
    pub fn hip_height(&self, pos: f32) -> f32 {
        self.hip_height + self.hip_bob * (2.0 * TAU * (pos - LEFT_LANDS - 0.5 * self.stance)).cos()
    }

    /// The left leg (`side` 0) or the right (1) at `pos` of the loop.
    pub fn leg(&self, side: usize, pos: f32) -> Leg {
        let phase = (pos - LEFT_LANDS - 0.5 * side as f32).rem_euclid(1.0);
        let h = self.hip_height(pos);
        let (ankle, pitch, ankle_turn) = if phase < self.stance {
            let (a, p) = self.stance_ankle(phase);
            (a, p, None)
        } else {
            self.swing_ankle(phase)
        };
        let (hip, knee) = reach(ankle, h);
        let ankle = ankle_turn.unwrap_or(pitch - hip - knee);
        Leg {
            hip: hip.to_degrees(),
            knee: knee.to_degrees(),
            ankle: ankle.to_degrees(),
        }
    }

    /// The foot's pitch (radians) a share `s` of the way through its time on the ground.
    fn stance_pitch(&self, s: f32) -> f32 {
        if s < self.heel_roll {
            self.strike_pitch.to_radians() * (1.0 - smooth(s / self.heel_roll))
        } else {
            self.push_pitch.to_radians() * smooth((s - self.flat_until) / (1.0 - self.flat_until))
        }
    }

    /// Where the ankle is (along the walk from the hips, and over the ground) and the foot's pitch, `phase` of the loop
    /// after the heel landed: the heel's bottom edge pinned while the toe is up, the toe's while the heel is.
    fn stance_ankle(&self, phase: f32) -> ([f32; 2], f32) {
        let pitch = self.stance_pitch(phase / self.stance);
        let heel = self.strike_ahead - self.stride() * phase;
        let (pinned, at) = if pitch >= 0.0 {
            ([FOOT_HEEL, -ANKLE_UP], heel)
        } else {
            ([FOOT_TOE, -ANKLE_UP], heel + FOOT_TOE - FOOT_HEEL)
        };
        let q = rot(pitch, pinned);
        ([at - q[0], -q[1]], pitch)
    }

    /// The rate of change of [`Gait::stance_ankle`] over the loop at `phase`.
    fn stance_rate(&self, phase: f32) -> ([f32; 2], f32) {
        let d = 1e-3;
        let (a, p) = self.stance_ankle(phase + d);
        let (b, q) = self.stance_ankle(phase - d);
        (
            [(a[0] - b[0]) / (2.0 * d), (a[1] - b[1]) / (2.0 * d)],
            (p - q) / (2.0 * d),
        )
    }

    /// The ankle's place in the swing (`phase` from the stance's end to 1), with the ankle's own turn: from the foot's last
    /// pose on the ground to its first, leaving and landing at the ground's speed (so the foot neither scuffs forward as it
    /// leaves nor skids as it lands), lifted straight off and set straight down; the ankle turning on as it turned on the
    /// ground at either end, the toe lifted at the middle.
    fn swing_ankle(&self, phase: f32) -> ([f32; 2], f32, Option<f32>) {
        let span = 1.0 - self.stance;
        let s = (phase - self.stance) / span;
        let leave = self.stance - 2e-3;
        let (from, _) = self.stance_ankle(leave);
        let (to, _) = self.stance_ankle(0.0);
        let (v_from, _) = self.stance_rate(leave - 1e-3);
        let (v_to, _) = self.stance_rate(1e-3);
        let hermite = |p0: f32, p1: f32, m0: f32, m1: f32| {
            let (s2, s3) = (s * s, s * s * s);
            (2.0 * s3 - 3.0 * s2 + 1.0) * p0
                + (s3 - 2.0 * s2 + s) * m0
                + (-2.0 * s3 + 3.0 * s2) * p1
                + (s3 - s2) * m1
        };
        let y = hermite(from[0], to[0], v_from[0] * span, v_to[0] * span);
        let lifted = if self.lift_early == 1.0 {
            s
        } else {
            s.powf(self.lift_early)
        };
        let z = hermite(from[1], to[1], v_from[1] * span, v_to[1] * span)
            + self.lift * (PI * lifted).sin();
        let turn_at = |p: f32| {
            let (a, pitch) = self.stance_ankle(p);
            let (hip, knee) = reach(a, self.hip_height(p + LEFT_LANDS));
            pitch - hip - knee
        };
        let d = 1e-3;
        let rate = |p: f32| (turn_at(p + d) - turn_at(p - d)) / (2.0 * d) * span;
        let turn = hermite(turn_at(leave), turn_at(0.0), rate(leave - d), rate(d))
            + self.swing_toe_up.to_radians() * (PI * s).sin().powi(2);
        ([y, z], 0.0, Some(turn))
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
