//! Script languages an extension runs: files with its extension that `exec` hands to it (a mod's `.lua`, say), the
//! functions it defines at run time, and what a call of it can do with the script engine (run its functions and methods,
//! read and write objects' fields, charge its work to the step budget).

use crate::{Call, Side};

/// A limit stopped the run: the script engine's step budget or call depth, or the nesting of calls back into the
/// extension. Hand it back from [`ScriptLanguage::run_file`] or [`ScriptLanguage::call_defined`], so that the script
/// that started the run stops too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit;

impl std::fmt::Display for Limit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the script ran out of steps or went too deep")
    }
}

impl std::error::Error for Limit {}

/// Why a call into the script engine failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallError {
    /// A limit stopped the run ([`Limit`]): stop too.
    Limit,
    /// The engine refused the request, with its status (a bad name, too many arguments, no such object, a handle that
    /// is not live).
    Refused(u32),
}

impl From<Limit> for CallError {
    fn from(_: Limit) -> CallError {
        CallError::Limit
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Limit => Limit.fmt(f),
            CallError::Refused(status) => {
                write!(f, "the engine refused the request (status {status})")
            }
        }
    }
}

impl std::error::Error for CallError {}

/// What a call's script engine is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VmInfo {
    /// Its number: the same for its whole life, never another's.
    pub vm: u64,
    /// A server's or a client's; [`Side::Both`] for one made outside either (a tool's).
    pub side: Side,
    /// Its step limit for one outermost run, if it has one.
    pub step_limit: Option<u64>,
}

/// A console function a language defines at run time ([`Call::define_function`]).
#[derive(Clone, Copy, Debug)]
pub struct Definition<'a> {
    /// The namespace or class (empty for the global namespace): `Player` makes `Player::name`, which a method call on a
    /// player finds, with the object first among the arguments.
    pub namespace: &'a str,
    /// The name scripts call.
    pub name: &'a str,
    /// The package it goes into (empty for none): one [`Call::script_package`] gave the language in this script engine.
    pub package: &'a str,
    /// Its usage text.
    pub usage: &'a str,
    /// What [`ScriptLanguage::call_defined`] is handed when a script calls it.
    pub id: u64,
}

/// A script language: the engine's `exec` hands it every file whose extension it was declared for
/// ([`Registrar::script_language`](crate::Registrar::script_language)), after the checks a script's file gets (so a
/// file a server sent is never handed over).
///
/// It is `Send + Sync`: the engine calls it from any of its threads, one call at a time; and a call may nest inside
/// another on the same thread when the language runs a script function that calls it back ([`Call::call_function`]),
/// so it must hold no lock of its own across such a call.
pub trait ScriptLanguage: Send + Sync + 'static {
    /// Runs the file `path`, whose text is `source`, inside a run of the call's script engine. `Err(Limit)` when a limit
    /// stopped it; an error of the script's own is printed ([`Call::error`]) and is still `Ok`.
    fn run_file(&self, call: Call<'_>, path: &str, source: &[u8]) -> Result<(), Limit>;

    /// Runs the function defined with `id` ([`Call::define_function`]): its script arguments are the call's
    /// ([`Call::arg_count`], [`Call::arg_bytes`]), and its result is the text it returns. An error of the script's own is
    /// printed ([`Call::error`]) and returns the empty text.
    fn call_defined(&self, call: Call<'_>, id: u64) -> Result<Vec<u8>, Limit> {
        let _ = (call, id);
        Ok(Vec::new())
    }

    /// The script engine `vm` is gone: what the language kept for it can go too.
    fn vm_dropped(&self, vm: u64) {
        let _ = vm;
    }
}
