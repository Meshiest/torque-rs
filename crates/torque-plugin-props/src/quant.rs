//! A prop's state at the wire's precision: positions to 1/1024 of a unit, rotations as the three smallest components of the
//! quaternion in 15 bits each, speeds to 1/256 of a unit (or radian) a second.
//!
//! The server holds its props to exactly this: after each step it quantizes every awake prop's state and writes the
//! dequantized state back into its world ([`Quantized::state`]), so what a client is sent is the server's state bit for bit,
//! and a client that restores a prop from it ([`Quantized::state`] on its side) holds what the server held. Both directions
//! are integer and IEEE operations only (a square root, no transcendental), so they give the same bits on every target.
//!
//! **Stable.** A quantized state quantizes back to the same position and speeds, and to the same turn (two rotation components of
//! nearly one size are settled on one before a state is given out), so a writeback never drifts.

use torque_collide::rotation_code;
use torque_physics_rapier::BodyState;

/// Steps a unit of position.
pub const POS_SCALE: f64 = 1024.0;
/// Steps a unit a second of speed (and a radian a second of spin).
pub const VEL_SCALE: f64 = 256.0;
/// The farthest a prop may be from the origin along an axis (the physics world's reach).
pub const MAX_COORD: f32 = torque_collide::types::WORLD_REACH;
/// The fastest a prop may move, units a second.
pub const MAX_SPEED: f32 = 500.0;
/// The fastest it may spin, radians a second.
pub const MAX_SPIN: f32 = 200.0;
pub use torque_collide::rotation_code::{ROT_BITS, ROT_MID};

const POS_MAX: i32 = (MAX_COORD as f64 * POS_SCALE) as i32;
const LIN_MAX: i32 = (MAX_SPEED as f64 * VEL_SCALE) as i32;
const ANG_MAX: i32 = (MAX_SPIN as f64 * VEL_SCALE) as i32;

/// A prop's state as integers: what the wire carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Quantized {
    /// The position in 1/1024 units.
    pub pos: [i32; 3],
    /// The rotation: which component of `x, y, z, w` is the largest (it is left out and made positive), then the other three
    /// in their order, each `ROT_MID + c / 0.7125 * ROT_MID` rounded.
    pub rot_index: u8,
    pub rot: [u16; 3],
    /// The speed and the spin in 1/256 units (radians) a second.
    pub lin: [i32; 3],
    pub ang: [i32; 3],
}

impl Default for Quantized {
    /// At the origin, unturned, still.
    fn default() -> Quantized {
        Quantized {
            pos: [0; 3],
            rot_index: 3,
            rot: [ROT_MID as u16; 3],
            lin: [0; 3],
            ang: [0; 3],
        }
    }
}

fn round(v: f64, scale: f64, max: i32) -> i32 {
    let x = v * scale;
    if x.is_nan() {
        return 0;
    }
    (x.round() as i64).clamp(-i64::from(max), i64::from(max)) as i32
}

impl Quantized {
    /// The state at the wire's precision (each value held to the world's reach and speeds). The rotation is taken round again
    /// until it quantizes to itself (a near tie of two components settles on one in a round or two).
    pub fn of(s: &BodyState) -> Quantized {
        let (rot_index, rot) = rotation_code::pack_stable(s.rotation);
        Quantized {
            pos: s.position.map(|v| round(f64::from(v), POS_SCALE, POS_MAX)),
            rot_index,
            rot,
            lin: s.linvel.map(|v| round(f64::from(v), VEL_SCALE, LIN_MAX)),
            ang: s.angvel.map(|v| round(f64::from(v), VEL_SCALE, ANG_MAX)),
        }
    }

    /// The state the integers stand for (not asleep).
    pub fn state(&self) -> BodyState {
        BodyState {
            position: self.position(),
            rotation: rotation_code::unpack(self.rot_index, self.rot),
            linvel: self.lin.map(|v| (f64::from(v) / VEL_SCALE) as f32),
            angvel: self.ang.map(|v| (f64::from(v) / VEL_SCALE) as f32),
            sleeping: false,
        }
    }

    /// The position.
    pub fn position(&self) -> [f32; 3] {
        self.pos.map(|v| (f64::from(v) / POS_SCALE) as f32)
    }

    /// The rotation, `x, y, z, w`.
    pub fn rotation(&self) -> [f32; 4] {
        rotation_code::unpack(self.rot_index, self.rot)
    }

    /// Whether it moves or turns at all.
    pub fn moving(&self) -> bool {
        self.lin != [0; 3] || self.ang != [0; 3]
    }

    /// The same state, stopped.
    pub fn stopped(mut self) -> Quantized {
        self.lin = [0; 3];
        self.ang = [0; 3];
        self
    }

    /// Every value held to its range: what a decoder makes of hostile integers.
    pub fn held(mut self) -> Quantized {
        self.pos = self.pos.map(|v| v.clamp(-POS_MAX, POS_MAX));
        self.rot_index = self.rot_index.min(3);
        self.rot = self.rot.map(|v| v.min(2 * ROT_MID as u16));
        self.lin = self.lin.map(|v| v.clamp(-LIN_MAX, LIN_MAX));
        self.ang = self.ang.map(|v| v.clamp(-ANG_MAX, ANG_MAX));
        self
    }

    /// The speed in units a second.
    pub fn speed(&self) -> f32 {
        let v = self.lin.map(|c| f64::from(c) / VEL_SCALE);
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() as f32
    }

    /// The thirteen integers, in the order the server's console fields hold them.
    pub fn words(&self) -> [i32; 13] {
        [
            self.pos[0],
            self.pos[1],
            self.pos[2],
            i32::from(self.rot_index),
            i32::from(self.rot[0]),
            i32::from(self.rot[1]),
            i32::from(self.rot[2]),
            self.lin[0],
            self.lin[1],
            self.lin[2],
            self.ang[0],
            self.ang[1],
            self.ang[2],
        ]
    }

    /// The state of [`Quantized::words`], held to its ranges.
    pub fn from_words(w: [i32; 13]) -> Quantized {
        Quantized {
            pos: [w[0], w[1], w[2]],
            rot_index: w[3].clamp(0, 3) as u8,
            rot: [w[4], w[5], w[6]].map(|v| v.clamp(0, 2 * ROT_MID) as u16),
            lin: [w[7], w[8], w[9]],
            ang: [w[10], w[11], w[12]],
        }
        .held()
    }
}

/// The conjugate of a quaternion `x, y, z, w`: the other turn. Torque's `QuatF` turns the other way from the physics world's
/// (its `setMatrix` writes the transpose), so a pose's quaternion is conjugated on its way to and from Torque's maths.
pub use torque_collide::math::quat::conjugate;

/// The product `a b` of two quaternions `x, y, z, w` of the physics world (`b`'s turn, then `a`'s), each component's terms
/// summed in the order of `a`'s `w, x, y, z` ([`torque_collide::math::quat::mul_wxyz`], not `QuatF::mul`'s rounding).
pub use torque_collide::math::quat::mul_wxyz as quat_mul;

/// `a` at `t` 0 and `b` at `t` 1, blended straight the short way and made unit again (a zero result is no turn).
pub use torque_collide::math::quat::nlerp as quat_nlerp;

/// A row-major 4x4 transform (translation in elements 3, 7 and 11, as the collision library's) of a position and a unit
/// quaternion `x, y, z, w` of the physics world: a point `p` of the prop is at `M p`. Torque's own matrix maths
/// ([`torque_ts_vm::geometry::matrix_from_quat`]), so the transform is the one a script's `rotation` would give.
pub fn matrix(position: [f32; 3], q: [f32; 4]) -> [f32; 16] {
    let m = torque_ts_vm::geometry::matrix_from_quat(conjugate(q));
    let mut out: [f32; 16] = std::array::from_fn(|i| m[i / 4][i % 4]);
    (out[3], out[7], out[11]) = (position[0], position[1], position[2]);
    out
}

/// The position and the unit quaternion of the physics world that a transform `m` (row-major, as [`matrix`] makes one) puts a prop
/// at: the inverse of [`matrix`] up to the rounding of the turn. None when the turn is not a number.
pub fn pose_of(m: &[f32; 16]) -> Option<([f32; 3], [f32; 4])> {
    let rows: torque_ts_vm::geometry::Mat4 =
        std::array::from_fn(|r| std::array::from_fn(|c| m[r * 4 + c]));
    let rotation = conjugate(torque_ts_vm::geometry::quat_from_matrix(&rows));
    let position = [m[3], m[7], m[11]];
    position
        .iter()
        .chain(&rotation)
        .all(|v| v.is_finite())
        .then_some((position, rotation))
}

#[cfg(test)]
#[path = "quant_tests.rs"]
mod tests;
