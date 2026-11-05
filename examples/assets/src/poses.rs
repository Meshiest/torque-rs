//! The figure's crouched and prone sequences and its hit reactions, posed from where its feet and hands stand rather than
//! keyed by hand, so a foot or a hand on the ground stays put while the body moves over it.
//!
//! **Placing the limbs.** Each key says where the body is (the root's place and turn, the torso's and the head's turns) and
//! where each foot (its ankle, how the foot is turned, which way the knee points) and each hand (its grip, which way the elbow
//! points) should be, in the figure's own space; the hip and the knee, the shoulder and the elbow are then turned to reach there
//! (a two-bone reach, the knee and the elbow bending in the plane the pole gives), and the ankle turns the foot as asked.
//!
//! - `crouch_root`: 2 s, looped: crouched low on bent knees (the hips [`CROUCH_HIPS`] up), leaning forward with the head up,
//!   the forearms forward over the knees, breathing a little. The feet stand where the standing feet do, so a crouch lowers
//!   the body over them.
//! - `crouch_forward`: a crouched walk at [`CROUCH_SPEED`] ([`crate::gait::CROUCH`], the walk's model with the hips low).
//!   `crouch_backward` is the same loop played backward, as is `prone_backward` of `prone_forward`.
//! - `crouch_side`: a crouched side step to the left at [`CROUCH_SIDE_SPEED`] (played backward to the right, as `side` is): each
//!   foot stands still for 0.6 of the loop while the hips pass over it, then lifts and moves across.
//! - `prone_root`: 2 s, looped: lying on the front, propped on the forearms, the chest lifted [`PRONE_CHEST`] degrees and the
//!   head up to look ahead, the legs back with the toes tucked under.
//! - `prone_forward`: a crawl at [`CRAWL_SPEED`]: a hand and the other side's foot pull and push together while the other two
//!   reach forward, each hand down 0.6 of the loop and each foot half, the knees drawn up and out.
//! - `prone_hold` and `prone_aim`: `hold` and `aim` for a figure lying down (a pose's own arm action, `<pose>_<arm>`): the arm
//!   raised [`PRONE_TILT`] degrees further, so the pistol points ahead along the ground.
//! - Hit reactions, blend sequences played once over any pose or gait: `flinch_front`, `flinch_back`, `flinch_left` and
//!   `flinch_right` ([`FLINCH_SECONDS`], a body shot from that side: the torso punched away from it, the hips pushed back a few
//!   centimetres and a small step to catch the balance); `flinch_head_front` and the rest ([`HEAD_FLINCH_SECONDS`], a head shot:
//!   the head snapped sharply away, the torso barely). Each is at rest at its start and its end.

use std::f32::consts::{PI, TAU};

use torque_shapes::shape::Quat16;

use crate::builder::{SequenceDef, ShapeBuilder, Track, Vec3};
use crate::gait;
use crate::shapes::{
    AIM_ELBOW, AIM_IN, AIM_SHOULDER, FOOT_HEEL, FOOT_TOE, FOREARM, GRIP, HOLD_ELBOW, HOLD_SHOULDER,
    LOOK_KEYS, LOOK_TORSO, Limbs, Rot, SHIN, SOLE, THIGH, UPPER_ARM, X, Y, Z, apply, cross3, dot3,
    hip_track, leg_tracks, look_pitch, quat16_of, rot_about, rot_mul, rot_t, sub3, travel,
    travel_along, turns, unit,
};

/// The ground speeds of `crouch_forward`, `crouch_side` and `prone_forward` (units a second).
pub const CROUCH_SPEED: f32 = 1.2;
pub const CROUCH_SIDE_SPEED: f32 = 0.9;
pub const CRAWL_SPEED: f32 = 0.75;
/// The crouch: the hips' height, the torso's lean forward and the head's turn back up (degrees).
pub const CROUCH_HIPS: f32 = 0.56;
pub const CROUCH_LEAN: f32 = 24.0;
pub const CROUCH_HEAD: f32 = 20.0;
/// Crouched still, the ankles' distance to each side and how far ahead the left and the right one are: the balls of the feet
/// where the standing figure's are, the left a little ahead.
pub const CROUCH_FEET: [f32; 3] = [0.13, 0.03, -0.02];
/// Crouched still, the heels are lifted this far (degrees) onto the balls of the feet.
pub const CROUCH_TOES: f32 = -15.0;

/// The ankle's height crouched on the balls of the feet: the sole's front edge just touching the floor.
fn crouch_ankle() -> f32 {
    let (s, c) = CROUCH_TOES.to_radians().sin_cos();
    -(FOOT_TOE * s - SOLE * c) + TOUCH
}
/// Prone: the hips' place along and over the ground, the chest's lift and the head's turn up (degrees).
pub const PRONE_HIPS: [f32; 2] = [-0.12, 0.22];
pub const PRONE_CHEST: f32 = 15.0;
pub const PRONE_HEAD: f32 = 60.0;
/// Where prone's hands rest ahead of the figure's middle, and its ankles behind its hips.
pub const PRONE_GRIP_Y: f32 = 0.82;
pub const PRONE_ANKLE_Y: f32 = -0.74;
/// How much further `prone_hold` and `prone_aim` raise the arm than `hold` and `aim` (degrees): the torso lies that much
/// further over.
pub const PRONE_TILT: f32 = 90.0 - PRONE_CHEST;
/// The hit reactions' lengths (seconds).
pub const FLINCH_SECONDS: f32 = 0.32;
pub const HEAD_FLINCH_SECONDS: f32 = 0.24;
/// The crawl's loop and the crouched side step's (seconds).
pub const CRAWL_LOOP: f32 = 1.2;
pub const SIDE_LOOP: f32 = 0.6;

/// The sockets of the hips and the shoulders, the pelvis's height (the standing figure's).
const HIP_X: f32 = 0.12;
const PELVIS: f32 = THIGH + SHIN;
const SHOULDER: Vec3 = [0.33, 0.0, 0.66];
/// The shin from the knee to the ankle, and the arm from the shoulder to the grip.
const LOWER: f32 = SHIN - SOLE;
const REACH: [f32; 2] = [UPPER_ARM, GRIP];
const IDENTITY: Rot = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The nodes the poses turn.
pub(crate) struct Body<'a> {
    pub root: usize,
    pub torso: usize,
    pub head: usize,
    pub limbs: &'a Limbs,
}

/// A foot's place: its ankle, the foot's turn, the way the knee points (all in the figure's space).
#[derive(Clone, Copy)]
struct Foot {
    ankle: Vec3,
    turn: Rot,
    pole: Vec3,
}

/// An arm: its grip's place and the way the elbow points.
#[derive(Clone, Copy)]
struct Arm {
    grip: Vec3,
    pole: Vec3,
}

/// One key of a pose (see the module docs). Side 0 is the left (-x).
#[derive(Clone, Copy)]
struct Key {
    root_t: Vec3,
    root_r: Rot,
    torso: Rot,
    head: Rot,
    feet: [Foot; 2],
    arms: [Arm; 2],
}

fn deg(axis: Vec3, degrees: f32) -> Rot {
    rot_about(axis, degrees.to_radians())
}

fn add3(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale3(a: Vec3, k: f32) -> Vec3 {
    a.map(|v| v * k)
}

fn side_x(side: usize) -> f32 {
    if side == 0 { -1.0 } else { 1.0 }
}

/// The frame whose -z runs along `down` and whose +y leans toward `side`: a limb's turn in the world.
fn frame(down: Vec3, side: Vec3) -> Rot {
    let z = unit(scale3(down, -1.0));
    let mut y = sub3(side, scale3(z, dot3(side, z)));
    if dot3(y, y) < 1e-8 {
        y = if z[1].abs() < 0.9 { Y } else { Z };
        y = sub3(y, scale3(z, dot3(y, z)));
    }
    let y = unit(y);
    let x = cross3(y, z);
    [0, 1, 2].map(|i| [x[i], y[i], z[i]])
}

/// The middle joint of a two-bone reach from `start` toward `target` (bones `a` and `b`), bent toward `pole`, and where the end
/// gets to (the target, or as near as the bones reach).
fn reach(start: Vec3, target: Vec3, a: f32, b: f32, pole: Vec3) -> (Vec3, Vec3) {
    let to = sub3(target, start);
    let d = dot3(to, to)
        .sqrt()
        .clamp((a - b).abs() + 1e-4, a + b - 1e-4);
    let dir = unit(to);
    let mut perp = sub3(pole, scale3(dir, dot3(pole, dir)));
    if dot3(perp, perp) < 1e-8 {
        perp = if dir[1].abs() < 0.9 { Y } else { Z };
        perp = sub3(perp, scale3(dir, dot3(perp, dir)));
    }
    let perp = unit(perp);
    let cos_a = ((a * a + d * d - b * b) / (2.0 * a * d)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).sqrt();
    let joint = add3(
        start,
        scale3(add3(scale3(dir, cos_a), scale3(perp, sin_a)), a),
    );
    (joint, add3(start, scale3(dir, d)))
}

/// The hinge's turn about x that takes the limb's -z (in `upper`'s frame) to `dir` (the world).
fn hinge(upper: &Rot, dir: Vec3) -> Rot {
    let l = apply(&rot_t(upper), unit(dir));
    deg(X, l[1].atan2(-l[2]).to_degrees())
}

/// A leg's local turns (hip, knee, ankle) from its socket `hip` under a parent turned `parent` to `foot`.
fn leg(hip: Vec3, parent: &Rot, foot: &Foot) -> [Rot; 3] {
    let (knee, ankle) = reach(hip, foot.ankle, THIGH, LOWER, foot.pole);
    let thigh = frame(sub3(knee, hip), foot.pole);
    let shin = hinge(&thigh, sub3(ankle, knee));
    let lower = rot_mul(&thigh, &shin);
    [
        rot_mul(&rot_t(parent), &thigh),
        shin,
        rot_mul(&rot_t(&lower), &foot.turn),
    ]
}

/// An arm's local turns (shoulder, elbow) from its socket `shoulder` under a torso turned `parent` to its grip `grip`, the
/// elbow pointing toward `pole` (the forearm bends the other way).
fn arm(shoulder: Vec3, parent: &Rot, grip: Vec3, pole: Vec3) -> [Rot; 2] {
    let (elbow, hand) = reach(shoulder, grip, REACH[0], REACH[1], pole);
    let upper = frame(sub3(elbow, shoulder), scale3(pole, -1.0));
    let fore = hinge(&upper, sub3(hand, elbow));
    [rot_mul(&rot_t(parent), &upper), fore]
}

/// The local turns of a key: root, torso, head, then per side hip, knee, ankle, shoulder, elbow.
struct Turned {
    root: Rot,
    torso: Rot,
    head: Rot,
    legs: [[Rot; 3]; 2],
    arms: [[Rot; 2]; 2],
}

fn solve(k: &Key) -> Turned {
    let torso_at = add3(k.root_t, apply(&k.root_r, [0.0, 0.0, PELVIS]));
    let torso_turn = rot_mul(&k.root_r, &k.torso);
    let legs = [0, 1].map(|side| {
        let socket = add3(
            k.root_t,
            apply(&k.root_r, [side_x(side) * HIP_X, 0.0, PELVIS]),
        );
        leg(socket, &k.root_r, &k.feet[side])
    });
    let arms = [0, 1].map(|side| {
        let socket = add3(
            torso_at,
            apply(
                &torso_turn,
                [side_x(side) * SHOULDER[0], SHOULDER[1], SHOULDER[2]],
            ),
        );
        arm(socket, &torso_turn, k.arms[side].grip, k.arms[side].pole)
    });
    Turned {
        root: k.root_r,
        torso: k.torso,
        head: k.head,
        legs,
        arms,
    }
}

/// The tracks of a list of keys (one key each, in order).
fn tracks(body: &Body<'_>, keys: &[Key]) -> (Vec<Track<Quat16>>, Vec<Track<Vec3>>) {
    let turned: Vec<Turned> = keys.iter().map(solve).collect();
    let track = |node: usize, f: &dyn Fn(&Turned) -> Rot| Track {
        node,
        keys: turned.iter().map(|t| quat16_of(&f(t))).collect(),
    };
    let l = body.limbs;
    let mut rotations = vec![
        track(body.root, &|t| t.root),
        track(body.torso, &|t| t.torso),
        track(body.head, &|t| t.head),
    ];
    for side in 0..2 {
        rotations.push(track(l.legs[side], &|t| t.legs[side][0]));
        rotations.push(track(l.knees[side], &|t| t.legs[side][1]));
        rotations.push(track(l.ankles[side], &|t| t.legs[side][2]));
        rotations.push(track(l.arms[side], &|t| t.arms[side][0]));
        rotations.push(track(l.elbows[side], &|t| t.arms[side][1]));
    }
    let translations = vec![Track {
        node: body.root,
        keys: keys.iter().map(|k| k.root_t).collect(),
    }];
    (rotations, translations)
}

/// The same loop played the other way: key `k` of the result is key `n - k` of `keys`.
fn reversed<T: Clone>(keys: &[T]) -> Vec<T> {
    let n = keys.len();
    (0..n).map(|k| keys[(n - k) % n].clone()).collect()
}

fn reversed_tracks<T: Clone>(tracks: &[Track<T>]) -> Vec<Track<T>> {
    tracks
        .iter()
        .map(|t| Track {
            node: t.node,
            keys: reversed(&t.keys),
        })
        .collect()
}

/// A Hermite curve from `p0` to `p1` with end rates `m0` and `m1`, at `s` (0 to 1).
fn hermite(p0: f32, p1: f32, m0: f32, m1: f32, s: f32) -> f32 {
    let (s2, s3) = (s * s, s * s * s);
    (2.0 * s3 - 3.0 * s2 + 1.0) * p0
        + (s3 - 2.0 * s2 + s) * m0
        + (-2.0 * s3 + 3.0 * s2) * p1
        + (s3 - s2) * m1
}

/// A limb that stands `stance` of the loop and swings the rest, at `phase` (0 to 1, its landing at 0): its place along the
/// way it travels relative to the body, from `land` to `land - travel` while down and back through the air, and its lift.
/// The swing leaves and lands at the ground's speed, so the limb neither scuffs nor skids.
fn stride(phase: f32, stance: f32, land: f32, travel: f32, lift: f32) -> (f32, f32) {
    let phase = phase.rem_euclid(1.0);
    if phase < stance {
        return (land - travel * phase / stance, 0.0);
    }
    let s = (phase - stance) / (1.0 - stance);
    let rate = travel / stance * (1.0 - stance);
    (
        hermite(land - travel, land, -rate, -rate, s),
        lift * (PI * s).sin(),
    )
}

/// The crouched body at breath `b` (-1 to 1): hips low, leaning forward, the head up, the feet under the hips.
fn crouch_key(b: f32) -> Key {
    let foot = |side: usize, y: f32| Foot {
        ankle: [side_x(side) * CROUCH_FEET[0], y, crouch_ankle()],
        turn: deg(X, CROUCH_TOES),
        pole: [side_x(side) * 0.25, 1.0, 0.0],
    };
    let forearm = |side: usize| Arm {
        grip: [side_x(side) * 0.2, 0.36, 0.6],
        pole: [side_x(side) * 0.25, -0.6, -0.4],
    };
    Key {
        root_t: [0.0, 0.0, CROUCH_HIPS - PELVIS + 0.005 * b],
        root_r: IDENTITY,
        torso: deg(X, -(CROUCH_LEAN + b)),
        head: deg(X, CROUCH_HEAD + 0.5 * b),
        feet: [foot(0, CROUCH_FEET[1]), foot(1, CROUCH_FEET[2])],
        arms: [forearm(0), forearm(1)],
    }
}

/// The prone body at breath `b`, the root carried `sway` (a turn about the long axis, degrees, and a shift across), each hand's
/// grip and each foot's ankle given (the figure's space).
fn prone_key(b: f32, roll: f32, shift: f32, grips: [Vec3; 2], ankles: [Vec3; 2]) -> Key {
    let root_r = rot_mul(&deg(Y, roll), &deg(X, -90.0));
    let [hy, hz] = PRONE_HIPS;
    let foot = |side: usize| Foot {
        ankle: ankles[side],
        turn: deg(X, -90.0),
        pole: [side_x(side) * 1.0, 0.0, -0.15],
    };
    let hand = |side: usize| Arm {
        grip: grips[side],
        pole: [side_x(side) * 0.7, -0.5, -0.2],
    };
    Key {
        root_t: [shift, hy - PELVIS, hz + 0.004 * b],
        root_r,
        torso: deg(X, PRONE_CHEST + 0.8 * b),
        head: deg(X, PRONE_HEAD),
        feet: [foot(0), foot(1)],
        arms: [hand(0), hand(1)],
    }
}

/// Where the prone figure's grips and ankles rest (side 0 the left).
fn prone_grip(side: usize, y: f32, lift: f32) -> Vec3 {
    [side_x(side) * 0.3, y, 0.1 + lift]
}

fn prone_ankle(side: usize, y: f32, lift: f32) -> Vec3 {
    [side_x(side) * 0.3, PRONE_HIPS[0] + y, 0.19 + lift]
}

fn sequence(
    name: &'static str,
    duration: f32,
    (rotations, translations): (Vec<Track<Quat16>>, Vec<Track<Vec3>>),
    ground: Vec<Vec3>,
) -> SequenceDef {
    let keyframes = rotations
        .iter()
        .map(|t| t.keys.len())
        .chain(translations.iter().map(|t| t.keys.len()))
        .max()
        .unwrap_or(1);
    SequenceDef {
        name,
        duration,
        cyclic: true,
        priority: 0,
        keyframes,
        rotations,
        translations,
        ground,
    }
}

/// The crouched sequences (see the module docs).
pub(crate) fn crouch(b: &mut ShapeBuilder, body: &Body<'_>) {
    let breath = |k: usize, n: usize| (TAU * k as f32 / n as f32).sin();
    let keys: Vec<Key> = (0..8).map(|k| crouch_key(breath(k, 8))).collect();
    b.sequence(sequence(
        "crouch_root",
        2.0,
        tracks(body, &keys),
        Vec::new(),
    ));

    // The crouched walk: the walk's legs with the hips low; the torso leaned, the arms swinging a little.
    let g = gait::CROUCH;
    let n = 128;
    let l = body.limbs;
    let mut rotations = vec![
        Track {
            node: body.torso,
            keys: vec![quat16_of(&deg(X, -(CROUCH_LEAN + 2.0)))],
        },
        Track {
            node: body.head,
            keys: vec![quat16_of(&deg(X, CROUCH_HEAD))],
        },
    ];
    for side in 0..2 {
        rotations.extend(leg_tracks(&g, l, side, n));
        let lands = gait::LEFT_LANDS + 0.5 * side as f32;
        let swing = move |k: usize| -(TAU * (k as f32 / n as f32 - lands)).cos();
        rotations.push(Track {
            node: l.arms[side],
            keys: (0..n)
                .map(|k| quat16_of(&deg(X, 10.0 + 10.0 * swing(k))))
                .collect(),
        });
        rotations.push(Track {
            node: l.elbows[side],
            keys: (0..n)
                .map(|k| quat16_of(&deg(X, 35.0 + 10.0 * swing(k).max(0.0))))
                .collect(),
        });
    }
    let translations = vec![hip_track(&g, body.root, n)];
    b.sequence(sequence(
        "crouch_forward",
        g.duration,
        (rotations.clone(), translations.clone()),
        travel(n, CROUCH_SPEED, g.duration),
    ));
    b.sequence(sequence(
        "crouch_backward",
        g.duration,
        (reversed_tracks(&rotations), reversed_tracks(&translations)),
        travel_along(n, [0.0, -1.0, 0.0], CROUCH_SPEED, g.duration),
    ));

    // The crouched side step to the left: each foot down 0.6 of the loop, the hips passing over it.
    let n = 32;
    let travel_x = CROUCH_SIDE_SPEED * SIDE_LOOP * 0.6;
    let keys: Vec<Key> = (0..n)
        .map(|k| {
            let p = k as f32 / n as f32;
            let mut key = crouch_key(0.0);
            key.root_t[2] = CROUCH_HIPS + 0.01 - PELVIS + 0.008 * (2.0 * TAU * p).cos();
            key.torso = rot_mul(&deg(Y, -4.0), &deg(X, -CROUCH_LEAN));
            for side in 0..2 {
                // The foot travels toward +x under the body while down (the body moving left).
                let (x, lift) = stride(p - 0.5 * side as f32, 0.6, travel_x * 0.5, travel_x, 0.07);
                let base = side_x(side) * 0.2;
                key.feet[side].ankle = [base - x, [0.06, -0.04][side], SOLE + lift];
                key.feet[side].pole = [side_x(side) * 0.4, 1.0, 0.0];
                key.feet[side].turn = IDENTITY;
            }
            key
        })
        .collect();
    b.sequence(sequence(
        "crouch_side",
        SIDE_LOOP,
        tracks(body, &keys),
        travel_along(n, [-1.0, 0.0, 0.0], CROUCH_SIDE_SPEED, SIDE_LOOP),
    ));
}

/// The prone sequences and the prone arm actions (see the module docs).
pub(crate) fn prone(b: &mut ShapeBuilder, body: &Body<'_>) {
    let grip_y = PRONE_GRIP_Y - 0.24;
    let ankle_y = PRONE_ANKLE_Y;
    let keys: Vec<Key> = (0..8)
        .map(|k| {
            let breath = (TAU * k as f32 / 8.0).sin();
            prone_key(
                breath,
                0.0,
                0.0,
                [0, 1].map(|s| prone_grip(s, grip_y + 0.24, 0.0)),
                [0, 1].map(|s| prone_ankle(s, ankle_y, 0.0)),
            )
        })
        .collect();
    b.sequence(sequence("prone_root", 2.0, tracks(body, &keys), Vec::new()));

    // The crawl: the left hand with the right foot, then the right hand with the left foot.
    let n = 96;
    let stride_len = CRAWL_SPEED * CRAWL_LOOP;
    let keys: Vec<Key> = (0..n)
        .map(|k| {
            let p = k as f32 / n as f32;
            let grips = [0, 1].map(|side| {
                let (y, lift) = stride(
                    p - 0.5 * side as f32,
                    0.6,
                    grip_y + 0.6 * stride_len * 0.5,
                    0.6 * stride_len,
                    0.1,
                );
                prone_grip(side, y, lift)
            });
            let ankles = [0, 1].map(|side| {
                let (y, lift) = stride(
                    p - 0.5 * (1 - side) as f32,
                    0.5,
                    -0.3,
                    0.5 * stride_len,
                    0.06,
                );
                prone_ankle(side, y, lift)
            });
            let sway = (TAU * p).sin();
            prone_key(0.0, 4.0 * sway, 0.02 * sway, grips, ankles)
        })
        .collect();
    let t = tracks(body, &keys);
    b.sequence(sequence(
        "prone_forward",
        CRAWL_LOOP,
        (t.0.clone(), t.1.clone()),
        travel(n, CRAWL_SPEED, CRAWL_LOOP),
    ));
    b.sequence(sequence(
        "prone_backward",
        CRAWL_LOOP,
        (reversed_tracks(&t.0), reversed_tracks(&t.1)),
        travel_along(n, [0.0, -1.0, 0.0], CRAWL_SPEED, CRAWL_LOOP),
    ));

    // Holding and aiming lying down: `hold` and `aim` with the arm raised as far again as the torso lies over.
    let arm_r = body.limbs.arms[1];
    let elbow_r = body.limbs.elbows[1];
    let pitches: Vec<f32> = (0..LOOK_KEYS)
        .map(|k| (1.0 - LOOK_TORSO) * look_pitch(k as f32 / (LOOK_KEYS - 1) as f32))
        .collect();
    let arm_action = |name: &'static str, shoulder: Vec<Quat16>, elbow: Vec<Quat16>| SequenceDef {
        name,
        duration: 1.0,
        cyclic: false,
        priority: 1,
        keyframes: LOOK_KEYS,
        rotations: vec![
            Track {
                node: arm_r,
                keys: shoulder,
            },
            Track {
                node: elbow_r,
                keys: elbow,
            },
        ],
        translations: Vec::new(),
        ground: Vec::new(),
    };
    b.sequence(arm_action(
        "prone_hold",
        pitches
            .iter()
            .map(|d| turns(&[(X, HOLD_SHOULDER + PRONE_TILT + d)]))
            .collect(),
        vec![turns(&[(X, HOLD_ELBOW)])],
    ));
    let (shoulder, elbow): (Vec<Quat16>, Vec<Quat16>) = pitches
        .iter()
        .map(|d| {
            let a = AIM_SHOULDER + PRONE_TILT + d;
            (
                turns(&[(Z, AIM_IN), (X, a)]),
                turns(&[(X, -a), (Z, -AIM_IN), (X, a + AIM_ELBOW)]),
            )
        })
        .unzip();
    b.sequence(arm_action("prone_aim", shoulder, elbow));
}

/// A hit reaction's envelope at key `k` of `n`: a sharp rise to its peak by `peak` of the way, then an eased fall to rest.
fn envelope(k: usize, n: usize, peak: f32) -> f32 {
    let t = k as f32 / (n - 1) as f32;
    if t <= peak {
        let u = t / peak;
        u * (2.0 - u)
    } else {
        let u = (t - peak) / (1.0 - peak);
        let s = 1.0 - u;
        s * s * (3.0 - 2.0 * s)
    }
}

/// The hit reactions (see the module docs), as blend sequences.
pub(crate) fn flinches(b: &mut ShapeBuilder, body: &Body<'_>) {
    let l = body.limbs;
    let n = 9;
    let punch: Vec<f32> = (0..n).map(|k| envelope(k, n, 0.18)).collect();
    // The step lags the punch: the foot goes out as the body is pushed and comes back as it settles.
    let step: Vec<f32> = (0..n).map(|k| envelope(k, n, 0.38)).collect();
    let keyed = |node: usize, f: &dyn Fn(usize) -> Quat16| Track {
        node,
        keys: (0..n).map(f).collect(),
    };
    // (name, torso, head, shoulders, root shift, the stepping leg and its hip's turn).
    type Spin = fn(f32) -> Quat16;
    let body_hits: [(&'static str, Spin, Spin, Spin, Vec3, usize, Spin); 4] = [
        (
            "flinch_front",
            |e| turns(&[(X, 14.0 * e)]),
            |e| turns(&[(X, -8.0 * e)]),
            |e| turns(&[(X, -12.0 * e)]),
            [0.0, -0.06, 0.0],
            1,
            |s| turns(&[(X, -12.0 * s)]),
        ),
        (
            "flinch_back",
            |e| turns(&[(X, -14.0 * e)]),
            |e| turns(&[(X, 10.0 * e)]),
            |e| turns(&[(X, 12.0 * e)]),
            [0.0, 0.06, 0.0],
            0,
            |s| turns(&[(X, 14.0 * s)]),
        ),
        (
            "flinch_left",
            |e| turns(&[(Y, 12.0 * e), (Z, 8.0 * e)]),
            |e| turns(&[(Y, -6.0 * e)]),
            |e| turns(&[(Y, 6.0 * e)]),
            [0.06, 0.0, 0.0],
            1,
            |s| turns(&[(Y, -12.0 * s)]),
        ),
        (
            "flinch_right",
            |e| turns(&[(Y, -12.0 * e), (Z, -8.0 * e)]),
            |e| turns(&[(Y, 6.0 * e)]),
            |e| turns(&[(Y, -6.0 * e)]),
            [-0.06, 0.0, 0.0],
            0,
            |s| turns(&[(Y, 12.0 * s)]),
        ),
    ];
    for (name, torso, head, shoulders, shift, leg, hip) in body_hits {
        let mut rotations = vec![
            keyed(body.torso, &|k| torso(punch[k])),
            keyed(body.head, &|k| head(punch[k])),
            keyed(l.legs[leg], &|k| hip(step[k])),
            keyed(l.knees[leg], &|k| turns(&[(X, -22.0 * step[k])])),
        ];
        for side in 0..2 {
            rotations.push(keyed(l.arms[side], &|k| shoulders(punch[k])));
        }
        b.blend_sequence(SequenceDef {
            name,
            duration: FLINCH_SECONDS,
            cyclic: false,
            priority: 0,
            keyframes: n,
            rotations,
            translations: vec![Track {
                node: body.root,
                keys: (0..n).map(|k| scale3(shift, punch[k])).collect(),
            }],
            ground: Vec::new(),
        });
    }
    let n = 8;
    let snap: Vec<f32> = (0..n).map(|k| envelope(k, n, 1.0 / 7.0)).collect();
    let head_hits: [(&'static str, Spin, Spin); 4] = [
        (
            "flinch_head_front",
            |e| turns(&[(X, 30.0 * e)]),
            |e| turns(&[(X, 5.0 * e)]),
        ),
        (
            "flinch_head_back",
            |e| turns(&[(X, -26.0 * e)]),
            |e| turns(&[(X, -4.0 * e)]),
        ),
        (
            "flinch_head_left",
            |e| turns(&[(Y, 24.0 * e), (Z, 14.0 * e)]),
            |e| turns(&[(Y, 4.0 * e)]),
        ),
        (
            "flinch_head_right",
            |e| turns(&[(Y, -24.0 * e), (Z, -14.0 * e)]),
            |e| turns(&[(Y, -4.0 * e)]),
        ),
    ];
    for (name, head, torso) in head_hits {
        let keyed = |node: usize, f: &dyn Fn(f32) -> Quat16| Track {
            node,
            keys: snap.iter().map(|&e| f(e)).collect(),
        };
        b.blend_sequence(SequenceDef {
            name,
            duration: HEAD_FLINCH_SECONDS,
            cyclic: false,
            priority: 0,
            keyframes: n,
            rotations: vec![keyed(body.head, &head), keyed(body.torso, &torso)],
            translations: Vec::new(),
            ground: Vec::new(),
        });
    }
}

/// `prone_down` and `prone_up`'s lengths (seconds): the ticks the step holds the Player still while it gets down or up (24 and
/// 27 ticks of 32 ms).
pub const PRONE_DOWN_SECONDS: f32 = 0.768;
pub const PRONE_UP_SECONDS: f32 = 0.864;

/// One stage of the way down (see [`way_key`]): the pelvis's pitch (degrees, forward negative), the torso's turn on it and the
/// head's pitch in the world (looking ahead), the hip sockets' middle (along, up), each foot's ankle, pitch (toe down negative)
/// and knee pole, the left hand's grip and elbow pole, the right hand's reach from its shoulder and elbow pole, how much each
/// knee and the left hand stand on the ground (0 to 1).
#[derive(Clone, Copy)]
struct Stage {
    pitch: f32,
    torso: f32,
    head: f32,
    socket: [f32; 2],
    ankles: [Vec3; 2],
    foot_pitch: [f32; 2],
    knee_poles: [Vec3; 2],
    left_grip: Vec3,
    left_pole: Vec3,
    right_reach: Vec3,
    right_pole: Vec3,
    knees_down: [f32; 2],
    hand_down: f32,
}

/// The stages of the way down from the crouch to prone, and when each is reached (shares of the way): crouched; down on the
/// right knee, leaning forward, the left hand reaching; on both knees with the left hand down where it lies prone; the chest
/// lowered and the legs sliding back; prone. The right arm reaches ahead throughout (a pistol held ahead stays ahead).
fn stages() -> [(f32, Stage); 5] {
    let crouch = Stage {
        pitch: 0.0,
        torso: -CROUCH_LEAN,
        head: CROUCH_HEAD - CROUCH_LEAN,
        socket: [0.0, CROUCH_HIPS],
        ankles: [
            [-CROUCH_FEET[0], CROUCH_FEET[1], crouch_ankle()],
            [CROUCH_FEET[0], CROUCH_FEET[2], crouch_ankle()],
        ],
        foot_pitch: [CROUCH_TOES, CROUCH_TOES],
        knee_poles: [[-0.25, 1.0, 0.0], [0.25, 1.0, 0.0]],
        left_grip: [-0.2, 0.36, 0.6],
        left_pole: [-0.25, -0.6, -0.4],
        right_reach: [0.0, 0.55, -0.35],
        right_pole: [0.4, -0.2, -1.0],
        knees_down: [0.0, 0.0],
        hand_down: 0.0,
    };
    let kneel = Stage {
        pitch: -8.0,
        torso: -36.0,
        head: -6.0,
        socket: [-0.04, 0.47],
        ankles: [
            [-CROUCH_FEET[0], CROUCH_FEET[1], crouch_ankle()],
            [0.16, -0.36, 0.12],
        ],
        foot_pitch: [CROUCH_TOES, -60.0],
        knee_poles: [[-0.25, 1.0, 0.0], [0.1, 1.0, -0.2]],
        left_grip: [-0.28, 0.62, 0.22],
        left_pole: [-0.5, -0.5, -0.3],
        right_reach: [0.0, 0.6, -0.3],
        right_pole: [0.4, -0.2, -1.0],
        knees_down: [0.0, 1.0],
        hand_down: 0.0,
    };
    let hands = Stage {
        pitch: -18.0,
        torso: -52.0,
        head: -8.0,
        socket: [-0.04, 0.42],
        ankles: [[-0.16, -0.34, 0.12], [0.16, -0.38, 0.12]],
        foot_pitch: [-60.0, -60.0],
        knee_poles: [[-0.1, 1.0, -0.2], [0.1, 1.0, -0.2]],
        left_grip: [-0.3, PRONE_GRIP_Y, 0.08],
        left_pole: [-0.7, -0.5, -0.2],
        right_reach: [0.0, 0.65, -0.2],
        right_pole: [0.4, -0.2, -1.0],
        knees_down: [1.0, 1.0],
        hand_down: 1.0,
    };
    let lowering = Stage {
        pitch: -58.0,
        torso: -14.0,
        head: -12.0,
        socket: [-0.11, 0.27],
        ankles: [[-0.24, -0.66, 0.16], [0.24, -0.66, 0.16]],
        foot_pitch: [-80.0, -80.0],
        knee_poles: [[-0.7, 0.2, -1.0], [0.7, 0.2, -1.0]],
        left_grip: [-0.3, PRONE_GRIP_Y, 0.08],
        left_pole: [-0.7, -0.5, -0.2],
        right_reach: [0.0, 0.75, -0.05],
        right_pole: [0.4, -0.2, -1.0],
        knees_down: [0.3, 0.3],
        hand_down: 1.0,
    };
    let prone = Stage {
        pitch: -90.0,
        torso: PRONE_CHEST,
        head: PRONE_HEAD + PRONE_CHEST - 90.0,
        socket: PRONE_HIPS,
        ankles: [
            prone_ankle(0, PRONE_ANKLE_Y, 0.0),
            prone_ankle(1, PRONE_ANKLE_Y, 0.0),
        ],
        foot_pitch: [-90.0, -90.0],
        knee_poles: [[-1.0, 0.0, -0.15], [1.0, 0.0, -0.15]],
        left_grip: prone_grip(0, PRONE_GRIP_Y, 0.0),
        left_pole: [-0.7, -0.5, -0.2],
        right_reach: [0.0, 0.8, 0.0],
        right_pole: [0.4, -0.2, -1.0],
        knees_down: [0.0, 0.0],
        hand_down: 1.0,
    };
    [
        (0.0, crouch),
        (0.35, kneel),
        (0.55, hands),
        (0.8, lowering),
        (1.0, prone),
    ]
}

/// The stage at share `t` of the way down: each number on a smooth curve through the stages (Catmull-Rom, still at both
/// ends), so the body moves through them without stopping at each or jumping between them.
fn stage_at(t: f32) -> Stage {
    let st = stages();
    let n = st.len();
    let t = t.clamp(0.0, 1.0);
    let i = (0..n - 1).find(|&i| t <= st[i + 1].0).unwrap_or(n - 2);
    let (t0, t1) = (st[i].0, st[i + 1].0);
    let u = (t - t0) / (t1 - t0);
    let at = |k: usize| st[k.min(n - 1)].1;
    let (a, b) = (at(i), at(i + 1));
    let before = if i == 0 {
        None
    } else {
        Some((st[i - 1].0, at(i - 1)))
    };
    let after = if i + 2 < n {
        Some((st[i + 2].0, at(i + 2)))
    } else {
        None
    };
    let span = t1 - t0;
    let mix = |f: &dyn Fn(&Stage) -> f32| -> f32 {
        let (p0, p1) = (f(&a), f(&b));
        // The rate at each stage: the slope between its neighbors (none at the ends), in this span's terms.
        let m0 = before.map_or(0.0, |(tb, sb)| (p1 - f(&sb)) / (t1 - tb) * span);
        let m1 = after.map_or(0.0, |(ta, sa)| (f(&sa) - p0) / (ta - t0) * span);
        hermite(p0, p1, m0, m1, u)
    };
    let v3 = |f: &dyn Fn(&Stage) -> Vec3| -> Vec3 { [0, 1, 2].map(|k| mix(&|s: &Stage| f(s)[k])) };
    Stage {
        pitch: mix(&|s| s.pitch),
        torso: mix(&|s| s.torso),
        head: mix(&|s| s.head),
        socket: [mix(&|s| s.socket[0]), mix(&|s| s.socket[1])],
        ankles: [v3(&|s| s.ankles[0]), v3(&|s| s.ankles[1])],
        foot_pitch: [mix(&|s| s.foot_pitch[0]), mix(&|s| s.foot_pitch[1])],
        knee_poles: [v3(&|s| s.knee_poles[0]), v3(&|s| s.knee_poles[1])],
        // The hand keeps still where a stage plants it: eased between stages, not carried past them.
        left_grip: {
            let e = u * u * (3.0 - 2.0 * u);
            [0, 1, 2].map(|k| a.left_grip[k] + (b.left_grip[k] - a.left_grip[k]) * e)
        },
        left_pole: v3(&|s| s.left_pole),
        right_reach: v3(&|s| s.right_reach),
        right_pole: v3(&|s| s.right_pole),
        knees_down: [
            mix(&|s| s.knees_down[0]).clamp(0.0, 1.0),
            mix(&|s| s.knees_down[1]).clamp(0.0, 1.0),
        ],
        hand_down: mix(&|s| s.hand_down).clamp(0.0, 1.0),
    }
}

/// The world frames (place and turn) of the nodes a contact is measured on: each knee, ankle and elbow, and the right shoulder.
struct Frames {
    thighs: [(Vec3, Rot); 2],
    knees: [(Vec3, Rot); 2],
    ankles: [(Vec3, Rot); 2],
    elbows: [(Vec3, Rot); 2],
    shoulders: [Vec3; 2],
}

fn frames(k: &Key, t: &Turned) -> Frames {
    let torso_at = add3(k.root_t, apply(&k.root_r, [0.0, 0.0, PELVIS]));
    let torso_turn = rot_mul(&k.root_r, &t.torso);
    let leg = |side: usize| {
        let at = add3(
            k.root_t,
            apply(&k.root_r, [side_x(side) * HIP_X, 0.0, PELVIS]),
        );
        let turn = rot_mul(&k.root_r, &t.legs[side][0]);
        let knee_at = add3(at, apply(&turn, [0.0, 0.0, -THIGH]));
        let knee_turn = rot_mul(&turn, &t.legs[side][1]);
        let ankle_at = add3(knee_at, apply(&knee_turn, [0.0, 0.0, SOLE - SHIN]));
        let ankle_turn = rot_mul(&knee_turn, &t.legs[side][2]);
        ((at, turn), (knee_at, knee_turn), (ankle_at, ankle_turn))
    };
    let shoulder = |side: usize| {
        add3(
            torso_at,
            apply(
                &torso_turn,
                [side_x(side) * SHOULDER[0], SHOULDER[1], SHOULDER[2]],
            ),
        )
    };
    let elbow = |side: usize| {
        let turn = rot_mul(&torso_turn, &t.arms[side][0]);
        let at = add3(shoulder(side), apply(&turn, [0.0, 0.0, -UPPER_ARM]));
        (at, rot_mul(&turn, &t.arms[side][1]))
    };
    let (l, r) = (leg(0), leg(1));
    Frames {
        thighs: [l.0, r.0],
        knees: [l.1, r.1],
        ankles: [l.2, r.2],
        elbows: [elbow(0), elbow(1)],
        shoulders: [shoulder(0), shoulder(1)],
    }
}

/// The lowest point of a box `min..max` on a node at `frame`.
fn lowest(frame: &(Vec3, Rot), min: Vec3, max: Vec3) -> f32 {
    (0..8)
        .map(|c| {
            let p = [
                if c & 1 == 0 { min[0] } else { max[0] },
                if c & 2 == 0 { min[1] } else { max[1] },
                if c & 4 == 0 { min[2] } else { max[2] },
            ];
            frame.0[2] + apply(&frame.1, p)[2]
        })
        .fold(f32::MAX, f32::min)
}

/// The shin's, the foot's and the hand's (with the forearm's) boxes, as the figure draws them.
const SHIN_BOX: (Vec3, Vec3) = ([-0.09, -0.1, -SHIN + 0.08], [0.09, 0.1, 0.0]);
const THIGH_BOX: (Vec3, Vec3) = ([-0.1, -0.11, -THIGH - 0.02], [0.1, 0.11, 0.0]);
const FOOT_BOX: (Vec3, Vec3) = ([-0.095, FOOT_HEEL, -SOLE], [0.095, FOOT_TOE, 0.0]);
const HAND_BOX: (Vec3, Vec3) = ([-0.065, -0.065, -FOREARM], [0.065, 0.065, 0.0]);
/// How far over the floor a part that stands on it is kept (a hair, so it touches without sinking).
const TOUCH: f32 = 0.004;

/// The key at share `t` of the way down: the stage's body placed, each foot kept out of the floor, the left hand set on it as much
/// as it stands there, and the body raised or lowered so a kneeling knee rests on it (a few passes, each placing the limbs
/// again).
fn way_key(t: f32) -> Key {
    let s = stage_at(t);
    let root_r = deg(X, s.pitch);
    let mut socket = [0.0, s.socket[0], s.socket[1]];
    let mut ankles = s.ankles;
    let mut grip = s.left_grip;
    let make = |socket: Vec3, ankles: [Vec3; 2], grip: Vec3, right: Vec3| Key {
        root_t: sub3(socket, apply(&root_r, [0.0, 0.0, PELVIS])),
        root_r,
        torso: deg(X, s.torso),
        head: deg(X, s.head - s.pitch - s.torso),
        feet: [0, 1].map(|side| Foot {
            ankle: ankles[side],
            turn: deg(X, s.foot_pitch[side]),
            pole: s.knee_poles[side],
        }),
        arms: [
            Arm {
                grip,
                pole: s.left_pole,
            },
            Arm {
                grip: right,
                pole: s.right_pole,
            },
        ],
    };
    let mut right = [0.0; 3];
    for _ in 0..6 {
        let key = make(socket, ankles, grip, right);
        let f = frames(&key, &solve(&key));
        right = add3(f.shoulders[1], s.right_reach);
        // A hand still reaching keeps its elbow bent: its grip no further than most of the arm's reach.
        let to = sub3(grip, f.shoulders[0]);
        let d = dot3(to, to).sqrt();
        let most = 0.9 * (UPPER_ARM + GRIP);
        if s.hand_down < 1.0 && d > most {
            grip = add3(f.shoulders[0], scale3(to, most / d));
        }
        for (ankle, frame) in ankles.iter_mut().zip(&f.ankles) {
            ankle[2] += (TOUCH - lowest(frame, FOOT_BOX.0, FOOT_BOX.1)).max(0.0);
        }
        let hand = lowest(&f.elbows[0], HAND_BOX.0, HAND_BOX.1);
        grip[2] += s.hand_down * (TOUCH - hand);
        // A leg's lowest point: its shin's near the knee, or its thigh's.
        let legs = [0, 1].map(|side| {
            lowest(
                &f.knees[side],
                [SHIN_BOX.0[0], SHIN_BOX.0[1], -0.15],
                SHIN_BOX.1,
            )
            .min(lowest(&f.thighs[side], THIGH_BOX.0, THIGH_BOX.1))
        });
        let (w, low) = (0..2)
            .filter(|&side| s.knees_down[side] > 0.0)
            .map(|side| (s.knees_down[side], legs[side]))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((0.0, TOUCH));
        // A kneeling knee rests on the floor as much as it kneels, and no leg ever sinks into it.
        let floor = legs[0].min(legs[1]);
        socket[2] += (w * (TOUCH - low)).max(TOUCH - floor);
    }
    make(socket, ankles, grip, right)
}

/// `prone_down` and `prone_up` (see the module docs): the way down from the crouch to prone and the same way back up, each a
/// key a tick, played once; over every other thread (priority 2), so a held pistol's arm goes down and up with the body.
pub(crate) fn ways(b: &mut ShapeBuilder, body: &Body<'_>) {
    for (name, seconds, up) in [
        ("prone_down", PRONE_DOWN_SECONDS, false),
        ("prone_up", PRONE_UP_SECONDS, true),
    ] {
        let n = (seconds / 0.032).round() as usize + 1;
        let keys: Vec<Key> = (0..n)
            .map(|k| {
                let t = k as f32 / (n - 1) as f32;
                way_key(if up { 1.0 - t } else { t })
            })
            .collect();
        let mut def = sequence(name, seconds, tracks(body, &keys), Vec::new());
        def.cyclic = false;
        def.priority = 2;
        b.sequence(def);
    }
}

#[cfg(test)]
#[path = "poses_tests.rs"]
mod tests;
