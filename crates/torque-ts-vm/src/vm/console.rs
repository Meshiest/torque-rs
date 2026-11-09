//! Console output (`echo`, `warn`, `error`), the build constants and the real-time clock (`getRealTime`).
//! The version answers come from the installed [`BuildInfo`](crate::policy::BuildInfo).
//!
//! Follows Torque3D's MIT-licensed `console/consoleFunctions.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here
//! is this crate's own.

use super::{Level, Native, Vm};
use crate::text::until_nul;

/// The arguments of `echo`, `warn` and `error`, joined with nothing between them (each ends at its first
/// NUL, as C strings do), up to the [`MAX_LOG_LINE`](super::MAX_LOG_LINE) bytes the log keeps of a line: nothing past
/// them is copied, however many or long the arguments.
fn joined(argv: &[Vec<u8>]) -> Vec<u8> {
    let mut line = Vec::new();
    for arg in argv.iter().skip(1) {
        if line.len() >= super::MAX_LOG_LINE {
            break;
        }
        super::push_capped(&mut line, until_nul(arg));
    }
    line
}

pub(super) fn register(vm: &mut Vm) {
    vm.register("echo", 2, 0, "echo(text...) - prints a line", |vm, c| {
        let line = joined(&c.argv);
        vm.print(&line);
        Native::Void
    });
    vm.register(
        "warn",
        2,
        0,
        "warn(text...) - prints a warning line",
        |vm, c| {
            let line = joined(&c.argv);
            vm.log_bytes(Level::Warning, &line);
            Native::Void
        },
    );
    vm.register(
        "error",
        2,
        0,
        "error(text...) - prints an error line",
        |vm, c| {
            let line = joined(&c.argv);
            vm.log_bytes(Level::Error, &line);
            Native::Void
        },
    );
    vm.register(
        "getBuildString",
        1,
        1,
        "getBuildString() - the kind of build",
        |vm, _| Native::Str(vm.build_info().build_string.as_bytes().to_vec()),
    );
    vm.register(
        "getCompileTimeString",
        1,
        1,
        "getCompileTimeString() - when the build was made",
        |vm, _| Native::Str(vm.build_info().compile_time.as_bytes().to_vec()),
    );
    vm.register(
        "getVersionNumber",
        1,
        1,
        "getVersionNumber() - the build's version number",
        |vm, _| Native::Int(vm.build_info().version_number),
    );
    vm.register(
        "getVersionString",
        1,
        1,
        "getVersionString() - the build's version text",
        |vm, _| Native::Str(vm.build_info().version_string.as_bytes().to_vec()),
    );
    // `GetTickCount` as a signed int.
    vm.register(
        "getRealTime",
        1,
        1,
        "getRealTime() - a millisecond counter of the computer",
        |vm, _| Native::Int(vm.clock.real_time_ms() as i32),
    );
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
