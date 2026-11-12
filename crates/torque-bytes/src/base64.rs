//! Standard base64 (RFC 4648 section 4, the `+/` alphabet): [`encode`] with padding, and [`decode`] for text that may not be
//! trusted.

use std::fmt;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Why [`decode`] refused its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The decoded bytes would be more than the caller's limit (`bytes` is what they would be; nothing was decoded).
    TooLarge { bytes: u64, cap: u64 },
    /// A length no encoder writes (one digit more than a whole number of groups of four).
    Length,
    /// A byte that is not in the alphabet (padding anywhere but the end included).
    Character,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::TooLarge { bytes, cap } => {
                write!(f, "{bytes} decoded bytes are above the limit of {cap}")
            }
            DecodeError::Length => f.write_str("a base64 length"),
            DecodeError::Character => f.write_str("a base64 character"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Standard base64 of `bytes`, with padding.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The bytes of standard base64 `text` (one or two `=` of padding, or none), at most `max` of them. The size the text decodes to
/// is checked against `max` before anything is allocated or decoded, so text of any length costs only a scan.
///
/// # Errors
/// [`DecodeError::TooLarge`] when the bytes would pass `max`, [`DecodeError::Length`] for a length no encoder writes and
/// [`DecodeError::Character`] for any byte outside the alphabet.
pub fn decode(text: &[u8], max: usize) -> Result<Vec<u8>, DecodeError> {
    let text = text
        .strip_suffix(b"==")
        .or_else(|| text.strip_suffix(b"="))
        .unwrap_or(text);
    let out_len = text.len() / 4 * 3 + (text.len() % 4).saturating_sub(1);
    if out_len > max {
        return Err(DecodeError::TooLarge {
            bytes: out_len as u64,
            cap: max as u64,
        });
    }
    if text.len() % 4 == 1 {
        return Err(DecodeError::Length);
    }
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(out_len);
    for chunk in text.chunks(4) {
        let mut word = 0u32;
        for (k, &c) in chunk.iter().enumerate() {
            let v = value(c).ok_or(DecodeError::Character)?;
            word |= v << (18 - 6 * k);
        }
        let bytes = word.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "base64_tests.rs"]
mod tests;
