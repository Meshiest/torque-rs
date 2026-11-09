//! The Torque `BitStream`: a writer and a reader over bytes, bits
//! LSB first and nothing byte-aligned, so a field's low bit comes first and a 32-bit word is its four little-endian bytes.
//!
//! **Fields:** `int(n)` ([`BitWriter::write_bits`]: the low `n` bits of the value, no
//! range check), `flag`, raw `u8`/`u32`/`f32` (8 and 32 bits), `ranged_u32(v, lo, hi)` (`int(bitlen(hi - lo), v - lo)`,
//! **no clamp**: a value out of range wraps in the low bits), `ranged_f32` (clamp, `t = f32((v - min) / (max -
//! min))`, `int(bits, floor(f32(t * f32(2^bits - 1))))`), `signed` (a flag then the magnitude), the unit and signed floats,
//! `ColorI` (four bytes), the normal vector (a signed float of `bits + 1` and one of `bits`), the affine transform (position and
//! the quaternion's x, y, z as raw floats, then the flag `w < 0`: 193 bits) and the compressed point against the stream's
//! compression point. Strings are in [`crate::strcoder`]; the 256-byte buffer they share within one packet lives here.
//!
//! **Floats.** The float fields (the unit, signed and ranged floats, the colour, the normal vector and the compressed point) are
//! computed through the [`crate::library`] table: by default the TGE 1.4 source's expressions, with the types the source gives them;
//! only the low bits of the integer go out.
//!
//! **Hostile input.** The reader returns [`NetError::Overrun`] where TGE's returns zeros, never panics at any bit offset, and a
//! failed read leaves the position where it was. The writer has a hard capacity: a write past it is dropped and the writer says
//! so ([`BitWriter::is_overflowed`]); nothing panics.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `core/stream/bitStream.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `core/stream/bitStream.h` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.),
//! `math/mathIO.h` and `core/color.h` (Copyright (c) 2012 GarageGames, LLC each); the float fields follow TGE 1.4's
//! `core/bitStream.cc` (Copyright (C) GarageGames.com, Inc.; the Torque engine is MIT-licensed as Torque3D). The code here
//! is this crate's own.

use std::sync::Arc;

use crate::NetError;
use crate::cap::{self, NET_MAX_STREAM_BITS, NET_STRING_BUFFER};
use crate::library;

/// The bytes a writer reserves when made: a packet's worth, whatever its limit.
const PREALLOCATED_BYTES: usize = 2_048;

/// The number of bits of `n` (0 for 0): `ranged`'s width for a range of `n`.
pub fn bit_length(n: u32) -> u8 {
    (32 - n.leading_zeros()) as u8
}

/// The 16 bits in the widths of the compressed point's three sizes (the sign flag included), and the bound each takes (in units
/// of `scale`): below 2^15, 2^17 and 2^19; beyond that the absolute position goes out as three raw floats.
const POINT_BITS: [u8; 3] = [16, 18, 20];
const POINT_LIMITS: [f32; 3] = [32_768.0, 131_072.0, 524_288.0];

/// A colour as four bytes: what `ColorI` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorI {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl ColorI {
    /// The bytes of a `ColorF` (`ColorF::operator ColorI`, through [`crate::library::MathLibrary::color_byte`]).
    pub fn from_f32(c: [f32; 4]) -> ColorI {
        let byte = library::get().color_byte;
        ColorI {
            r: byte(c[0]),
            g: byte(c[1]),
            b: byte(c[2]),
            a: byte(c[3]),
        }
    }
}

/// A transform as Torque sends it: the position and the unit quaternion `(x, y, z, w)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub position: [f32; 3],
    pub quaternion: [f32; 4],
}

/// A [`BitWriter`]'s position and side state (see [`BitWriter::mark`]).
#[derive(Debug, Clone)]
pub struct Mark {
    bits: usize,
    overflowed: bool,
    string_buffer: Option<Arc<[u8; NET_STRING_BUFFER]>>,
    compression_point: [f32; 3],
}

/// A growing bit buffer, bits LSB first, the unused bits of the last byte zero.
#[derive(Debug, Clone)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bits: usize,
    limit: usize,
    overflowed: bool,
    /// Shared with the [`Mark`]s taken of the writer: a write to it copies it only while one still holds it, so a mark costs a
    /// reference count, not 256 bytes.
    string_buffer: Option<Arc<[u8; NET_STRING_BUFFER]>>,
    compression_point: [f32; 3],
}

impl Default for BitWriter {
    fn default() -> BitWriter {
        BitWriter::new()
    }
}

impl BitWriter {
    /// A writer that takes up to [`NET_MAX_STREAM_BITS`] bits.
    pub fn new() -> BitWriter {
        BitWriter::with_limit(NET_MAX_STREAM_BITS)
    }

    /// A writer that takes up to `limit_bits` bits (a packet: 1,023 bytes). A write that does not fit is dropped whole and the
    /// writer is marked overflowed.
    pub fn with_limit(limit_bits: usize) -> BitWriter {
        BitWriter {
            // A packet's worth up front; a stream with a larger limit grows as it is written.
            bytes: Vec::with_capacity((limit_bits / 8).min(PREALLOCATED_BYTES)),
            bits: 0,
            limit: limit_bits.min(NET_MAX_STREAM_BITS),
            overflowed: false,
            string_buffer: None,
            compression_point: [0.0; 3],
        }
    }

    /// How many bits were written.
    pub fn bits(&self) -> usize {
        self.bits
    }

    /// How many bits more fit.
    pub fn remaining_bits(&self) -> usize {
        self.limit.saturating_sub(self.bits)
    }

    /// Keeps the last `bits` of the limit for a later write: until [`BitWriter::release`] the writer takes `bits` fewer (a section
    /// keeps the room of the sections after it). Returns what it held, to pass to `release`.
    pub fn hold_back(&mut self, bits: usize) -> usize {
        let held = bits.min(self.limit);
        self.limit -= held;
        held
    }

    /// Gives back what [`BitWriter::hold_back`] held.
    pub fn release(&mut self, held: usize) {
        self.limit = self.limit.saturating_add(held).min(NET_MAX_STREAM_BITS);
    }

    /// Whether a write was dropped because it did not fit.
    pub fn is_overflowed(&self) -> bool {
        self.overflowed
    }

    /// The bytes so far (`(bits + 7) >> 3` of them).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The stream's bytes: `(bits + 7) >> 3` of them.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Gives the stream a cleared 256-byte string buffer, as a game packet has (`GameConnection::writePacket`).
    pub fn enable_string_buffer(&mut self) {
        self.string_buffer = Some(Arc::new([0; NET_STRING_BUFFER]));
    }

    /// The string buffer to write, when the stream has one (copied first if a [`Mark`] still shares it).
    pub fn string_buffer(&mut self) -> Option<&mut [u8; NET_STRING_BUFFER]> {
        self.string_buffer.as_mut().map(Arc::make_mut)
    }

    /// The string buffer to read, when the stream has one; never copies.
    pub fn string_buffer_ref(&self) -> Option<&[u8; NET_STRING_BUFFER]> {
        self.string_buffer.as_deref()
    }

    /// The point compressed points are written against (the zero point at the start of a packet).
    pub fn set_compression_point(&mut self, point: [f32; 3]) {
        self.compression_point = point;
    }

    pub fn compression_point(&self) -> [f32; 3] {
        self.compression_point
    }

    /// The writer's state, to [`BitWriter::rewind`] to (the ghost layer rolls back a record that does not fit: bits, the overflow
    /// flag, the string buffer and the compression point are restored).
    pub fn mark(&self) -> Mark {
        Mark {
            bits: self.bits,
            overflowed: self.overflowed,
            string_buffer: self.string_buffer.clone(),
            compression_point: self.compression_point,
        }
    }

    /// Returns to a [`BitWriter::mark`] taken from this writer earlier (a mark past the current end is ignored).
    pub fn rewind(&mut self, mark: Mark) {
        if mark.bits > self.bits {
            return;
        }
        self.bits = mark.bits;
        self.bytes.truncate(mark.bits.div_ceil(8));
        let used = mark.bits & 7;
        if used != 0
            && let Some(last) = self.bytes.last_mut()
        {
            *last &= (1u8 << used) - 1;
        }
        self.overflowed = mark.overflowed;
        self.string_buffer = mark.string_buffer;
        self.compression_point = mark.compression_point;
    }

    /// `writeInt(n)`: the `n` low bits of `value` (at most 32), the lowest first; higher bits of `value` are dropped.
    pub fn write_bits(&mut self, value: u32, n: u8) {
        let n = usize::from(n.min(32));
        if n == 0 {
            return;
        }
        if self.bits.saturating_add(n) > self.limit {
            self.overflowed = true;
            return;
        }
        // The value shifted to its place in the last, part-filled byte: at most 7 + 32 bits, five bytes.
        let used = self.bits & 7;
        let mut word = (u64::from(value) & ((1u64 << n) - 1)) << used;
        let mut left = used + n;
        if used != 0 {
            if let Some(last) = self.bytes.last_mut() {
                *last |= word as u8;
            }
            word >>= 8;
            left = left.saturating_sub(8);
        }
        while left > 0 {
            self.bytes.push(word as u8);
            word >>= 8;
            left = left.saturating_sub(8);
        }
        self.bits += n;
    }

    pub fn write_flag(&mut self, flag: bool) {
        self.write_bits(u32::from(flag), 1);
    }

    /// `write(1)`: a byte, 8 bits.
    pub fn write_u8(&mut self, value: u8) {
        self.write_bits(u32::from(value), 8);
    }

    /// `write(4)`: a 32-bit word.
    pub fn write_u32(&mut self, value: u32) {
        self.write_bits(value, 32);
    }

    /// `write(4)` of a float: its 32 bits.
    pub fn write_f32(&mut self, value: f32) {
        self.write_bits(value.to_bits(), 32);
    }

    /// Bytes, 8 bits each.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        // The bytes that fit are written; the first that does not marks the stream overflowed and is dropped, with the rest.
        let fit = bytes.len().min(self.remaining_bits() / 8);
        let used = self.bits & 7;
        if used == 0 {
            self.bytes.extend_from_slice(&bytes[..fit]);
        } else {
            self.bytes.reserve(fit);
            for &b in &bytes[..fit] {
                if let Some(last) = self.bytes.last_mut() {
                    *last |= b << used;
                }
                self.bytes.push(b >> (8 - used));
            }
        }
        self.bits += fit * 8;
        if fit < bytes.len() {
            self.overflowed = true;
        }
    }

    /// `writeRangedU32(value, lo, hi)`: `int(bit_length(hi - lo), value - lo)`. No clamp: a value outside the range
    /// wraps in the low bits.
    pub fn write_ranged_u32(&mut self, value: u32, lo: u32, hi: u32) {
        self.write_bits(value.wrapping_sub(lo), bit_length(hi.wrapping_sub(lo)));
    }

    /// `writeRangedF32(value, min, max, bits)`: the value clamped to the range, scaled to `2^bits - 1` and floored
    /// ([`crate::library::MathLibrary::write_ranged_f32`]); the low `bits` bits go out.
    pub fn write_ranged_f32(&mut self, value: f32, min: f32, max: f32, bits: u8) {
        let bits = bits.min(32);
        self.write_bits(
            (library::get().write_ranged_f32)(value, min, max, bits),
            bits,
        );
    }

    /// `writeSignedInt(value, bits)`: a flag (negative) and the magnitude in `bits - 1` bits.
    pub fn write_signed_int(&mut self, value: i32, bits: u8) {
        self.write_flag(value < 0);
        self.write_bits(value.unsigned_abs(), bits.saturating_sub(1));
    }

    /// `writeFloat(f, bits)`: `int(bits, (S32)(f * (2^bits - 1)))`, no clamp ([`crate::library::MathLibrary::write_float`]).
    pub fn write_float(&mut self, value: f32, bits: u8) {
        let bits = bits.min(32);
        self.write_bits((library::get().write_float)(value, bits), bits);
    }

    /// `writeSignedFloat(f, bits)`: `int(bits, (S32)((f + 1) * 0.5 * (2^bits - 1)))` (so `0.0` is `127.5` -> 127 at 8 bits, and
    /// `1.0` is 255; [`crate::library::MathLibrary::write_signed_float`]).
    pub fn write_signed_float(&mut self, value: f32, bits: u8) {
        let bits = bits.clamp(1, 31);
        self.write_bits((library::get().write_signed_float)(value, bits), bits);
    }

    /// `ColorI`: four bytes.
    pub fn write_color_i(&mut self, color: ColorI) {
        self.write_u8(color.r);
        self.write_u8(color.g);
        self.write_u8(color.b);
        self.write_u8(color.a);
    }

    /// `writeNormalVector(v, bits)`: `phi = atan2(x, y) / pi` as a signed float of `bits + 1` bits, then
    /// `theta = atan2(z, sqrt(x*x + y*y)) / (pi / 2)` as one of `bits` ([`crate::library::MathLibrary::normal_angles`]).
    /// The default direction `(0, 0, 1)` is phi 255, theta 255.
    pub fn write_normal_vector(&mut self, v: [f32; 3], bits: u8) {
        let (phi, theta) = (library::get().normal_angles)(v);
        self.write_signed_float(phi, bits.saturating_add(1));
        self.write_signed_float(theta, bits);
    }

    /// `writeAffineTransform`: the position as three raw floats, the quaternion's x, y, z as three raw floats, then the
    /// flag `w < 0`.
    pub fn write_affine_transform(&mut self, t: &Affine) {
        for p in t.position {
            self.write_f32(p);
        }
        for q in &t.quaternion[..3] {
            self.write_f32(*q);
        }
        self.write_flag(t.quaternion[3] < 0.0);
    }

    /// `writeCompressedPoint(p, scale)`: the offset from the compression point, its length `r` in units of `scale` and the
    /// axes as integers ([`crate::library::MathLibrary::compress_point`]). The form is 0 for `r < 32768`, 1 for `< 131072`, 2 for
    /// `< 524288` (strict; a NaN fails every test) and 3 beyond: `int(2) = class`, then per axis x, y, z the integer as a sign flag
    /// and the magnitude in 15, 17 or 19 bits (widths 16, 18, 20 with the flag); form 3 is `int(2) = 3` and the ABSOLUTE position as
    /// three raw floats.
    pub fn write_compressed_point(&mut self, p: [f32; 3], scale: f32) {
        let (r, axes) = (library::get().compress_point)(p, self.compression_point, scale);
        let class = POINT_LIMITS.iter().position(|&limit| r < limit);
        match class {
            Some(class) => {
                self.write_bits(class as u32, 2);
                let bits = POINT_BITS.get(class).copied().unwrap_or(16);
                for v in axes {
                    self.write_signed_int(v, bits);
                }
            }
            None => {
                self.write_bits(3, 2);
                for c in p {
                    self.write_f32(c);
                }
            }
        }
    }
}

/// A reader over bytes, bits LSB first.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
    string_buffer: Option<Box<[u8; NET_STRING_BUFFER]>>,
    compression_point: [f32; 3],
}

impl<'a> BitReader<'a> {
    /// A reader over all the bits of `bytes`.
    pub fn new(bytes: &'a [u8]) -> BitReader<'a> {
        BitReader::with_bit_len(bytes, bytes.len().saturating_mul(8))
    }

    /// A reader over the first `bit_len` bits of `bytes` (at most all of them).
    pub fn with_bit_len(bytes: &'a [u8], bit_len: usize) -> BitReader<'a> {
        BitReader {
            bytes,
            at: 0,
            end: bit_len.min(bytes.len().saturating_mul(8)),
            string_buffer: None,
            compression_point: [0.0; 3],
        }
    }

    /// The bit the next read starts at.
    pub fn position(&self) -> usize {
        self.at
    }

    /// How many bits are left to read.
    pub fn bits_left(&self) -> usize {
        self.end.saturating_sub(self.at)
    }

    /// Gives the stream a cleared 256-byte string buffer, as a game packet has.
    pub fn enable_string_buffer(&mut self) {
        self.string_buffer = Some(Box::new([0; NET_STRING_BUFFER]));
    }

    /// The string buffer, when the stream has one.
    pub fn string_buffer(&mut self) -> Option<&mut [u8; NET_STRING_BUFFER]> {
        self.string_buffer.as_deref_mut()
    }

    pub fn set_compression_point(&mut self, point: [f32; 3]) {
        self.compression_point = point;
    }

    pub fn compression_point(&self) -> [f32; 3] {
        self.compression_point
    }

    /// `readInt(n)`: `n` bits (at most 32), the lowest first. Fails with [`NetError::Overrun`] (position unchanged) when fewer
    /// are left.
    pub fn read_bits(&mut self, n: u8) -> Result<u32, NetError> {
        let n = usize::from(n.min(32));
        if n > self.bits_left() {
            return Err(NetError::Overrun);
        }
        let (first, used) = (self.at >> 3, self.at & 7);
        let span = (used + n).div_ceil(8);
        let mut word = 0u64;
        for k in 0..span {
            let byte = *self.bytes.get(first + k).ok_or(NetError::Overrun)?;
            word |= u64::from(byte) << (8 * k);
        }
        self.at += n;
        Ok(((word >> used) & ((1u64 << n) - 1)) as u32)
    }

    pub fn read_flag(&mut self) -> Result<bool, NetError> {
        self.read_bits(1).map(|b| b != 0)
    }

    pub fn read_u8(&mut self) -> Result<u8, NetError> {
        self.read_bits(8).map(|v| v as u8)
    }

    pub fn read_u32(&mut self) -> Result<u32, NetError> {
        self.read_bits(32)
    }

    pub fn read_f32(&mut self) -> Result<f32, NetError> {
        self.read_bits(32).map(f32::from_bits)
    }

    /// `len` bytes, 8 bits each; the length is checked against the bits left before anything is reserved.
    pub fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>, NetError> {
        let mut out = cap::vec_for("bytes", len as u64, usize::MAX, self.bits_left(), 8)?;
        let (first, used) = (self.at >> 3, self.at & 7);
        if len == 0 {
            return Ok(out);
        }
        if used == 0 {
            out.extend_from_slice(
                self.bytes
                    .get(first..first + len)
                    .ok_or(NetError::Overrun)?,
            );
        } else {
            // Each byte is the high bits of one source byte and the low bits of the next.
            let source = self
                .bytes
                .get(first..=first + len)
                .ok_or(NetError::Overrun)?;
            out.extend(
                source
                    .windows(2)
                    .map(|w| (w[0] >> used) | (w[1] << (8 - used))),
            );
        }
        self.at += len * 8;
        Ok(out)
    }

    /// `readRangedU32(lo, hi)`: `lo + int(bit_length(hi - lo))`, wrapping, no clamp.
    pub fn read_ranged_u32(&mut self, lo: u32, hi: u32) -> Result<u32, NetError> {
        let v = self.read_bits(bit_length(hi.wrapping_sub(lo)))?;
        Ok(v.wrapping_add(lo))
    }

    /// `readRangedF32(min, max, bits)`: `min + (max - min) * int(bits) / (2^bits - 1)`.
    pub fn read_ranged_f32(&mut self, min: f32, max: f32, bits: u8) -> Result<f32, NetError> {
        let bits = bits.min(32);
        let raw = self.read_bits(bits)?;
        if bits == 0 {
            return Ok(min);
        }
        let top = ((1u64 << bits) - 1) as f32;
        Ok(min + (raw as f32 / top) * (max - min))
    }

    /// `readSignedInt(bits)`.
    pub fn read_signed_int(&mut self, bits: u8) -> Result<i32, NetError> {
        let negative = self.read_flag()?;
        let magnitude = self.read_bits(bits.saturating_sub(1))?;
        let magnitude = i64::from(magnitude);
        let v = if negative { -magnitude } else { magnitude };
        Ok(v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
    }

    /// `readFloat(bits)`: `int(bits) / (2^bits - 1)`.
    pub fn read_float(&mut self, bits: u8) -> Result<f32, NetError> {
        let bits = bits.min(32);
        let raw = self.read_bits(bits)?;
        if bits == 0 {
            return Ok(0.0);
        }
        Ok(raw as f32 / ((1u64 << bits) - 1) as f32)
    }

    /// `readSignedFloat(bits)`: `2 * raw / (2^bits - 1) - 1` ([`crate::library::MathLibrary::read_signed_float`]).
    pub fn read_signed_float(&mut self, bits: u8) -> Result<f32, NetError> {
        let bits = bits.clamp(1, 31);
        let raw = self.read_bits(bits)?;
        Ok((library::get().read_signed_float)(raw, bits))
    }

    pub fn read_color_i(&mut self) -> Result<ColorI, NetError> {
        Ok(ColorI {
            r: self.read_u8()?,
            g: self.read_u8()?,
            b: self.read_u8()?,
            a: self.read_u8()?,
        })
    }

    /// `readNormalVector(bits)`: `phi = sf(bits + 1) * pi`, `theta = sf(bits) * pi / 2`, then `x = sin(phi) * cos(theta)`,
    /// `y = cos(phi) * cos(theta)`, `z = sin(theta)` ([`crate::library::MathLibrary::normal_vector`]).
    pub fn read_normal_vector(&mut self, bits: u8) -> Result<[f32; 3], NetError> {
        let phi = self.read_signed_float(bits.saturating_add(1))?;
        let theta = self.read_signed_float(bits)?;
        Ok((library::get().normal_vector)(phi, theta))
    }

    /// `readAffineTransform`: `w = sqrt(1 - min(x^2 + y^2 + z^2, 1))`, negated when the flag is set.
    pub fn read_affine_transform(&mut self) -> Result<Affine, NetError> {
        let position = [self.read_f32()?, self.read_f32()?, self.read_f32()?];
        let (x, y, z) = (self.read_f32()?, self.read_f32()?, self.read_f32()?);
        let negative = self.read_flag()?;
        let sum = x * x + y * y + z * z;
        let mut w = (1.0 - sum.min(1.0)).max(0.0).sqrt();
        if negative {
            w = -w;
        }
        Ok(Affine {
            position,
            quaternion: [x, y, z, w],
        })
    }

    /// `readCompressedPoint(scale)`: form 3 is three raw floats (absolute, no compression point added); otherwise per axis
    /// `cp + i * scale` with `i` the signed int of 16, 18 or 20 bits ([`crate::library::MathLibrary::decompress_axis`]).
    pub fn read_compressed_point(&mut self, scale: f32) -> Result<[f32; 3], NetError> {
        let class = self.read_bits(2)? as usize;
        let Some(&bits) = POINT_BITS.get(class) else {
            return Ok([self.read_f32()?, self.read_f32()?, self.read_f32()?]);
        };
        let mut out = [0.0f32; 3];
        for (slot, base) in out.iter_mut().zip(self.compression_point) {
            let i = self.read_signed_int(bits)?;
            *slot = (library::get().decompress_axis)(i, scale, base);
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "bits_tests.rs"]
mod tests;
