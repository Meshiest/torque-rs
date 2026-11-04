//! The math library table: the float conversions the bit stream, the moves and the audio events call through one set of function
//! pointers.
//!
//! Torque calls its math routines through pointers (`mMathFn.h`), which an installer points at one implementation or another.
//! This crate does the same for the float conversions of the wire, so a host may install its own conversions: every entry of
//! [`MathLibrary`] is called through [`get`]. [`PLAIN`] holds this crate's own code,
//! the expressions of the Torque Game Engine 1.4 source with the types the source gives them (an `F32` expression in `f32`, a
//! double literal or `M_PI` in `f64`); a host installs another table once, before its first packet, with [`install`].
//!
//! Credit: the plain routines follow TGE 1.4's `core/bitStream.cc`, `core/color.h`, `math/mMathFn.h`,
//! `game/gameConnectionMoves.cc` and `game/gameConnectionEvents.cc`, carried into the MIT-licensed Torque3D as
//! `core/stream/bitStream.cpp` (Copyright (c) 2012 GarageGames, LLC), `core/stream/bitStream.h` (Copyright (c) 2012 GarageGames,
//! LLC; Copyright (C) 2015 Faust Logic, Inc.), `core/color.h`, `math/mMathFn.h` and `math/mBox.h` (each Copyright (c) 2012
//! GarageGames, LLC), `T3D/gameBase/moveManager.cpp` (Copyright (c) 2012 GarageGames, LLC) and
//! `T3D/gameBase/gameConnectionEvents.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code
//! here is this crate's own.

use std::sync::OnceLock;

/// [`MathLibrary::compress_point`]'s type: `(p, cp, scale) -> (length in units of scale, the axes as integers)`.
pub type CompressPoint = fn([f32; 3], [f32; 3], f32) -> (f32, [i32; 3]);

/// The routines the wire code calls through the table. Each entry names the Torque routine it stands for.
#[derive(Debug, Clone, Copy)]
pub struct MathLibrary {
    /// A float converted to `S32` (the source's `(S32)` casts).
    pub to_int: fn(f32) -> i32,
    /// `BitStream::writeFloat(f, bits)`: the integer written (its low `bits` bits go out).
    pub write_float: fn(f32, u8) -> u32,
    /// `BitStream::writeSignedFloat(f, bits)`: the integer written.
    pub write_signed_float: fn(f32, u8) -> u32,
    /// `BitStream::readSignedFloat(bits)`: the float of the integer read.
    pub read_signed_float: fn(u32, u8) -> f32,
    /// `BitStream::writeRangedF32(value, min, max, bits)`: the integer written.
    pub write_ranged_f32: fn(f32, f32, f32, u8) -> u32,
    /// `BitStream::writeNormalVector`: the angles `(phi, theta)` it writes as signed floats.
    pub normal_angles: fn([f32; 3]) -> (f32, f32),
    /// `BitStream::readNormalVector`: the vector from the two signed floats read (phi's, then theta's).
    pub normal_vector: fn(f32, f32) -> [f32; 3],
    /// `BitStream::writeCompressedPoint(p, scale)` against the compression point `cp` (the arguments are `p`, `cp`, `scale`): the
    /// offset's length in units of `scale`, which picks the form, and the three axes as the integers written.
    pub compress_point: CompressPoint,
    /// `BitStream::readCompressedPoint`: one axis from the integer read, the scale and the compression point's axis.
    pub decompress_axis: fn(i32, f32, f32) -> f32,
    /// `ColorF::operator ColorI`: one channel.
    pub color_byte: fn(f32) -> u8,
    /// `IANG2FANG` (`Move::unclamp`): a move angle from its 16 bits.
    pub move_angle: fn(u16) -> f32,
    /// `Sim3DAudioEvent::unpack`: a cone's quaternion `w` (before its sign) from `x`, `y` and `z`.
    pub cone_w: fn(f32, f32, f32) -> f32,
    /// `Box3F::getCenter`: the centre of a box from its min and max corners.
    pub box_centre: fn([f32; 3], [f32; 3]) -> [f32; 3],
}

/// This crate's own routines: the TGE 1.4 source's expressions.
pub static PLAIN: MathLibrary = MathLibrary {
    to_int: plain::to_int,
    write_float: plain::write_float,
    write_signed_float: plain::write_signed_float,
    read_signed_float: plain::read_signed_float,
    write_ranged_f32: plain::write_ranged_f32,
    normal_angles: plain::normal_angles,
    normal_vector: plain::normal_vector,
    compress_point: plain::compress_point,
    decompress_axis: plain::decompress_axis,
    color_byte: plain::color_byte,
    move_angle: plain::move_angle,
    cone_w: plain::cone_w,
    box_centre: plain::box_centre,
};

static INSTALLED: OnceLock<&'static MathLibrary> = OnceLock::new();

/// The table in use: the installed one, else [`PLAIN`].
#[inline]
pub fn get() -> &'static MathLibrary {
    match INSTALLED.get() {
        Some(lib) => lib,
        None => &PLAIN,
    }
}

/// Installs `library` for the rest of the process. Only the first install takes; installing the same table again is a
/// no-op. Returns false when another table was installed first.
pub fn install(library: &'static MathLibrary) -> bool {
    std::ptr::eq(*INSTALLED.get_or_init(|| library), library)
}

/// The plain routines.
pub mod plain {
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    /// `(S32)f`: Rust's conversion (truncation toward zero; out of range saturates, NaN is 0).
    pub fn to_int(f: f32) -> i32 {
        f as i32
    }

    fn top(bits: u8) -> u32 {
        ((1u64 << bits.min(32)) - 1) as u32
    }

    /// `writeInt((S32)(f * ((1 << bitCount) - 1)), bitCount)`, the product in `F32`.
    pub fn write_float(f: f32, bits: u8) -> u32 {
        to_int(f * top(bits) as f32) as u32
    }

    /// `writeInt((S32)(((f + 1) * .5) * ((1 << bitCount) - 1)), bitCount)`: `f + 1` in `F32`, the rest in double (`.5` is one).
    pub fn write_signed_float(f: f32, bits: u8) -> u32 {
        let v = f64::from(f + 1.0) * 0.5 * f64::from(top(bits));
        (v as i32) as u32
    }

    /// `readInt(bitCount) * 2 / F32((1 << bitCount) - 1) - 1.0f`.
    pub fn read_signed_float(raw: u32, bits: u8) -> f32 {
        raw.wrapping_mul(2) as f32 / top(bits) as f32 - 1.0
    }

    /// `value = (mClampF(value, min, max) - min) / (max - min)`, then `writeInt((S32)mFloor(value * F32((1 << numBits) - 1)),
    /// numBits)`, all in `F32`.
    pub fn write_ranged_f32(value: f32, min: f32, max: f32, bits: u8) -> u32 {
        // mClampF: below min is min, above max is max, anything else (NaN too) is kept.
        let clamped = if value < min {
            min
        } else if value > max {
            max
        } else {
            value
        };
        let t = (clamped - min) / (max - min);
        to_int((t * top(bits) as f32).floor()) as u32
    }

    /// `phi = mAtan(x, y) / M_PI`, `theta = mAtan(z, mSqrt(x*x + y*y)) / (M_PI / 2.0)`: `mAtan` and `mSqrt` return `F32`, the
    /// quotients are doubles stored to `F32`.
    pub fn normal_angles(v: [f32; 3]) -> (f32, f32) {
        let [x, y, z] = v;
        let atan = |a: f32, b: f32| f64::from(a).atan2(f64::from(b)) as f32;
        let h = f64::from(x * x + y * y).sqrt() as f32;
        let phi = (f64::from(atan(x, y)) / PI) as f32;
        let theta = (f64::from(atan(z, h)) / FRAC_PI_2) as f32;
        (phi, theta)
    }

    /// `phi = sf * M_PI`, `theta = sf * (M_PI / 2.0)` stored to `F32`; `x = mSin(phi) * mCos(theta)`, `y = mCos(phi) *
    /// mCos(theta)`, `z = mSin(theta)`.
    pub fn normal_vector(phi_sf: f32, theta_sf: f32) -> [f32; 3] {
        let phi = f64::from((f64::from(phi_sf) * PI) as f32);
        let theta = f64::from((f64::from(theta_sf) * FRAC_PI_2) as f32);
        let (sin_phi, cos_phi) = (phi.sin() as f32, phi.cos() as f32);
        let (sin_theta, cos_theta) = (theta.sin() as f32, theta.cos() as f32);
        [sin_phi * cos_theta, cos_phi * cos_theta, sin_theta]
    }

    /// `invScale = 1 / scale`, `vec = p - mCompressPoint`, `dist = vec.len() * invScale`, each axis `S32(vec.axis * invScale)`,
    /// all in `F32`.
    pub fn compress_point(p: [f32; 3], cp: [f32; 3], scale: f32) -> (f32, [i32; 3]) {
        let inv = 1.0 / scale;
        let d = [p[0] - cp[0], p[1] - cp[1], p[2] - cp[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        (len * inv, d.map(|c| to_int(c * inv)))
    }

    /// `mCompressPoint.axis + (F32)i * scale`, in `F32`.
    pub fn decompress_axis(i: i32, scale: f32, base: f32) -> f32 {
        base + i as f32 * scale
    }

    /// `U8(c * 255.0f + 0.5)`: the product in `F32`, the sum in double, converted to an integer whose low byte is kept.
    pub fn color_byte(c: f32) -> u8 {
        (f64::from(c * 255.0) + 0.5) as i32 as u8
    }

    /// `(F32)((M_2PI / F32(0x10000)) * (F32)((S16)x))`: the product in double.
    pub fn move_angle(p: u16) -> f32 {
        (TAU / 65_536.0 * f64::from(p as i16)) as f32
    }

    /// `value = x*x + y*y + z*z` clamped to 1, `mSqrt(1.f - value)`, in `F32`.
    pub fn cone_w(x: f32, y: f32, z: f32) -> f32 {
        let value = (x * x + y * y + z * z).min(1.0);
        (1.0 - value).sqrt()
    }

    /// `F32((min + max) * 0.5)` per axis: the sum in `F32`, the half in double.
    pub fn box_centre(min: [f32; 3], max: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| (f64::from(min[i] + max[i]) * 0.5) as f32)
    }
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
