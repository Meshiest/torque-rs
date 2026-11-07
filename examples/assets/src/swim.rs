//! The figure's swimming sequences (the engine's swim pose, after Torque3D's `swim_*` actions), posed like the crouch and prone
//! ([`crate::poses`]): each key places the body (the hips, the body's pitch and roll, the chest's lift, the head's turn) and
//! where each hand and foot should be, and the arms and legs are turned to reach there.
//!
//! The figure floats with the water at [`SWIM_WATER`] in its own space (its feet's origin): a Player that floats there (the
//! level demo's `density` gives it) has its eye about 15 cm out of the water while it treads.
//!
//! - `swim_root`: [`TREAD_LOOP`] s, looped: treading water upright, the head out, a slow eggbeater kick (each foot circling
//!   under its knee, the two half a loop apart) and the hands sculling in and out in front of the chest.
//! - `swim_forward`: [`STROKE_LOOP`] s, looped, a breaststroke at [`SWIM_SPEED`] (the level demo's `maxUnderwaterForwardSpeed`):
//!   the body lies 71 to 83 degrees over toward level (the chest lifting to 61 for the breath), just under the surface. The hands sweep out and back, in under the
//!   chin as the chest and head lift for a breath, and shoot forward; the heels come up to the seat with the knees apart, the
//!   feet turned out, and kick out, back and together; then a glide, the arms and legs long and the face down. The body surges
//!   ahead after the kick and lags as the legs come up.
//! - `swim_backward`: [`BACK_LOOP`] s, looped, at [`SWIM_BACK_SPEED`]: the same family upright and leaning back, the hands
//!   pushing the water forward from the hips to together in front of the chest and slicing back, the legs kicking out,
//!   forward and together under the body.
//! - `swim_side`: [`SIDE_LOOP`] s, looped, to the left at [`SWIM_SIDE_SPEED`] (played backward to the right, as `side` is;
//!   the engine also takes Torque3D's `swim_left` or `swim_right`): leaning a little into the way it goes, the left hand
//!   reaches out and pulls in to the chest as the right pushes out from it, and the left leg sweeps in as the right sweeps
//!   out; then both come back with the knees bent.
//! - `swim_point`, `swim_hold` and `swim_aim`: `point`, `hold` and `aim` for a swimming body (a pose's own arm action,
//!   `<pose>_<arm>`), the arm raised [`TREAD_TILT`] degrees further for the treading body's lean; `swim_forward_point`,
//!   `swim_forward_hold` and `swim_forward_aim` (an action's own, `<action>_<arm>`) raised [`STROKE_TILT`] degrees for the
//!   breaststroke's. Each turns the right arm only, so the other arm and the legs keep swimming under it.

use std::f32::consts::{PI, TAU};

use glam::Vec3 as V3;

use crate::builder::{ShapeBuilder, Vec3};
use crate::poses::{
    Arm, Body, Foot, HIP_X, Key, PELVIS, SHOULDER, deg, sequence, side_x, tilted_aim, tilted_hold,
    tilted_point, tracks,
};
use crate::shapes::{X, Y, Z, apply, rot_mul, travel, travel_along};

/// The waterline in the figure's space while it floats (units over its feet's origin).
pub const SWIM_WATER: f32 = 1.45;
/// The ground speeds of `swim_forward`, `swim_backward` and `swim_side` (units a second): the level demo's underwater caps.
pub const SWIM_SPEED: f32 = 3.0;
pub const SWIM_BACK_SPEED: f32 = 2.0;
pub const SWIM_SIDE_SPEED: f32 = 2.0;
/// The loops' lengths (seconds).
pub const TREAD_LOOP: f32 = 2.8;
pub const STROKE_LOOP: f32 = 1.3;
pub const BACK_LOOP: f32 = 1.6;
pub const SIDE_LOOP: f32 = 1.4;
/// Treading, the hips' height over the feet's origin: sitting deep, the thighs well forward.
pub const TREAD_HIPS: f32 = 0.7;
/// How far the swimming arm actions raise the arm past standing's (degrees): the treading body's lean forward, and the
/// breaststroke's (its torso 61 to 83 degrees over).
pub const TREAD_TILT: f32 = 6.0;
pub const STROKE_TILT: f32 = 78.0;
/// The keys a loop of each (one every 40 ms or so).
const TREAD_KEYS: usize = 48;
const STROKE_KEYS: usize = 52;
const BACK_KEYS: usize = 40;
const SIDE_KEYS: usize = 36;

/// One moment of a stroke. The hips (the pelvis's middle) are in the figure's space; the body pitches forward `pitch` degrees
/// (about x; 90 lies on the front), rolls `roll` degrees (about y; negative leans to the left), the chest lifts `lift` degrees
/// on it (back positive) and the head turns `head` degrees on the chest (back positive). Side 0 is the left. Each hand's grip
/// and its elbow's pole are in the chest's frame from its shoulder (x right, y the chest's front, z up the spine); each ankle and
/// its knee's pole in the body's frame from its hip; each foot turned out `feet[side][0]` degrees and pointed `feet[side][1]`
/// (about x, toes down negative).
#[derive(Clone, Copy)]
pub(crate) struct Beat {
    pub hips: Vec3,
    pub pitch: f32,
    pub roll: f32,
    pub lift: f32,
    pub head: f32,
    pub hands: [Vec3; 2],
    pub elbows: [Vec3; 2],
    pub ankles: [Vec3; 2],
    pub knees: [Vec3; 2],
    pub feet: [[f32; 2]; 2],
}

const FIELDS: usize = 3 + 4 + 4 * 6 + 4;

impl Beat {
    fn to_array(self) -> [f32; FIELDS] {
        let mut out = [0.0; FIELDS];
        let mut i = 0;
        let mut put = |v: &[f32]| {
            out[i..i + v.len()].copy_from_slice(v);
            i += v.len();
        };
        put(&self.hips);
        put(&[self.pitch, self.roll, self.lift, self.head]);
        for v in [self.hands, self.elbows, self.ankles, self.knees] {
            put(&v[0]);
            put(&v[1]);
        }
        put(&self.feet[0]);
        put(&self.feet[1]);
        out
    }

    fn from_array(a: &[f32; FIELDS]) -> Beat {
        let v3 = |i: usize| [a[i], a[i + 1], a[i + 2]];
        let pair = |i: usize| [v3(i), v3(i + 3)];
        Beat {
            hips: v3(0),
            pitch: a[3],
            roll: a[4],
            lift: a[5],
            head: a[6],
            hands: pair(7),
            elbows: pair(13),
            ankles: pair(19),
            knees: pair(25),
            feet: [[a[31], a[32]], [a[33], a[34]]],
        }
    }
}

/// The beat at phase `p` of a loop through `beats` (phases from 0 up, under 1): every number on a looped Catmull-Rom curve,
/// so the loop has no seam and no stop at a beat.
fn looped(beats: &[(f32, Beat)], p: f32) -> Beat {
    let n = beats.len() as isize;
    let p = p.rem_euclid(1.0);
    let i = (0..n)
        .rev()
        .find(|&i| beats[i as usize].0 <= p)
        .unwrap_or(0);
    // A beat `k` beats on (any whole number), its phase carried past the loop's ends.
    let at = |k: isize| {
        let (lap, j) = (k.div_euclid(n), k.rem_euclid(n));
        (
            beats[j as usize].0 + lap as f32,
            beats[j as usize].1.to_array(),
        )
    };
    let (t0, a) = at(i);
    let (t1, b) = at(i + 1);
    let (tb, before) = at(i - 1);
    let (ta, after) = at(i + 2);
    let span = t1 - t0;
    let s = (p - t0) / span;
    let (s2, s3) = (s * s, s * s * s);
    let mut out = [0.0; FIELDS];
    for k in 0..FIELDS {
        let m0 = (b[k] - before[k]) / (t1 - tb) * span;
        let m1 = (after[k] - a[k]) / (ta - t0) * span;
        out[k] = (2.0 * s3 - 3.0 * s2 + 1.0) * a[k]
            + (s3 - 2.0 * s2 + s) * m0
            + (-2.0 * s3 + 3.0 * s2) * b[k]
            + (s3 - s2) * m1;
    }
    Beat::from_array(&out)
}

/// The pose key of a beat (see [`Beat`]).
pub(crate) fn key_of(b: &Beat) -> Key {
    let root_r = rot_mul(&deg(Y, b.roll), &deg(X, -b.pitch));
    let hips = V3::from(b.hips);
    let chest = rot_mul(&root_r, &deg(X, b.lift));
    let foot = |side: usize| {
        let s = side_x(side);
        let socket = hips + apply(&root_r, V3::new(s * HIP_X, 0.0, 0.0));
        let [out, point] = b.feet[side];
        Foot {
            ankle: (socket + apply(&root_r, b.ankles[side].into())).to_array(),
            turn: rot_mul(&root_r, &rot_mul(&deg(Z, -s * out), &deg(X, point))),
            pole: apply(&root_r, b.knees[side].into()).to_array(),
        }
    };
    let hand = |side: usize| {
        let s = side_x(side);
        let shoulder = hips + apply(&chest, V3::new(s * SHOULDER[0], SHOULDER[1], SHOULDER[2]));
        Arm {
            grip: (shoulder + apply(&chest, b.hands[side].into())).to_array(),
            pole: apply(&chest, b.elbows[side].into()).to_array(),
        }
    };
    Key {
        root_t: (hips - apply(&root_r, V3::new(0.0, 0.0, PELVIS))).to_array(),
        root_r,
        torso: deg(X, b.lift),
        head: deg(X, b.head),
        feet: [foot(0), foot(1)],
        arms: [hand(0), hand(1)],
    }
}

/// `[x, y, z]` for the right side, mirrored across for the left (side 0).
fn mirror(side: usize, v: Vec3) -> Vec3 {
    [side_x(side) * v[0], v[1], v[2]]
}

/// A beat whose two sides mirror each other, from the right side's hand, elbow pole, ankle, knee pole and foot.
#[allow(clippy::too_many_arguments)]
fn even(
    hips: Vec3,
    [pitch, lift, head]: [f32; 3],
    hand: Vec3,
    elbow: Vec3,
    ankle: Vec3,
    knee: Vec3,
    foot: [f32; 2],
) -> Beat {
    let both = |v: Vec3| [mirror(0, v), mirror(1, v)];
    Beat {
        hips,
        pitch,
        roll: 0.0,
        lift,
        head,
        hands: both(hand),
        elbows: both(elbow),
        ankles: both(ankle),
        knees: both(knee),
        feet: [foot, foot],
    }
}

/// Treading water at phase `p` (see the module docs).
pub(crate) fn tread(p: f32) -> Beat {
    let kick = (2.0 * TAU * p).sin();
    let scull = (2.0 * TAU * p).sin();
    let mut b = even(
        [0.0, -0.02, TREAD_HIPS + 0.012 * kick],
        [TREAD_TILT, 0.0, TREAD_TILT],
        [0.0; 3],
        [0.8, -0.3, -0.5],
        [0.0; 3],
        [0.6, 1.0, 0.1],
        [25.0, -15.0],
    );
    for side in 0..2 {
        // Each foot circles under its knee, the two half a loop apart and mirrored.
        let a = TAU * p + PI * side as f32;
        b.ankles[side] = mirror(
            side,
            [
                0.24 + 0.1 * a.cos(),
                0.3 + 0.1 * a.sin(),
                -0.51 + 0.03 * a.cos(),
            ],
        );
        // The hands scull out and in together, tracing a flat figure eight.
        b.hands[side] = mirror(
            side,
            [
                -0.02 + 0.12 * scull,
                0.34,
                -0.32 + 0.025 * (4.0 * TAU * p).cos(),
            ],
        );
    }
    b
}

/// The breaststroke's beats (see the module docs): the hips 21 to 26 cm under the waterline.
pub(crate) fn stroke_beats() -> Vec<(f32, Beat)> {
    let w = SWIM_WATER;
    let reach = [-0.27, 0.1, 0.7];
    let long = [0.0, 0.02, -0.835];
    let pointed = [0.0, -90.0];
    let high = [0.4, -1.0, 0.0];
    let straight = [0.3, 1.0, 0.0];
    vec![
        // The glide's end: the pull starts.
        (
            0.0,
            even(
                [0.0, -0.1, w - 0.22],
                [82.0, 0.0, 5.0],
                reach,
                high,
                long,
                straight,
                pointed,
            ),
        ),
        // The hands sweep out wide.
        (
            0.12,
            even(
                [0.0, -0.11, w - 0.22],
                [80.0, 2.0, 12.0],
                [0.22, 0.14, 0.62],
                [0.6, -0.8, 0.0],
                long,
                straight,
                pointed,
            ),
        ),
        // Down and in under the chest, the elbows high; the chest begins to lift.
        (
            0.24,
            even(
                [0.0, -0.13, w - 0.24],
                [75.0, 6.0, 30.0],
                [0.1, 0.42, 0.22],
                [1.0, -0.5, 0.1],
                [0.0, 0.03, -0.83],
                straight,
                pointed,
            ),
        ),
        // Together under the chin with the breath; the heels on their way up.
        (
            0.32,
            even(
                [0.0, -0.18, w - 0.26],
                [71.0, 10.0, 45.0],
                [-0.25, 0.3, 0.3],
                [0.8, 0.5, -0.5],
                [0.08, 0.04, -0.62],
                [0.8, 0.7, 0.0],
                [30.0, -50.0],
            ),
        ),
        // The arms long again; the heels at the seat, the knees down and apart, the feet turned out.
        (
            0.44,
            even(
                [0.0, -0.25, w - 0.25],
                [76.0, 4.0, 15.0],
                reach,
                high,
                [0.22, -0.1, -0.36],
                [0.9, 0.6, 0.0],
                [60.0, 0.0],
            ),
        ),
        // The kick out and back.
        (
            0.51,
            even(
                [0.0, -0.22, w - 0.23],
                [80.0, 1.0, 5.0],
                [-0.27, 0.1, 0.71],
                high,
                [0.4, 0.06, -0.6],
                [0.9, 0.6, -0.2],
                [50.0, -20.0],
            ),
        ),
        // Together: the glide.
        (
            0.58,
            even(
                [0.0, -0.17, w - 0.22],
                [82.0, 0.0, 2.0],
                [-0.26, 0.09, 0.72],
                high,
                [0.0, 0.03, -0.835],
                straight,
                pointed,
            ),
        ),
        (
            0.8,
            even(
                [0.0, -0.09, w - 0.21],
                [83.0, 0.0, 3.0],
                [-0.26, 0.09, 0.72],
                high,
                long,
                straight,
                pointed,
            ),
        ),
    ]
}

/// The swim backward's beats (see the module docs).
pub(crate) fn back_beats() -> Vec<(f32, Beat)> {
    let hips = |dy: f32| [0.0, dy, TREAD_HIPS + 0.03];
    let lean = [-15.0, 0.0, -15.0];
    let elbow = [0.7, -0.6, -0.3];
    vec![
        // The hands by the hips, the heels drawn back: the push starts.
        (
            0.0,
            even(
                hips(0.04),
                lean,
                [0.1, -0.05, -0.55],
                elbow,
                [0.18, -0.25, -0.45],
                [0.5, 1.0, 0.0],
                [40.0, 15.0],
            ),
        ),
        (
            0.18,
            even(
                hips(0.0),
                lean,
                [0.3, 0.25, -0.35],
                elbow,
                [0.33, 0.12, -0.56],
                [0.5, 1.0, 0.0],
                [40.0, 10.0],
            ),
        ),
        // Together in front of the chest, the legs together under the body.
        (
            0.36,
            even(
                hips(-0.04),
                lean,
                [-0.22, 0.48, -0.25],
                elbow,
                [0.0, 0.32, -0.66],
                [0.3, 1.0, 0.0],
                [10.0, -30.0],
            ),
        ),
        (
            0.62,
            even(
                hips(-0.02),
                lean,
                [0.05, 0.22, -0.45],
                elbow,
                [0.0, 0.24, -0.68],
                [0.3, 1.0, 0.0],
                [0.0, -40.0],
            ),
        ),
        (
            0.84,
            even(
                hips(0.03),
                lean,
                [0.12, -0.02, -0.52],
                elbow,
                [0.08, -0.08, -0.62],
                [0.4, 1.0, 0.0],
                [20.0, -10.0],
            ),
        ),
    ]
}

/// The side stroke's beats, to the left (see the module docs).
pub(crate) fn side_beats() -> Vec<(f32, Beat)> {
    let beat = |dx: f32, hands: [Vec3; 2], ankles: [Vec3; 2]| Beat {
        hips: [dx, -0.02, TREAD_HIPS],
        pitch: 5.0,
        roll: -6.0,
        lift: 0.0,
        head: 5.0,
        hands,
        elbows: [[-0.6, -0.6, -0.5], [0.6, -0.6, -0.5]],
        ankles,
        knees: [[-0.3, 1.0, 0.0], [0.3, 1.0, 0.0]],
        feet: [[20.0, -20.0]; 2],
    };
    vec![
        // The left hand and leg out to the left, the right ones in: the pull starts.
        (
            0.0,
            beat(
                0.04,
                [[-0.62, 0.2, -0.18], [-0.22, 0.3, -0.28]],
                [[-0.36, 0.12, -0.46], [0.0, 0.14, -0.53]],
            ),
        ),
        (
            0.2,
            beat(
                -0.01,
                [[-0.25, 0.35, -0.3], [0.2, 0.3, -0.35]],
                [[-0.18, 0.14, -0.52], [0.18, 0.14, -0.52]],
            ),
        ),
        // The left ones in, the right ones out.
        (
            0.4,
            beat(
                -0.05,
                [[0.2, 0.3, -0.28], [0.55, 0.12, -0.4]],
                [[0.0, 0.14, -0.53], [0.36, 0.12, -0.46]],
            ),
        ),
        // Back with the knees bent.
        (
            0.7,
            beat(
                -0.01,
                [[-0.25, 0.28, -0.4], [0.18, 0.24, -0.42]],
                [[-0.2, 0.02, -0.45], [0.15, 0.02, -0.45]],
            ),
        ),
    ]
}

/// The keys of a loop of `n` from `beat` (its share of the loop, from 0 up to 1).
fn keys(n: usize, beat: impl Fn(f32) -> Beat) -> Vec<Key> {
    (0..n).map(|k| key_of(&beat(k as f32 / n as f32))).collect()
}

/// The swimming sequences and arm actions (see the module docs).
pub(crate) fn swim(b: &mut ShapeBuilder, body: &Body<'_>) {
    b.sequence(sequence(
        "swim_root",
        TREAD_LOOP,
        tracks(body, &keys(TREAD_KEYS, tread)),
        Vec::new(),
    ));
    let stroke = stroke_beats();
    b.sequence(sequence(
        "swim_forward",
        STROKE_LOOP,
        tracks(body, &keys(STROKE_KEYS, |p| looped(&stroke, p))),
        travel(STROKE_KEYS, SWIM_SPEED, STROKE_LOOP),
    ));
    let back = back_beats();
    b.sequence(sequence(
        "swim_backward",
        BACK_LOOP,
        tracks(body, &keys(BACK_KEYS, |p| looped(&back, p))),
        travel_along(BACK_KEYS, [0.0, -1.0, 0.0], SWIM_BACK_SPEED, BACK_LOOP),
    ));
    let side = side_beats();
    b.sequence(sequence(
        "swim_side",
        SIDE_LOOP,
        tracks(body, &keys(SIDE_KEYS, |p| looped(&side, p))),
        travel_along(SIDE_KEYS, [-1.0, 0.0, 0.0], SWIM_SIDE_SPEED, SIDE_LOOP),
    ));
    for (point, hold, aim, tilt) in [
        ("swim_point", "swim_hold", "swim_aim", TREAD_TILT),
        (
            "swim_forward_point",
            "swim_forward_hold",
            "swim_forward_aim",
            STROKE_TILT,
        ),
    ] {
        b.sequence(tilted_point(body, point, tilt));
        b.sequence(tilted_hold(body, hold, tilt));
        b.sequence(tilted_aim(body, aim, tilt));
    }
}

/// A loop's sequence name, its keys and the beat at each phase.
#[cfg(test)]
pub(crate) type Loop = (&'static str, usize, Box<dyn Fn(f32) -> Beat>);

/// Every loop and the beats its keys come from (for the tests).
#[cfg(test)]
pub(crate) fn loops() -> [Loop; 4] {
    let (stroke, back, side) = (stroke_beats(), back_beats(), side_beats());
    [
        ("swim_root", TREAD_KEYS, Box::new(tread)),
        (
            "swim_forward",
            STROKE_KEYS,
            Box::new(move |p| looped(&stroke, p)),
        ),
        (
            "swim_backward",
            BACK_KEYS,
            Box::new(move |p| looped(&back, p)),
        ),
        ("swim_side", SIDE_KEYS, Box::new(move |p| looped(&side, p))),
    ]
}

#[cfg(test)]
#[path = "swim_tests.rs"]
mod tests;
