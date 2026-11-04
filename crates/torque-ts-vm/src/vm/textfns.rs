//! The text natives a host may run without a script calling them: the same functions the natives `getWord`, `getWords`,
//! `getField`, `getSubStr`, `strstr`, `strpos`, `trim`, `mFloor`, `VectorAdd` and `fileBase` are made of, so a host that does a
//! script's work natively gets the script's text byte for byte. Each takes its arguments as a native gets them (cut at a NUL)
//! and answers what the native answers, as the text the interpreter would hold (an integer result as its `%d` text).

use crate::text::{atof, find, to_f32, to_f64, until_nul};

use super::math::ftoi;
use super::words::{FIELDS, WORDS, unit, unit_count, units};

/// `getWord(text, index)`.
pub fn get_word(text: &[u8], index: i32) -> Vec<u8> {
    unit(until_nul(text), index, WORDS)
}

/// Every word of `text` at once: `words(text)[i]` is `getWord(text, i)` for every `i` it holds, and `getWord` of any
/// later index is "" (each separator ends a word, two in a row make an empty one, the last runs to the end).
pub fn words(text: &[u8]) -> Vec<&[u8]> {
    until_nul(text).split(|b| WORDS.contains(b)).collect()
}

/// `getWords(text, start, end)`.
pub fn get_words(text: &[u8], start: i32, end: i32) -> Vec<u8> {
    units(until_nul(text), start, end, WORDS)
}

/// `getField(text, index)`.
pub fn get_field(text: &[u8], index: i32) -> Vec<u8> {
    unit(until_nul(text), index, FIELDS)
}

/// `getFieldCount(text)`.
pub fn get_field_count(text: &[u8]) -> i32 {
    unit_count(until_nul(text), FIELDS)
}

/// `getSubStr(text, start, count)` for a start and count that are not negative (a negative one is the native's error, which
/// prints; a host does not take this path for it).
pub fn sub_str(text: &[u8], start: usize, count: usize) -> &[u8] {
    let text = until_nul(text);
    text.get(start..)
        .map(|rest| rest.get(..count).unwrap_or(rest))
        .unwrap_or_default()
}

/// `strstr(hay, needle)`: where the needle starts, -1 when it is not there.
pub fn strstr(hay: &[u8], needle: &[u8]) -> i32 {
    find(until_nul(hay), until_nul(needle), 0, false)
        .map_or(-1, |at| i32::try_from(at).unwrap_or(-1))
}

/// `strpos(hay, needle)` (offset 0): where the needle starts (0 for an empty one), -1 when it is not there.
pub fn strpos(hay: &[u8], needle: &[u8]) -> i32 {
    find(until_nul(hay), until_nul(needle), 0, false)
        .map_or(-1, |at| i32::try_from(at).unwrap_or(-1))
}

/// `trim(text)`: spaces, tabs and line feeds off both ends.
pub fn trim(text: &[u8]) -> &[u8] {
    let s = until_nul(text);
    let is_trim = |b: &u8| matches!(b, b' ' | b'\n' | b'\t');
    let start = s.iter().take_while(|b| is_trim(b)).count();
    let end = s.iter().rposition(|b| !is_trim(b)).map_or(start, |i| i + 1);
    s.get(start..end.max(start)).unwrap_or_default()
}

/// `mFloor(text)`: `(F32)atof`, floored, converted to an integer as `mFloor` converts.
pub fn m_floor(text: &[u8]) -> i32 {
    ftoi(to_f64(to_f32(atof(until_nul(text)))).floor())
}

/// `VectorAdd(a, b)`.
pub fn vector_add(a: &[u8], b: &[u8]) -> Vec<u8> {
    super::vectors::add_text(until_nul(a), until_nul(b))
}

/// `fileBase(text)`.
pub fn file_base(text: &[u8]) -> &[u8] {
    super::file_natives::file_base(until_nul(text))
}
