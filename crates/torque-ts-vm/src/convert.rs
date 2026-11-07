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
//! A typed native's parameters read their arguments through [`ScriptArg`] (one argument for each [`FromScript`] type,
//! none or one for `Option<T>`, the rest for [`Rest`]) and its result goes back through [`ScriptReturn`] (an
//! [`IntoScript`] value, or `Result<T, ScriptError>`, whose error is printed).
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
    /// A required argument was not passed (the interpreter checks the count first, so only a host's own call that skips
    /// that check meets this).
    Missing,
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::NotUtf8 => f.write_str("the argument is not valid UTF-8 text"),
            ConvertError::Missing => f.write_str("a required argument is missing"),
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

impl<'a> FromScript<'a> for &'a [u8] {
    /// The bytes up to the first NUL, as they are.
    fn from_script(arg: &'a [u8]) -> Result<Self, ConvertError> {
        Ok(until_nul(arg))
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

impl IntoScript for Native {
    /// The result as it is: for a native that picks the kind itself (or fails with [`Native::Fail`]).
    fn into_script(self) -> Native {
        self
    }
}

/// How a typed native's parameter takes script arguments: what its argument counts and its usage text are made of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgKind {
    /// One argument the script must pass.
    Required,
    /// One argument the script may leave out (`Option<T>`): only after the required ones.
    Optional,
    /// Every argument left ([`Rest`]): the last one.
    Rest,
}

/// A parameter of a typed native that reads script arguments: one ([`FromScript`] types), one or none (`Option<T>`), or
/// the rest ([`Rest`]).
///
/// `Item<'a>` is the type with the arguments' lifetime: `&str` borrows the argument's text. The engine's natives take
/// these beside injected parameters; the argument counts come from the kinds in order.
pub trait ScriptArg {
    /// The parameter's type, borrowing the arguments for `'a`.
    type Item<'a>;
    /// How many arguments it takes.
    const KIND: ArgKind;
    /// Takes its argument (or arguments) from the front of `args`, the ones not taken yet.
    fn take<'a>(args: &mut &'a [Vec<u8>]) -> Result<Self::Item<'a>, ConvertError>;
}

/// The front argument of `args`, taken off.
fn next<'a>(args: &mut &'a [Vec<u8>]) -> Option<&'a [u8]> {
    let (first, rest) = args.split_first()?;
    *args = rest;
    Some(first)
}

macro_rules! owned_script_args {
    ($($ty:ty),* $(,)?) => {
        $(
            impl ScriptArg for $ty {
                type Item<'a> = $ty;
                const KIND: ArgKind = ArgKind::Required;
                fn take(args: &mut &[Vec<u8>]) -> Result<$ty, ConvertError> {
                    <$ty>::from_script(next(args).ok_or(ConvertError::Missing)?)
                }
            }
        )*
    };
}

owned_script_args!(i32, u32, f32, bool, String);

impl ScriptArg for &str {
    type Item<'a> = &'a str;
    const KIND: ArgKind = ArgKind::Required;
    fn take<'a>(args: &mut &'a [Vec<u8>]) -> Result<&'a str, ConvertError> {
        <&str>::from_script(next(args).ok_or(ConvertError::Missing)?)
    }
}

impl ScriptArg for &[u8] {
    type Item<'a> = &'a [u8];
    const KIND: ArgKind = ArgKind::Required;
    fn take<'a>(args: &mut &'a [Vec<u8>]) -> Result<&'a [u8], ConvertError> {
        <&[u8]>::from_script(next(args).ok_or(ConvertError::Missing)?)
    }
}

impl ScriptArg for Cow<'_, str> {
    type Item<'a> = Cow<'a, str>;
    const KIND: ArgKind = ArgKind::Required;
    fn take<'a>(args: &mut &'a [Vec<u8>]) -> Result<Cow<'a, str>, ConvertError> {
        Cow::from_script(next(args).ok_or(ConvertError::Missing)?)
    }
}

impl<T: ScriptArg> ScriptArg for Option<T> {
    type Item<'a> = Option<T::Item<'a>>;
    const KIND: ArgKind = match T::KIND {
        ArgKind::Required => ArgKind::Optional,
        _ => panic!(
            "`Option<T>` takes a type that reads one argument, not another `Option` or `Rest`"
        ),
    };
    fn take<'a>(args: &mut &'a [Vec<u8>]) -> Result<Self::Item<'a>, ConvertError> {
        if args.is_empty() {
            return Ok(None);
        }
        T::take(args).map(Some)
    }
}

/// The arguments left after the others, for a native that takes any number (`schedule(delay, method, args...)`): the
/// last script parameter. Each is the argument's bytes as passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rest<'a>(&'a [Vec<u8>]);

impl<'a> Rest<'a> {
    /// The arguments in `args`.
    pub fn new(args: &'a [Vec<u8>]) -> Rest<'a> {
        Rest(args)
    }

    /// How many arguments there are.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The argument at `index`, as passed.
    pub fn get(&self, index: usize) -> Option<&'a [u8]> {
        self.0.get(index).map(Vec::as_slice)
    }

    /// Each argument, as passed.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &'a [u8]> + 'a {
        self.0.iter().map(Vec::as_slice)
    }

    /// Each argument read as `T` (the rules of [`FromScript`]).
    pub fn parse<T: FromScript<'a>>(&self) -> Result<Vec<T>, ConvertError> {
        self.iter().map(T::from_script).collect()
    }
}

impl ScriptArg for Rest<'_> {
    type Item<'a> = Rest<'a>;
    const KIND: ArgKind = ArgKind::Rest;
    fn take<'a>(args: &mut &'a [Vec<u8>]) -> Result<Rest<'a>, ConvertError> {
        Ok(Rest(std::mem::take(args)))
    }
}

/// A typed native's failure: printed as a console error (with the native's name and usage), and the call returns the
/// empty string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError(String);

impl ScriptError {
    /// An error saying `message`.
    pub fn new(message: impl Into<String>) -> ScriptError {
        ScriptError(message.into())
    }

    /// What it says.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScriptError {}

impl From<ConvertError> for ScriptError {
    fn from(error: ConvertError) -> ScriptError {
        ScriptError(error.to_string())
    }
}

/// What a typed native returns: a value ([`IntoScript`]) or `Result<T, ScriptError>`, whose error is printed.
pub trait ScriptReturn {
    /// The result for the interpreter, or the error to print.
    fn into_result(self) -> Result<Native, ScriptError>;
}

impl<T: IntoScript> ScriptReturn for T {
    fn into_result(self) -> Result<Native, ScriptError> {
        Ok(self.into_script())
    }
}

impl<T: IntoScript> ScriptReturn for Result<T, ScriptError> {
    fn into_result(self) -> Result<Native, ScriptError> {
        self.map(IntoScript::into_script)
    }
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
