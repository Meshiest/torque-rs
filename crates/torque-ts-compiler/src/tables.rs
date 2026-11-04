//! The compiler's string, float and identifier tables (`CompilerStringTable::add`, `CompilerFloatTable::add`
//! and `CompilerIdentTable::add` via `STEtoU32`). The order of entries and the deduplication rules decide the
//! bytes of the DSO. A linear walk over a linked list finds the first matching entry; here a map per lookup
//! rule gives the same answer (the first entry wins).
//! Follows Torque3D's `compiler.h` and `compiler.cpp` (MIT); the credits are in the crate documentation.

use std::collections::HashMap;

/// A tagged string takes at least this many bytes: the tag byte, five digits and the terminator, which
/// `OP_TAG_TO_STR` writes over the text when it runs.
const MIN_TAG_BYTES: usize = 7;

/// The lookups of one tag flag: text -> the offset of the first entry with exactly this text, and text in
/// lower case -> the first entry equal to it ignoring ASCII case (`_stricmp`).
#[derive(Debug, Clone, Default)]
struct Lookups {
    exact: HashMap<Vec<u8>, u32>,
    folded: HashMap<Vec<u8>, u32>,
}

/// The bytes of a string table and the lookups `add` makes.
#[derive(Debug, Clone, Default)]
pub struct StringTable {
    bytes: Vec<u8>,
    plain: Lookups,
    tagged: Lookups,
}

/// `text` in lower case, borrowed when it has no upper-case letter.
fn folded(text: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if text.iter().any(u8::is_ascii_uppercase) {
        std::borrow::Cow::Owned(text.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

impl StringTable {
    /// `add(str, caseSens, tag)`: the offset of the first entry with the same tag flag whose text is equal
    /// (byte for byte, or ignoring ASCII case), else a new entry at the end. The text is a C string: the
    /// caller has already cut it at its first NUL. A new entry takes the text, a NUL and, for a tagged
    /// string shorter than six bytes, zeros up to seven bytes.
    pub fn add(&mut self, text: &[u8], case_sensitive: bool, tag: bool) -> u32 {
        let lookups = if tag {
            &mut self.tagged
        } else {
            &mut self.plain
        };
        let found = if case_sensitive {
            lookups.exact.get(text)
        } else {
            lookups.folded.get(folded(text).as_ref())
        };
        if let Some(&start) = found {
            return start;
        }
        let start = u32::try_from(self.bytes.len()).unwrap_or(u32::MAX);
        self.bytes.extend_from_slice(text);
        self.bytes.push(0);
        if tag {
            let padding = MIN_TAG_BYTES.saturating_sub(text.len() + 1);
            self.bytes.resize(self.bytes.len() + padding, 0);
        }
        lookups
            .folded
            .entry(folded(text).into_owned())
            .or_insert(start);
        lookups.exact.entry(text.to_vec()).or_insert(start);
        start
    }

    /// The table's bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// A float table: `add` returns the index of the first entry `==` to the value (so `0.0` and `-0.0` share
/// one entry, the first spelling's, and a NaN never matches and always takes a new one).
#[derive(Debug, Clone, Default)]
pub struct FloatTable {
    values: Vec<f64>,
    /// Bit pattern -> index of the first entry with this exact pattern; `==` is checked through it for
    /// every value but zero, which is looked up by `zero`.
    by_bits: HashMap<u64, u32>,
    zero: Option<u32>,
}

impl FloatTable {
    pub fn add(&mut self, value: f64) -> u32 {
        if value == 0.0 {
            if let Some(i) = self.zero {
                return i;
            }
        } else if !value.is_nan()
            && let Some(&i) = self.by_bits.get(&value.to_bits())
        {
            return i;
        }
        let index = u32::try_from(self.values.len()).unwrap_or(u32::MAX);
        self.values.push(value);
        if value == 0.0 {
            self.zero = Some(index);
        } else if !value.is_nan() {
            self.by_bits.entry(value.to_bits()).or_insert(index);
        }
        index
    }

    pub fn values(&self) -> &[f64] {
        &self.values
    }
}

/// The identifier table of a DSO: for each global-string offset, the code positions that receive it when the
/// file is loaded. The list keeps every new offset at its head and every further position of an offset right
/// after the head entry, so the file lists offsets by last first appearance and an offset's positions as the
/// first one, then the others newest first.
#[derive(Debug, Clone, Default)]
pub struct IdentTable {
    /// Offsets in order of first appearance, each with its positions in appearance order.
    order: Vec<(u32, Vec<u32>)>,
    index: HashMap<u32, usize>,
}

impl IdentTable {
    pub fn add(&mut self, offset: u32, ip: u32) {
        match self.index.get(&offset) {
            Some(&i) => {
                if let Some((_, positions)) = self.order.get_mut(i) {
                    positions.push(ip);
                }
            }
            None => {
                self.index.insert(offset, self.order.len());
                self.order.push((offset, vec![ip]));
            }
        }
    }

    /// The entries in file order.
    pub fn entries(&self) -> Vec<torque_ts_bytecode::IdentEntry> {
        self.order
            .iter()
            .rev()
            .map(|(offset, positions)| {
                let mut ordered = Vec::with_capacity(positions.len());
                ordered.extend(positions.first().copied());
                ordered.extend(positions.iter().skip(1).rev().copied());
                torque_ts_bytecode::IdentEntry {
                    string_index: *offset,
                    positions: ordered,
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "tables_tests.rs"]
mod tests;
