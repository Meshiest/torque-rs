//! Primitives and records: [`Writer`] appends them, [`Reader`] and [`Records`] read them back, refusing anything that
//! does not follow the format.

use std::fmt;

use crate::MAX_RECORD;

/// Bytes that do not follow the format: what was expected where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed: {}", self.0)
    }
}

impl std::error::Error for Malformed {}

/// Appends primitives and records to a buffer.
#[derive(Clone, Debug, Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// An empty buffer.
    pub fn new() -> Writer {
        Writer::default()
    }

    /// The bytes written.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// How many bytes are written.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing is written.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Drops what is written from byte `len` on (to undo a record that turned out too long).
    pub fn truncate(&mut self, len: usize) {
        self.bytes.truncate(len);
    }

    /// Empties the buffer.
    pub fn clear(&mut self) {
        self.bytes.clear();
    }

    /// The buffer, given away.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Bytes already in the format (an encoded value).
    pub fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// A `u8`.
    pub fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    /// A `u16`.
    pub fn u16(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    /// A `u32`.
    pub fn u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    /// A `u64`.
    pub fn u64(&mut self, v: u64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    /// An `f64`.
    pub fn f64(&mut self, v: f64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    /// A `bool`.
    pub fn bool(&mut self, v: bool) {
        self.bytes.push(u8::from(v));
    }

    /// `bytes`: a `u32` length and the bytes. Lengths past `u32::MAX` never occur (records are capped far below).
    pub fn bytes_field(&mut self, v: &[u8]) {
        self.u32(u32::try_from(v.len()).unwrap_or(u32::MAX));
        self.bytes.extend_from_slice(v);
    }

    /// A `text`.
    pub fn text(&mut self, v: &str) {
        self.bytes_field(v.as_bytes());
    }

    /// Starts a record of `tier` and `kind`: its body follows, and [`Writer::end`] with the returned mark fills in the
    /// length.
    pub fn begin(&mut self, tier: u8, kind: u8) -> usize {
        self.u8(tier);
        self.u8(kind);
        let mark = self.bytes.len();
        self.u32(0);
        mark
    }

    /// Ends the record [`Writer::begin`] started at `mark`: `Err` (and the record dropped) when its body is longer than
    /// [`MAX_RECORD`].
    pub fn end(&mut self, mark: usize) -> Result<(), Malformed> {
        let len = self.bytes.len() - mark - 4;
        if len > MAX_RECORD {
            self.bytes.truncate(mark - 2);
            return Err(Malformed("a record longer than MAX_RECORD"));
        }
        let len = (len as u32).to_le_bytes();
        self.bytes[mark..mark + 4].copy_from_slice(&len);
        Ok(())
    }
}

/// Reads primitives from bytes.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// A reader of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes }
    }

    /// The bytes not read yet.
    pub fn rest(&self) -> &'a [u8] {
        self.bytes
    }

    /// Whether everything is read.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// `Ok` when everything is read, as a body must be.
    pub fn finish(&self) -> Result<(), Malformed> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(Malformed("bytes left over after a body"))
        }
    }

    /// The next `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        if n > self.bytes.len() {
            return Err(Malformed("a field past the end"));
        }
        let (taken, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Malformed> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// A `u8`.
    pub fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.array::<1>()?[0])
    }

    /// A `u16`.
    pub fn u16(&mut self) -> Result<u16, Malformed> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    /// A `u32`.
    pub fn u32(&mut self) -> Result<u32, Malformed> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// A `u64`.
    pub fn u64(&mut self) -> Result<u64, Malformed> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// An `f64`, any bits.
    pub fn f64(&mut self) -> Result<f64, Malformed> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    /// A finite `f64`.
    pub fn finite(&mut self) -> Result<f64, Malformed> {
        let v = self.f64()?;
        if v.is_finite() {
            Ok(v)
        } else {
            Err(Malformed("a number that is not finite"))
        }
    }

    /// A `bool`: 0 or 1.
    pub fn bool(&mut self) -> Result<bool, Malformed> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Malformed("a bool that is neither 0 nor 1")),
        }
    }

    /// `bytes`.
    pub fn bytes_field(&mut self) -> Result<&'a [u8], Malformed> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    /// A `text`.
    pub fn text(&mut self) -> Result<&'a str, Malformed> {
        std::str::from_utf8(self.bytes_field()?).map_err(|_| Malformed("a text that is not UTF-8"))
    }
}

/// One record: its tier and kind ids and its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record<'a> {
    /// The tier's id.
    pub tier: u8,
    /// The kind within the tier.
    pub kind: u8,
    /// The body.
    pub body: &'a [u8],
}

impl<'a> Record<'a> {
    /// A reader of the body.
    pub fn reader(&self) -> Reader<'a> {
        Reader::new(self.body)
    }
}

/// The records of a flush or a batch, read one by one: at most `max_records`, in at most `max_bytes`.
#[derive(Clone, Debug)]
pub struct Records<'a> {
    reader: Reader<'a>,
    left: usize,
}

impl<'a> Records<'a> {
    /// The records of `bytes`, or `Err` when there are more bytes than `max_bytes`.
    pub fn new(
        bytes: &'a [u8],
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Records<'a>, Malformed> {
        if bytes.len() > max_bytes {
            return Err(Malformed("more bytes than the cap"));
        }
        Ok(Records {
            reader: Reader::new(bytes),
            left: max_records,
        })
    }

    /// Every record, or the first fault.
    pub fn collect_all(self) -> Result<Vec<Record<'a>>, Malformed> {
        self.collect()
    }
}

impl<'a> Iterator for Records<'a> {
    type Item = Result<Record<'a>, Malformed>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.reader.is_empty() {
            return None;
        }
        if self.left == 0 {
            self.reader = Reader::new(&[]);
            return Some(Err(Malformed("more records than the cap")));
        }
        self.left -= 1;
        let read = (|| {
            let tier = self.reader.u8()?;
            let kind = self.reader.u8()?;
            let len = self.reader.u32()? as usize;
            if len > MAX_RECORD {
                return Err(Malformed("a record longer than MAX_RECORD"));
            }
            let body = self.reader.take(len)?;
            Ok(Record { tier, kind, body })
        })();
        if read.is_err() {
            self.reader = Reader::new(&[]);
        }
        Some(read)
    }
}
