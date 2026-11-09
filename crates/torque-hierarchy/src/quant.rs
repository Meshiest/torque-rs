//! The local pose on the wire: translation in 1/1024 units, the rotation as a smallest-three quaternion in 15 bits a
//! component (`torque_collide::rotation_code`, the code other rotations on the wire use) and the scale as a 16-bit
//! float-like code. Only integer steps and `f32` arithmetic that rounds the same everywhere (no `exp`, `ln` or `sin`), so a
//! server and a client agree on the bits.

use torque_collide::math::Quat;
use torque_collide::rotation_code::{self, ROT_BITS, ROT_MID};

use crate::caps::{MAX_SCALE, MAX_TRANSLATION, MIN_SCALE, ParentError};
use crate::types::Local;

/// Translation steps per unit.
pub const TRANSLATION_STEPS: f32 = 1024.0;
/// The largest translation code in magnitude (`MAX_TRANSLATION` in steps).
// 10,240,000 fits an `i32` exactly.
pub const MAX_TRANSLATION_CODE: i32 = (MAX_TRANSLATION * TRANSLATION_STEPS) as i32;
/// Bits of each of the three kept rotation components.
// A widening cast (`u32::from` is not `const`).
pub const ROTATION_BITS: u32 = ROT_BITS as u32;
/// The largest rotation component code.
// 32,766 fits a `u16`.
pub const MAX_ROTATION_CODE: u16 = (2 * ROT_MID) as u16;

/// The scale code of 1: what a child of its own size carries (the per-ghost block sends no scale for it).
pub const UNIT_SCALE_CODE: u16 = scale_code(1.0);

/// A scale code's lowest value is the exponent of 2^-14 (the one `MIN_SCALE` falls in), counted in 2^-11 mantissa steps.
const SCALE_BASE: u32 = 113 << 11;

/// A rotation as the smallest-three form: the index of the largest component (dropped, positive) and the other three in
/// order, each coded over a little more than `[-1/sqrt 2, 1/sqrt 2]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QuantRotation {
    /// Which of `x, y, z, w` (0 to 3) was dropped.
    pub dropped: u8,
    /// The other three components, each `0..=MAX_ROTATION_CODE`.
    pub parts: [u16; 3],
}

/// A local pose as whole numbers, ready for a bit writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QuantLocal {
    /// Each axis in 1/1024 units, within `+-MAX_TRANSLATION_CODE`.
    pub translation: [i32; 3],
    pub rotation: QuantRotation,
    /// Each axis as a code of 5 exponent bits and 11 mantissa bits.
    pub scale: [u16; 3],
}

impl QuantLocal {
    /// The nearest representable code of `local`. Out-of-range values are clamped and NaN gets some code: it never panics.
    #[must_use]
    pub fn from_local(local: &Local) -> QuantLocal {
        QuantLocal {
            translation: local.translation.map(encode_translation),
            rotation: encode_rotation(&local.rotation),
            scale: local.scale.map(encode_scale),
        }
    }

    /// The pose this code stands for. Any code decodes: an out-of-range one is clamped (see [`QuantLocal::check`] to refuse
    /// it instead), so the result always passes [`Local::validate`].
    #[must_use]
    pub fn to_local(&self) -> Local {
        Local {
            translation: self.translation.map(decode_translation),
            rotation: decode_rotation(&self.rotation),
            scale: self.scale.map(decode_scale),
        }
    }

    /// Whether every field is inside the range an encoder writes.
    ///
    /// # Errors
    /// [`ParentError::OutOfRange`] for a translation past the cap, a rotation index past 3 or a part past its bits, or a
    /// scale outside `MIN_SCALE..=MAX_SCALE` (by its decoded value).
    pub fn check(&self) -> Result<(), ParentError> {
        let translation = self
            .translation
            .iter()
            .all(|t| t.unsigned_abs() <= MAX_TRANSLATION_CODE.unsigned_abs());
        let rotation = self.rotation.dropped <= 3
            && self.rotation.parts.iter().all(|p| *p <= MAX_ROTATION_CODE);
        let scale = self.scale.iter().all(|c| {
            let s = scale_value(*c);
            (MIN_SCALE..=MAX_SCALE).contains(&s)
        });
        if translation && rotation && scale {
            Ok(())
        } else {
            Err(ParentError::OutOfRange)
        }
    }
}

/// `local` after a trip through the wire form: what a client has once it has read what a server wrote.
#[must_use]
pub fn quantize(local: &Local) -> Local {
    QuantLocal::from_local(local).to_local()
}

fn encode_translation(t: f32) -> i32 {
    // The float to integer cast saturates and maps NaN to 0; the clamp then holds the range.
    ((t * TRANSLATION_STEPS).round() as i32).clamp(-MAX_TRANSLATION_CODE, MAX_TRANSLATION_CODE)
}

fn decode_translation(code: i32) -> f32 {
    // Within 10,240,000 an `i32` is exact in an `f32` to a step of 1.
    code.clamp(-MAX_TRANSLATION_CODE, MAX_TRANSLATION_CODE) as f32 / TRANSLATION_STEPS
}

fn encode_rotation(q: &Quat) -> QuantRotation {
    // A near tie of two components settles on one in a round or two; a code that is stable is what a client reads back.
    let (mut dropped, mut parts) = rotation_code::pack(*q);
    for _ in 0..4 {
        let (d, p) = rotation_code::pack(rotation_code::unpack(dropped, parts));
        if (d, p) == (dropped, parts) {
            break;
        }
        (dropped, parts) = (d, p);
    }
    QuantRotation { dropped, parts }
}

fn decode_rotation(r: &QuantRotation) -> Quat {
    rotation_code::unpack(r.dropped, r.parts)
}

fn encode_scale(s: f32) -> u16 {
    scale_code(s.clamp(MIN_SCALE, MAX_SCALE))
}

/// The code of a scale already in range.
const fn scale_code(s: f32) -> u16 {
    let field = (s.to_bits() + 2_048) >> 12;
    let code = field.saturating_sub(SCALE_BASE);
    // Held to `u16::MAX` first, so the cast keeps every bit.
    if code > u16::MAX as u32 {
        u16::MAX
    } else {
        code as u16
    }
}

/// The value of scale code `code`, unclamped.
fn scale_value(code: u16) -> f32 {
    f32::from_bits((u32::from(code) + SCALE_BASE) << 12)
}

fn decode_scale(code: u16) -> f32 {
    scale_value(code).clamp(MIN_SCALE, MAX_SCALE)
}

#[cfg(test)]
#[path = "quant_tests.rs"]
mod tests;
