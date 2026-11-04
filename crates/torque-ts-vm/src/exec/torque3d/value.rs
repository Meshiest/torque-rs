//! The typed value of the Torque3D executor's stacks and registers: an integer, a float or a string, each
//! convertible to the others when an operation asks for another type.
//!
//! The conversions follow Torque3D's MIT-licensed `ConsoleValue` (`console.h`, `console.cpp`), Copyright (c) 2012
//! GarageGames, LLC; the code here is this crate's own.

use crate::text::{atof, atoi, format_g9, to_f32, to_f64, until_nul};

/// A value on the executor's stacks or in a register.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Value {
    Int(i64),
    Float(f64),
    /// Text without a NUL (the empty string is the empty value).
    Str(Vec<u8>),
}

impl Default for Value {
    fn default() -> Value {
        Value::Str(Vec::new())
    }
}

impl Value {
    /// A string value: the text up to its first NUL.
    #[inline]
    pub(super) fn text(bytes: &[u8]) -> Value {
        Value::Str(until_nul(bytes).to_vec())
    }

    /// [`text`](Self::text) of an owned buffer, without a copy.
    #[inline]
    pub(super) fn from_text(mut bytes: Vec<u8>) -> Value {
        if let Some(end) = bytes.iter().position(|&b| b == 0) {
            bytes.truncate(end);
        }
        Value::Str(bytes)
    }

    #[inline]
    pub(super) fn from_bool(b: bool) -> Value {
        Value::Int(i64::from(b))
    }

    /// As an integer: a float truncates toward zero (saturating), a string is read with `atoi`.
    #[inline]
    pub(super) fn int(&self) -> i64 {
        match self {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i64,
            Value::Str(s) => i64::from(atoi(s)),
        }
    }

    /// As a float: a string is read as a 32-bit float (`strtof`), widened.
    #[inline]
    pub(super) fn float(&self) -> f64 {
        match self {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            Value::Str(s) if s.is_empty() => 0.0,
            Value::Str(s) => to_f64(to_f32(atof(s))),
        }
    }

    /// As a truth value: a number is true when it is not 0; a string when it is not empty and not one of
    /// `0`, `0.0`, `0.0f`, `null` and `false` (any case).
    #[inline]
    pub(super) fn truth(&self) -> bool {
        match self {
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty() && !is_false_word(s),
        }
    }

    /// As text: an integer in decimal, a float with nine significant digits (`%.9g`).
    #[inline]
    pub(super) fn string(&self) -> std::borrow::Cow<'_, [u8]> {
        match self {
            Value::Str(s) => std::borrow::Cow::Borrowed(s.as_slice()),
            Value::Int(i) => std::borrow::Cow::Owned(i.to_string().into_bytes()),
            Value::Float(f) => std::borrow::Cow::Owned(format_float(*f)),
        }
    }

    /// The text, taken out of the value.
    #[inline]
    pub(super) fn into_string(self) -> Vec<u8> {
        match self {
            Value::Str(s) => s,
            other => other.string().into_owned(),
        }
    }

    /// The bytes the value's text takes (for the work budget): a number's text is short.
    #[inline]
    pub(super) fn text_len(&self) -> usize {
        match self {
            Value::Str(s) => s.len(),
            _ => 0,
        }
    }
}

/// `%.9g`, the text a float value converts to.
pub(super) fn format_float(f: f64) -> Vec<u8> {
    format_g9(f)
}

/// The strings `dAtob` reads as false besides the empty one.
fn is_false_word(s: &[u8]) -> bool {
    [&b"0"[..], b"0.0", b"0.0f", b"null", b"false"]
        .iter()
        .any(|w| s.eq_ignore_ascii_case(w))
}

/// The bytes `isspace` accepts (the separators of `foreach$`).
pub(super) fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_follow_the_value_type() {
        assert_eq!(Value::Float(1.5).int(), 1);
        assert_eq!(Value::Float(-1.5).int(), -1);
        assert_eq!(Value::text(b"12abc").int(), 12);
        assert_eq!(Value::text(b"").float(), 0.0);
        assert_eq!(Value::text(b"1.1").float(), f64::from(1.1f32));
        assert_eq!(&*Value::Float(f64::from(1.1f32)).string(), b"1.10000002");
        assert_eq!(&*Value::Float(0.5).string(), b"0.5");
        assert_eq!(&*Value::Int(-3).string(), b"-3");
        assert!(!Value::text(b"FALSE").truth());
        assert!(!Value::text(b"0.0").truth());
        assert!(Value::text(b"0.00").truth());
        assert!(Value::text(b"abc").truth());
        assert!(!Value::Float(0.0).truth());
    }
}
