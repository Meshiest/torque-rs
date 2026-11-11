//! Helpers for tests that drive a [`Vm`] with script text: the result of an `eval` as text, a global's text, the console's
//! lines. Text is read lossily (invalid UTF-8 becomes U+FFFD), so a test never fails on how a script's bytes decode.
//!
//! Behind the `testing` feature (always built for this crate's own tests). A crate takes it as a dev-dependency only.

use crate::vm::{Vm, VmError};

/// Runs `code` and gives its result as text; panics, naming the error, when the script fails.
pub fn eval(vm: &mut Vm, code: &str) -> String {
    try_eval(vm, code).unwrap_or_else(|e| panic!("the script runs: {e:?}"))
}

/// Runs `code` and gives its result as text, or the error that ended it.
///
/// # Errors
///
/// What [`Vm::eval`] returns.
pub fn try_eval(vm: &mut Vm, code: &str) -> Result<String, VmError> {
    vm.eval(code.as_bytes())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// The global `name` as text ("" when it is unset).
pub fn global(vm: &Vm, name: &str) -> String {
    vm.global(name)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// The console's lines since the last drain, each as text.
pub fn log_lines(vm: &mut Vm) -> Vec<String> {
    vm.drain_log()
        .into_iter()
        .map(|line| String::from_utf8_lossy(&line.text).into_owned())
        .collect()
}

/// The console's lines since the last drain, joined by newlines.
pub fn console(vm: &mut Vm) -> String {
    log_lines(vm).join("\n")
}
