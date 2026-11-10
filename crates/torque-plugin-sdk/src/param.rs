//! What a native's parameters are: script arguments, read through the engine by its own rules, or [`Call`], the call
//! itself (the engine's console, globals and the call's object).

use std::fmt;

use crate::Call;

/// How a parameter takes script arguments (the same three kinds the engine's natives have).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgKind {
    /// One argument the script must pass.
    Required,
    /// One argument the script may leave out (`Option<T>`): only after the required ones.
    Optional,
    /// Every argument left ([`Rest`]): the last one.
    Rest,
}

impl ArgKind {
    /// The kind's code in the ABI (`TORQUE_ARG_`).
    pub const fn code(self) -> u32 {
        match self {
            ArgKind::Required => torque_plugin_abi::TORQUE_ARG_REQUIRED,
            ArgKind::Optional => torque_plugin_abi::TORQUE_ARG_OPTIONAL,
            ArgKind::Rest => torque_plugin_abi::TORQUE_ARG_REST,
        }
    }
}

/// What a parameter of a native is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamKind {
    /// Something the engine hands over, invisible to scripts ([`Call`], [`This`]).
    Injected,
    /// Script arguments.
    Script(ArgKind),
}

/// Why a native's signature cannot run, or `None`: script arguments go required first, then optional, then one rest
/// last. The `#[function]` attribute checks it when the crate is compiled.
pub const fn native_shape_error(kinds: &[ParamKind]) -> Option<&'static str> {
    // 0: required, 1: optional, 2: after the rest.
    let mut at = 0;
    let mut i = 0;
    while i < kinds.len() {
        if let ParamKind::Script(kind) = kinds[i] {
            let next = match kind {
                ArgKind::Required => 0,
                ArgKind::Optional => 1,
                ArgKind::Rest => 2,
            };
            if at == 2 || next < at {
                return Some(
                    "a native's script arguments go required first, then `Option`s, then one `Rest` last",
                );
            }
            at = next;
        }
        i += 1;
    }
    None
}

/// Where the parameters of one call are read from: the call, and the next script argument's index.
#[doc(hidden)]
pub struct Fetch<'a> {
    pub(crate) call: Call<'a>,
    pub(crate) next: std::cell::Cell<u32>,
    pub(crate) count: u32,
}

/// A parameter of a native: a script argument type (numbers, truth values, text, `Option<T>`, [`Rest`]) or an injected
/// one ([`Call`], [`This`]).
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a parameter of an extension's native",
    label = "a script argument (i32, u32, f32, bool, &str, String, &[u8], Option<T>, Rest) or Call or This"
)]
pub trait Param {
    /// What the native is handed.
    type Item<'a>;
    /// What kind of parameter it is.
    const KIND: ParamKind;
    /// Reads it for one call.
    #[doc(hidden)]
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<Self::Item<'a>, String>;
}

/// The script arguments left, as bytes: the last parameter of a native that takes any number of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rest<'a>(pub Vec<&'a [u8]>);

impl<'a> Rest<'a> {
    /// The arguments, each the bytes up to its first NUL.
    pub fn args(&self) -> &[&'a [u8]] {
        &self.0
    }
}

/// A method call's object id, for a native registered as a method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct This(pub u32);

fn take_index(cx: &Fetch<'_>) -> Option<u32> {
    let i = cx.next.get();
    if i >= cx.count {
        return None;
    }
    cx.next.set(i + 1);
    Some(i)
}

fn missing() -> String {
    "a required argument is missing".to_string()
}

macro_rules! required_args {
    ($($ty:ty => $read:ident),* $(,)?) => {
        $(
            impl Param for $ty {
                type Item<'a> = $ty;
                const KIND: ParamKind = ParamKind::Script(ArgKind::Required);
                fn fetch<'a>(cx: &Fetch<'a>) -> Result<$ty, String> {
                    let i = take_index(cx).ok_or_else(missing)?;
                    cx.call.$read(i).ok_or_else(|| format!("argument {} could not be read", i + 1))
                }
            }
        )*
    };
}

required_args!(i32 => arg_int, f32 => arg_float, bool => arg_bool);

impl Param for u32 {
    type Item<'a> = u32;
    const KIND: ParamKind = ParamKind::Script(ArgKind::Required);
    /// `dAtoi`, its bits read as unsigned, as the engine reads a `u32`.
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<u32, String> {
        <i32 as Param>::fetch(cx).map(|v| v as u32)
    }
}

impl Param for &[u8] {
    type Item<'a> = &'a [u8];
    const KIND: ParamKind = ParamKind::Script(ArgKind::Required);
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<&'a [u8], String> {
        let i = take_index(cx).ok_or_else(missing)?;
        cx.call
            .arg_bytes(i)
            .ok_or_else(|| format!("argument {} could not be read", i + 1))
    }
}

impl Param for &str {
    type Item<'a> = &'a str;
    const KIND: ParamKind = ParamKind::Script(ArgKind::Required);
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<&'a str, String> {
        let bytes = <&[u8] as Param>::fetch(cx)?;
        std::str::from_utf8(bytes).map_err(|_| "the argument is not valid UTF-8 text".to_string())
    }
}

impl Param for String {
    type Item<'a> = String;
    const KIND: ParamKind = ParamKind::Script(ArgKind::Required);
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<String, String> {
        <&[u8] as Param>::fetch(cx).map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

impl<T: Param> Param for Option<T> {
    type Item<'a> = Option<T::Item<'a>>;
    const KIND: ParamKind = match T::KIND {
        ParamKind::Script(ArgKind::Required) => ParamKind::Script(ArgKind::Optional),
        _ => panic!(
            "`Option<T>` takes a type that reads one argument, not another `Option` or `Rest`"
        ),
    };
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<Self::Item<'a>, String> {
        if cx.next.get() >= cx.count {
            return Ok(None);
        }
        T::fetch(cx).map(Some)
    }
}

impl Param for Rest<'_> {
    type Item<'a> = Rest<'a>;
    const KIND: ParamKind = ParamKind::Script(ArgKind::Rest);
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<Rest<'a>, String> {
        let mut rest = Vec::new();
        while let Some(i) = take_index(cx) {
            rest.push(
                cx.call
                    .arg_bytes(i)
                    .ok_or_else(|| format!("argument {} could not be read", i + 1))?,
            );
        }
        Ok(Rest(rest))
    }
}

impl Param for Call<'_> {
    type Item<'a> = Call<'a>;
    const KIND: ParamKind = ParamKind::Injected;
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<Call<'a>, String> {
        Ok(cx.call)
    }
}

impl Param for This {
    type Item<'a> = This;
    const KIND: ParamKind = ParamKind::Injected;
    fn fetch<'a>(cx: &Fetch<'a>) -> Result<This, String> {
        cx.call
            .this_object()
            .map(This)
            .ok_or_else(|| "the call has no object (`This` is for methods)".to_string())
    }
}

/// What a native returns: nothing, a number, a truth value or text, or `Result<T, E>` whose error is printed on the
/// console with the native's name and usage (and the call returns the empty string).
pub trait IntoReturn {
    /// The result as the engine is told it.
    #[doc(hidden)]
    fn into_return(self) -> Return;
}

/// A native's result.
#[doc(hidden)]
pub enum Return {
    Void,
    Int(i32),
    Float(f32),
    Bool(bool),
    Text(Vec<u8>),
    Error(String),
}

impl IntoReturn for () {
    fn into_return(self) -> Return {
        Return::Void
    }
}

impl IntoReturn for i32 {
    fn into_return(self) -> Return {
        Return::Int(self)
    }
}

impl IntoReturn for u32 {
    /// The 32 bits as a signed integer, as the engine returns a `u32`.
    fn into_return(self) -> Return {
        Return::Int(self as i32)
    }
}

impl IntoReturn for f32 {
    fn into_return(self) -> Return {
        Return::Float(self)
    }
}

impl IntoReturn for bool {
    fn into_return(self) -> Return {
        Return::Bool(self)
    }
}

impl IntoReturn for String {
    fn into_return(self) -> Return {
        Return::Text(self.into_bytes())
    }
}

impl IntoReturn for &str {
    fn into_return(self) -> Return {
        Return::Text(self.as_bytes().to_vec())
    }
}

impl IntoReturn for Vec<u8> {
    fn into_return(self) -> Return {
        Return::Text(self)
    }
}

impl<T: IntoReturn, E: fmt::Display> IntoReturn for Result<T, E> {
    fn into_return(self) -> Return {
        match self {
            Ok(value) => value.into_return(),
            Err(error) => Return::Error(error.to_string()),
        }
    }
}
