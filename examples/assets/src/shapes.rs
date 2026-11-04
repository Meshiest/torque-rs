//! The kit's shapes, each a few boxes ([`crate::builder`]).
//!
//! | file | nodes | sequences | material |
//! |---|---|---|---|
//! | `cube.dts` | `cube` | | `grid` |
//! | `figure.dts` | `root`, `torso`, `head`, `eye`, `cam`, `arm_l`, `arm_r`, `mount0`, `leg_l`, `leg_r` | `root`, `idle`, `walk`, `run`, `jump` | `figure` |
//! | `pistol.dts` | `pistol`, `mountPoint`, `muzzlePoint`, `ejectPoint`, `slide` | `fire` | `pistol` |
//! | `projectile.dts` | `projectile` | | `projectile` |
//!
//! **The cube** is the unit box centred on its origin (-0.5 to 0.5 on each axis; scale it into walls and floors), with a
//! `Collision-1` detail (its hull, the same box) and an `LOS-9` detail (the box again) beside the drawn `detail1`.
//!
//! **The figure** stands on its origin facing +y, 2 units tall: legs from the hips (0.95 up) down, the torso above them, the
//! head on the torso. `eye` is in front of the head (a first-person view), `cam` at its centre (the point a third-person camera
//! orbits), `mount0` at the right hand (where a held item's `mountPoint` goes). Its texture is four swatches: skin, shirt,
//! trousers and a face. The sequences turn the limbs about x:
//!
//! - `idle` (and `root`, the same keys under the name a player's rest action looks for): 2 s, looped, the arms sway 3°.
//! - `walk`: 1 s, looped, legs ±25° and arms ±20° against them; the ground frames carry it [`WALK_SPEED`] units a second.
//! - `run`: 0.6 s, looped, legs ±45°, arms ±40°, the torso leant 10° forward; [`RUN_SPEED`] units a second.
//! - `jump`: 0.5 s, played once: a crouch, then the legs tucked and the arms raised, held on the last key.
//!
//! **The pistol** is held at its `mountPoint` (the top of the grip, the origin) and points along +y; `muzzlePoint` is at the end
//! of the barrel, `ejectPoint` above the chamber. `fire` (0.15 s, once) moves the `slide` back 6 cm and returns it.
//!
//! **The projectile** is a 30 cm bolt along +y, self-lit.

use std::f32::consts::TAU;

use torque_shapes::shape::{Model, Quat16};

use crate::builder::{
    FULL_UV, Face, MeshData, SequenceDef, ShapeBuilder, Track, UvRect, Vec3, material_flags, turn,
};

/// The figure's ground speed in its `walk` and `run` sequences, units a second.
pub const WALK_SPEED: f32 = 1.6;
pub const RUN_SPEED: f32 = 5.0;

const X: Vec3 = [1.0, 0.0, 0.0];
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

/// The figure's texture swatches (`figure.png`'s quarters).
const SKIN: UvRect = [0.0, 0.0, 0.5, 0.5];
const SHIRT: UvRect = [0.5, 0.0, 1.0, 0.5];
const TROUSERS: UvRect = [0.0, 0.5, 0.5, 1.0];
const FACE: UvRect = [0.5, 0.5, 1.0, 1.0];

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

/// `degrees * sin(phase)` at each of `n` keys of a loop, as turns about x.
fn swing(n: usize, degrees: f32, phase: f32) -> Vec<Quat16> {
    (0..n)
        .map(|k| turn(X, degrees * (TAU * k as f32 / n as f32 + phase).sin()))
        .collect()
}

/// The turns about x of `degrees`, one key each.
fn keys(degrees: &[f32]) -> Vec<Quat16> {
    degrees.iter().map(|&d| turn(X, d)).collect()
}

/// Ground frames moving `speed` units a second forward over `duration`, one a key.
fn travel(n: usize, speed: f32, duration: f32) -> Vec<Vec3> {
    (1..=n)
        .map(|k| [0.0, speed * duration * k as f32 / n as f32, 0.0])
        .collect()
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
    let arm_r = b.node("arm_r", Some(torso), [0.33, 0.0, 0.66]);
    b.node("mount0", Some(arm_r), [0.0, 0.02, -0.62]);
    let leg_l = b.node("leg_l", Some(root), [-0.12, 0.0, 0.95]);
    let leg_r = b.node("leg_r", Some(root), [0.12, 0.0, 0.95]);
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
    let face = |f| if f == Face::YPos { FACE } else { SKIN };
    part(
        &mut b,
        "head",
        head,
        m,
        [-0.16, -0.16, 0.02],
        [0.16, 0.16, 0.36],
        face,
    );
    let arm = ([-0.07, -0.07, -0.66], [0.07, 0.07, 0.0]);
    part(&mut b, "arm_l", arm_l, m, arm.0, arm.1, |_| SHIRT);
    part(&mut b, "arm_r", arm_r, m, arm.0, arm.1, |_| SHIRT);
    let leg = ([-0.1, -0.11, -0.95], [0.1, 0.11, 0.0]);
    part(&mut b, "leg_l", leg_l, m, leg.0, leg.1, |_| TROUSERS);
    part(&mut b, "leg_r", leg_r, m, leg.0, leg.1, |_| TROUSERS);

    let half = TAU / 2.0;
    for name in ["root", "idle"] {
        b.sequence(SequenceDef {
            name,
            duration: 2.0,
            cyclic: true,
            priority: 0,
            keyframes: 4,
            rotations: vec![
                Track {
                    node: arm_l,
                    keys: swing(4, 3.0, 0.0),
                },
                Track {
                    node: arm_r,
                    keys: swing(4, 3.0, half),
                },
            ],
            translations: Vec::new(),
            ground: Vec::new(),
        });
    }
    let gait = |name, duration, legs: f32, arms: f32, lean: f32, speed: f32| SequenceDef {
        name,
        duration,
        cyclic: true,
        priority: 0,
        keyframes: 8,
        rotations: vec![
            Track {
                node: torso,
                keys: vec![turn(X, -lean)],
            },
            Track {
                node: leg_l,
                keys: swing(8, legs, 0.0),
            },
            Track {
                node: leg_r,
                keys: swing(8, legs, half),
            },
            Track {
                node: arm_l,
                keys: swing(8, arms, half),
            },
            Track {
                node: arm_r,
                keys: swing(8, arms, 0.0),
            },
        ],
        translations: Vec::new(),
        ground: travel(8, speed, duration),
    };
    b.sequence(gait("walk", 1.0, 25.0, 20.0, 0.0, WALK_SPEED));
    b.sequence(gait("run", 0.6, 45.0, 40.0, 10.0, RUN_SPEED));
    b.sequence(SequenceDef {
        name: "jump",
        duration: 0.5,
        cyclic: false,
        priority: 0,
        keyframes: 4,
        rotations: vec![
            Track {
                node: torso,
                keys: keys(&[0.0, -8.0, 0.0, 0.0]),
            },
            Track {
                node: leg_l,
                keys: keys(&[0.0, 30.0, 40.0, 40.0]),
            },
            Track {
                node: leg_r,
                keys: keys(&[0.0, 10.0, 20.0, 20.0]),
            },
            Track {
                node: arm_l,
                keys: keys(&[0.0, -20.0, 150.0, 150.0]),
            },
            Track {
                node: arm_r,
                keys: keys(&[0.0, -20.0, 150.0, 150.0]),
            },
        ],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    b.build()
}

/// The pistol.
pub fn pistol() -> Model {
    let mut b = ShapeBuilder::new();
    let root = b.node("pistol", None, [0.0; 3]);
    b.node("mountPoint", Some(root), [0.0; 3]);
    b.node("muzzlePoint", Some(root), [0.0, 0.3, 0.075]);
    b.node("ejectPoint", Some(root), [0.04, 0.07, 0.09]);
    let rest = [0.0, 0.0, 0.075];
    let slide = b.node("slide", Some(root), rest);
    b.detail("detail1", 1.0);
    let m = b.material("pistol", OPAQUE);
    let mut grip = MeshData::new(Some(m));
    grip.add_box([-0.025, -0.05, -0.13], [0.025, 0.02, 0.05], |_| FULL_UV);
    b.object("grip", root, 0, grip);
    let mut top = MeshData::new(Some(m));
    top.add_box([-0.03, -0.06, -0.035], [0.03, 0.24, 0.035], |_| FULL_UV);
    b.object("slide", slide, 0, top);
    b.sequence(SequenceDef {
        name: "fire",
        duration: 0.15,
        cyclic: false,
        priority: 0,
        keyframes: 3,
        rotations: Vec::new(),
        translations: vec![Track {
            node: slide,
            keys: vec![rest, [0.0, -0.06, 0.075], rest],
        }],
        ground: Vec::new(),
    });
    b.build()
}

/// The projectile.
pub fn projectile() -> Model {
    let mut b = ShapeBuilder::new();
    let node = b.node("projectile", None, [0.0; 3]);
    b.detail("detail1", 1.0);
    let m = b.material("projectile", OPAQUE | material_flags::SELF_ILLUMINATING);
    let mut bolt = MeshData::new(Some(m));
    bolt.add_box([-0.03, -0.15, -0.03], [0.03, 0.15, 0.03], |_| FULL_UV);
    b.object("bolt", node, 0, bolt);
    b.build()
}
