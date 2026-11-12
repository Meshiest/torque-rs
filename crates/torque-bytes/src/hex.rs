//! Lower-case hexadecimal text of bytes (a digest, an id) and back.

use std::fmt::Write;

/// The lower-case hexadecimal text of `bytes`, two digits a byte.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// The `N` bytes of `text`, which must be exactly `2 * N` lower-case hexadecimal digits (`0-9`, `a-f`); `None` for any other
/// text, of any length.
#[must_use]
pub fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.as_bytes();
    if digits.len() != N * 2 {
        return None;
    }
    let digit = |d: u8| match d {
        b'0'..=b'9' => Some(d - b'0'),
        b'a'..=b'f' => Some(d - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (slot, pair) in out.iter_mut().zip(digits.as_chunks::<2>().0) {
        *slot = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(out)
}

#[cfg(test)]
#[path = "hex_tests.rs"]
mod tests;
