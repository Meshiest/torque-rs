//! Typed native arguments and results: how a Rust value is read from the text a script passes and handed back as the
//! result a native returns.
//!
//! The rules are the ones the runtime's hand-written natives already follow, so a native written over these traits
//! behaves exactly like one that reads `argv` itself:
//!
//! - an integer is `dAtoi` ([`atoi`]: leading white space, a sign, decimal digits, saturating; `"0x10"` is 0);
//! - a float is `dAtof` ([`atof`]) rounded to a 32-bit float, the way the interpreter reads one;
//! - a truth value is `dAtob` ([`atob`](crate::fields::atob)): `true` in any case, or a number that is not 0 once
//!   it is a float;
//! - text is the bytes up to the first NUL, as a C string.
//!
//! Reading never fails for a number: text that is not a number reads as 0, as in TGE. Only text asked for as `&str`
//! can be refused, when it is not UTF-8.

use std::borrow::Cow;
use std::fmt;

use crate::text::{atof, atoi, to_f32, to_f64, until_nul};
use crate::vm::Native;

/// Why an argument could not be read as the type asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertError {
    /// The text is not valid UTF-8 (a script's text is bytes; only `&str` needs more than that).
    NotUtf8,
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::NotUtf8 => f.write_str("the argument is not valid UTF-8 text"),
        }
    }
}

impl std::error::Error for ConvertError {}

/// A value a native reads from one argument's text.
///
/// The lifetime lets `&str` borrow the argument. Numbers and truth values never fail; see the [module](self) for the
/// rules each type follows.
///
/// `ObjId` is `u32`, so it reads as the number `atoi` gives: finding an object by name or path is the registry's
/// `find`, which needs the VM.
pub trait FromScript<'a>: Sized {
    /// Reads the argument `arg` (its bytes up to a NUL).
    fn from_script(arg: &'a [u8]) -> Result<Self, ConvertError>;
}

/// A value a native returns, as the [`Native`] result the interpreter turns into the script's value.
///
/// A float is widened exactly, an unsigned integer keeps its 32 bits (a script prints it signed, as `%d`).
pub trait IntoScript {
    /// The result for the interpreter.
    fn into_script(self) -> Native;
}

impl FromScript<'_> for i32 {
    /// `dAtoi`.
    fn from_script(arg: &[u8]) -> Result<Self, ConvertError> {
        Ok(atoi(arg))
    }
}

impl FromScript<'_> for u32 {
    /// `dAtoi`, its bits read as unsigned (`"-1"` is `u32::MAX`), as an unsigned field is read.
    fn from_script(arg: &[u8]) -> Result<Self, ConvertError> {
        Ok(atoi(arg) as u32)
    }
}

impl FromScript<'_> for f32 {
    /// `dAtof`, rounded to a float (`(F32)atof(s)`).
    fn from_script(arg: &[u8]) -> Result<Self, ConvertError> {
        Ok(to_f32(atof(arg)))
    }
}

impl FromScript<'_> for bool {
    /// `dAtob`.
    fn from_script(arg: &[u8]) -> Result<Self, ConvertError> {
        Ok(crate::fields::atob(arg))
    }
}

impl<'a> FromScript<'a> for &'a str {
    /// The text up to the first NUL; [`ConvertError::NotUtf8`] when it is not UTF-8.
    fn from_script(arg: &'a [u8]) -> Result<Self, ConvertError> {
        std::str::from_utf8(until_nul(arg)).map_err(|_| ConvertError::NotUtf8)
    }
}

impl FromScript<'_> for String {
    /// The text up to the first NUL, with each byte sequence that is not UTF-8 replaced by U+FFFD.
    fn from_script(arg: &[u8]) -> Result<Self, ConvertError> {
        Ok(String::from_utf8_lossy(until_nul(arg)).into_owned())
    }
}

impl<'a> FromScript<'a> for Cow<'a, str> {
    /// As [`String`], borrowing when the text is UTF-8.
    fn from_script(arg: &'a [u8]) -> Result<Self, ConvertError> {
        Ok(String::from_utf8_lossy(until_nul(arg)))
    }
}

impl IntoScript for () {
    fn into_script(self) -> Native {
        Native::Void
    }
}

impl IntoScript for i32 {
    fn into_script(self) -> Native {
        Native::Int(self)
    }
}

impl IntoScript for u32 {
    /// The 32 bits as a signed integer, which is how a script prints a word it holds unsigned.
    fn into_script(self) -> Native {
        Native::Int(self as i32)
    }
}

impl IntoScript for f32 {
    fn into_script(self) -> Native {
        Native::Float(to_f64(self))
    }
}

impl IntoScript for bool {
    fn into_script(self) -> Native {
        Native::Bool(self)
    }
}

impl IntoScript for &str {
    fn into_script(self) -> Native {
        Native::Str(self.as_bytes().to_vec())
    }
}

impl IntoScript for String {
    fn into_script(self) -> Native {
        Native::Str(self.into_bytes())
    }
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
