//! A `.dml` material list (a sky's, or a precipitation's). `MaterialList::read` reads a first byte: 1 starts the
//! binary form (a count and length-prefixed names), anything else is the text form, one name a line up to the first empty
//! line or the end. Each name is cut after its last `/` or `\` and a line's `\r` is dropped (the stream's `readLine`). The text
//! form follows Torque's own reading; the binary one is Torque3D's MIT-licensed `MaterialList::read`
//! (`Engine/source/materials/materialList.cpp`, `Copyright (c) 2012 GarageGames, LLC` and `Copyright (C) 2015 Faust Logic,
//! Inc.`).

use crate::read::Reader;
use crate::{MAX_DML_BYTES, MapError, latin1, size_ok};

/// The most names of one list (a sky has 6 to 9).
pub const MAX_NAMES: usize = 1_024;
/// The longest line `readLine` keeps (its buffer of 1,024 bytes holds 1,023 and the terminator); the rest of a longer line is
/// read as the next line.
pub const MAX_LINE: usize = 1_023;

fn cut(name: &[u8]) -> &[u8] {
    let start = name
        .iter()
        .rposition(|&b| b == b'/' || b == b'\\')
        .map_or(0, |i| i + 1);
    &name[start..]
}

/// Reads a `.dml`.
pub fn parse(bytes: &[u8]) -> Result<Vec<String>, MapError> {
    size_ok(bytes.len(), MAX_DML_BYTES)?;
    if bytes.first() == Some(&1) {
        let mut r = Reader::new(&bytes[1..]);
        return r.vec("material names", 1, |r| {
            let name = r.string("material name")?;
            Ok(latin1(cut(name.as_bytes())))
        });
    }
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let end = rest.iter().position(|&b| b == b'\n');
        let (mut line, next) = match end {
            Some(e) => (&rest[..e], &rest[e + 1..]),
            None => (rest, &rest[rest.len()..]),
        };
        let mut next = next;
        if line.len() > MAX_LINE {
            next = &rest[MAX_LINE..];
            line = &rest[..MAX_LINE];
        }
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || line[0] == 0 {
            break;
        }
        if out.len() == MAX_NAMES {
            return Err(MapError::Count {
                what: "material names",
                value: MAX_NAMES as u64 + 1,
                cap: MAX_NAMES as u64,
            });
        }
        let name = cut(line);
        crate::record(name.len());
        out.push(latin1(name));
        rest = next;
    }
    Ok(out)
}

#[cfg(test)]
#[path = "dml_tests.rs"]
mod tests;
