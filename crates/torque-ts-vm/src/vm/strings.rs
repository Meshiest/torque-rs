//! The string functions: comparisons, searches, case, trimming, escapes and `StripMLControlChars`, as TGE 1.4
//! has them. Searches use `text::find` (linear time), not quadratic `strstr` loops, with the same results.
//!
//! Work bounds: the arguments' bytes are charged when the call collects them, and every native here is
//! linear in them, but for the ones whose result can outgrow the arguments or whose passes repeat:
//! `strreplace` charges each replacement it writes and refuses a result past the string-size cap,
//! `StripMLControlChars` charges every pass and every scan that did not end in a tag (and stops at the
//! step limit between passes). Choices where a C implementation can abort:
//! `strnicmp` with a negative count compares nothing; `getSubStr` with a start inside the text and a
//! count that makes the sum pass 2^31-1 gives the rest of the text; `strreplace` stops at the cap.
//!
//! Follows Torque3D's MIT-licensed `console/consoleFunctions.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here
//! is this crate's own.

use super::words::ByteSet;
use super::{Native, NativeCall, Vm, VmError};
use crate::text::{atoi, collapse_escape, expand_escape_capped, find, until_nul};
use crate::vm::MAX_STRING_BYTES;

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    until_nul(c.argv.get(i).map_or(&b""[..], Vec::as_slice))
}

/// The sign of a comparison, as the C runtime's `strcmp` family returns it (-1, 0 or 1).
fn sign(o: std::cmp::Ordering) -> Native {
    Native::Int(o as i32)
}

/// `strncmp`/`_strnicmp` over at most `n` bytes, with the bytes mapped by `fold` first.
fn compare(a: &[u8], b: &[u8], n: usize, fold: fn(u8) -> u8) -> Native {
    let a = a.iter().take(n).map(|&x| fold(x));
    let b = b.iter().take(n).map(|&x| fold(x));
    sign(a.cmp(b))
}

/// `_stricmp` and `_strnicmp` fold to lower case (so `[` sorts before `a`).
fn lower(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

/// `strpos`: the first match at or after `offset`, or -1 (also for a negative offset). An empty needle matches at
/// the offset itself while that is inside the text or at its end.
fn position(hay: &[u8], needle: &[u8], offset: i32) -> Native {
    let Ok(from) = usize::try_from(offset) else {
        return Native::Int(-1);
    };
    if needle.is_empty() {
        return Native::Int(if from <= hay.len() { offset } else { -1 });
    }
    Native::Int(find(hay, needle, from, false).map_or(-1, |at| i32::try_from(at).unwrap_or(-1)))
}

/// `strreplace` (TGE): every occurrence of `from`, left to right without overlap; an empty `from` leaves
/// the text as it is. A result past the string-size cap stops the script (TGE's buffer overflows).
///
/// `charge` is told the length of each replacement written, so the growth a few argument bytes can ask for
/// (`strreplace(a*65536, "a", b*2048)`) is work like the string stack's writes and meets the step limit.
pub(crate) fn replace(
    s: &[u8],
    from: &[u8],
    to: &[u8],
    mut charge: impl FnMut(usize) -> Result<(), VmError>,
) -> Result<Vec<u8>, VmError> {
    if from.is_empty() {
        return Ok(s.to_vec());
    }
    let mut out = Vec::with_capacity(s.len());
    let mut at = 0;
    while let Some(hit) = find(s, from, at, false) {
        out.extend_from_slice(s.get(at..hit).unwrap_or_default());
        out.extend_from_slice(to);
        if out.len() > MAX_STRING_BYTES {
            return Err(VmError::StringTooLong);
        }
        charge(to.len())?;
        at = hit.saturating_add(from.len());
    }
    out.extend_from_slice(s.get(at..).unwrap_or_default());
    if out.len() > MAX_STRING_BYTES {
        return Err(VmError::StringTooLong);
    }
    Ok(out)
}

/// The bytes `ltrim`, `rtrim` and `trim` remove (TGE: not `\r`).
fn is_trim(b: &u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\t')
}

/// What `StripMLControlChars` does with a `<` that may open a markup tag: the first value is
/// `Some(n)` to skip `n` bytes of `t` (which starts at the `<`), `None` to leave the `<` as text. The
/// prefixes are tried in a fixed order and compared ignoring case; a prefix whose argument does not
/// scan is text, and the later prefixes are not tried. The second value is how many bytes the argument scan
/// looked at (work for the caller to charge: a scan that fails still walks to the end of the text).
fn markup_tag(t: &[u8]) -> (Option<usize>, usize) {
    /// How the bytes after a prefix are checked: a fixed tag, an argument up to `>` that stops at `:`
    /// (`Scan`) or not (`ScanColons`), a colour of 6 or 8 digits, or a link that is dropped whole.
    enum Arg {
        Fixed,
        Scan,
        ScanColons,
        Color,
        Link,
    }
    const TAGS: [(&[u8], Arg); 27] = [
        (b"br>", Arg::Fixed),
        (b"font:", Arg::ScanColons),
        (b"tag:", Arg::Scan),
        (b"color:", Arg::Color),
        (b"bitmap:", Arg::Scan),
        (b"bitmapk:", Arg::Scan),
        (b"shadow:", Arg::ScanColons),
        (b"shadowcolor:", Arg::Scan),
        (b"linkcolor:", Arg::Scan),
        (b"linkcolorHL:", Arg::Scan),
        (b"tag:", Arg::Scan),
        (b"spush>", Arg::Fixed),
        (b"spop>", Arg::Fixed),
        (b"sbreak>", Arg::Fixed),
        (b"just:left>", Arg::Fixed),
        (b"just:right>", Arg::Fixed),
        (b"just:center>", Arg::Fixed),
        (b"a:", Arg::Link),
        (b"/a>", Arg::Fixed),
        (b"lmargin%:", Arg::Scan),
        (b"lmargin:", Arg::Scan),
        (b"rmargin%:", Arg::Scan),
        (b"rmargin:", Arg::Scan),
        (b"clip:", Arg::Scan),
        (b"/clip>", Arg::Fixed),
        (b"div:", Arg::Scan),
        (b"tab:", Arg::Scan),
    ];
    // `scanforchar`: from `start`, the index of the `>` that ends the argument, if one does, and the
    // number of bytes looked at.
    let scan = |start: usize, colons: bool| {
        let mut j = start;
        while let Some(&b) = t.get(j) {
            if b == b'>' || b == b'\n' || (b == b':' && !colons) {
                break;
            }
            j += 1;
        }
        let end = (t.get(j) == Some(&b'>') && j != start).then_some(j);
        (end, j.saturating_sub(start) + 1)
    };
    let rest = t.get(1..).unwrap_or_default();
    for (prefix, kind) in &TAGS {
        if rest
            .get(..prefix.len())
            .is_none_or(|head| !head.eq_ignore_ascii_case(prefix))
        {
            continue;
        }
        let start = prefix.len() + 1;
        return match kind {
            Arg::Fixed => (Some(start), 0),
            Arg::Scan => {
                let (end, looked) = scan(start, false);
                (end.map(|end| end + 1), looked)
            }
            Arg::ScanColons => {
                let (end, looked) = scan(start, true);
                (end.map(|end| end + 1), looked)
            }
            Arg::Color => {
                let (end, looked) = scan(start, false);
                (
                    end.filter(|end| *end == 13 || *end == 15)
                        .map(|end| end + 1),
                    looked,
                )
            }
            // A link whose target does not scan still loses its `<a:`.
            Arg::Link => {
                let (end, looked) = scan(start, false);
                (Some(end.map_or(start, |end| end + 1)), looked)
            }
        };
    }
    (None, 0)
}

/// The output length after which `StripMLControlChars` drops the rest (checked between tokens only).
const ML_KEEP: usize = 63;

impl Vm {
    /// `StripMLControlChars` (`GuiMLTextCtrl::stripControlChars`): drops the bytes below 0x20 and
    /// the markup tags [`markup_tag`] knows; passes repeat until one strips nothing (so `<fo<br>nt:A:1>z` is
    /// `z`). Once the output has 63 bytes the rest is dropped, but only between tokens: a run of plain text
    /// is copied whole.
    ///
    /// Work: a pass reads at most the previous pass's output, and the output holds only a run past 63 bytes,
    /// so passes are few (each strips something, and what is kept beyond 63 bytes is one run); but a `<` that
    /// begins no tag can still make a scan walk to the end of the text, up to 63 times a pass. Every pass is
    /// charged its text, checked against the step budget before it starts, and each scan that did not end in a
    /// tag is charged and checked as it ends.
    pub(crate) fn strip_ml_control_chars(&self, input: &[u8]) -> Result<Vec<u8>, VmError> {
        let mut src = until_nul(input).to_vec();
        loop {
            self.objects.add_work(src.len() as u64);
            self.ensure_budget()?;
            let mut out = Vec::with_capacity(src.len().min(ML_KEEP + 1));
            let mut stripped = false;
            let mut i = 0;
            while out.len() < ML_KEEP {
                let Some(&c) = src.get(i) else { break };
                if c < 0x20 {
                    i += 1;
                    stripped = true;
                    continue;
                }
                if c == b'<' {
                    let (skip, looked) = markup_tag(src.get(i..).unwrap_or_default());
                    if let Some(skip) = skip {
                        i += skip;
                        stripped = true;
                        continue;
                    }
                    self.objects.add_work(looked as u64);
                    self.ensure_budget()?;
                }
                out.push(c);
                i += 1;
                while let Some(&next) = src.get(i) {
                    if next == b'<' || next < 0x20 {
                        break;
                    }
                    out.push(next);
                    i += 1;
                }
            }
            if !stripped {
                return Ok(out);
            }
            src = out;
        }
    }

    /// Prints `text` at the error level between two empty normal lines, with a BackTrace before the second:
    /// a way for natives to report a script's mistake loudly.
    /// The BackTrace runs script code, so its error (call depth, step limit) is returned for the native to
    /// hand on as `Native::Fail`.
    pub fn error_framed(&mut self, text: &[u8]) -> Result<(), VmError> {
        self.print(b"");
        self.log_bytes(super::Level::Error, text);
        self.print_backtrace()?;
        self.print(b"");
        Ok(())
    }
}

/// A string native's result, or the error that stops the script.
fn result(r: Result<Vec<u8>, VmError>) -> Native {
    match r {
        Ok(s) => Native::Str(s),
        Err(e) => Native::Fail(e),
    }
}

pub(super) fn register(vm: &mut Vm) {
    vm.register(
        "strcmp",
        3,
        3,
        "strcmp(a, b) - -1, 0 or 1 as a sorts before, with or after b",
        |_, c| sign(arg(c, 1).cmp(arg(c, 2))),
    );
    vm.register(
        "stricmp",
        3,
        3,
        "stricmp(a, b) - strcmp ignoring case",
        |_, c| compare(arg(c, 1), arg(c, 2), usize::MAX, lower),
    );
    vm.register(
        "strlen",
        2,
        2,
        "strlen(text) - the length in bytes",
        |_, c| Native::Int(i32::try_from(arg(c, 1).len()).unwrap_or(i32::MAX)),
    );
    vm.register(
        "strstr",
        3,
        3,
        "strstr(text, needle) - where the needle first occurs, or -1",
        |_, c| {
            let at = find(arg(c, 1), arg(c, 2), 0, false);
            Native::Int(at.map_or(-1, |at| i32::try_from(at).unwrap_or(-1)))
        },
    );
    vm.register(
        "strpos",
        3,
        4,
        "strpos(text, needle [, offset]) - where the needle first occurs at or after offset, or -1",
        |_, c| position(arg(c, 1), arg(c, 2), atoi(arg(c, 3))),
    );
    vm.register(
        "strreplace",
        4,
        4,
        "strreplace(text, from, to) - the text with every from replaced by to",
        |vm, c| {
            result(replace(arg(c, 1), arg(c, 2), arg(c, 3), |written| {
                vm.objects.add_work(written as u64);
                vm.ensure_budget()
            }))
        },
    );
    // Only ASCII letters change case (the "C" locale).
    vm.register(
        "strupr",
        2,
        2,
        "strupr(text) - the text in upper case",
        |_, c| Native::Str(arg(c, 1).to_ascii_uppercase()),
    );
    vm.register(
        "strlwr",
        2,
        2,
        "strlwr(text) - the text in lower case",
        |_, c| Native::Str(arg(c, 1).to_ascii_lowercase()),
    );
    // The text from the first occurrence of the second argument's first byte; "" when there is none
    // (an empty second argument finds the terminator: "").
    vm.register(
        "strchr",
        3,
        3,
        "strchr(text, char) - the text from the first occurrence of the character",
        |_, c| {
            let s = arg(c, 1);
            let rest = arg(c, 2)
                .first()
                .and_then(|b| s.iter().position(|x| x == b))
                .and_then(|at| s.get(at..))
                .unwrap_or_default();
            Native::Str(rest.to_vec())
        },
    );
    vm.register(
        "getSubStr",
        4,
        4,
        "getSubStr(text, start, count) - count bytes of the text from start",
        |vm, c| {
            let s = arg(c, 1);
            let (start, count) = (atoi(arg(c, 2)), atoi(arg(c, 3)));
            let (Ok(from), Ok(count)) = (usize::try_from(start), usize::try_from(count)) else {
                vm.error(format!(
                    "getSubStr(...): error, starting position and desired length must be >= 0: ({start}, {count})"
                ));
                return Native::Str(Vec::new());
            };
            let piece = s
                .get(from..)
                .map(|rest| rest.get(..count).unwrap_or(rest))
                .unwrap_or_default();
            Native::Str(piece.to_vec())
        },
    );
    vm.register(
        "ltrim",
        2,
        2,
        "ltrim(text) - the text without leading white space",
        |_, c| {
            let s = arg(c, 1);
            let start = s.iter().take_while(|b| is_trim(b)).count();
            Native::Str(s.get(start..).unwrap_or_default().to_vec())
        },
    );
    vm.register(
        "rtrim",
        2,
        2,
        "rtrim(text) - the text without trailing white space",
        |_, c| {
            let s = arg(c, 1);
            let end = s.iter().rposition(|b| !is_trim(b)).map_or(0, |i| i + 1);
            Native::Str(s.get(..end).unwrap_or_default().to_vec())
        },
    );
    vm.register(
        "trim",
        2,
        2,
        "trim(text) - the text without leading and trailing white space",
        |_, c| {
            let s = arg(c, 1);
            let start = s.iter().take_while(|b| is_trim(b)).count();
            let end = s.iter().rposition(|b| !is_trim(b)).map_or(start, |i| i + 1);
            Native::Str(s.get(start..end.max(start)).unwrap_or_default().to_vec())
        },
    );
    vm.register(
        "stripChars",
        3,
        3,
        "stripChars(text, chars) - the text without any of the characters",
        |_, c| {
            let set = ByteSet::new(arg(c, 2));
            Native::Str(
                arg(c, 1)
                    .iter()
                    .copied()
                    .filter(|&b| !set.contains(b))
                    .collect(),
            )
        },
    );
    // Trailing spaces AND underscores, as TGE's.
    vm.register(
        "stripTrailingSpaces",
        2,
        2,
        "stripTrailingSpaces(text) - the text without trailing spaces and underscores",
        |_, c| {
            let s = arg(c, 1);
            let end = s
                .iter()
                .rposition(|b| *b != b' ' && *b != b'_')
                .map_or(0, |i| i + 1);
            Native::Str(s.get(..end).unwrap_or_default().to_vec())
        },
    );
    vm.register(
        "expandEscape",
        2,
        2,
        "expandEscape(text) - the text with its control bytes and quotes escaped",
        |_, c| {
            result(expand_escape_capped(arg(c, 1), MAX_STRING_BYTES).ok_or(VmError::StringTooLong))
        },
    );
    vm.register("collapseEscape", 2, 2, "collapseEscape(text) - the text with its escape sequences turned into the bytes they stand for", |_, c| {
        Native::Str(collapse_escape(arg(c, 1)))
    });
    vm.register(
        "StripMLControlChars",
        2,
        2,
        "StripMLControlChars(text) - the text without its markup tags and control bytes",
        |vm, c| result(vm.strip_ml_control_chars(arg(c, 1))),
    );
    // `strToPlayerName(string)`: the leading spaces and underscores dropped ("" if nothing is left), then at most 16
    // bytes kept, skipping control bytes and `,` `.` `'` `` ` `` and every space or underscore right after another; the
    // result through `StripMLControlChars`.
    vm.register(
        "strToPlayerName",
        2,
        2,
        "strToPlayerName(text) - the text as a short display name",
        |vm, c| {
            let text = arg(c, 1);
            let start = text
                .iter()
                .position(|b| *b != b' ' && *b != b'_')
                .unwrap_or(text.len());
            let rest = &text[start..];
            if rest.is_empty() {
                return Native::Str(Vec::new());
            }
            let mut name = Vec::with_capacity(16);
            let mut after_space = false;
            for &b in rest {
                if name.len() > 15 {
                    break;
                }
                if b < 0x20 || matches!(b, b',' | b'.' | b'\'' | b'`') {
                    continue;
                }
                if b == b' ' || b == b'_' {
                    if after_space {
                        continue;
                    }
                    after_space = true;
                } else {
                    after_space = false;
                }
                name.push(b);
            }
            result(vm.strip_ml_control_chars(&name))
        },
    );
}

#[cfg(test)]
#[path = "strings_tests.rs"]
mod tests;
