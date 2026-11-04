//! The ground transform of a sequence and the per-action motion `PlayerData::preload` derives from it: `TSThread::getGround`,
//! `TSShapeInstance::animateGround`'s path and the action table's `getGroundInfo`.
//!
//! A frame index past the shape's ground arrays (a hostile or NaN position) reads the unit rotation and the zero translation.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `ts/tsThread.cpp` and
//! `ts/tsAnimate.cpp` (Copyright (c) 2012 GarageGames, LLC); `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright
//! (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use collide::math::{F3, Matrix, float_to_int, inverse, matrix_product, quat_matrix};
use shapes::Shape;
use shapes::instance::interpolate_quat;
use shapes::shape::Sequence;

/// `TSShape::Sequence` flags.
const CYCLIC: u32 = shapes::instance::FLAG_CYCLIC;
const MAKE_PATH: u32 = shapes::instance::FLAG_MAKE_PATH;

/// `Quat16::get`.
fn quat16(q: [i16; 4]) -> [f32; 4] {
    q.map(|s| f32::from(s) / 32767.0)
}

/// Ground frame `i` of the shape (absolute index), or the unit transform's parts when the index is outside the arrays.
fn frame(shape: &Shape, i: i64) -> ([f32; 4], F3) {
    let Ok(i) = usize::try_from(i) else {
        return ([0.0, 0.0, 0.0, 1.0], [0.0; 3]);
    };
    match (
        shape.ground_rotations.get(i),
        shape.ground_translations.get(i),
    ) {
        (Some(q), Some(p)) => (quat16(*q), *p),
        _ => ([0.0, 0.0, 0.0, 1.0], [0.0; 3]),
    }
}

/// `TSThread::getGround(t)`: `p = t * 0.99999 * numGroundFrames`, the frames `kf - 1` (the unit transform for `kf = 0`) and
/// `kf` from `firstGroundFrame`, the rotation interpolated (`QuatF::interpolate`) and set as a matrix, the
/// translation `(p2 - p1) * k + p1`.
pub fn ground_at(shape: &Shape, seq: &Sequence, t: f32) -> Matrix {
    let lib = crate::library::get();
    let (kf, k) = (lib.ground_frame)(t, seq.num_ground_frames);
    let first = i64::from(seq.first_ground_frame);
    let (q1, p1) = if kf == 0 {
        ([0.0, 0.0, 0.0, 1.0], [0.0; 3])
    } else {
        frame(shape, first + i64::from(kf) - 1)
    };
    let (q2, p2) = frame(shape, first + i64::from(kf));
    let q = interpolate_quat(q1, q2, k);
    let mut m = quat_matrix(q);
    let lerp = |a: f32, b: f32| (lib.ground_lerp)(a, b, k);
    m[3] = lerp(p1[0], p2[0]);
    m[7] = lerp(p1[1], p2[1]);
    m[11] = lerp(p1[2], p2[2]);
    m
}

/// The ground displacement of a thread from `from` to `to`: `G(to) * inverse(G(from)) * onto`.
pub fn displacement(shape: &Shape, seq: &Sequence, from: f32, to: f32, onto: &Matrix) -> Matrix {
    let g_from = inverse(&ground_at(shape, seq, from));
    let g_to = ground_at(shape, seq, to);
    matrix_product(&matrix_product(&g_to, &g_from), onto)
}

/// Work charged for one `displacement` (two ground frames, an inverse and two products) and for one action's `getGroundInfo`.
pub const GROUND_WORK: u64 = 8;
pub const ACTION_WORK: u64 = 4;

/// The loops `animateGround` composes, at most (one product per whole loop of the thread, a count a script cannot
/// raise past what one advance makes; this bounds a hostile duration).
pub const SIM_GROUND_LOOPS: i32 = 1024;

/// `TSShapeInstance::animateGround` for a thread whose path ran from `last` to `cur` over `loops` whole loops: the identity,
/// then the displacement to the loop's end, one full loop for each further loop, and the last part. Each displacement is
/// charged ([`GROUND_WORK`]) before it is made; `None` when the budget refuses.
pub fn ground_path(
    shape: &Shape,
    seq: &Sequence,
    last: f32,
    cur: f32,
    loops: i32,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Option<Matrix> {
    let mut g = collide::math::IDENTITY;
    if seq.num_ground_frames < 1 {
        return Some(g);
    }
    let loops = loops.clamp(-SIM_GROUND_LOOPS, SIM_GROUND_LOOPS);
    let mut step = |g: &Matrix, from: f32, to: f32| {
        charge(GROUND_WORK).then(|| displacement(shape, seq, from, to, g))
    };
    let from = if loops >= 1 {
        g = step(&g, last, 1.0)?;
        for _ in 1..loops {
            g = step(&g, 0.0, 1.0)?;
        }
        0.0
    } else if loops < 0 {
        g = step(&g, last, 0.0)?;
        for _ in (loops + 1)..0 {
            g = step(&g, 1.0, 0.0)?;
        }
        1.0
    } else {
        last
    };
    step(&g, from, cur)
}

/// An action's motion (`PlayerData::ActionAnimation`): its direction, speed, the velocity-scale flag and the death flag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActionMotion {
    pub dir: F3,
    pub speed: f32,
    pub velocity_scale: bool,
    pub death: bool,
}

/// `getGroundInfo` for an action named `name` playing sequence `seq` (an index the shape has), with the table's direction
/// `saved` and velocity-scale flag: a "death..." action has no motion; otherwise the thread is set on the sequence at 0,
/// advanced by `1 / duration` (one second) and the ground path's translation is the motion, normalised; a motion under 0.01
/// falls back to the table's direction at speed 1 (and no velocity scale) when that has a length over 0.01, else speed 0.
pub fn ground_info(
    shape: &Shape,
    name: &[u8],
    seq: usize,
    saved: F3,
    velocity_scale: bool,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Option<ActionMotion> {
    if !charge(ACTION_WORK) {
        return None;
    }
    let death = name.len() >= 5 && name[..5].eq_ignore_ascii_case(b"death");
    if death {
        return Some(ActionMotion {
            dir: [0.0; 3],
            speed: 0.0,
            velocity_scale,
            death: true,
        });
    }
    let Some(s) = shape.sequences.get(seq) else {
        return Some(ActionMotion {
            dir: saved,
            speed: 0.0,
            velocity_scale,
            death: false,
        });
    };
    // The thread at 0, then `advancePos(timeScale / duration)`.
    let delta = if 0.0 < s.duration {
        1.0 / s.duration
    } else {
        0.0
    };
    let (mut cur, mut loops) = (0.0f32, 0i32);
    if s.flags & MAKE_PATH != 0 {
        let moved = 0.0 + delta;
        if s.flags & CYCLIC == 0 {
            cur = if moved <= 1.0 {
                if moved <= 0.0 { 0.0 } else { moved }
            } else {
                1.0
            };
        } else {
            loops = float_to_int(f64::from(moved));
            if moved < 0.0 {
                loops = loops.wrapping_sub(1);
            }
            let mut p = moved - loops as f32;
            if p < 0.0 {
                p += 1.0;
            }
            if 1.0 <= p {
                p -= 1.0;
            }
            cur = p;
        }
    }
    let g = ground_path(shape, s, 0.0, cur, loops, charge)?;
    let dir = [g[3], g[7], g[11]];
    let motion = crate::library::get().ground_motion;
    if let Some((dir, speed)) = motion(dir) {
        return Some(ActionMotion {
            dir,
            speed,
            velocity_scale,
            death: false,
        });
    }
    if motion(saved).is_none() {
        return Some(ActionMotion {
            dir,
            speed: 0.0,
            velocity_scale,
            death: false,
        });
    }
    Some(ActionMotion {
        dir: saved,
        speed: 1.0,
        velocity_scale: false,
        death: false,
    })
}

#[cfg(test)]
#[path = "ground_tests.rs"]
mod tests;
