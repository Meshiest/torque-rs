//! The C-library text functions the script engine depends on (`atoi`, `atof`, `printf("%g")`, `printf("%d")`,
//! `sscanf("%f")`, `qsort`), written to the C standard: decimal and hexadecimal floats, `inf`/`infinity`,
//! `nan` and `nan(chars)` are read as C99's `strtod` reads them; a NaN prints as `nan` or `-nan`. Shared by the
//! script interpreter and the script compiler.
//!
//! Where the C standard leaves a choice to the implementation (the bits of a NaN read from text and how a NaN
//! prints, the conversion of an out-of-range float to an integer, a format's integer conversion given a double,
//! the order a sort leaves equal keys in), this crate makes the plain choice documented on each function, and a
//! host can make another by installing its [`TextRules`] once with [`install_rules`].
//!
//! [`expand_escape`] and [`collapse_escape`] follow `expandEscape` and `collapseEscape` of Torque3D's MIT-licensed
//! `console/torquescript/CMDscan.l` (part of the Torque3D tree, Copyright (c) 2012 GarageGames, LLC; the file itself
//! carries no notice). [`is_match`] follows the Torque Game Engine's `FindMatch::isMatch` (`core/findMatch.cc`, TGE
//! 1.4, Copyright (C) GarageGames.com, Inc.; the Torque engine is MIT-licensed as Torque3D, Copyright (c) 2012 GarageGames,
//! LLC, whose own `findMatch.cpp` matches differently). The code here is this crate's own.
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - [`atoi`] reads only the bytes it takes (whitespace, a sign, digits): callers pass it the rest of a text, so a scan of the
//!   whole input per call would make a pass over many numbers quadratic
//!   (`atoi_stops_at_a_nul_as_at_any_other_byte_and_reads_only_what_it_takes`).

use std::sync::OnceLock;

/// The implementation-defined choices of this crate's functions, for a host that needs other ones than
/// [`DefaultRules`]. Every method has the default's behaviour unless overridden.
pub trait TextRules: Sync {
    /// The value [`atof`] reads from `nan` (any case) after an optional sign (`negative`); `tail` is the text after
    /// `nan`. The default is a quiet NaN with that sign.
    fn nan(&self, negative: bool, tail: &[u8]) -> f64 {
        let _ = tail;
        if negative { -f64::NAN } else { f64::NAN }
    }

    /// One `%f` conversion of `nan` for [`scan_floats`]: the value and how many bytes of `tail` (the text after `nan`)
    /// it takes, or None when the conversion fails. The default takes C99's optional `(n-char-sequence)` (letters,
    /// digits and `_`, closed by `)`; an unclosed one fails) and gives a quiet NaN with the sign.
    fn scan_nan(&self, negative: bool, tail: &[u8]) -> Option<(f32, usize)> {
        let taken = match tail.first() {
            Some(b'(') => nan_parens(tail)?.len() + 2,
            _ => 0,
        };
        Some((if negative { -f32::NAN } else { f32::NAN }, taken))
    }

    /// How `%g`, `%f` and `%e` print the NaN `v`, its sign included. The default is `nan`, or `-nan` when the sign
    /// bit is set.
    fn nan_text(&self, v: f64) -> String {
        if v.is_sign_negative() { "-nan" } else { "nan" }.to_string()
    }

    /// [`f64_to_u32`] of an `x` outside `-2^63..2^63`, an infinity or a NaN: the default converts to a 64-bit integer
    /// as Rust's `as` does (saturating at its ends, NaN 0) and keeps the low 32 bits.
    fn f64_to_u32(&self, x: f64) -> u32 {
        (x as i64) as u32
    }

    /// The 32-bit word that an integer conversion (`%d %i %u %x %X %o %c`) of [`sprintf_double`] prints for its double
    /// argument; None (the default) prints nothing for it, as the format does not match the argument.
    fn double_as_word(&self, v: f64) -> Option<u32> {
        let _ = v;
        None
    }

    /// Sorts the indices in `order` by `cmp` (negative, zero or positive, as `qsort`'s comparator). The default is a
    /// stable sort: equal keys keep their order.
    fn sort(&self, order: &mut [usize], cmp: &mut dyn FnMut(usize, usize) -> i32) {
        merge_sort(order, cmp);
    }
}

/// A stable bottom-up merge sort of `order` by `cmp`. The comparator comes from a script, so it may be no order at
/// all (NaN keys, a comparator that contradicts itself): the sort then still ends after `n log n` comparisons with a
/// permutation of `order` and never panics, where `slice::sort_by` may panic on a comparator that is not a total order.
fn merge_sort(order: &mut [usize], cmp: &mut dyn FnMut(usize, usize) -> i32) {
    let n = order.len();
    let mut from = order.to_vec();
    let mut to = vec![0usize; n];
    let mut width = 1;
    while width < n {
        let mut start = 0;
        while start < n {
            let mid = (start + width).min(n);
            let end = (start + 2 * width).min(n);
            let (mut i, mut j) = (start, mid);
            for slot in to.iter_mut().take(end).skip(start) {
                // The left run's element goes first unless the right one is strictly smaller: stable.
                let take_right = match (from.get(i), from.get(j)) {
                    (Some(&a), Some(&b)) if i < mid && j < end => cmp(a, b) > 0,
                    _ => i >= mid,
                };
                if take_right {
                    *slot = from.get(j).copied().unwrap_or_default();
                    j += 1;
                } else {
                    *slot = from.get(i).copied().unwrap_or_default();
                    i += 1;
                }
            }
            start = end;
        }
        std::mem::swap(&mut from, &mut to);
        width = width.saturating_mul(2);
    }
    order.copy_from_slice(&from);
}

/// The rules this crate uses when no host installed others: every [`TextRules`] method's default.
pub struct DefaultRules;

impl TextRules for DefaultRules {}

static RULES: OnceLock<&'static dyn TextRules> = OnceLock::new();

/// Installs `rules` for the whole process, once: true when this call installed them, false (changing nothing) when
/// rules were installed before.
pub fn install_rules(rules: &'static dyn TextRules) -> bool {
    RULES.set(rules).is_ok()
}

/// The rules in force: the installed ones, else [`DefaultRules`].
pub fn rules() -> &'static dyn TextRules {
    RULES.get().copied().unwrap_or(&DefaultRules)
}

fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The bytes before the first NUL (C string semantics).
pub fn until_nul(s: &[u8]) -> &[u8] {
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    &s[..end]
}

/// ASCII lower-case copy, for case-insensitive lookup keys.
pub fn fold(s: &[u8]) -> Vec<u8> {
    s.iter().map(u8::to_ascii_lowercase).collect()
}

/// [`fold`] handed to `f` without an allocation when `s` is short (a name looked up in a map keyed by folded names).
pub fn with_folded<R>(s: &[u8], f: impl FnOnce(&[u8]) -> R) -> R {
    let mut buf = [0u8; 64];
    match buf.get_mut(..s.len()) {
        Some(short) => {
            for (d, c) in short.iter_mut().zip(s) {
                *d = c.to_ascii_lowercase();
            }
            f(short)
        }
        None => f(&fold(s)),
    }
}

/// C `atoi`: optional whitespace, optional sign, decimal digits, stop at the first other byte (a NUL included).
/// Out-of-range values saturate to `i32::MIN` / `i32::MAX`, as C's `strtol` does. Its cost is the bytes it takes, not the
/// length of `s`: an ML reflow reads every tag's number from the rest of its text.
pub fn atoi(s: &[u8]) -> i32 {
    let mut i = 0;
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    let mut negative = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        negative = s[i] == b'-';
        i += 1;
    }
    let mut value: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        value = (value * 10 + i64::from(s[i] - b'0')).min(1 << 40);
        i += 1;
    }
    if negative {
        value = -value;
    }
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn starts_with_ignore_case(s: &[u8], prefix: &str) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn hex_digit(b: u8) -> Option<u32> {
    (b as char).to_digit(16)
}

/// Parses what follows `0x`: hex digits, an optional `.` and more digits, an optional `p` exponent.
/// `None` when there is no hex digit at all.
fn parse_hex(s: &[u8]) -> Option<f64> {
    let mut i = 0;
    let mut mantissa = 0.0f64;
    let mut digits: usize = 0;
    while i < s.len()
        && let Some(d) = hex_digit(s[i])
    {
        mantissa = mantissa * 16.0 + f64::from(d);
        digits += 1;
        i += 1;
    }
    let mut exponent: i32 = 0;
    if i < s.len() && s[i] == b'.' {
        let mut j = i + 1;
        while j < s.len()
            && let Some(d) = hex_digit(s[j])
        {
            mantissa = mantissa * 16.0 + f64::from(d);
            exponent = exponent.saturating_sub(4);
            digits += 1;
            j += 1;
        }
        if digits > 0 {
            i = j;
        }
    }
    if digits == 0 {
        return None;
    }
    // A zero mantissa is zero whatever the exponent (0 * 2^huge would otherwise be NaN); the caller
    // applies the sign.
    if mantissa == 0.0 {
        return Some(0.0);
    }
    if i < s.len() && (s[i] == b'p' || s[i] == b'P') {
        let mut j = i + 1;
        let mut negative = false;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            negative = s[j] == b'-';
            j += 1;
        }
        let start = j;
        let mut p: i32 = 0;
        while j < s.len() && s[j].is_ascii_digit() {
            p = p.saturating_mul(10).saturating_add(i32::from(s[j] - b'0'));
            j += 1;
        }
        if j > start {
            exponent = exponent.saturating_add(if negative { p.saturating_neg() } else { p });
        }
    }
    // The scale is applied in two halves so that a large mantissa with a very negative exponent does
    // not lose everything to an early underflow of the scale factor alone.
    let half = exponent / 2;
    Some(mantissa * 2f64.powi(half) * 2f64.powi(exponent - half))
}

/// The C99 `n-char-sequence` inside the parentheses that may follow "nan": letters, digits and `_` up to a
/// closing `)`. None for no parentheses, another character or no `)`.
pub fn nan_parens(tail: &[u8]) -> Option<&[u8]> {
    let after = tail.strip_prefix(b"(")?;
    let inner = after
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
        .count();
    (after.get(inner) == Some(&b')')).then(|| after.get(..inner).unwrap_or_default())
}

/// C `atof` (`strtod`, as C99 defines it): leading whitespace, sign, `inf`, `infinity`, `nan` (its value from
/// [`TextRules::nan`]), hex (`0x1A`, `0x1p4`) or decimal with optional exponent. Anything else is 0.
pub fn atof(s: &[u8]) -> f64 {
    let s = until_nul(s);
    let mut i = 0;
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    let mut negative = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        negative = s[i] == b'-';
        i += 1;
    }
    let rest = &s[i..];
    let signed = |v: f64| if negative { -v } else { v };

    if starts_with_ignore_case(rest, "infinity") || starts_with_ignore_case(rest, "inf") {
        return signed(f64::INFINITY);
    }
    if starts_with_ignore_case(rest, "nan") {
        return rules().nan(negative, rest.get(3..).unwrap_or_default());
    }
    if rest.len() >= 2
        && rest[0] == b'0'
        && (rest[1] == b'x' || rest[1] == b'X')
        && let Some(v) = parse_hex(&rest[2..])
    {
        return signed(v);
    }

    let mut j = 0;
    let mut digits: usize = 0;
    while j < rest.len() && rest[j].is_ascii_digit() {
        j += 1;
        digits += 1;
    }
    if j < rest.len() && rest[j] == b'.' {
        let mut k = j + 1;
        let mut frac: usize = 0;
        while k < rest.len() && rest[k].is_ascii_digit() {
            k += 1;
            frac += 1;
        }
        if digits + frac > 0 {
            j = k;
            digits += frac;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    if j < rest.len() && (rest[j] == b'e' || rest[j] == b'E') {
        let mut k = j + 1;
        if k < rest.len() && (rest[k] == b'+' || rest[k] == b'-') {
            k += 1;
        }
        let exp_start = k;
        while k < rest.len() && rest[k].is_ascii_digit() {
            k += 1;
        }
        if k > exp_start {
            j = k;
        }
    }
    if let Some(v) = decimal_fast(&rest[..j]) {
        return signed(v);
    }
    // The scanned prefix is plain ASCII, so this cannot fail.
    let text = std::str::from_utf8(&rest[..j]).unwrap_or("0");
    signed(text.parse::<f64>().unwrap_or(0.0))
}

/// The powers of ten a double holds exactly.
const EXACT_POWERS_OF_TEN: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// A scanned decimal (`digits[.digits][e[sign]digits]`) read without the general parser when that is exact: at most 15
/// significant digits (the integer they make is below 2^53, so exact as a double) and a power of ten from 10^-22 to 10^22 (each
/// exact): one multiplication or division, rounded once to nearest, is then the correctly rounded value, as the general parser
/// gives it (Clinger's fast path). None otherwise.
fn decimal_fast(text: &[u8]) -> Option<f64> {
    let mut mantissa: u64 = 0;
    let mut significant = 0usize;
    let mut fraction = 0i32;
    let mut after_point = false;
    let mut i = 0;
    while let Some(&c) = text.get(i) {
        if c == b'.' {
            after_point = true;
        } else if c.is_ascii_digit() {
            if mantissa != 0 || c != b'0' {
                significant += 1;
                if significant > 15 {
                    return None;
                }
            }
            mantissa = mantissa * 10 + u64::from(c - b'0');
            if after_point {
                fraction += 1;
            }
        } else {
            break;
        }
        i += 1;
    }
    let mut exponent = 0i32;
    if let Some(&c) = text.get(i)
        && (c == b'e' || c == b'E')
    {
        i += 1;
        let negative = match text.get(i) {
            Some(b'-') => {
                i += 1;
                true
            }
            Some(b'+') => {
                i += 1;
                false
            }
            _ => false,
        };
        let start = i;
        while let Some(&c) = text.get(i).filter(|c| c.is_ascii_digit()) {
            if i - start >= 4 {
                return None;
            }
            exponent = exponent * 10 + i32::from(c - b'0');
            i += 1;
        }
        if i == start {
            return None;
        }
        if negative {
            exponent = -exponent;
        }
    }
    if i != text.len() {
        return None;
    }
    if mantissa == 0 {
        return Some(0.0);
    }
    let power = exponent - fraction;
    let m = mantissa as f64;
    if (0..=22).contains(&power) {
        Some(m * EXACT_POWERS_OF_TEN[power as usize])
    } else if (-22..0).contains(&power) {
        Some(m / EXACT_POWERS_OF_TEN[(-power) as usize])
    } else {
        None
    }
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// C `printf("%g", v)` with the default precision of six significant digits: the exact binary value rounded to
/// nearest, `inf`, `-inf`, and a NaN as [`TextRules::nan_text`] spells it.
pub fn format_g(v: f64) -> String {
    with_format_g(v, |text| String::from_utf8_lossy(text).into_owned())
}

/// [`format_g`] handed to `f` as bytes: whole numbers and short decimals, which are nearly every value a script
/// prints, are written to the stack, so a conversion that only reads the text allocates nothing.
pub fn with_format_g<R>(v: f64, f: impl FnOnce(&[u8]) -> R) -> R {
    if v.is_nan() {
        return f(rules().nan_text(v).as_bytes());
    }
    if v.is_infinite() {
        return f(if v < 0.0 { b"-inf" as &[u8] } else { b"inf" });
    }
    if v == 0.0 {
        return f(if v.is_sign_negative() {
            b"-0" as &[u8]
        } else {
            b"0"
        });
    }
    if let Some(text) = format_whole(v, 1e6).or_else(|| format_g_short(v)) {
        return f(text.as_bytes());
    }
    f(format_g_exact(v).as_bytes())
}

/// `%.<p>g` of a whole number `v` (not 0) below `limit` = `10^p` in magnitude: its decimal digits, as the conversion
/// prints them (the exponent is below the precision, so the fixed form, and nothing after the point survives the
/// trimming). None for any other value.
fn format_whole(v: f64, limit: f64) -> Option<NumText> {
    if v.fract() != 0.0 || v.abs() >= limit {
        return None;
    }
    // Below 10^9 in magnitude, so exact in an `i64`.
    let mut out = NumText::new();
    out.push_bytes(i64_text(v as i64, &mut [0; I64_TEXT]))?;
    Some(out)
}

/// The longest decimal text of an `i64`: `-9223372036854775808`.
pub const I64_TEXT: usize = 20;

/// The decimal text of `value`, written at the end of `buf`: the one integer-to-text routine of the crate (and of the
/// script engine's stacks), which never allocates.
pub fn i64_text(value: i64, buf: &mut [u8; I64_TEXT]) -> &[u8] {
    let mut n = value.unsigned_abs();
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if value < 0 {
        at -= 1;
        buf[at] = b'-';
    }
    &buf[at..]
}

/// Text written on the stack, up to `N` bytes. A write that does not fit is refused, never cut: `push` and `push_str`
/// answer `None` (the fast paths below then give up and the exact one runs), and `write!` gets a `fmt::Error`.
#[derive(Debug, PartialEq, Eq)]
struct StackText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

/// The text of a number as `%g` prints it: at most 16 bytes (`-1.23456789e-300`), so it always fits.
type NumText = StackText<40>;

impl<const N: usize> StackText<N> {
    fn new() -> StackText<N> {
        StackText {
            bytes: [0; N],
            len: 0,
        }
    }

    fn push(&mut self, byte: u8) -> Option<()> {
        self.push_bytes(&[byte])
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> Option<()> {
        let end = self.len.checked_add(bytes.len())?;
        self.bytes.get_mut(self.len..end)?.copy_from_slice(bytes);
        self.len = end;
        Some(())
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl<const N: usize> std::ops::Deref for StackText<N> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<const N: usize> PartialEq<String> for StackText<N> {
    fn eq(&self, other: &String) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl<const N: usize> std::fmt::Write for StackText<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.push_bytes(s.as_bytes()).ok_or(std::fmt::Error)
    }
}

/// [`format_g`] of a finite non-zero value whose shortest round-trip decimal (as an `f32` when the value is one, else as an `f64`)
/// has at most six significant digits: that decimal is then also the value's correct rounding to six digits, since it lies within
/// half a binary unit of the value (at most 2^-24 of it) while any other six-digit decimal lies at least 10^-6 of it away. The
/// rounding then carries nothing, and `%g` prints those digits as they are. None for any other value (the exact path's).
fn format_g_short(v: f64) -> Option<NumText> {
    format_g_shortest(v, 6, true)
}

/// `%.<precision>g` (without flags) of a finite non-zero value whose shortest round-trip decimal has at most `precision`
/// significant digits, for the reason [`format_g_short`] gives: with `narrow`, the decimal of an `f32` when the value is one
/// (half a binary unit is 2^-24 of the value: sound for six digits), else of the `f64` (2^-53: sound up to 15 digits).
fn format_g_shortest(v: f64, precision: usize, narrow: bool) -> Option<NumText> {
    use std::fmt::Write as _;
    // Subnormal binary values have fewer digits than the argument needs.
    if !v.is_normal() {
        return None;
    }
    let mut text = StackText::<48>::new();
    let narrowed = v as f32;
    if narrow && narrowed.is_normal() && f64::from(narrowed) == v {
        write!(text, "{narrowed:e}").ok()?;
    } else {
        write!(text, "{v:e}").ok()?;
    }
    let s = text.as_bytes();
    let (negative, s) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, s),
    };
    let e_at = s.iter().position(|&b| b == b'e')?;
    let (mantissa, exponent) = (&s[..e_at], &s[e_at + 1..]);
    let mut digits = [0u8; 16];
    let mut count = 0usize;
    for &b in mantissa {
        if b == b'.' {
            continue;
        }
        if !b.is_ascii_digit() || count == precision.min(digits.len()) {
            return None;
        }
        digits[count] = b;
        count += 1;
    }
    // The shortest form has no trailing zeros but a lone "0"; strip any to be sure.
    while count > 1 && digits[count - 1] == b'0' {
        count -= 1;
    }
    let exponent: i32 = std::str::from_utf8(exponent).ok()?.parse().ok()?;
    let digits = &digits[..count];
    let mut out = NumText::new();
    if negative {
        out.push(b'-')?;
    }
    if (-4..precision as i32).contains(&exponent) {
        if exponent >= 0 {
            let whole = exponent as usize + 1;
            if count <= whole {
                out.push_bytes(digits)?;
                for _ in count..whole {
                    out.push(b'0')?;
                }
            } else {
                out.push_bytes(&digits[..whole])?;
                out.push(b'.')?;
                out.push_bytes(&digits[whole..])?;
            }
        } else {
            out.push_bytes(b"0.")?;
            for _ in 0..(-exponent - 1) {
                out.push(b'0')?;
            }
            out.push_bytes(digits)?;
        }
    } else {
        out.push_bytes(&digits[..1])?;
        if count > 1 {
            out.push(b'.')?;
            out.push_bytes(&digits[1..])?;
        }
        out.push(b'e')?;
        out.push(if exponent < 0 { b'-' } else { b'+' })?;
        if exponent.unsigned_abs() < 10 {
            out.push(b'0')?;
        }
        out.push_bytes(i64_text(
            i64::from(exponent.unsigned_abs()),
            &mut [0; I64_TEXT],
        ))?;
    }
    Some(out)
}

/// [`format_g`] by the exact decimal expansion (any finite non-zero value).
fn format_g_exact(v: f64) -> String {
    // Rust rounds the exact decimal expansion, like printf, so the exponent after rounding is right.
    let scientific = format!("{v:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if !(-4..6).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim_zeros(mantissa), exponent.abs())
    } else {
        let decimals = (5 - exponent) as usize;
        trim_zeros(&format!("{v:.decimals$}")).to_string()
    }
}

/// `sprintf_double(b"%.9g", v, 31)`: whole numbers and values with a short decimal without the general conversion.
pub fn format_g9(v: f64) -> Vec<u8> {
    with_format_g9(v, <[u8]>::to_vec)
}

/// [`format_g9`] handed to `f`, with no allocation for the values the fast paths cover.
pub fn with_format_g9<R>(v: f64, f: impl FnOnce(&[u8]) -> R) -> R {
    if v != 0.0 && v.is_finite() {
        let fast = format_whole(v, 1e9).or_else(|| format_g_shortest(v, 9, false));
        if let Some(text) = fast {
            return f(text.as_bytes());
        }
    }
    f(&sprintf_double(b"%.9g", v, 31))
}

/// `printf("%d", value)` for a 32-bit word a script holds unsigned: it prints as signed.
pub fn format_int(value: u32) -> String {
    (value as i32).to_string()
}

/// [`format_int`] handed to `f` as bytes, written to the stack.
pub fn with_format_int<R>(value: u32, f: impl FnOnce(&[u8]) -> R) -> R {
    f(i64_text(i64::from(value as i32), &mut [0; I64_TEXT]))
}

/// The script engine's float to unsigned conversion (`(U32)value`): for `-2^63 <= x < 2^63` the value is truncated
/// toward zero and the low 32 bits are kept (so it wraps: `-1` gives `0xFFFFFFFF`); other values, infinities and NaN
/// convert as [`TextRules::f64_to_u32`] says (by default, saturating at the ends of a 64-bit integer, NaN 0).
pub fn f64_to_u32(x: f64) -> u32 {
    // NaN is in no range, so it takes the rules' branch too.
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&x) {
        (x as i64) as u32
    } else {
        rules().f64_to_u32(x)
    }
}

// The standard CRC-32 (zlib's). A script's file CRC (`getFileCRC`) is this value complemented.
pub use torque_bytes::crc32;

/// Torque's `FindMatch::isMatch(pattern, text, false)`, used by `export`,
/// `deleteVariables` and the file searches: `?` is any one byte, other bytes compare ignoring ASCII case,
/// and `*` tries every later position where the byte after it occurs (found case-sensitively, like
/// `strchr`; a `*` at the end takes the rest). The text must not run out before a `*` is reached:
/// `"abc*"` does not match `"abc"`. The plain recursive matcher recurses once per `*` and can take exponential time; this
/// gives the same answer without recursion, in O(`match_cost`) steps and O(text) memory: the stars are
/// handled from the last to the first, each keeping one row (for every text position, whether the rest of
/// the pattern matches from some later position holding the byte after the star), and a run of the
/// plain matching loop that meets a star reads the row of the next star instead of recursing.
pub fn is_match(pattern: &[u8], text: &[u8]) -> bool {
    let e = until_nul(pattern);
    let s = until_nul(text);
    let mut next: Vec<bool> = Vec::new();
    for p in (0..e.len()).rev().filter(|&p| e[p] == b'*') {
        let wanted = byte_at(e, p + 1);
        let mut row = vec![false; s.len() + 2];
        for j in (0..=s.len()).rev() {
            row[j] =
                row[j + 1] || (byte_at(s, j) == wanted && match_segment(e, s, (p + 1, j), &next));
        }
        next = row;
    }
    match_segment(e, s, (0, 0), &next)
}

/// An upper bound of the work [`is_match`] does, in the units `Vm` charges: the pattern, plus for every star
/// one row over the text, each cell of which compares at most the longest run of pattern bytes between
/// stars. Callers charge it BEFORE matching, so a hostile pattern against a long text ends the run at the
/// step limit instead of running.
pub fn match_cost(pattern: &[u8], text_len: usize) -> u64 {
    let e = until_nul(pattern);
    // `longest` is the longest run of pattern bytes after a star (the part a row cell compares); the run
    // before the first star is one pass over the pattern, which the first term covers.
    let (mut stars, mut longest, mut run) = (0u64, 0u64, 0u64);
    for &b in e {
        if b == b'*' {
            stars += 1;
            if stars > 1 {
                longest = longest.max(run);
            }
            run = 0;
        } else {
            run += 1;
        }
    }
    if stars > 0 {
        longest = longest.max(run);
    }
    let row = (text_len as u64).saturating_add(1);
    (e.len() as u64).saturating_add(stars.saturating_mul(row).saturating_mul(longest + 1))
}

fn byte_at(v: &[u8], k: usize) -> u8 {
    v.get(k).copied().unwrap_or(0)
}

/// The plain matching loop from pattern index `i` and text index `j` up to the next star; a star answers from
/// `next`, the row of that star (see [`is_match`]).
fn match_segment(e: &[u8], s: &[u8], (mut i, mut j): (usize, usize), next: &[bool]) -> bool {
    let mut matched = true;
    while matched && byte_at(e, i) != 0 && byte_at(s, j) != 0 {
        match byte_at(e, i) {
            b'*' => return next.get(j).copied().unwrap_or(false),
            b'?' => {}
            c => matched = c.eq_ignore_ascii_case(&byte_at(s, j)),
        }
        i += 1;
        j += 1;
    }
    matched && byte_at(e, i) == byte_at(s, j)
}

/// Torque's `expandEscape`: `"` `\` CR LF TAB and `'` with a backslash, the bytes 1-7,
/// 11, 12 and 14 as `\c0`..`\c9` (`\c0`-`\c6`, `\c7`, `\c8`, `\c9`), 15 as `\cr`, other bytes below 32 as
/// `\xHH` (upper case), everything else unchanged.
pub fn expand_escape(text: &[u8]) -> Vec<u8> {
    expand_escape_capped(text, usize::MAX).unwrap_or_default()
}

/// [`expand_escape`] that gives up (`None`) as soon as the result would pass `cap` bytes, so a script cannot
/// make the interpreter build a result several times the string-size cap before the cap is looked at (every
/// byte can become four).
pub fn expand_escape_capped(text: &[u8], cap: usize) -> Option<Vec<u8>> {
    const REMAP: [u8; 16] = [0, 0, 1, 2, 3, 4, 5, 6, 0, 0, 0, 7, 8, 0, 9, 0];
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let text = until_nul(text);
    let mut out = Vec::with_capacity(text.len());
    for &c in text {
        match c {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\'' => out.extend_from_slice(b"\\'"),
            1..=7 | 11 | 12 | 14 => {
                out.extend_from_slice(b"\\c");
                out.push(b'0' + REMAP[usize::from(c)]);
            }
            15 => out.extend_from_slice(b"\\cr"),
            0..=31 => {
                out.extend_from_slice(b"\\x");
                out.push(HEX[usize::from(c >> 4)]);
                out.push(HEX[usize::from(c & 15)]);
            }
            _ => out.push(c),
        }
        if out.len() > cap {
            return None;
        }
    }
    Some(out)
}

/// The first index at or after `from` where `needle` occurs in `hay`, comparing ASCII letters without case
/// when `fold`. An empty needle is found at `from` (when `from` is inside `hay` or at its end). Linear time
/// (Knuth-Morris-Pratt): naive `strstr` and `strncmp` loops give the same answers but can take
/// time proportional to the product of the lengths, which a script must not be able to ask for.
pub fn find(hay: &[u8], needle: &[u8], from: usize, fold: bool) -> Option<usize> {
    let norm = |b: u8| if fold { b.to_ascii_lowercase() } else { b };
    if needle.is_empty() {
        return (from <= hay.len()).then_some(from);
    }
    let room = hay.len().saturating_sub(from);
    if needle.len() > room {
        return None;
    }
    // fail[i]: the length of the longest proper prefix of needle[..=i] that is also its suffix (`u32`: a
    // needle is at most `MAX_STRING_BYTES` long, and the table is the one allocation that grows with it).
    let mut fail = vec![0u32; needle.len()];
    let mut k = 0usize;
    for (i, &b) in needle.iter().enumerate().skip(1) {
        let b = norm(b);
        while k > 0 && needle.get(k).is_none_or(|&n| norm(n) != b) {
            k = fail.get(k - 1).map_or(0, |&f| f as usize);
        }
        if needle.get(k).is_some_and(|&n| norm(n) == b) {
            k += 1;
        }
        if let Some(slot) = fail.get_mut(i) {
            *slot = u32::try_from(k).unwrap_or(0);
        }
    }
    let mut k = 0usize;
    for (i, &b) in hay.iter().enumerate().skip(from) {
        let b = norm(b);
        while k > 0 && needle.get(k).is_none_or(|&n| norm(n) != b) {
            k = fail.get(k - 1).map_or(0, |&f| f as usize);
        }
        if needle.get(k).is_some_and(|&n| norm(n) == b) {
            k += 1;
        }
        if k == needle.len() {
            return Some((i + 1).saturating_sub(k));
        }
    }
    None
}

/// `(float)x`, rounded to nearest; a NaN keeps its sign, becomes quiet and keeps the top 22 bits of its payload, as
/// IEEE 754 recommends for a conversion between formats (Rust's `as` leaves a NaN's bits unspecified, so they are set
/// here to be the same on every host).
pub fn to_f32(x: f64) -> f32 {
    if x.is_nan() {
        let bits = x.to_bits();
        let sign = ((bits >> 63) as u32) << 31;
        let payload = ((bits >> 29) & 0x003f_ffff) as u32;
        f32::from_bits(sign | 0x7fc0_0000 | payload)
    } else {
        x as f32
    }
}

/// `(double)x`, exact, with the NaN rule of [`to_f32`] the other way round.
pub fn to_f64(x: f32) -> f64 {
    if x.is_nan() {
        let bits = x.to_bits();
        let sign = u64::from(bits >> 31) << 63;
        let payload = u64::from(bits & 0x003f_ffff) << 29;
        f64::from_bits(sign | 0x7ff8_0000_0000_0000 | payload)
    } else {
        f64::from(x)
    }
}

/// C `printf("%.*f", decimals, v)`: the exact binary value rounded to nearest with ties to even, `-0` kept, `inf`,
/// `-inf`, and a NaN as [`format_g`] prints it.
pub fn format_fixed(v: f64, decimals: usize) -> String {
    if v.is_nan() {
        return rules().nan_text(v);
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    format!("{v:.decimals$}")
}

fn is_c_space_byte(b: Option<&u8>) -> bool {
    b.is_some_and(|b| is_c_space(*b))
}

/// The number of bytes at the start of `s` that are ASCII digits.
fn digit_run(s: &[u8]) -> usize {
    s.iter().take_while(|b| b.is_ascii_digit()).count()
}

/// One `%f` conversion of C's `sscanf` from `s[*at..]`: leading white space, a sign, then `inf`/`infinity`,
/// `nan` (read by [`TextRules::scan_nan`]), a hex float (`0x1.8p3`) or a decimal with an optional exponent. The
/// decimal is rounded straight to the nearest float, as `strtof` does (not through a double:
/// `"1.00000005960464477539062500000001"` gives 1.0000001); the hex float goes through [`atof`]'s double
/// (a hex mantissa of more than 53 bits could round twice; no script writes one). As C99 specifies (the input item
/// is the longest prefix of a matching sequence, with one byte of push-back), a form that starts but does not
/// finish fails the whole conversion: `"1e"`, `"1e+"`, `"0x"`, `"infinit"`, `"nan(x"`. None on failure; `*at` is
/// then meaningless.
fn scan_f32(s: &[u8], at: &mut usize) -> Option<f32> {
    while is_c_space_byte(s.get(*at)) {
        *at += 1;
    }
    let negative = s.get(*at) == Some(&b'-');
    if matches!(s.get(*at), Some(b'+' | b'-')) {
        *at += 1;
    }
    let signed = |v: f32| if negative { -v } else { v };
    let rest = s.get(*at..).unwrap_or_default();
    if starts_with_ignore_case(rest, "inf") {
        *at += 3;
        let tail = s.get(*at..).unwrap_or_default();
        if starts_with_ignore_case(tail, "i") {
            if !starts_with_ignore_case(tail, "inity") {
                return None;
            }
            *at += 5;
        }
        return Some(signed(f32::INFINITY));
    }
    if starts_with_ignore_case(rest, "nan") {
        *at += 3;
        let tail = s.get(*at..).unwrap_or_default();
        let (value, taken) = rules().scan_nan(negative, tail)?;
        *at += taken;
        return Some(value);
    }
    if starts_with_ignore_case(rest, "0x") {
        let digits = rest.get(2..).unwrap_or_default();
        let mut j = digits.iter().take_while(|b| b.is_ascii_hexdigit()).count();
        let mut any = j > 0;
        if digits.get(j) == Some(&b'.') {
            let frac = digits
                .get(j + 1..)
                .unwrap_or_default()
                .iter()
                .take_while(|b| b.is_ascii_hexdigit())
                .count();
            any |= frac > 0;
            j += 1 + frac;
        }
        if !any {
            return None;
        }
        if matches!(digits.get(j), Some(b'p' | b'P')) {
            let mut k = j + 1;
            if matches!(digits.get(k), Some(b'+' | b'-')) {
                k += 1;
            }
            let exp = digit_run(digits.get(k..).unwrap_or_default());
            if exp == 0 {
                return None;
            }
            j = k + exp;
        }
        *at += 2 + j;
        return Some(signed(to_f32(atof(rest.get(..2 + j).unwrap_or_default()))));
    }
    let int = digit_run(rest);
    let mut j = int;
    let mut frac = 0;
    if rest.get(j) == Some(&b'.') {
        frac = digit_run(rest.get(j + 1..).unwrap_or_default());
        j += 1 + frac;
    }
    if int + frac == 0 {
        return None;
    }
    if matches!(rest.get(j), Some(b'e' | b'E')) {
        let mut k = j + 1;
        if matches!(rest.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        let exp = digit_run(rest.get(k..).unwrap_or_default());
        if exp == 0 {
            return None;
        }
        j = k + exp;
    }
    *at += j;
    // The scanned text is ASCII digits, '.', 'e' and a sign: Rust parses it correctly rounded.
    let text = std::str::from_utf8(rest.get(..j)?).ok()?;
    Some(signed(text.parse::<f32>().ok()?))
}

/// `sscanf(s, "%f %f ...")` with one `%f` per slot of `out`: fills the slots it converts, leaves the
/// others as they are, and returns how many it converted, or -1 (`EOF`, an input failure as C defines it) when the
/// text ends (or is only white space) before the first one. The text ends at its first NUL.
pub fn scan_floats(s: &[u8], out: &mut [f32]) -> i32 {
    let s = until_nul(s);
    let mut at = 0;
    let mut count = 0;
    for slot in out.iter_mut() {
        let mut cursor = at;
        while is_c_space_byte(s.get(cursor)) {
            cursor += 1;
        }
        if cursor >= s.len() {
            return if count == 0 { -1 } else { count };
        }
        match scan_f32(s, &mut at) {
            Some(v) => {
                *slot = v;
                count += 1;
            }
            None => return count,
        }
    }
    count
}

/// The byte value of a hex digit, either case.
fn hex(b: Option<&u8>) -> Option<u8> {
    let d = char::from(*b?).to_digit(16)?;
    u8::try_from(d).ok()
}

/// The colour code bytes of `\c0`..`\c9` (the inverse of `expandEscape`'s table).
const COLOR_CODES: [u8; 10] = [1, 2, 3, 4, 5, 6, 7, 11, 12, 14];

/// `collapseEscape` (reading `expandEscape`'s escapes back): `\n` `\r` `\t`, `\xHH`, `\c0`..`\c9`,
/// `\cr` `\cp` `\co`, and a backslash before any other byte gives that byte. Quirks: a malformed `\x` or `\c` stops the
/// decoding there (the rest is kept as written); `\x00` ends the string; `\c0` as the very first thing of the result becomes the two bytes 2 and 1; a backslash
/// at the end is dropped. Torque decodes in place with a `memmove` per escape; this is one pass.
pub fn collapse_escape(s: &[u8]) -> Vec<u8> {
    collapse_escape_checked(s).0
}

/// [`collapse_escape`] and whether every escape was well formed. The compiler's scanner runs the
/// same routine on every string literal and refuses a literal whose `\x` or `\c` escape is malformed.
pub fn collapse_escape_checked(s: &[u8]) -> (Vec<u8>, bool) {
    let s = until_nul(s);
    let mut out = Vec::with_capacity(s.len());
    // After `\x00` the result has ended (a C string), but the in-place decoder goes on, so a malformed
    // escape later still makes the call report failure.
    let mut ended = false;
    let mut i = 0;
    while let Some(&byte) = s.get(i) {
        if byte != b'\\' {
            if !ended {
                out.push(byte);
            }
            i += 1;
            continue;
        }
        // The rest of the text from this backslash, kept as written where decoding stops.
        let malformed = s.get(i..).unwrap_or_default();
        match s.get(i + 1).copied().unwrap_or(0) {
            0 => break,
            b'x' => {
                let (Some(high), Some(low)) = (hex(s.get(i + 2)), hex(s.get(i + 3))) else {
                    if !ended {
                        out.extend_from_slice(malformed);
                    }
                    return (out, false);
                };
                let byte = (high << 4) | low;
                if byte == 0 {
                    ended = true;
                } else if !ended {
                    out.push(byte);
                }
                i += 4;
            }
            b'c' => {
                let code = match s.get(i + 2) {
                    Some(b'r') => 0x0f,
                    Some(b'p') => 0x10,
                    Some(b'o') => 0x11,
                    Some(d @ b'0'..=b'9') => COLOR_CODES
                        .get(usize::from(d.saturating_sub(b'0')))
                        .copied()
                        .unwrap_or(1),
                    _ => {
                        if !ended {
                            out.extend_from_slice(malformed);
                        }
                        return (out, false);
                    }
                };
                if !ended {
                    if code == 1 && out.is_empty() {
                        out.extend_from_slice(&[2, 1]);
                    } else {
                        out.push(code);
                    }
                }
                i += 3;
            }
            c => {
                if !ended {
                    out.push(match c {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        other => other,
                    });
                }
                i += 2;
            }
        }
    }
    (out, true)
}

/// One conversion of a C format string with its flags, width and precision.
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    zero: bool,
    alt: bool,
    width: usize,
    precision: Option<usize>,
}

/// The digits of `%e` for the finite `v`: `d.ddde±XX` with `prec` decimals (an exponent of at least two digits).
fn format_exp(v: f64, prec: usize, upper: bool, alt: bool) -> String {
    let s = format!("{:.*e}", prec, v.abs());
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let mut m = mantissa.to_string();
    if alt && prec == 0 {
        m.push('.');
    }
    let e = if upper { 'E' } else { 'e' };
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{m}{e}{sign}{:02}", exp.abs())
}

/// `%g` of the finite, non-negative `v` with `prec` significant digits (0 is 1).
fn format_general(v: f64, prec: usize, upper: bool, alt: bool) -> String {
    let p = prec.max(1);
    if v == 0.0 {
        return if alt {
            format!("{:.*}", p - 1, 0.0)
        } else {
            "0".to_string()
        };
    }
    let sci = format!("{:.*e}", p - 1, v);
    let exp: i32 = sci
        .split_once('e')
        .and_then(|(_, e)| e.parse().ok())
        .unwrap_or(0);
    let out = if exp < -4 || exp >= p as i32 {
        format_exp(v, p - 1, upper, alt)
    } else {
        format!("{:.*}", (p as i32 - 1 - exp).max(0) as usize, v)
    };
    if alt {
        return out;
    }
    match out.find(['e', 'E']) {
        Some(at) => format!("{}{}", trim_zeros(&out[..at]), &out[at..]),
        None => trim_zeros(&out).to_string(),
    }
}

/// C `sprintf` of a script-supplied format string with ONE double argument, at most `cap` bytes: the text and `%%`
/// copied, the first conversion of the double (`f`, `e`, `g` and their capitals), with the flags `-`, `+`, space,
/// `0`, `#`, a width and a precision. The integer conversions `d`, `i`, `u`, `x`, `X`, `o`, `c` print the word
/// [`TextRules::double_as_word`] gives (nothing by default). A conversion with `*`, any conversion after the first (it
/// has no argument) and `%s`, `%p`, `%n` print nothing.
pub fn sprintf_double(fmt: &[u8], v: f64, cap: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut used = false;
    let mut i = 0;
    while i < fmt.len() && out.len() < cap {
        let b = fmt[i];
        if b != b'%' {
            out.push(b);
            i += 1;
            continue;
        }
        i += 1;
        if fmt.get(i) == Some(&b'%') {
            out.push(b'%');
            i += 1;
            continue;
        }
        let mut spec = Spec {
            left: false,
            plus: false,
            space: false,
            zero: false,
            alt: false,
            width: 0,
            precision: None,
        };
        while let Some(&f) = fmt.get(i) {
            match f {
                b'-' => spec.left = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'0' => spec.zero = true,
                b'#' => spec.alt = true,
                _ => break,
            }
            i += 1;
        }
        let mut star = false;
        while let Some(&d) = fmt.get(i) {
            if d.is_ascii_digit() {
                spec.width = (spec.width * 10 + usize::from(d - b'0')).min(1 << 16);
            } else if d == b'*' {
                star = true;
            } else {
                break;
            }
            i += 1;
        }
        if fmt.get(i) == Some(&b'.') {
            i += 1;
            let mut p = 0usize;
            while let Some(&d) = fmt.get(i) {
                if d.is_ascii_digit() {
                    p = (p * 10 + usize::from(d - b'0')).min(1 << 16);
                } else if d == b'*' {
                    star = true;
                } else {
                    break;
                }
                i += 1;
            }
            spec.precision = Some(p);
        }
        // Length modifiers are read and ignored.
        while matches!(fmt.get(i), Some(b'h' | b'l' | b'L' | b'I' | b'q' | b'w')) {
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        if used || star {
            continue;
        }
        used = true;
        let word = || rules().double_as_word(v);
        let sign_of = |neg: bool| {
            if neg {
                "-"
            } else if spec.plus {
                "+"
            } else if spec.space {
                " "
            } else {
                ""
            }
        };
        let (sign, body): (&str, String) = match conv {
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let nan = if v.is_nan() {
                    rules().nan_text(v)
                } else {
                    String::new()
                };
                let neg = if v.is_nan() {
                    nan.starts_with('-')
                } else {
                    v.is_sign_negative()
                };
                let a = v.abs();
                let body = if v.is_nan() {
                    nan.trim_start_matches('-').to_string()
                } else if v.is_infinite() {
                    "inf".to_string()
                } else {
                    let p = spec.precision.unwrap_or(6);
                    match conv {
                        b'f' | b'F' => {
                            let mut s = format_fixed(a, p);
                            if spec.alt && p == 0 {
                                s.push('.');
                            }
                            s
                        }
                        b'e' | b'E' => format_exp(a, p, conv == b'E', spec.alt),
                        _ => format_general(a, p, conv == b'G', spec.alt),
                    }
                };
                (sign_of(neg), body)
            }
            b'd' | b'i' | b'u' | b'x' | b'X' | b'o' | b'c' => {
                let Some(bits) = word() else { continue };
                match conv {
                    b'd' | b'i' => {
                        let n = bits as i32;
                        (sign_of(n < 0), n.unsigned_abs().to_string())
                    }
                    b'u' => ("", bits.to_string()),
                    b'x' => ("", format!("{bits:x}")),
                    b'X' => ("", format!("{bits:X}")),
                    b'o' => ("", format!("{bits:o}")),
                    _ => ("", char::from(bits as u8).to_string()),
                }
            }
            _ => continue,
        };
        let len = sign.len() + body.len();
        let pad = spec.width.saturating_sub(len);
        let zero_pad = spec.zero && conv != b'c' && v.is_finite();
        if spec.left {
            out.extend_from_slice(sign.as_bytes());
            out.extend_from_slice(body.as_bytes());
            out.extend(std::iter::repeat_n(b' ', pad));
        } else if zero_pad {
            out.extend_from_slice(sign.as_bytes());
            out.extend(std::iter::repeat_n(b'0', pad));
            out.extend_from_slice(body.as_bytes());
        } else {
            out.extend(std::iter::repeat_n(b' ', pad));
            out.extend_from_slice(sign.as_bytes());
            out.extend_from_slice(body.as_bytes());
        }
    }
    out.truncate(cap);
    out
}

#[cfg(test)]
mod atof_tests {
    use super::{atof, decimal_fast};

    fn general(text: &str) -> f64 {
        text.parse::<f64>().unwrap()
    }

    /// The fast path gives the general parser's double wherever it answers: typical stored numbers, random digit strings with
    /// points and exponents, and the edges of its range.
    #[test]
    fn the_fast_path_agrees_with_the_general_parser() {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            s.wrapping_mul(0x2545_f491_4f6c_dd1d)
        };
        let mut checked = 0;
        for _ in 0..300_000 {
            let r = next();
            let digits = 1 + (r % 17) as usize;
            let mut text = String::new();
            let mut v = next();
            for k in 0..digits {
                if k == (r >> 8) as usize % (digits + 1) {
                    text.push('.');
                }
                text.push(char::from(b'0' + (v % 10) as u8));
                v /= 10;
                if v == 0 {
                    v = next();
                }
            }
            if r >> 20 & 1 == 1 {
                text.push_str(&format!("e{}", (r >> 24) as i64 % 30 - 15));
            }
            if text.starts_with('.') && text.len() == 1 {
                continue;
            }
            if let Some(fast) = decimal_fast(text.as_bytes()) {
                assert_eq!(fast.to_bits(), general(&text).to_bits(), "{text}");
                checked += 1;
            }
        }
        assert!(checked > 100_000, "{checked}");
        for text in [
            "0",
            "0.0",
            "12.5",
            "0.6",
            "-3.2",
            "1e22",
            "1e-22",
            "123456789012345",
            "1234567890123456",
            "0.000000000000001",
            "999999999999999e22",
            "1e23",
            "4.9e-324",
            "1.7976931348623157e308",
            "00000000000000000001",
            "1.",
            ".5",
        ] {
            let trimmed = text.trim_start_matches('-');
            if let Some(fast) = decimal_fast(trimmed.as_bytes()) {
                assert_eq!(fast.to_bits(), general(trimmed).to_bits(), "{text}");
            }
        }
        assert_eq!(atof(b"-0"), -0.0);
        assert!(atof(b"-0").is_sign_negative());
        assert_eq!(atof(b"  12.5 0 0"), 12.5);
        assert_eq!(atof(b"1e400"), f64::INFINITY);
    }
}

#[cfg(test)]
mod format_g_tests {
    use super::{format_g_exact, format_g_short};

    /// A small deterministic generator (xorshift64*).
    fn next(state: &mut u64) -> u64 {
        *state ^= *state >> 12;
        *state ^= *state << 25;
        *state ^= *state >> 27;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn same(v: f64) {
        if !v.is_finite() || v == 0.0 {
            return;
        }
        if let Some(short) = format_g_short(v) {
            assert_eq!(short, format_g_exact(v), "{v:e} ({:#x})", v.to_bits());
        }
    }

    /// The short path gives the exact path's text wherever it answers: the decimal grids stored numbers usually sit on, every float's bits
    /// at random, every double's bits at random, and the decade edges.
    #[test]
    fn the_short_path_agrees_with_the_exact_one() {
        for k in -100_000i32..100_000 {
            for scale in [0.1f64, 0.5, 0.2, 0.25, 0.01, 1.0, 1000.0, 0.001] {
                let x = f64::from(k) * scale;
                same(x);
                same(f64::from(x as f32));
            }
        }
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..500_000 {
            let r = next(&mut s);
            same(f64::from(f32::from_bits(r as u32)));
            same(f64::from_bits(r));
            // Few digits: a small integer over a power of ten.
            let m = (r % 1_000_000) as f64;
            let e = ((r >> 32) % 40) as i32 - 20;
            same(m * 10f64.powi(e));
            same(f64::from((m * 10f64.powi(e)) as f32));
        }
        for e in -40..40 {
            let p = 10f64.powi(e);
            for x in [
                p,
                p * 0.999_999,
                p * 0.999_999_5,
                p * 1.000_001,
                p * 9.999_99,
            ] {
                same(x);
                same(-x);
                same(f64::from(x as f32));
            }
        }
    }

    #[test]
    fn the_short_path_answers_for_a_saves_numbers() {
        for (v, text) in [
            (12.5, "12.5"),
            (f64::from(0.6f32), "0.6"),
            (f64::from(-3.2f32), "-3.2"),
            (100000.0, "100000"),
            (1e6, "1e+06"),
            (0.0001, "0.0001"),
            (1e-5, "1e-05"),
            (f64::from(1.570_796_4f32), "1.5708"),
        ] {
            let short = format_g_short(v);
            assert!(short.is_none_or(|s| s.as_bytes() == text.as_bytes()), "{v}");
            assert_eq!(format_g_exact(v), text);
        }
        assert_eq!(format_g_short(12.5).as_deref(), Some(&b"12.5"[..]));
        assert_eq!(
            format_g_short(f64::from(0.6f32)).as_deref(),
            Some(&b"0.6"[..])
        );
    }
}

#[cfg(test)]
mod sprintf_tests {
    use super::sprintf_double;

    #[test]
    fn one_double_through_a_c_format() {
        let f =
            |fmt: &str, v: f64| String::from_utf8(sprintf_double(fmt.as_bytes(), v, 19)).unwrap();
        assert_eq!(f("%3.2f", 3.0), "3.00");
        assert_eq!(f("%3.2f", 10.0), "10.00");
        assert_eq!(f("%3.0f", 2.0), "  2");
        assert_eq!(f("%-6.1f|", 1.0), "1.0   |");
        assert_eq!(f("%+08.3f", -1.5), "-001.500");
        assert_eq!(f("%e", 12345.0), "1.234500e+04");
        assert_eq!(f("%g", 0.0001), "0.0001");
        assert_eq!(f("%g", 1e-5), "1e-05");
        assert_eq!(f("%.3g", 1234.5), "1.23e+03");
        assert_eq!(f("v=%f %f", 1.0), "v=1.000000 ");
        assert_eq!(f("%%%5.1f%%", 7.0), "%  7.0%");
        assert_eq!(
            f("%f", 123456789012.0),
            "123456789012.000000"[..19].to_string()
        );
        assert_eq!(f("abc", 1.0), "abc");
        assert_eq!(f("%s|%d", 1.0), "|");
        assert_eq!(f("[%d]", 1.0), "[]");
        assert_eq!(f("[%x %f]", 1.0), "[ ]");
        assert_eq!(f("%f", f64::NAN), "nan");
        assert_eq!(f("%5f|", -f64::NAN), " -nan|");
    }
}

/// `qsort(v, cmp)` for the script engine's list sorts: the order [`TextRules::sort`] gives (by default a stable sort, so
/// equal keys keep their order). `cmp` is a `qsort` comparator (negative, zero or positive).
pub fn sort_by<T>(v: &mut [T], mut cmp: impl FnMut(&T, &T) -> i32) {
    if v.len() < 2 {
        return;
    }
    let mut order: Vec<usize> = (0..v.len()).collect();
    rules().sort(&mut order, &mut |a, b| match (v.get(a), v.get(b)) {
        (Some(x), Some(y)) => cmp(x, y),
        _ => 0,
    });
    // Apply the permutation: `order[k]` is the index of the element that ends at `k`.
    let mut place: Vec<usize> = vec![0; v.len()];
    for (k, &from) in order.iter().enumerate() {
        if let Some(p) = place.get_mut(from) {
            *p = k;
        }
    }
    for i in 0..v.len() {
        while let Some(&to) = place.get(i).filter(|&&to| to != i) {
            v.swap(i, to);
            place.swap(i, to);
        }
    }
}

#[cfg(test)]
mod default_rules_tests {
    use super::{atof, f64_to_u32, format_fixed, format_g, scan_floats, sort_by};

    #[test]
    fn nan_reads_and_prints_with_its_sign() {
        for (text, negative) in [
            ("nan", false),
            ("NaN", false),
            ("-nan", true),
            ("+nan", false),
            (" -NAN(abc_1)", true),
            ("nan(ind)", false),
        ] {
            let v = atof(text.as_bytes());
            assert!(v.is_nan(), "{text}");
            assert_eq!(v.is_sign_negative(), negative, "{text}");
            assert_eq!(format_g(v), if negative { "-nan" } else { "nan" }, "{text}");
            assert_eq!(format_fixed(v, 2), format_g(v), "{text}");
        }
        assert_eq!(format_g(f64::NAN), "nan");
        assert_eq!(format_g(-f64::NAN), "-nan");
        assert_eq!(format_g(f64::INFINITY), "inf");
        assert_eq!(format_g(f64::NEG_INFINITY), "-inf");
    }

    #[test]
    fn scanf_takes_c99_nan_forms() {
        let mut out = [0.0f32; 3];
        assert_eq!(scan_floats(b"nan -nan(x_1) 2", &mut out), 3);
        assert!(out[0].is_nan() && out[0].is_sign_positive());
        assert!(out[1].is_nan() && out[1].is_sign_negative());
        assert_eq!(out[2], 2.0);
        let mut out = [7.0f32; 2];
        assert_eq!(scan_floats(b"1 nan(x", &mut out), 1);
        assert_eq!(out, [1.0, 7.0]);
        assert_eq!(scan_floats(b"1e", &mut out), 0);
        assert_eq!(scan_floats(b"   ", &mut out), -1);
    }

    #[test]
    fn out_of_range_floats_saturate_to_64_bits_then_wrap_to_32() {
        assert_eq!(f64_to_u32(3.9), 3);
        assert_eq!(f64_to_u32(-1.0), 0xFFFF_FFFF);
        assert_eq!(f64_to_u32(4_294_967_297.0), 1);
        assert_eq!(f64_to_u32(1e30), 0xFFFF_FFFF);
        assert_eq!(f64_to_u32(f64::INFINITY), 0xFFFF_FFFF);
        assert_eq!(f64_to_u32(-1e30), 0);
        assert_eq!(f64_to_u32(f64::NEG_INFINITY), 0);
        assert_eq!(f64_to_u32(f64::NAN), 0);
    }

    #[test]
    fn the_sort_is_stable() {
        let mut rows = vec![(1, "b"), (2, "a"), (3, "b"), (4, "a"), (5, "c")];
        sort_by(&mut rows, |a, b| a.1.cmp(b.1) as i32);
        assert_eq!(
            rows.iter().map(|r| r.0).collect::<Vec<_>>(),
            [2, 4, 1, 3, 5]
        );
        sort_by(&mut rows, |a, b| -(a.1.cmp(b.1) as i32));
        assert_eq!(
            rows.iter().map(|r| r.0).collect::<Vec<_>>(),
            [5, 1, 3, 2, 4]
        );
        let mut v: Vec<i32> = (0..500).map(|i| (i * 7919) % 503).collect();
        sort_by(&mut v, |a, b| (*a - *b).signum());
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
        let mut w: Vec<i32> = vec![5; 40];
        sort_by(&mut w, |a, b| (*a - *b).signum());
        assert!(w.iter().all(|x| *x == 5));
    }

    type Cmp<'a> = &'a mut dyn FnMut(&f32, &f32) -> i32;

    /// A comparator that is no order (NaN keys, random answers, "always less") ends with a permutation, never a panic.
    #[test]
    fn a_comparator_that_is_no_order_never_panics() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for len in [0usize, 1, 2, 3, 7, 8, 9, 33, 100, 1000] {
            let keys: Vec<f32> = (0..len)
                .map(|i| {
                    if i % 3 == 0 {
                        f32::NAN
                    } else {
                        (i * 7919 % 101) as f32
                    }
                })
                .collect();
            let nan_cmp = |a: &f32, b: &f32| {
                let d = a - b;
                if 0.0 <= d { i32::from(0.0 < d) } else { -1 }
            };
            let mut random = |_: &f32, _: &f32| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 3) as i32 - 1
            };
            let comparators: [Cmp<'_>; 3] =
                [&mut { nan_cmp }, &mut random, &mut |_: &f32, _: &f32| -1];
            for cmp in comparators {
                let mut v = keys.clone();
                sort_by(&mut v, |a, b| cmp(a, b));
                let mut got: Vec<u32> = v.iter().map(|x| x.to_bits()).collect();
                let mut want: Vec<u32> = keys.iter().map(|x| x.to_bits()).collect();
                got.sort_unstable();
                want.sort_unstable();
                assert_eq!(got, want, "{len}");
            }
        }
    }
}

#[cfg(test)]
mod format_fast_tests {
    use super::{format_g, format_g_exact, format_g9, format_whole, sprintf_double};

    /// `%.9g`'s fast paths print what the general conversion prints: whole numbers, short decimals, `f32` values, random
    /// bit patterns, and the edges of the fixed and exponent forms.
    #[test]
    fn the_nine_digit_conversion_agrees_with_sprintf() {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            s.wrapping_mul(0x2545_f491_4f6c_dd1d)
        };
        let mut values: Vec<f64> = vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.1,
            0.5,
            1e9,
            -1e9,
            999_999_999.0,
            999_999_999.5,
            1e-4,
            1e-5,
            0.000_123_456_789,
            123_456_789.0,
            1_234_567_891.0,
            1e300,
            -1e-300,
            5e-324,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::NAN,
            -f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.3,
            1.0 / 3.0,
            2.5,
            1e8,
            1e10,
            12_345.678_9,
        ];
        for _ in 0..40_000 {
            let r = next();
            values.push(f64::from_bits(r));
            values.push((r >> 34) as f64 - (1u64 << 29) as f64);
            values.push(f64::from(f32::from_bits((r >> 32) as u32)));
            let digits = (r % 1_000_000_000) as f64;
            let scale = 10f64.powi((r >> 40) as i32 % 30 - 15);
            values.push(digits * scale);
            values.push(digits / 10f64.powi((r >> 50) as i32 % 12));
        }
        for v in values {
            assert_eq!(
                format_g9(v),
                sprintf_double(b"%.9g", v, 31),
                "{v:e} ({:#x})",
                v.to_bits()
            );
        }
    }

    /// The whole-number path prints what the exact `%g` conversion prints, on both sides of its limit.
    #[test]
    fn whole_numbers_print_as_the_exact_conversion_does() {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut values: Vec<f64> = vec![
            1.0, -1.0, 9.0, 10.0, 99_999.0, 100_000.0, 999_999.0, -999_999.0, 1e6, -1e6, 123_456.0,
        ];
        for _ in 0..20_000 {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            let n = (s.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as i64 - (1 << 23);
            values.push(n as f64);
        }
        for v in values {
            if v == 0.0 {
                continue;
            }
            assert_eq!(format_g(v), format_g_exact(v), "{v}");
            if let Some(whole) = format_whole(v, 1e6) {
                assert_eq!(whole, format_g_exact(v), "{v}");
            }
        }
        assert_eq!(format_whole(1e6, 1e6), None);
        assert_eq!(format_whole(0.5, 1e6), None);
        assert_eq!(format_whole(f64::NAN, 1e6), None);
    }
}

#[cfg(test)]
mod stack_text_tests {
    use super::{I64_TEXT, i64_text, with_format_g, with_format_g9, with_format_int};

    fn g(v: f64) -> String {
        with_format_g(v, |t| String::from_utf8(t.to_vec()).unwrap())
    }

    fn g9(v: f64) -> String {
        with_format_g9(v, |t| String::from_utf8(t.to_vec()).unwrap())
    }

    fn int(v: u32) -> String {
        with_format_int(v, |t| String::from_utf8(t.to_vec()).unwrap())
    }

    /// The expected texts are what C's `%g` and `%.9g` print, written out by hand.
    #[test]
    fn the_stack_forms_print_what_printf_does() {
        for (v, six, nine) in [
            (0.0, "0", "0"),
            (-0.0, "-0", "-0"),
            (1.0, "1", "1"),
            (-1.0, "-1", "-1"),
            (0.5, "0.5", "0.5"),
            (0.1, "0.1", "0.1"),
            (-2.5, "-2.5", "-2.5"),
            (12.5, "12.5", "12.5"),
            (100_000.0, "100000", "100000"),
            (123_456.0, "123456", "123456"),
            (999_999.0, "999999", "999999"),
            (1e6, "1e+06", "1000000"),
            (1_234_567.0, "1.23457e+06", "1234567"),
            (123_456_789.0, "1.23457e+08", "123456789"),
            (1e9, "1e+09", "1e+09"),
            (1_234_567_890.0, "1.23457e+09", "1.23456789e+09"),
            (0.0001, "0.0001", "0.0001"),
            (0.000_123_456_789, "0.000123457", "0.000123456789"),
            (0.00001, "1e-05", "1e-05"),
            (1e-7, "1e-07", "1e-07"),
            (1.5e-7, "1.5e-07", "1.5e-07"),
            (1e21, "1e+21", "1e+21"),
            (1e100, "1e+100", "1e+100"),
            (1e300, "1e+300", "1e+300"),
            (1.5e-300, "1.5e-300", "1.5e-300"),
            (-1.234_567_89e-300, "-1.23457e-300", "-1.23456789e-300"),
            (f64::MAX, "1.79769e+308", "1.79769313e+308"),
            (f64::MIN_POSITIVE, "2.22507e-308", "2.22507386e-308"),
            (f64::from(1.1f32), "1.1", "1.10000002"),
            (0.123_456_789_012_345, "0.123457", "0.123456789"),
            (f64::INFINITY, "inf", "inf"),
            (f64::NEG_INFINITY, "-inf", "-inf"),
            (f64::NAN, "nan", "nan"),
        ] {
            assert_eq!(g(v), six, "%g of {v:e}");
            assert_eq!(g9(v), nine, "%.9g of {v:e}");
        }
    }

    #[test]
    fn integers_print_as_signed_words() {
        for (v, text) in [
            (0, "0"),
            (1, "1"),
            (7, "7"),
            (255, "255"),
            (65_535, "65535"),
            (i32::MAX as u32, "2147483647"),
            (i32::MIN as u32, "-2147483648"),
            (u32::MAX, "-1"),
            (4_000_000_000, "-294967296"),
        ] {
            assert_eq!(int(v), text);
        }
        for (v, text) in [
            (i64::MIN, "-9223372036854775808"),
            (i64::MAX, "9223372036854775807"),
            (0, "0"),
            (-10, "-10"),
        ] {
            assert_eq!(i64_text(v, &mut [0; I64_TEXT]), text.as_bytes());
        }
    }
}
