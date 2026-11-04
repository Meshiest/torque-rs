//! The pieces the simulation records share: the compressed point (`writeCompressedPoint` at its default scale 0.01), the
//! velocity block (a normal vector and a 13-bit speed, as `Player::packUpdate` and `Projectile::packUpdate` send it), the normal
//! vector ([`Normal`]), `writeFloat` and the signed float, and Torque's move as `Move::pack` sends it ([`PackedMove`]), as wire
//! values.
//!
//! **Records hold wire values.** A compressed point is its form and its three sign/magnitude pairs (or three raw floats for form
//! 3), a normal its two integers. The packers quantise through `torque_net::bits`' writers (the one copy of those formulas, through
//! `torque_net::library`) and read the wire values back; the client's floats come from the `value` helpers.
//!
//! **The velocity block** (`Player::packUpdate`): `len = mVelocity.len()` (an `F32`); `flag(len > 0.02)`, a double compare
//! (false for a NaN); `outVel = mVelocity * (1 / len)` in `F32`, written as a normal vector; `len *= 32.0` (five bits of
//! fraction), `8191` when `len > 8191`, then `(S32)len` in 13 bits. The client reads `normal * (int / 32.0f)`.
//!
//! Credit: these follow TGE 1.4's `game/player.cc` and `game/projectile.cc` and `core/bitStream.h`, carried into the
//! MIT-licensed Torque3D as `T3D/player.cpp`, `T3D/projectile.cpp` and `core/stream/bitStream.h` (each Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), and TGE 1.4's `game/gameConnectionMoves.cc` (`Move::pack`;
//! Copyright (C) GarageGames.com, Inc.), carried into Torque3D as `T3D/gameBase/moveManager.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use torque_net::bits::{BitReader, BitWriter};
use torque_net::library;
use torque_net::moves::{MAX_TRIGGER_KEYS, Move};

use crate::codec::{Codec, F32, R, mask_bits, opt};
use crate::{ObjectView, boolean, word};

/// A sign flag and a magnitude (the low bits that the field holds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Signed {
    pub negative: bool,
    pub magnitude: u32,
}

impl Signed {
    /// The sign and magnitude of `n`, the magnitude cut to `bits`.
    pub fn of(n: i32, bits: u8) -> Signed {
        let (negative, magnitude) = if n < 0 {
            (true, n.wrapping_neg() as u32)
        } else {
            (false, n as u32)
        };
        Signed {
            negative,
            magnitude: mask_bits(magnitude, bits),
        }
    }

    /// The number the sign and magnitude give (`readSignedInt`'s; the magnitude is taken as 31 bits, so it cannot overflow).
    pub fn value(self) -> i32 {
        let m = (self.magnitude & 0x7fff_ffff) as i32;
        if self.negative { -m } else { m }
    }
}

/// The compressed point's scale (`writeCompressedPoint`'s default).
pub const POINT_SCALE: f32 = 0.01;

/// The widths of the compressed point's three relative forms, the sign flag included (`torque_net::bits`).
const POINT_BITS: [u8; 3] = [16, 18, 20];

/// A compressed point as it travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CPoint {
    /// Forms 0..=2: per axis a sign and the magnitude in 15, 17 or 19 bits, against the stream's compression point.
    Relative { form: u32, axes: [Signed; 3] },
    /// Form 3: the absolute position as three raw floats.
    Absolute([F32; 3]),
}

impl Default for CPoint {
    fn default() -> CPoint {
        CPoint::Relative {
            form: 0,
            axes: [Signed::default(); 3],
        }
    }
}

impl CPoint {
    /// Quantises `p` against the stream's compression point `cp` exactly as `writeCompressedPoint` does.
    pub fn pack(p: [f32; 3], cp: [f32; 3]) -> CPoint {
        let mut w = BitWriter::new();
        w.set_compression_point(cp);
        w.write_compressed_point(p, POINT_SCALE);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let mut out = CPoint::default();
        // The writer's own output always parses.
        let _ = read_point(&mut r, &mut out);
        out
    }

    /// The client's position (`readCompressedPoint`): per axis `cp + (F32)i * scale`, through `torque_net::library`'s
    /// `decompress_axis`.
    pub fn value(&self, cp: [f32; 3]) -> [f32; 3] {
        match self {
            CPoint::Absolute(p) => [p[0].get(), p[1].get(), p[2].get()],
            CPoint::Relative { axes, .. } => {
                let decompress = library::get().decompress_axis;
                let mut out = [0.0f32; 3];
                for ((slot, a), base) in out.iter_mut().zip(axes).zip(cp) {
                    *slot = decompress(a.value(), POINT_SCALE, base);
                }
                out
            }
        }
    }

    pub fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        let mut form = match self {
            CPoint::Relative { form, .. } => (*form).min(2),
            CPoint::Absolute(_) => 3,
        };
        c.int("cp.form", &mut form, 2)?;
        if form == 3 {
            let mut p = match self {
                CPoint::Absolute(p) => *p,
                CPoint::Relative { .. } => [F32::default(); 3],
            };
            c.floats("cp.absolute", &mut p)?;
            *self = CPoint::Absolute(p);
        } else {
            let mut axes = match self {
                CPoint::Relative { axes, .. } => *axes,
                CPoint::Absolute(_) => [Signed::default(); 3],
            };
            let bits = POINT_BITS.get(form as usize).copied().unwrap_or(16) - 1;
            for a in axes.iter_mut() {
                c.flag("cp.sign", &mut a.negative)?;
                c.int("cp.magnitude", &mut a.magnitude, bits)?;
                a.magnitude = mask_bits(a.magnitude, bits);
            }
            *self = CPoint::Relative { form, axes };
        }
        Ok(())
    }
}

fn read_point(r: &mut BitReader, out: &mut CPoint) -> Result<(), torque_net::NetError> {
    let form = r.read_bits(2)?;
    if form == 3 {
        *out = CPoint::Absolute([F32(r.read_u32()?), F32(r.read_u32()?), F32(r.read_u32()?)]);
        return Ok(());
    }
    let bits = POINT_BITS.get(form as usize).copied().unwrap_or(16) - 1;
    let mut axes = [Signed::default(); 3];
    for a in axes.iter_mut() {
        a.negative = r.read_flag()?;
        a.magnitude = r.read_bits(bits)?;
    }
    *out = CPoint::Relative { form, axes };
    Ok(())
}

/// The velocity block's body: the direction as a normal vector of `bits` (phi `bits + 1`, theta `bits`) and the 13-bit speed in
/// 1/32 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Velocity {
    pub phi: u32,
    pub theta: u32,
    pub speed: u32,
}

/// The length the velocity block tests and scales, `mVelocity.len()`: `mSqrt(x*x + y*y + z*z)` in `F32`.
pub fn speed_length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

impl Velocity {
    /// `flag(len > 0.02)` and the block, `len` being [`speed_length`]: `None` when the flag is 0 (a NaN length too).
    pub fn pack(v: [f32; 3], bits: u8) -> Option<Velocity> {
        Velocity::pack_with_length(v, speed_length(v), bits)
    }

    /// [`Velocity::pack`] with the length `len` of `v` measured by the caller (a host whose float evaluation of the length
    /// differs passes its own).
    pub fn pack_with_length(v: [f32; 3], len: f32, bits: u8) -> Option<Velocity> {
        if len.is_nan() || f64::from(len) <= 0.02 {
            return None;
        }
        let inv = 1.0 / len;
        let n = v.map(|c| c * inv);
        let mut w = BitWriter::new();
        w.write_normal_vector(n, bits);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let phi = r.read_bits(bits.saturating_add(1)).unwrap_or(0);
        let theta = r.read_bits(bits).unwrap_or(0);
        let mut sp = (f64::from(len) * 32.0) as f32;
        if sp > 8191.0 {
            sp = 8191.0;
        }
        Some(Velocity {
            phi,
            theta,
            speed: mask_bits((library::get().to_int)(sp) as u32, 13),
        })
    }

    /// The client's velocity: the normal vector times `int / 32.0f`.
    pub fn value(&self, bits: u8) -> [f32; 3] {
        let mut w = BitWriter::new();
        w.write_bits(self.phi, bits.saturating_add(1));
        w.write_bits(self.theta, bits);
        let bytes = w.into_bytes();
        let n = BitReader::new(&bytes)
            .read_normal_vector(bits)
            .unwrap_or([0.0; 3]);
        let s = (self.speed & 0x1fff) as f32 / 32.0;
        n.map(|c| c * s)
    }

    pub fn layout<C: Codec>(&mut self, c: &mut C, bits: u8) -> R {
        c.int("vel.phi", &mut self.phi, bits.saturating_add(1))?;
        c.int("vel.theta", &mut self.theta, bits)?;
        c.int("vel.speed", &mut self.speed, 13)
    }
}

/// `writeSignedFloat(x, bits)` as its integer: `(S32)(((x + 1) * 0.5) * (2^bits - 1))` in the low bits.
pub fn signed_float_bits(x: f32, bits: u8) -> u32 {
    let mut w = BitWriter::new();
    w.write_signed_float(x, bits);
    let bytes = w.into_bytes();
    BitReader::new(&bytes).read_bits(bits).unwrap_or(0)
}

/// `readSignedFloat(bits)` of an integer.
pub fn signed_float_value(v: u32, bits: u8) -> f32 {
    let mut w = BitWriter::new();
    w.write_bits(v, bits);
    let bytes = w.into_bytes();
    BitReader::new(&bytes)
        .read_signed_float(bits)
        .unwrap_or(0.0)
}

/// A normal vector as `writeNormalVector(v, bits)` sends it: phi in `bits + 1` bits, theta in `bits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Normal {
    pub phi: u32,
    pub theta: u32,
}

impl Normal {
    /// Quantises the direction `v` exactly as `writeNormalVector` does.
    pub fn pack(v: [f32; 3], bits: u8) -> Normal {
        let mut w = BitWriter::new();
        w.write_normal_vector(v, bits);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let phi = r.read_bits(bits.saturating_add(1)).unwrap_or(0);
        let theta = r.read_bits(bits).unwrap_or(0);
        Normal { phi, theta }
    }

    /// The client's direction (`readNormalVector`).
    pub fn value(&self, bits: u8) -> [f32; 3] {
        let mut w = BitWriter::new();
        w.write_bits(self.phi, bits.saturating_add(1));
        w.write_bits(self.theta, bits);
        let bytes = w.into_bytes();
        BitReader::new(&bytes)
            .read_normal_vector(bits)
            .unwrap_or([0.0; 3])
    }

    pub fn layout<C: Codec>(&mut self, c: &mut C, bits: u8) -> R {
        c.int("normal.phi", &mut self.phi, bits.saturating_add(1))?;
        c.int("normal.theta", &mut self.theta, bits)
    }
}

/// `writeFloat(f, bits)` as its integer: `(S32)(f * (2^bits - 1))` in the low bits (no clamp).
pub fn float_bits(f: f32, bits: u8) -> u32 {
    mask_bits((library::get().write_float)(f, bits), bits)
}

/// `readFloat(bits)` of an integer: `int / F32(2^bits - 1)`.
pub fn float_value(v: u32, bits: u8) -> f32 {
    let top = mask_bits(u32::MAX, bits);
    if top == 0 {
        return 0.0;
    }
    mask_bits(v, bits) as f32 / top as f32
}

/// A move as `Move::pack` sends it, with Torque's six trigger keys: per angle `flag(p != 0)` and `int(16)` when set, three
/// `int(6)` axes, `flag(freeLook)`, the trigger flags. The record keeps the wire values (an axis is not clamped here; a set angle
/// flag with a zero angle stays as it came).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PackedMove {
    /// Yaw, pitch and roll: `None` when the flag is 0.
    pub angles: [Option<u32>; 3],
    pub axes: [u32; 3],
    pub free_look: bool,
    pub triggers: [bool; MAX_TRIGGER_KEYS],
}

impl PackedMove {
    /// What `Move::pack` writes for `m`.
    pub fn from_move(m: &Move) -> PackedMove {
        let angle = |p: u32| {
            let p = p & 0xffff;
            (p != 0).then_some(p)
        };
        PackedMove {
            angles: [angle(m.pyaw), angle(m.ppitch), angle(m.proll)],
            axes: [m.px, m.py, m.pz].map(|a| mask_bits(a, 6)),
            free_look: m.free_look,
            triggers: m.triggers,
        }
    }

    /// The move `Move::unpack` makes of it (axes clamped, floats derived).
    pub fn to_move(&self) -> Move {
        Move::from_raw(
            self.axes,
            self.angles.map(|a| a.unwrap_or(0)),
            self.free_look,
            self.triggers,
        )
    }

    /// The move of a view: `move_angles` 0..3 (16-bit values), `move_axes` 0..3 (0..=32, 16 is zero), `move_free_look`,
    /// `move_triggers` 0..6.
    pub fn from_view(view: &dyn ObjectView) -> PackedMove {
        let m = Move {
            pyaw: word(view, "move_angles", 0),
            ppitch: word(view, "move_angles", 1),
            proll: word(view, "move_angles", 2),
            px: word(view, "move_axes", 0),
            py: word(view, "move_axes", 1),
            pz: word(view, "move_axes", 2),
            free_look: boolean(view, "move_free_look", 0),
            triggers: std::array::from_fn(|i| boolean(view, "move_triggers", i)),
            ..Move::default()
        };
        PackedMove::from_move(&m)
    }

    pub fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        for a in self.angles.iter_mut() {
            opt(c, "move.angle?", a, |c, v| c.int("move.angle", v, 16))?;
        }
        for a in self.axes.iter_mut() {
            c.int("move.axis", a, 6)?;
        }
        c.flag("move.freeLook", &mut self.free_look)?;
        for t in self.triggers.iter_mut() {
            c.flag("move.trigger", t)?;
        }
        Ok(())
    }
}
