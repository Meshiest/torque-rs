//! Bounds-checked little-endian reads over a byte slice: every read is a value or an [`Error`], never a panic, and every count is
//! checked against the bytes left before anything is allocated for it.

use std::fmt;
use std::fs::File;
use std::path::Path;

use torque_resource::plain::{self, PlainError};

/// Why a file did not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A read ran past the end of the file (or of a part of it).
    Truncated {
        /// What was being read.
        what: &'static str,
    },
    /// The file is not what it says it is (a bad magic number, version or value).
    Invalid(String),
    /// The file asks for more than the reader's caps allow.
    TooLarge {
        /// What was too large.
        what: &'static str,
        /// How much was asked for.
        asked: u64,
        /// The cap.
        cap: u64,
    },
    /// A file the format needs beside this one is missing or did not read.
    Missing(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { what } => write!(f, "truncated while reading {what}"),
            Error::Invalid(why) => write!(f, "{why}"),
            Error::TooLarge { what, asked, cap } => {
                write!(f, "{what} is {asked}, over the cap of {cap}")
            }
            Error::Missing(what) => write!(f, "missing {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// A reader's result.
pub type Result<T> = std::result::Result<T, Error>;

/// An [`Error::Invalid`] from a message.
pub(crate) fn invalid<T>(why: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(why.into()))
}

/// A [`PlainError`] for the file at `path` as this crate's [`Error`].
fn plain_error(path: &Path, e: PlainError) -> Error {
    match e {
        PlainError::Io(e) => Error::Missing(format!("{}: {e}", path.display())),
        PlainError::NotPlain => Error::Invalid(format!("{} is not a plain file", path.display())),
        PlainError::TooLarge { len, cap } => Error::TooLarge {
            what: "a file's size",
            asked: len,
            cap,
        },
    }
}

/// Opens the plain file at `path` without following a link ([`torque_resource::plain::open_plain`]).
pub(crate) fn open_plain(path: &Path) -> Result<File> {
    plain::open_plain(path)
        .map(|(file, _)| file)
        .map_err(|e| plain_error(path, e))
}

/// The whole plain file at `path`, at most `max` bytes, and at most `listed` bytes when a listing gave its length
/// ([`torque_resource::plain::read_plain`]): the length is taken from the open file, no link is followed.
pub fn read_plain_file(path: &Path, max: u64, listed: Option<u64>) -> Result<Vec<u8>> {
    plain::read_plain(path, max, listed).map_err(|e| plain_error(path, e))
}

/// Refuses `asked` over `cap`.
pub(crate) fn cap(what: &'static str, asked: u64, cap: u64) -> Result<()> {
    if asked > cap {
        return Err(Error::TooLarge { what, asked, cap });
    }
    Ok(())
}

/// `len` bytes of `b` at `off`.
pub(crate) fn slice<'a>(
    b: &'a [u8],
    off: usize,
    len: usize,
    what: &'static str,
) -> Result<&'a [u8]> {
    off.checked_add(len)
        .and_then(|end| b.get(off..end))
        .ok_or(Error::Truncated { what })
}

/// `N` bytes of `b` at `off`.
pub(crate) fn array<const N: usize>(b: &[u8], off: usize, what: &'static str) -> Result<[u8; N]> {
    let s = slice(b, off, N, what)?;
    let mut out = [0u8; N];
    out.copy_from_slice(s);
    Ok(out)
}

pub(crate) fn u8_at(b: &[u8], off: usize, what: &'static str) -> Result<u8> {
    b.get(off).copied().ok_or(Error::Truncated { what })
}

pub(crate) fn u16_at(b: &[u8], off: usize, what: &'static str) -> Result<u16> {
    array::<2>(b, off, what).map(u16::from_le_bytes)
}

pub(crate) fn i16_at(b: &[u8], off: usize, what: &'static str) -> Result<i16> {
    array::<2>(b, off, what).map(i16::from_le_bytes)
}

pub(crate) fn u32_at(b: &[u8], off: usize, what: &'static str) -> Result<u32> {
    array::<4>(b, off, what).map(u32::from_le_bytes)
}

pub(crate) fn i32_at(b: &[u8], off: usize, what: &'static str) -> Result<i32> {
    array::<4>(b, off, what).map(i32::from_le_bytes)
}

pub(crate) fn f32_at(b: &[u8], off: usize, what: &'static str) -> Result<f32> {
    array::<4>(b, off, what).map(f32::from_le_bytes)
}

/// Three floats at `off`.
pub(crate) fn vec3_at(b: &[u8], off: usize, what: &'static str) -> Result<[f32; 3]> {
    Ok([
        f32_at(b, off, what)?,
        f32_at(b, off + 4, what)?,
        f32_at(b, off + 8, what)?,
    ])
}

/// A count read from the file as a usize: refused when negative, or when `count` items of `stride` bytes cannot fit in the
/// `room` bytes there are (so nothing is ever allocated for a count the file cannot hold).
pub(crate) fn count(n: i32, stride: usize, room: usize, what: &'static str) -> Result<usize> {
    let n =
        usize::try_from(n).map_err(|_| Error::Invalid(format!("{what}: negative count {n}")))?;
    if n.checked_mul(stride.max(1))
        .is_none_or(|bytes| bytes > room)
    {
        return Err(Error::Truncated { what });
    }
    Ok(n)
}

/// `base + rel` as an offset into a file (a structure's offset relative to another, as the model formats store them).
pub(crate) fn rel(base: usize, rel: i32, what: &'static str) -> Result<usize> {
    let r = i64::from(rel);
    let at = base as i64 + r;
    usize::try_from(at).map_err(|_| Error::Invalid(format!("{what}: offset {rel} before the file")))
}

/// The NUL-terminated string at `off` (at most `max` bytes; lossy UTF-8).
pub(crate) fn cstr_at(b: &[u8], off: usize, max: usize, what: &'static str) -> Result<String> {
    let rest = b.get(off..).ok_or(Error::Truncated { what })?;
    let rest = &rest[..rest.len().min(max)];
    let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// The string in a fixed field of `len` bytes (up to its first NUL).
pub(crate) fn fixed_str(b: &[u8], off: usize, len: usize, what: &'static str) -> Result<String> {
    let s = slice(b, off, len, what)?;
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

/// A sequential reader over a slice.
#[derive(Debug, Clone)]
pub(crate) struct Cursor<'a> {
    pub b: &'a [u8],
    pub pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(b: &'a [u8]) -> Cursor<'a> {
        Cursor { b, pos: 0 }
    }

    pub fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        let s = slice(self.b, self.pos, n, what)?;
        self.pos += n;
        Ok(s)
    }

    pub fn u16(&mut self, what: &'static str) -> Result<u16> {
        let v = u16_at(self.b, self.pos, what)?;
        self.pos += 2;
        Ok(v)
    }

    pub fn u32(&mut self, what: &'static str) -> Result<u32> {
        let v = u32_at(self.b, self.pos, what)?;
        self.pos += 4;
        Ok(v)
    }

    /// A NUL-terminated string (at most `max` bytes before the NUL).
    pub fn cstr(&mut self, max: usize, what: &'static str) -> Result<&'a [u8]> {
        let rest = self.b.get(self.pos..).ok_or(Error::Truncated { what })?;
        let end = rest
            .iter()
            .take(max + 1)
            .position(|&c| c == 0)
            .ok_or(Error::Truncated { what })?;
        self.pos += end + 1;
        Ok(&rest[..end])
    }
}

/// A half float's value.
pub(crate) fn f16(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f32::from(bits & 0x3ff);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 => {
            if frac == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// A path as the archives and the files' tables store it: `\` as `/`, lower case, no leading `/` or `./`, `.` parts dropped.
/// None for a path with `..`, a NUL, a drive or an empty part. Device names (`con`, `nul`) are kept: the result keys in-memory
/// tables only and is never joined onto a folder of the disk.
pub fn normalize_path(p: &str) -> Option<String> {
    let lower = p.replace('\\', "/").to_ascii_lowercase();
    let mut parts = Vec::new();
    for part in lower.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            _ if part.contains(['\0', ':']) => return None,
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_refuse_past_the_end() {
        let b = [1u8, 2, 3];
        assert_eq!(u16_at(&b, 1, "x"), Ok(0x0302));
        assert!(u16_at(&b, 2, "x").is_err());
        assert!(u32_at(&b, usize::MAX, "x").is_err());
        assert!(slice(&b, usize::MAX, 2, "x").is_err());
    }

    #[test]
    fn counts_must_fit() {
        assert_eq!(count(3, 4, 12, "x"), Ok(3));
        assert!(count(4, 4, 12, "x").is_err());
        assert!(count(-1, 4, 12, "x").is_err());
        assert!(count(i32::MAX, usize::MAX, 12, "x").is_err());
    }

    #[test]
    fn halves() {
        assert_eq!(f16(0x3c00), 1.0);
        assert_eq!(f16(0xc000), -2.0);
        assert_eq!(f16(0), 0.0);
    }

    #[test]
    fn paths() {
        assert_eq!(
            normalize_path("Materials\\Concrete/./A.vmt").as_deref(),
            Some("materials/concrete/a.vmt")
        );
        assert_eq!(normalize_path("a/../b"), None);
        assert_eq!(normalize_path("c:/x"), None);
        assert_eq!(normalize_path(""), None);
    }
}
