//! The word, field and record functions (`getWord`, `getFields`, `setRecord`, ...) and `nextToken`: TGE 1.4's
//! `getUnit` family. Their index is unsigned, as TGE's: a negative one is far past any text (so `getWord` gives "",
//! `removeWord` the text unchanged, and `setWord` pads past the string-size cap, which fails the run). `getWords`
//! with the end before the start gives "".
//!
//! Every loop here walks the text once (each round moves past one unit or stops), and the text is the
//! call's argument, whose bytes the call already charged; the one place that grows beyond the arguments is
//! the padding of `setWord` far past the end, which is charged before it is built.
//!
//! Follows Torque3D's MIT-licensed `console/consoleFunctions.cpp` (`StringUnit`) (Copyright (c) 2012 GarageGames, LLC);
//! the code here is this crate's own.

use super::{Native, NativeCall, Vm, VmError};
use crate::text::{atoi, until_nul};
use crate::vm::MAX_STRING_BYTES;

/// Words are separated by any of these bytes, fields by tab or newline, records by newline.
pub const WORDS: &[u8] = b" \t\n";
pub const FIELDS: &[u8] = b"\t\n";
pub const RECORDS: &[u8] = b"\n";

/// The `endIndex` of `getWords(text, index)` without one (TGE's "INF").
const NO_END: i32 = 1_000_000;

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    until_nul(c.argv.get(i).map_or(&b""[..], Vec::as_slice))
}

/// `strcspn`: the length of the run at the start of `s` that holds no byte of `set`. For the natives' own short sets;
/// a set a script passes goes through [`ByteSet`].
pub fn span(s: &[u8], set: &[u8]) -> usize {
    s.iter().position(|b| set.contains(b)).unwrap_or(s.len())
}

/// A set of bytes with a constant-time test, built once from a set a script passes, so that testing N bytes against
/// it is linear in N whatever the set's length (a scan of the set per byte is N times its length).
#[derive(Clone)]
pub struct ByteSet([bool; 256]);

impl ByteSet {
    #[must_use]
    pub fn new(bytes: &[u8]) -> ByteSet {
        let mut set = [false; 256];
        for &b in bytes {
            set[usize::from(b)] = true;
        }
        ByteSet(set)
    }

    #[must_use]
    pub fn contains(&self, byte: u8) -> bool {
        self.0[usize::from(byte)]
    }

    /// The length of the run at the start of `s` that holds no byte of the set (`strcspn`).
    #[must_use]
    pub fn span(&self, s: &[u8]) -> usize {
        s.iter().position(|&b| self.contains(b)).unwrap_or(s.len())
    }

    /// The length of the run at the start of `s` that holds only bytes of the set (`strspn`).
    #[must_use]
    pub fn run(&self, s: &[u8]) -> usize {
        s.iter().position(|&b| !self.contains(b)).unwrap_or(s.len())
    }
}

/// Where unit `index` starts, skipping each unit before it together with the one separator after it
/// (`getUnit`'s and `getUnits`' skipping loop); `None` when the text runs out first. A negative
/// index is a count of 2^32 rounds, which runs out of text long before: `None`.
fn skip_units(s: &[u8], index: i32, set: &[u8]) -> Option<usize> {
    let mut left = usize::try_from(index).ok()?;
    let mut at = 0;
    while left > 0 {
        left -= 1;
        let rest = s.get(at..)?;
        if rest.is_empty() {
            return None;
        }
        let n = span(rest, set);
        if n == rest.len() {
            return None;
        }
        at += n + 1;
    }
    Some(at)
}

/// The unit at `index` (`getUnit`): skip `index` units, each with the one separator after it; an
/// empty unit (two separators in a row, or a separator first) is "".
pub fn unit(s: &[u8], index: i32, set: &[u8]) -> Vec<u8> {
    let Some(rest) = skip_units(s, index, set).and_then(|at| s.get(at..)) else {
        return Vec::new();
    };
    rest.get(..span(rest, set)).unwrap_or_default().to_vec()
}

/// The units `start` to `end` with the separators between them (`getUnits`). A separator that ends
/// the text right after the last unit taken is kept (`getWords("a b ", 1)` is `"b "`).
pub fn units(s: &[u8], start: i32, end: i32, set: &[u8]) -> Vec<u8> {
    // "" when the end is before the start.
    if end < start {
        return Vec::new();
    }
    let Some(from) = skip_units(s, start, set) else {
        return Vec::new();
    };
    let mut at = from;
    // Take `end - start + 1` units, each with the separator after it, stopping at the end of the text.
    let mut count = i64::from(end) - i64::from(start) + 1;
    while count > 0 {
        count -= 1;
        at += s.get(at..).map_or(0, |rest| span(rest, set));
        if at >= s.len() {
            break;
        }
        at += 1;
    }
    // The copy stops one byte before where the walk stopped, unless it stopped at the terminator.
    let stop = if at >= s.len() {
        s.len()
    } else {
        at.saturating_sub(1)
    };
    s.get(from..stop.max(from)).unwrap_or_default().to_vec()
}

/// The number of units (`getUnitCount`): one per separator, plus one when the text does not end with one.
pub fn unit_count(s: &[u8], set: &[u8]) -> i32 {
    let separators = s.iter().filter(|b| set.contains(b)).count();
    let last = s.last().is_some_and(|b| !set.contains(b));
    i32::try_from(separators + usize::from(last)).unwrap_or(i32::MAX)
}

/// The text with unit `index` replaced (`setUnit`). Past the end, the text is padded with the first
/// separator up to that unit.
///
/// `charge` is told the padding length before the padding is built: the result is built whole, the padding
/// charged to the step budget, and a result past the string-size cap is refused.
pub fn set_unit(
    s: &[u8],
    index: i32,
    replace: &[u8],
    set: &[u8],
    charge: impl FnOnce(usize) -> Result<(), VmError>,
) -> Result<Vec<u8>, VmError> {
    // The index is unsigned: a negative one pads far past any text, which the string-size cap refuses.
    let mut left = index as u32 as usize;
    let mut at = 0;
    let mut pad = 0;
    while left > 0 {
        let before = left;
        left -= 1;
        at += s.get(at..).map_or(0, |rest| span(rest, set));
        if at >= s.len() {
            at = s.len();
            pad = before;
            break;
        }
        at += 1;
    }
    // The rest after the replaced unit, from the separator that ends it.
    let rest = s
        .get(at..)
        .and_then(|tail| tail.get(span(tail, set)..))
        .unwrap_or_default();
    let total = at
        .saturating_add(pad)
        .saturating_add(replace.len())
        .saturating_add(rest.len());
    if total > MAX_STRING_BYTES {
        return Err(VmError::StringTooLong);
    }
    charge(pad)?;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(s.get(..at).unwrap_or_default());
    out.resize(at + pad, set.first().copied().unwrap_or(b' '));
    out.extend_from_slice(replace);
    out.extend_from_slice(rest);
    Ok(out)
}

/// The text without unit `index` and one separator next to it (`removeUnit`). An index past the last unit
/// leaves the text as it is.
pub fn remove_unit(s: &[u8], index: i32, set: &[u8]) -> Vec<u8> {
    // The index is unsigned: a negative one is past the last unit.
    let mut left = index as u32 as usize;
    let mut at = 0;
    while left > 0 {
        left -= 1;
        let rest = s.get(at..).unwrap_or_default();
        let n = span(rest, set);
        if n == rest.len() {
            // No unit there: the text as it is.
            return s.to_vec();
        }
        at += n + 1;
    }
    let rest = s.get(at..).unwrap_or_default();
    let n = span(rest, set);
    if n == rest.len() {
        // The last unit: drop it and the separator before it.
        return s.get(..at.saturating_sub(1)).unwrap_or_default().to_vec();
    }
    let mut out = s.get(..at).unwrap_or_default().to_vec();
    out.extend_from_slice(rest.get(n + 1..).unwrap_or_default());
    out
}

/// A string native's result, or the error that stops the script.
fn result(r: Result<Vec<u8>, VmError>) -> Native {
    match r {
        Ok(s) => Native::Str(s),
        Err(e) => Native::Fail(e),
    }
}

/// `getWords`/`getFields`/`getRecords`: the end index is `dAtoi` of the third argument when there is one.
fn end_index(c: &NativeCall) -> i32 {
    if c.argv.len() >= 4 {
        atoi(arg(c, 3))
    } else {
        NO_END
    }
}

/// The three families of unit functions: the name part, the separators and the usage texts of `get<Unit>`,
/// `get<Unit>s`, `set<Unit>`, `remove<Unit>` and `get<Unit>Count`.
pub const FAMILIES: [(&str, &[u8], [&str; 5]); 3] = [
    (
        "Word",
        WORDS,
        [
            "getWord(text, index) - one space-, tab- or newline-separated word",
            "getWords(text, first [, last]) - the words from first to last",
            "setWord(text, index, value) - the text with that word replaced",
            "removeWord(text, index) - the text without that word",
            "getWordCount(text) - how many words the text has",
        ],
    ),
    (
        "Field",
        FIELDS,
        [
            "getField(text, index) - one tab- or newline-separated field",
            "getFields(text, first [, last]) - the fields from first to last",
            "setField(text, index, value) - the text with that field replaced",
            "removeField(text, index) - the text without that field",
            "getFieldCount(text) - how many fields the text has",
        ],
    ),
    (
        "Record",
        RECORDS,
        [
            "getRecord(text, index) - one newline-separated record",
            "getRecords(text, first [, last]) - the records from first to last",
            "setRecord(text, index, value) - the text with that record replaced",
            "removeRecord(text, index) - the text without that record",
            "getRecordCount(text) - how many records the text has",
        ],
    ),
];

pub(super) fn register(vm: &mut Vm) {
    for (unit_name, set, usage) in FAMILIES {
        vm.register(&format!("get{unit_name}"), 3, 3, usage[0], move |_, c| {
            Native::Str(unit(arg(c, 1), atoi(arg(c, 2)), set))
        });
        vm.register(&format!("get{unit_name}s"), 3, 4, usage[1], move |_, c| {
            Native::Str(units(arg(c, 1), atoi(arg(c, 2)), end_index(c), set))
        });
        vm.register(&format!("set{unit_name}"), 4, 4, usage[2], move |vm, c| {
            result(set_unit(
                arg(c, 1),
                atoi(arg(c, 2)),
                arg(c, 3),
                set,
                |pad| {
                    vm.objects.add_work(pad as u64);
                    vm.ensure_budget()
                },
            ))
        });
        vm.register(
            &format!("remove{unit_name}"),
            3,
            3,
            usage[3],
            move |_, c| Native::Str(remove_unit(arg(c, 1), atoi(arg(c, 2)), set)),
        );
        vm.register(
            &format!("get{unit_name}Count"),
            2,
            2,
            usage[4],
            move |_, c| Native::Int(unit_count(arg(c, 1), set)),
        );
    }
    // `firstWord`/`restWords` split at the first space only (tabs and newlines are part of the word).
    vm.register(
        "firstWord",
        2,
        2,
        "firstWord(text) - the text up to its first space",
        |_, c| {
            let s = arg(c, 1);
            Native::Str(s.get(..span(s, b" ")).unwrap_or_default().to_vec())
        },
    );
    vm.register(
        "restWords",
        2,
        2,
        "restWords(text) - the text after its first space",
        |_, c| {
            let s = arg(c, 1);
            let n = span(s, b" ");
            Native::Str(s.get(n + 1..).unwrap_or_default().to_vec())
        },
    );
    vm.register(
        "NextToken",
        4,
        4,
        "nextToken(text, tokenVariable, delimiters) - stores the first token, returns the rest",
        |vm, c| Native::Str(vm.next_token(arg(c, 1), arg(c, 2), arg(c, 3))),
    );
    // It stores its token in the variable its second argument names.
    vm.add_native_effects("", "nextToken", crate::effects::Effects::VARIABLES);
}

impl Vm {
    /// `nextToken(str, token, delim)`: skips the delimiters at the start of `str`, stores
    /// the token that follows in the variable `token`, skips the delimiters after it and returns the rest.
    /// The variable is a local of the innermost running frame (`%` prefixed unless the name starts with
    /// one), or a global (`$` prefixed unless it starts with one) when no frame runs. Only the frame's existence
    /// is tested (TGE also wants it named); the top level of a host's `run` has a frame.
    pub(crate) fn next_token(&mut self, s: &[u8], token: &[u8], delim: &[u8]) -> Vec<u8> {
        let delim = ByteSet::new(delim);
        let skip = |from: usize| from + delim.run(s.get(from..).unwrap_or_default());
        let start = skip(0);
        let tail = s.get(start..).unwrap_or_default();
        let end = start + delim.span(tail);
        let value = s.get(start..end).unwrap_or_default();
        let rest_from = if end < s.len() { skip(end + 1) } else { end };
        let local = !self.frames.is_empty();
        let prefix = if local { b'%' } else { b'$' };
        let mut name = Vec::with_capacity(token.len() + 1);
        if token.first() != Some(&prefix) {
            name.push(prefix);
        }
        name.extend_from_slice(token);
        let entry = match self.frames.last_mut() {
            Some(frame) if local => frame.add(&name),
            _ => self.globals.add(&name),
        };
        if let Some(entry) = entry {
            entry.set_string(value);
        }
        s.get(rest_from..).unwrap_or_default().to_vec()
    }
}
