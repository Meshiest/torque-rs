use crate::error::DsoError;

/// A bounds-checked little-endian reader. Every read reports what it was reading when it fails.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], DsoError> {
        if self.remaining() < len {
            return Err(DsoError::UnexpectedEof {
                what,
                offset: self.pos,
            });
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8, DsoError> {
        Ok(self.take(1, what)?[0])
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32, DsoError> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn f64(&mut self, what: &'static str) -> Result<f64, DsoError> {
        let b = self.take(8, what)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(f64::from_le_bytes(a))
    }

    pub(crate) fn bytes(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], DsoError> {
        self.take(len, what)
    }

    /// A one-byte length, then that many bytes.
    pub(crate) fn short_string(&mut self, what: &'static str) -> Result<Vec<u8>, DsoError> {
        let len = self.u8(what)?;
        Ok(self.take(usize::from(len), what)?.to_vec())
    }

    /// Refuses a declared count of items (each at least `min_item_bytes` long) that cannot fit in the
    /// remaining input. Call this before allocating for `count` items.
    pub(crate) fn check_count(
        &self,
        count: u64,
        min_item_bytes: u64,
        what: &'static str,
    ) -> Result<(), DsoError> {
        if count.saturating_mul(min_item_bytes) > self.remaining() as u64 {
            return Err(DsoError::TooLarge {
                what,
                declared: count,
                remaining: self.remaining(),
            });
        }
        Ok(())
    }

    /// A code word: one byte, or `0xFF` followed by a little-endian `u32`.
    pub(crate) fn op(&mut self, what: &'static str) -> Result<u32, DsoError> {
        match self.u8(what)? {
            0xFF => self.u32(what),
            b => Ok(u32::from(b)),
        }
    }
}
