//! The bounded cursor every reader of this crate reads through: Torque's `Stream` reads (little-endian words, a `bool` as one
//! byte, a string as a length byte and its bytes) with every read checked against the bytes that remain.

use crate::{MAX_ITEMS, MapError, record, reserved};

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn left(&self) -> usize {
        self.bytes.len() - self.pos
    }

    /// Moves back to an earlier position (the plain-surface retry of `dif`).
    pub(crate) fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.bytes.len());
    }

    pub(crate) fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], MapError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or(MapError::Short { what, at: self.pos })?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], MapError> {
        let b = self.take(N, what)?;
        let mut out = [0u8; N];
        out.copy_from_slice(b);
        Ok(out)
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8, MapError> {
        Ok(self.array::<1>(what)?[0])
    }

    pub(crate) fn bool(&mut self, what: &'static str) -> Result<bool, MapError> {
        Ok(self.u8(what)? != 0)
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16, MapError> {
        Ok(u16::from_le_bytes(self.array(what)?))
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32, MapError> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }

    pub(crate) fn i32(&mut self, what: &'static str) -> Result<i32, MapError> {
        Ok(i32::from_le_bytes(self.array(what)?))
    }

    pub(crate) fn f32(&mut self, what: &'static str) -> Result<f32, MapError> {
        Ok(f32::from_le_bytes(self.array(what)?))
    }

    pub(crate) fn f32s<const N: usize>(
        &mut self,
        what: &'static str,
    ) -> Result<[f32; N], MapError> {
        let mut out = [0f32; N];
        for v in &mut out {
            *v = self.f32(what)?;
        }
        Ok(out)
    }

    /// Four colour bytes (`Stream::read(ColorF*)` reads a `ColorI`).
    pub(crate) fn rgba(&mut self, what: &'static str) -> Result<[u8; 4], MapError> {
        self.array(what)
    }

    /// `Stream::readString`: a length byte and that many bytes.
    pub(crate) fn string(&mut self, what: &'static str) -> Result<String, MapError> {
        let n = self.u8(what)? as usize;
        let b = self.take(n, what)?;
        record(b.len());
        Ok(crate::latin1(b))
    }

    /// Torque3D's `Stream::read(String*)`: a length byte, or 255 and a 16-bit length, and the bytes.
    pub(crate) fn long_string(&mut self, what: &'static str) -> Result<String, MapError> {
        let n = match self.u8(what)? {
            255 => self.u16(what)? as usize,
            n => n as usize,
        };
        let b = self.take(n, what)?;
        record(b.len());
        Ok(crate::latin1(b))
    }

    /// A count of items of at least `min_item` bytes each: at most `cap` and at most what the remaining bytes could hold.
    pub(crate) fn count(
        &mut self,
        what: &'static str,
        min_item: usize,
        cap: usize,
    ) -> Result<usize, MapError> {
        let value = self.u32(what)?;
        self.fits(what, value as u64, min_item, cap)
    }

    /// `value` items of at least `min_item` bytes against `cap` and the remaining bytes.
    pub(crate) fn fits(
        &self,
        what: &'static str,
        value: u64,
        min_item: usize,
        cap: usize,
    ) -> Result<usize, MapError> {
        let room = (self.left() / min_item.max(1)) as u64;
        let cap = (cap as u64).min(room);
        if value > cap {
            Err(MapError::Count { what, value, cap })
        } else {
            Ok(value as usize)
        }
    }

    /// A counted vector: a 32-bit count, then that many items each read by `item` (at least `min_item` bytes each).
    pub(crate) fn vec<T>(
        &mut self,
        what: &'static str,
        min_item: usize,
        mut item: impl FnMut(&mut Reader<'a>) -> Result<T, MapError>,
    ) -> Result<Vec<T>, MapError> {
        let n = self.count(what, min_item, MAX_ITEMS)?;
        self.items(n, min_item, &mut item)
    }

    /// `n` items (already checked) each read by `item`.
    pub(crate) fn items<T>(
        &mut self,
        n: usize,
        min_item: usize,
        item: &mut impl FnMut(&mut Reader<'a>) -> Result<T, MapError>,
    ) -> Result<Vec<T>, MapError> {
        let mut out = reserved(n, self.left(), min_item);
        for _ in 0..n {
            out.push(item(self)?);
        }
        Ok(out)
    }

    /// A counted byte vector, copied.
    pub(crate) fn byte_vec(&mut self, what: &'static str) -> Result<Vec<u8>, MapError> {
        let n = self.count(what, 1, MAX_ITEMS)?;
        self.bytes_copy(n, what)
    }

    /// `n` bytes copied out.
    pub(crate) fn bytes_copy(&mut self, n: usize, what: &'static str) -> Result<Vec<u8>, MapError> {
        let b = self.take(n, what)?;
        let mut out = reserved(b.len(), b.len(), 1);
        out.extend_from_slice(b);
        Ok(out)
    }

    pub(crate) fn u16_vec(&mut self, what: &'static str) -> Result<Vec<u16>, MapError> {
        self.vec(what, 2, |r| r.u16(what))
    }

    pub(crate) fn u32_vec(&mut self, what: &'static str) -> Result<Vec<u32>, MapError> {
        self.vec(what, 4, |r| r.u32(what))
    }

    pub(crate) fn bad(&self, what: &'static str, value: u64) -> MapError {
        MapError::Bad {
            what,
            value,
            at: self.pos,
        }
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
