//! The kit's shapes, each a few boxes and prisms ([`crate::builder`]).
//!
//! | file | nodes | sequences | material |
//! |---|---|---|---|
//! | `cube.dts` | `cube` | | `grid` |
//! | `figure.dts` | `root`, `torso`, `head`, `eye`, `cam`, `arm_l`, `elbow_l`, `arm_r`, `elbow_r`, `mount0`, `leg_l`, `knee_l`, `leg_r`, `knee_r` | `root`, `idle`, `walk`, `run`, `back`, `side`, `jump`, `hold`, `look`, `aim`, `head`, `light_recoil`, `death1`, `death2` | `figure` |
//! | `pistol.dts` | `pistol`, `mountPoint`, `muzzlePoint`, `ejectPoint`, `slide` | `fire` | `pistol` |
//! | `projectile.dts` | `projectile` | | `projectile` |
//!
//! **The cube** is the unit box centred on its origin (-0.5 to 0.5 on each axis; scale it into walls and floors), with a
//! `Collision-1` detail (its hull, the same box) and an `LOS-9` detail (the box again) beside the drawn `detail1`.
//!
//! **The figure** stands on its origin facing +y, 2 units tall: legs from the hips (0.95 up) down, each a thigh (`leg_*`) and a
//! shin with its foot (`knee_*`); the torso above them, the head on the torso; arms from the shoulders, each an upper arm
//! (`arm_*`) and a forearm with its hand (`elbow_*`), the hands reaching mid-thigh. `eye` is in front of the head (a
//! first-person view), `cam` at its centre (the point a third-person camera orbits), `mount0` in the closed right hand (where a held
//! item's `mountPoint` goes), turned so the item's +y runs along the forearm. The eyes are two small dark boxes on the face.
//! Its texture is four swatches: skin, shirt, trousers (with the shoes) and the eyes' colour. The sequences turn the joints
//! about x (the side step and the falls also about y and z):
//!
//! - `idle` (and `root`, the same keys under the name a player's rest action looks for): 2 s, looped, the arms sway 3°, the
//!   elbows a little bent.
//! - `walk`: 1 s, looped, legs ±25° and arms ±20° against them; each knee bends (to 50°) as its leg swings through from
//!   behind and is straight while its foot is down; the elbows bend 12° to 24°. The ground frames carry it [`WALK_SPEED`]
//!   units a second.
//! - `run`: 0.6 s, looped, legs ±45°, the knees bending to 107°, arms ±40° with the elbows bent about 80°, the torso leaned
//!   10° forward; [`RUN_SPEED`] units a second.
//! - `back`: 0.7 s, looped, a walk backward: the gait run in reverse, each knee bending as its leg comes back under the hip;
//!   [`BACK_SPEED`] units a second backward.
//! - `side`: 0.6 s, looped, a side step to the left: the left leg lifts and swings out, then the right lifts and closes in to
//!   it, the arms a little out and the torso leaned into the step; [`SIDE_SPEED`] units a second along -x (the way the side
//!   action of a player's table points). A player plays it backward to step right.
//! - `jump`: 0.5 s, played once: a crouch with the arms back, then the knees tucked up and the arms swung forward to about
//!   shoulder height, held on the last key.
//! - `hold`: the right arm raised to aim, from fully up (position 0) through level and straight ahead (0.5) to fully down (1),
//!   [`LOOK_RANGE`] degrees each way; priority 1 and only the right arm, so played on a second thread at the aim's pitch it
//!   holds that arm over any of the others while the figure holds a pistol.
//! - `look`: the same as `hold` (a Player's arm thread starts on `look`; the server sets the arm's action on that thread).
//! - `aim`: as `hold`, the pistol raised to eye level in front of the face (aiming down the sights), over the same range.
//! - `head`: a blend sequence over the same range: the torso and the head bend up and down with the aim (a share each,
//!   [`LOOK_TORSO`] and [`LOOK_HEAD`]), added to whatever else plays.
//! - `light_recoil`: a blend of 0.14 s, played once when the held pistol fires: the right arm kicks up at the shoulder (14°)
//!   and the forearm back at the elbow (18°) within 0.04 s, then settles.
//! - `death1` and `death2`: 0.6 s, played once and held: the knees give, the body tips over at the feet, backward
//!   (`death1`, hit from the front) or forward (`death2`, hit from behind), lands flat with a small bounce and lies still, the
//!   arms flung out to the sides.
//!
//! **The pistol** is a large semi-automatic, 26 cm long: a long squared top (the slide, the whole length from the hammer to
//! the muzzle, with the front and rear sights on it, over a fixed barrel whose end shows as it runs back), an exposed hammer, a trigger in its guard and a grip raked back. It points
//! along +y with its origin at the top of the grip; `mountPoint` is in the grip where the hand holds it, `muzzlePoint` on the
//! bore at the front face, `sightFront` and `sightRear` on the sight line (the front post's top, the rear notch's), on the
//! slide, `ejectPoint` on the right of the top, where a spent case leaves. `fire` (0.15 s, once)
//! runs the whole `slide` back 4 cm and returns it.
//!
//! **The projectile** is a small cube ([`BOLT_SIZE`] a side, about an inch and a half; it hits along its path, not by its size) centered on its origin, self-lit, each face white
//! at its center and pale cyan at its edges.

use std::f32::consts::TAU;

use torque_shapes::shape::{Model, Quat16};

use crate::builder::{
    FULL_UV, Face, MeshData, NO_TURN, SequenceDef, ShapeBuilder, Track, UvRect, Vec3,
    material_flags, turn,
};

/// The figure's ground speed in its `walk`, `run`, `back` and `side` sequences, units a second.
pub const WALK_SPEED: f32 = 1.6;
pub const RUN_SPEED: f32 = 5.0;
pub const BACK_SPEED: f32 = 4.0;
pub const SIDE_SPEED: f32 = 4.5;

/// The pitch the `head` and `hold` sequences span each way (degrees): position 0 looks this far up, 1 this far down.
pub const LOOK_RANGE: f32 = 80.0;
/// How much of the look the torso and the head take (the rest is the eyes').
pub const LOOK_TORSO: f32 = 0.2;
pub const LOOK_HEAD: f32 = 0.35;

const X: Vec3 = [1.0, 0.0, 0.0];
const Y: Vec3 = [0.0, 1.0, 0.0];
const Z: Vec3 = [0.0, 0.0, 1.0];
const OPAQUE: u32 = material_flags::S_WRAP | material_flags::T_WRAP | material_flags::NEVER_ENV_MAP;

/// The unit cube.
pub fn cube() -> Model {
    let mut b = ShapeBuilder::new();
    let node = b.node("cube", None, [0.0; 3]);
    let drawn = b.detail("detail1", 1.0);
    let collision = b.detail("Collision-1", -1.0);
    let sight = b.detail("LOS-9", -1.0);
    let grid = b.material("grid", OPAQUE);
    let (min, max) = ([-0.5; 3], [0.5; 3]);
    let mut mesh = MeshData::new(Some(grid));
    mesh.add_box(min, max, |_| FULL_UV);
    b.object("cube", node, drawn, mesh);
    b.object("Col-1", node, collision, MeshData::hull_box(min, max));
    b.object("LOS-9", node, sight, MeshData::hull_box(min, max));
    b.build()
}

/// The figure's texture swatches (`figure.png`'s quarters; the shoes are the trousers quarter's bottom band).
const SKIN: UvRect = [0.0, 0.0, 0.5, 0.5];
const SHIRT: UvRect = [0.5, 0.0, 1.0, 0.44];
const TROUSERS: UvRect = [0.0, 0.5, 0.5, 0.94];
const SHOES: UvRect = [0.0, 0.95, 0.5, 1.0];
const EYES: UvRect = [0.55, 0.55, 0.95, 0.95];

/// One box of the figure, every face one swatch.
fn part(
    b: &mut ShapeBuilder,
    name: &str,
    node: usize,
    material: u32,
    min: Vec3,
    max: Vec3,
    uv: impl Fn(Face) -> UvRect,
) {
    let mut mesh = MeshData::new(Some(material));
    mesh.add_box(min, max, uv);
    b.object(name, node, 0, mesh);
}

/// The turns about x of `degrees(phase)` at each of `n` keys of a loop (phase 0 to a whole turn).
fn cycle(n: usize, degrees: impl Fn(f32) -> f32) -> Vec<Quat16> {
    (0..n)
        .map(|k| turn(X, degrees(TAU * k as f32 / n as f32)))
        .collect()
}

/// The turns about x of `degrees`, one key each.
fn keys(degrees: &[f32]) -> Vec<Quat16> {
    degrees.iter().map(|&d| turn(X, d)).collect()
}

/// Ground frames moving `speed` units a second forward over `duration`, one a key.
fn travel(n: usize, speed: f32, duration: f32) -> Vec<Vec3> {
    travel_along(n, [0.0, 1.0, 0.0], speed, duration)
}

/// Ground frames moving `speed` units a second along `dir` over `duration`, one a key.
fn travel_along(n: usize, dir: Vec3, speed: f32, duration: f32) -> Vec<Vec3> {
    (1..=n)
        .map(|k| dir.map(|d| d * speed * duration * k as f32 / n as f32))
        .collect()
}

/// One rotation made of turns `(axis, degrees)`, the first applied last (outermost).
fn turns(parts: &[(Vec3, f32)]) -> Quat16 {
    let mut q = [0.0f32, 0.0, 0.0, 1.0];
    for &(axis, degrees) in parts {
        let (s, c) = (0.5 * degrees.to_radians()).sin_cos();
        let r = [axis[0] * s, axis[1] * s, axis[2] * s, c];
        q = [
            q[3] * r[0] + q[0] * r[3] + q[1] * r[2] - q[2] * r[1],
            q[3] * r[1] - q[0] * r[2] + q[1] * r[3] + q[2] * r[0],
            q[3] * r[2] + q[0] * r[1] - q[1] * r[0] + q[2] * r[3],
            q[3] * r[3] - q[0] * r[0] - q[1] * r[1] - q[2] * r[2],
        ];
    }
    let w = q[3].clamp(-1.0, 1.0);
    let s = (1.0 - w * w).sqrt();
    if s < 1e-6 {
        return NO_TURN;
    }
    turn([q[0] / s, q[1] / s, q[2] / s], 2.0 * w.acos().to_degrees())
}

/// The figure's limb lengths: thigh and shin with the foot (the hips 0.95 up), upper arm and forearm with the hand (the hand
/// reaching mid-thigh), and how far down the forearm the hand grips.
pub const THIGH: f32 = 0.48;
pub const SHIN: f32 = 0.47;
pub const UPPER_ARM: f32 = 0.4;
pub const FOREARM: f32 = 0.46;
pub const GRIP: f32 = 0.4;

/// The figure's limbs: each side's hip, knee, shoulder and elbow node (left first).
struct Limbs {
    legs: [usize; 2],
    knees: [usize; 2],
    arms: [usize; 2],
    elbows: [usize; 2],
}

/// A gait's numbers (degrees): the hips' swing, the knees' bend through the swing and always, the arms' swing, the elbows'
/// bend always and as the arm swings forward, and the torso's lean.
struct Gait {
    hips: f32,
    knee_swing: f32,
    knee_always: f32,
    arms: f32,
    elbow_always: f32,
    elbow_swing: f32,
    lean: f32,
}

/// The rotation tracks of a looped gait over `n` keys. The left leg leads (forward at a quarter of the loop), the right half a
/// loop behind; each arm swings against its own side's leg. A knee bends as its leg comes through from behind (the most an
/// eighth of a loop before the leg passes under the hip) and is straight from before the foot lands until it pushes off; an
/// elbow bends more as its arm swings forward.
fn gait_tracks(torso: usize, limbs: &Limbs, n: usize, g: &Gait) -> Vec<Track<Quat16>> {
    gait_tracks_along(torso, limbs, n, g, 1.0)
}

/// [`gait_tracks`] run forward (`dir` 1) or backward (-1: the same loop in reverse, a walk backward).
fn gait_tracks_along(
    torso: usize,
    limbs: &Limbs,
    n: usize,
    g: &Gait,
    dir: f32,
) -> Vec<Track<Quat16>> {
    let half = TAU / 2.0;
    let mut tracks = vec![Track {
        node: torso,
        keys: vec![turn(X, -g.lean)],
    }];
    for side in 0..2 {
        let phase = half * side as f32;
        tracks.push(Track {
            node: limbs.legs[side],
            keys: cycle(n, |p| g.hips * (dir * p + phase).sin()),
        });
        tracks.push(Track {
            node: limbs.knees[side],
            keys: cycle(n, |p| {
                -(g.knee_always + g.knee_swing * (dir * p + phase + TAU / 8.0 * dir).cos().max(0.0))
            }),
        });
        tracks.push(Track {
            node: limbs.arms[side],
            keys: cycle(n, |p| g.arms * (dir * p + phase + half).sin()),
        });
        tracks.push(Track {
            node: limbs.elbows[side],
            keys: cycle(n, |p| {
                g.elbow_always + g.elbow_swing * (dir * p + phase + half).sin().max(0.0)
            }),
        });
    }
    tracks
}

/// The box figure.
pub fn figure() -> Model {
    let mut b = ShapeBuilder::new();
    let root = b.node("root", None, [0.0; 3]);
    let torso = b.node("torso", Some(root), [0.0, 0.0, 0.95]);
    let head = b.node("head", Some(torso), [0.0, 0.0, 0.7]);
    b.node("eye", Some(head), [0.0, 0.1, 0.2]);
    b.node("cam", Some(head), [0.0, 0.0, 0.2]);
    let arm_l = b.node("arm_l", Some(torso), [-0.33, 0.0, 0.66]);
    let elbow_l = b.node("elbow_l", Some(arm_l), [0.0, 0.0, -UPPER_ARM]);
    let arm_r = b.node("arm_r", Some(torso), [0.33, 0.0, 0.66]);
    let elbow_r = b.node("elbow_r", Some(arm_r), [0.0, 0.0, -UPPER_ARM]);
    // In the closed hand, turned so that a held item's +y runs down the forearm and its +z out of the back of the hand.
    b.node_turned("mount0", Some(elbow_r), [0.0, 0.0, -GRIP], turn(X, -90.0));
    let leg_l = b.node("leg_l", Some(root), [-0.12, 0.0, 0.95]);
    let knee_l = b.node("knee_l", Some(leg_l), [0.0, 0.0, -THIGH]);
    let leg_r = b.node("leg_r", Some(root), [0.12, 0.0, 0.95]);
    let knee_r = b.node("knee_r", Some(leg_r), [0.0, 0.0, -THIGH]);
    b.detail("detail1", 1.0);
    let m = b.material("figure", OPAQUE);

    part(
        &mut b,
        "torso",
        torso,
        m,
        [-0.25, -0.13, 0.0],
        [0.25, 0.13, 0.7],
        |_| SHIRT,
    );
    part(
        &mut b,
        "head",
        head,
        m,
        [-0.16, -0.16, 0.02],
        [0.16, 0.16, 0.36],
        |_| SKIN,
    );
    // The eyes: two small boxes standing out of the face (sharp edges at any distance, one flat colour).
    for (side, x) in [("l", -0.06), ("r", 0.06)] {
        let (min, max) = ([x - 0.02, 0.16, 0.222], [x + 0.02, 0.168, 0.264]);
        part(&mut b, &format!("pupil_{side}"), head, m, min, max, |_| {
            EYES
        });
    }
    let wrist = FOREARM - 0.11;
    for (side, arm, elbow) in [("l", arm_l, elbow_l), ("r", arm_r, elbow_r)] {
        let upper = ([-0.07, -0.07, -UPPER_ARM - 0.02], [0.07, 0.07, 0.0]);
        part(
            &mut b,
            &format!("upperarm_{side}"),
            arm,
            m,
            upper.0,
            upper.1,
            |_| SHIRT,
        );
        let fore = ([-0.065, -0.065, -wrist], [0.065, 0.065, 0.0]);
        part(
            &mut b,
            &format!("forearm_{side}"),
            elbow,
            m,
            fore.0,
            fore.1,
            |_| SHIRT,
        );
        let hand = ([-0.05, -0.06, -FOREARM], [0.05, 0.06, -wrist]);
        part(
            &mut b,
            &format!("hand_{side}"),
            elbow,
            m,
            hand.0,
            hand.1,
            |_| SKIN,
        );
    }
    for (side, leg, knee) in [("l", leg_l, knee_l), ("r", leg_r, knee_r)] {
        let thigh = ([-0.1, -0.11, -THIGH - 0.02], [0.1, 0.11, 0.0]);
        part(
            &mut b,
            &format!("thigh_{side}"),
            leg,
            m,
            thigh.0,
            thigh.1,
            |_| TROUSERS,
        );
        let shin = ([-0.09, -0.1, -SHIN + 0.08], [0.09, 0.1, 0.0]);
        part(
            &mut b,
            &format!("shin_{side}"),
            knee,
            m,
            shin.0,
            shin.1,
            |_| TROUSERS,
        );
        let foot = ([-0.095, -0.11, -SHIN], [0.095, 0.17, -SHIN + 0.08]);
        part(
            &mut b,
            &format!("foot_{side}"),
            knee,
            m,
            foot.0,
            foot.1,
            |_| SHOES,
        );
    }
    let limbs = Limbs {
        legs: [leg_l, leg_r],
        knees: [knee_l, knee_r],
        arms: [arm_l, arm_r],
        elbows: [elbow_l, elbow_r],
    };

    let half = TAU / 2.0;
    for name in ["root", "idle"] {
        let mut rotations = Vec::new();
        for side in 0..2 {
            let phase = half * side as f32;
            rotations.push(Track {
                node: limbs.arms[side],
                keys: cycle(4, |p| 3.0 * (p + phase).sin()),
            });
            rotations.push(Track {
                node: limbs.elbows[side],
                keys: cycle(4, |p| 6.0 + 2.0 * (p + phase).sin()),
            });
            rotations.push(Track {
                node: limbs.legs[side],
                keys: vec![NO_TURN],
            });
            rotations.push(Track {
                node: limbs.knees[side],
                keys: vec![NO_TURN],
            });
        }
        b.sequence(SequenceDef {
            name,
            duration: 2.0,
            cyclic: true,
            priority: 0,
            keyframes: 4,
            rotations,
            translations: Vec::new(),
            ground: Vec::new(),
        });
    }
    let walk = Gait {
        hips: 25.0,
        knee_swing: 50.0,
        knee_always: 0.0,
        arms: 20.0,
        elbow_always: 12.0,
        elbow_swing: 12.0,
        lean: 0.0,
    };
    let run = Gait {
        hips: 45.0,
        knee_swing: 95.0,
        knee_always: 12.0,
        arms: 40.0,
        elbow_always: 80.0,
        elbow_swing: 15.0,
        lean: 10.0,
    };
    for (name, duration, gait, speed) in [
        ("walk", 1.0, walk, WALK_SPEED),
        ("run", 0.6, run, RUN_SPEED),
    ] {
        let n = 16;
        b.sequence(SequenceDef {
            name,
            duration,
            cyclic: true,
            priority: 0,
            keyframes: n,
            rotations: gait_tracks(torso, &limbs, n, &gait),
            translations: Vec::new(),
            ground: travel(n, speed, duration),
        });
    }
    // Walking backward: a gait run in reverse, each knee bending as its leg comes back under the hip.
    let back = Gait {
        hips: 28.0,
        knee_swing: 55.0,
        knee_always: 5.0,
        arms: 18.0,
        elbow_always: 25.0,
        elbow_swing: 10.0,
        lean: 4.0,
    };
    let n = 16;
    b.sequence(SequenceDef {
        name: "back",
        duration: 0.7,
        cyclic: true,
        priority: 0,
        keyframes: n,
        rotations: gait_tracks_along(torso, &limbs, n, &back, -1.0),
        translations: Vec::new(),
        ground: travel_along(n, [0.0, -1.0, 0.0], BACK_SPEED, 0.7),
    });
    b.sequence(SequenceDef {
        name: "side",
        duration: 0.6,
        cyclic: true,
        priority: 0,
        keyframes: n,
        rotations: side_tracks(torso, &limbs, n),
        translations: Vec::new(),
        ground: travel_along(n, [-1.0, 0.0, 0.0], SIDE_SPEED, 0.6),
    });
    let track = |node, degrees: &[f32]| Track {
        node,
        keys: keys(degrees),
    };
    b.sequence(SequenceDef {
        name: "jump",
        duration: 0.5,
        cyclic: false,
        priority: 0,
        keyframes: 4,
        rotations: vec![
            track(torso, &[0.0, -8.0, 4.0, 4.0]),
            track(leg_l, &[0.0, 25.0, 70.0, 70.0]),
            track(knee_l, &[0.0, -50.0, -110.0, -110.0]),
            track(leg_r, &[0.0, 15.0, 45.0, 45.0]),
            track(knee_r, &[0.0, -40.0, -95.0, -95.0]),
            track(arm_l, &[0.0, -30.0, 62.0, 62.0]),
            track(elbow_l, &[6.0, 20.0, 30.0, 30.0]),
            track(arm_r, &[0.0, -34.0, 68.0, 68.0]),
            track(elbow_r, &[6.0, 22.0, 26.0, 26.0]),
        ],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    // Holding a pistol: the right arm raised to aim, over whatever else plays, from aiming fully up (position 0) through level
    // (0.5) to fully down (1); the `head` blend bends the torso a share of the way, so the shoulder takes the rest.
    let looks = |share: f32| -> Vec<f32> {
        (0..LOOK_KEYS)
            .map(|k| share * look_pitch(k as f32 / (LOOK_KEYS - 1) as f32))
            .collect()
    };
    let shoulder: Vec<f32> = looks(1.0 - LOOK_TORSO)
        .iter()
        .map(|d| HOLD_SHOULDER + d)
        .collect();
    // `look` is the same pose: a Player's arm thread starts on its shape's `look` (and has none without one), so the server can
    // set the arm's action (`setArmThread`: `aim`, back to `hold`).
    for name in ["hold", "look"] {
        b.sequence(SequenceDef {
            name,
            duration: 1.0,
            cyclic: false,
            priority: 1,
            keyframes: LOOK_KEYS,
            rotations: vec![track(arm_r, &shoulder), track(elbow_r, &[HOLD_ELBOW])],
            translations: Vec::new(),
            ground: Vec::new(),
        });
    }
    // Aiming down the sights: as `hold`, the shoulder raised higher and swung in toward the middle so the hand comes up in
    // front of the face; the elbow turns the forearm back out and down so the pistol still points along the aim.
    let aim_keys = |pitch_share: f32| -> (Vec<Quat16>, Vec<Quat16>) {
        (0..LOOK_KEYS)
            .map(|k| {
                let pitch = pitch_share * look_pitch(k as f32 / (LOOK_KEYS - 1) as f32);
                let a = AIM_SHOULDER + pitch;
                (
                    turns(&[(Z, AIM_IN), (X, a)]),
                    turns(&[(X, -a), (Z, -AIM_IN), (X, a + AIM_ELBOW)]),
                )
            })
            .unzip()
    };
    let (aim_shoulder, aim_elbow) = aim_keys(1.0 - LOOK_TORSO);
    b.sequence(SequenceDef {
        name: "aim",
        duration: 1.0,
        cyclic: false,
        priority: 1,
        keyframes: LOOK_KEYS,
        rotations: vec![
            Track {
                node: arm_r,
                keys: aim_shoulder,
            },
            Track {
                node: elbow_r,
                keys: aim_elbow,
            },
        ],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    // The look: a blend over every other sequence, the torso and the head bending up (position 0) and down (1) with the aim.
    b.blend_sequence(SequenceDef {
        name: "head",
        duration: 1.0,
        cyclic: false,
        priority: 0,
        keyframes: LOOK_KEYS,
        rotations: vec![
            track(torso, &looks(LOOK_TORSO)),
            track(head, &looks(LOOK_HEAD)),
        ],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    // A shot's kick (`light_recoil`, a blend over whatever else plays): the right arm kicks up at the shoulder and the forearm back at
    // the elbow, at their height after RECOIL_RISE and settled by the end.
    b.blend_sequence(SequenceDef {
        name: "light_recoil",
        duration: RECOIL_SECONDS,
        cyclic: false,
        priority: 0,
        keyframes: RECOIL_SHOULDER.len(),
        rotations: vec![
            track(arm_r, &RECOIL_SHOULDER),
            track(elbow_r, &RECOIL_ELBOW),
        ],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    death_sequences(&mut b, root, torso, head, &limbs);
    b.build()
}

/// `light_recoil`: how long it lasts (seconds) and its keys (degrees, evenly spaced: the height at 0.04 s).
pub const RECOIL_SECONDS: f32 = 0.14;
const RECOIL_SHOULDER: [f32; 8] = [0.0, 7.0, 14.0, 10.0, 6.0, 3.5, 1.5, 0.0];
const RECOIL_ELBOW: [f32; 8] = [0.0, 9.0, 18.0, 13.0, 8.0, 4.0, 1.5, 0.0];

/// The keys of the `head` and `hold` sequences.
const LOOK_KEYS: usize = 9;

/// The pitch (degrees, up positive) at position `pos` of the look sequences.
pub fn look_pitch(pos: f32) -> f32 {
    LOOK_RANGE * (1.0 - 2.0 * pos)
}

/// The `side` step's tracks over `n` keys, stepping to the left (-x, the way the side action of a player's table points; played
/// backward it steps right). The first half of the loop the left leg lifts and swings out while the right stays down; the
/// second half the right lifts and closes in to it. The
/// arms hang a little out from the body, the elbows bent, and the torso leans a little into the step.
fn side_tracks(torso: usize, limbs: &Limbs, n: usize) -> Vec<Track<Quat16>> {
    const SPREAD: f32 = 22.0;
    const LIFT: f32 = 38.0;
    let at = |k: usize| TAU * k as f32 / n as f32;
    let open = |p: f32| 0.5 * (1.0 - p.cos());
    let keys_of = |f: &dyn Fn(f32) -> Quat16| (0..n).map(|k| f(at(k))).collect::<Vec<_>>();
    let mut tracks = vec![Track {
        node: torso,
        keys: keys_of(&|p| turns(&[(Y, -(3.0 + 2.0 * open(p))), (X, -4.0)])),
    }];
    // Side 0 is the left (out toward -x: a positive turn about y), 1 the right.
    for side in 0..2 {
        let out = if side == 0 { 1.0 } else { -1.0 };
        let lifted = move |p: f32| {
            let s = p.sin();
            if side == 0 { s.max(0.0) } else { (-s).max(0.0) }
        };
        tracks.push(Track {
            node: limbs.legs[side],
            keys: keys_of(&|p| turns(&[(Y, out * SPREAD * open(p)), (X, 14.0 * lifted(p))])),
        });
        tracks.push(Track {
            node: limbs.knees[side],
            keys: keys_of(&|p| turn(X, -(4.0 + LIFT * lifted(p)))),
        });
        tracks.push(Track {
            node: limbs.arms[side],
            keys: keys_of(&|p| turns(&[(Y, out * (10.0 + 8.0 * open(p))), (X, 6.0)])),
        });
        tracks.push(Track {
            node: limbs.elbows[side],
            keys: keys_of(&|_| turn(X, 28.0)),
        });
    }
    tracks
}

/// The death sequences, each 0.6 s, played once and held on their last key: `death1` falls backward (hit from the front),
/// `death2` forward (hit from behind). The knees give first, the body tips over at the feet and lands flat on the floor (the
/// root lifted by the depth of the side that lands, so nothing sinks into it), with a small bounce, and lies still.
fn death_sequences(b: &mut ShapeBuilder, root: usize, torso: usize, head: usize, limbs: &Limbs) {
    // The share of the fall at each key, a small bounce near the end, and how far the knees give.
    const FALL: [f32; 9] = [0.0, 0.04, 0.12, 0.28, 0.52, 0.8, 1.0, 0.95, 1.0];
    const KNEES: [f32; 9] = [0.0, 0.5, 1.0, 0.9, 0.6, 0.3, 0.1, 0.05, 0.0];
    for (name, dir, lift) in [("death1", 1.0f32, 0.17), ("death2", -1.0, 0.18)] {
        let backward = dir > 0.0;
        let mut rotations = vec![
            Track {
                node: root,
                keys: FALL.iter().map(|f| turn(X, dir * 90.0 * f)).collect(),
            },
            Track {
                node: torso,
                keys: FALL
                    .iter()
                    .zip(KNEES)
                    .map(|(f, k)| turn(X, -dir * (8.0 * k + 6.0 * f)))
                    .collect(),
            },
            Track {
                node: head,
                keys: FALL
                    .iter()
                    .map(|f| {
                        if backward {
                            turn(X, -12.0 * f)
                        } else {
                            turns(&[(Z, 50.0 * f), (X, 10.0 * f)])
                        }
                    })
                    .collect(),
            },
        ];
        for side in 0..2 {
            let out = if side == 0 { 1.0 } else { -1.0 };
            rotations.push(Track {
                node: limbs.legs[side],
                keys: KNEES
                    .iter()
                    .zip(FALL)
                    .map(|(k, f)| turns(&[(Y, out * 8.0 * f), (X, 22.0 * k)]))
                    .collect(),
            });
            rotations.push(Track {
                node: limbs.knees[side],
                keys: KNEES.iter().map(|k| turn(X, -(40.0 * k + 3.0))).collect(),
            });
            // The arms fly up as the body goes, then fall out to the sides on the floor.
            rotations.push(Track {
                node: limbs.arms[side],
                keys: FALL
                    .iter()
                    .map(|&f| {
                        let fly = (f * TAU / 2.0).sin();
                        let forward = if backward {
                            70.0 * fly
                        } else {
                            60.0 * fly - 5.0 * f
                        };
                        turns(&[(Y, out * 55.0 * f), (X, forward)])
                    })
                    .collect(),
            });
            rotations.push(Track {
                node: limbs.elbows[side],
                keys: FALL.iter().map(|f| turn(X, 10.0 + 25.0 * f)).collect(),
            });
        }
        b.sequence(SequenceDef {
            name,
            duration: 0.6,
            cyclic: false,
            priority: 0,
            keyframes: FALL.len(),
            rotations,
            translations: vec![Track {
                node: root,
                keys: FALL.iter().map(|f| [0.0, 0.0, lift * f * f]).collect(),
            }],
            ground: Vec::new(),
        });
    }
}

/// The `hold` pose's shoulder and elbow (degrees): together a quarter turn, the forearm level.
pub const HOLD_SHOULDER: f32 = 78.0;
pub const HOLD_ELBOW: f32 = 12.0;
/// The `aim` pose (degrees): the shoulder raised past level and swung in toward the middle, the elbow straightened a little
/// past straight so the forearm (and the pistol) is level again.
pub const AIM_SHOULDER: f32 = 100.0;
pub const AIM_IN: f32 = 24.0;
pub const AIM_ELBOW: f32 = -10.0;

/// The pistol's texture rectangles: the plain metal and the grip's lined panel (`pistol.png`'s upper and lower halves).
const METAL: UvRect = [0.0, 0.14, 1.0, 0.46];
const GRIP_PANEL: UvRect = [0.0, 0.5, 1.0, 1.0];

/// One part of the pistol: the convex profile `[y, z]` drawn `width` wide about x = 0.
fn pistol_part(
    b: &mut ShapeBuilder,
    name: &str,
    node: usize,
    material: u32,
    width: f32,
    profile: &[[f32; 2]],
    uv: UvRect,
) {
    let mut mesh = MeshData::new(Some(material));
    mesh.add_prism(profile, -0.5 * width, 0.5 * width, uv, uv);
    b.object(name, node, 0, mesh);
}

/// The profile `y0..y1` by `z0..z1`.
fn rect(y0: f32, y1: f32, z0: f32, z1: f32) -> [[f32; 2]; 4] {
    [[y0, z0], [y1, z0], [y1, z1], [y0, z1]]
}

/// The middle of the grip, where the hand closes on it (`mountPoint`).
pub const GRIP_CENTRE: Vec3 = [0.0, -0.015, -0.049];

/// How far `fire` drives the slide back.
pub const RECOIL: f32 = 0.04;
/// The barrel's front face (y) and the bore's height (z): the muzzle point.
pub const MUZZLE_Y: f32 = 0.205;
/// The slide's top (z), the sights' height over it, where along the slide the front post and the rear notch stand (y), the
/// front post's width and the notch's.
pub const SLIDE_TOP: f32 = 0.05;
pub const SIGHT_HEIGHT: f32 = 0.009;
pub const FRONT_SIGHT_Y: f32 = 0.193;
pub const REAR_SIGHT_Y: f32 = -0.025;
pub const FRONT_POST_WIDTH: f32 = 0.004;
pub const NOTCH_WIDTH: f32 = 0.007;
pub const BORE_Z: f32 = 0.033;

/// The pistol: a large semi-automatic with a long squared top whose fixed barrel runs flush to its front, sights front and
/// rear, an exposed hammer, a trigger in its guard and a raked grip. It points along +y; the origin is the top of the grip
/// under the slide, and `mountPoint` is in the grip where the hand closes on it.
pub fn pistol() -> Model {
    let mut b = ShapeBuilder::new();
    let root = b.node("pistol", None, [0.0; 3]);
    b.node("mountPoint", Some(root), GRIP_CENTRE);
    b.node("muzzlePoint", Some(root), [0.0, MUZZLE_Y, BORE_Z]);
    b.node("ejectPoint", Some(root), [0.017, 0.04, 0.042]);
    let slide = b.node("slide", Some(root), [0.0; 3]);
    let top = SLIDE_TOP;
    // The sight line: over the front post's top and through the rear notch, level along +y (both on the slide).
    b.node(
        "sightFront",
        Some(slide),
        [0.0, FRONT_SIGHT_Y, top + SIGHT_HEIGHT],
    );
    b.node(
        "sightRear",
        Some(slide),
        [0.0, REAR_SIGHT_Y, top + SIGHT_HEIGHT],
    );
    b.detail("detail1", 1.0);
    let m = b.material("pistol", OPAQUE);
    let bottom = 0.011;
    let mut part = |name: &str, node: usize, width: f32, profile: &[[f32; 2]], uv: UvRect| {
        pistol_part(&mut b, name, node, m, width, profile, uv);
    };
    // The barrel, fixed, around the bore inside the slide: its end sits just behind the slide's front face and shows as the
    // slide runs back.
    part(
        "barrel",
        root,
        0.014,
        &rect(0.1, MUZZLE_Y - 0.002, BORE_Z - 0.008, BORE_Z + 0.008),
        METAL,
    );
    // The slide: the whole top, from the hammer to the muzzle, with both sights; all of it runs back as the pistol fires.
    let slide_profile = [
        [-0.036, bottom],
        [MUZZLE_Y, bottom],
        [MUZZLE_Y, top],
        [-0.03, top],
        [-0.036, top - 0.006],
    ];
    part("slide", slide, 0.034, &slide_profile, METAL);
    // The iron sights: a thin front post near the muzzle, and at the back a notch (two posts with a gap a little wider than
    // the front post), their tops at one height: aimed, the post sits in the notch, its top level with the notch's.
    let front_sight = [
        [FRONT_SIGHT_Y - 0.007, top],
        [FRONT_SIGHT_Y + 0.007, top],
        [FRONT_SIGHT_Y + 0.007, top + SIGHT_HEIGHT],
        [FRONT_SIGHT_Y - 0.003, top + SIGHT_HEIGHT],
    ];
    part("front_sight", slide, FRONT_POST_WIDTH, &front_sight, METAL);
    let notch = rect(
        REAR_SIGHT_Y - 0.005,
        REAR_SIGHT_Y + 0.005,
        top,
        top + SIGHT_HEIGHT,
    );
    let rear: Vec<MeshData> = [(-0.013, -0.5 * NOTCH_WIDTH), (0.5 * NOTCH_WIDTH, 0.013)]
        .into_iter()
        .map(|(x0, x1)| {
            let mut post = MeshData::new(Some(m));
            post.add_prism(&notch, x0, x1, METAL, METAL);
            post
        })
        .collect();
    // The frame under the slide, and the tang over the hand.
    part(
        "frame",
        root,
        0.03,
        &rect(-0.036, 0.19, -0.004, bottom),
        METAL,
    );
    let tang = [
        [-0.036, -0.004],
        [-0.036, bottom],
        [-0.058, 0.004],
        [-0.058, 0.0],
    ];
    part("tang", root, 0.028, &tang, METAL);
    // The hammer behind the slide, cocked back.
    let hammer = [
        [-0.036, 0.012],
        [-0.036, 0.03],
        [-0.047, 0.036],
        [-0.05, 0.03],
    ];
    part("hammer", root, 0.012, &hammer, METAL);
    // The trigger guard (its bottom bar and its front) and the trigger inside it.
    part(
        "guard_bar",
        root,
        0.012,
        &rect(0.012, 0.08, -0.028, -0.021),
        METAL,
    );
    let guard_front = [
        [0.07, -0.028],
        [0.08, -0.028],
        [0.074, -0.004],
        [0.064, -0.004],
    ];
    part("guard_front", root, 0.012, &guard_front, METAL);
    let trigger = [
        [0.036, -0.004],
        [0.044, -0.004],
        [0.04, -0.02],
        [0.034, -0.018],
    ];
    part("trigger", root, 0.007, &trigger, METAL);
    // The grip, raked back, and the magazine's base.
    let grip = [
        [0.026, -0.004],
        [-0.036, -0.004],
        [-0.056, -0.094],
        [0.006, -0.094],
    ];
    part("grip", root, 0.033, &grip, GRIP_PANEL);
    let base = [
        [0.008, -0.094],
        [-0.058, -0.094],
        [-0.06, -0.1],
        [0.008, -0.1],
    ];
    part("magazine_base", root, 0.035, &base, METAL);
    b.sequence(SequenceDef {
        name: "fire",
        duration: 0.15,
        cyclic: false,
        priority: 0,
        keyframes: 3,
        rotations: Vec::new(),
        translations: vec![Track {
            node: slide,
            keys: vec![[0.0; 3], [0.0, -RECOIL, 0.0], [0.0; 3]],
        }],
        ground: Vec::new(),
    });
    for (name, post) in ["rear_sight", "rear_sight_right"].into_iter().zip(rear) {
        b.object(name, slide, 0, post);
    }
    b.build()
}

/// The projectile cube's side (m).
pub const BOLT_SIZE: f32 = 0.04;

/// The projectile.
pub fn projectile() -> Model {
    let mut b = ShapeBuilder::new();
    let node = b.node("projectile", None, [0.0; 3]);
    b.detail("detail1", 1.0);
    let m = b.material("projectile", OPAQUE | material_flags::SELF_ILLUMINATING);
    let mut bolt = MeshData::new(Some(m));
    let h = BOLT_SIZE * 0.5;
    bolt.add_box([-h; 3], [h; 3], |_| FULL_UV);
    b.object("bolt", node, 0, bolt);
    b.build()
}
