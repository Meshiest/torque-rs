//! The functions a log line reports missing (the warnings of a call to a function or method that does not exist).

use super::{Level, Vm};

/// The warning a call of a function nothing defines prints (`<file> (<line>): Unable to find function X`).
const NOT_FOUND: &str = ": Unable to find function ";
/// The warning a call of a method nothing defines prints (`<file> (<line>): Unknown command X.`).
const UNKNOWN_COMMAND: &str = ": Unknown command ";

/// The names in "Unable to find function X" / "Unknown command X." warnings, counted, most frequent first and
/// equal counts by name: what a script called that nothing defines. A host's report of what it does not
/// have yet uses it too. It counts the lines the `Vm` no longer holds (`Vm::drain_log`, the log's cap) as well:
/// a host that drains the log after printing it still gets its report.
///
/// It is strict: only a *warning* line counts, and only when what follows the words is a function name
/// (`name` or `Namespace::name`: letters, digits and underscores, the `::`-separated parts not empty). A
/// compiler's error context quotes the script's own text, and a script can print what it likes, so a line
/// that merely contains the words is not a call: `error("Problem: Unknown command option: " ...)`, quoted
/// from the start-up script, is neither a warning nor followed by a name. (A script's `warn` of a
/// well-formed line does count: nothing in the log tells it from the VM's own.)
pub fn missing_functions(vm: &Vm) -> Vec<(String, usize)> {
    // The lines that left the log (a host drained them, or the log's cap dropped them) were tallied then.
    let mut counts = vm.gone_missing().clone();
    for line in vm.log() {
        if line.level == Level::Warning {
            count_called(&mut counts, &line.text);
        }
    }
    let mut missing: Vec<(String, usize)> = counts.into_iter().collect();
    missing.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    missing
}

/// `name` or `Namespace::name`: every `::`-separated part is letters, digits and underscores, and not empty.
fn is_function_name(name: &str) -> bool {
    name.split("::").all(|part| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

/// Adds the function a warning line names (if it names one) to `counts`.
pub fn count_called(counts: &mut std::collections::BTreeMap<String, usize>, text: &[u8]) {
    if let Some(name) = called_name(text) {
        match counts.get_mut(name) {
            Some(count) => *count = count.saturating_add(1),
            None => {
                counts.insert(name.to_string(), 1);
            }
        }
    }
}

/// What follows the first `marker` in `text`.
fn after<'a>(text: &'a [u8], marker: &str) -> Option<&'a [u8]> {
    let marker = marker.as_bytes();
    let at = text.windows(marker.len()).position(|w| w == marker)?;
    text.get(at + marker.len()..)
}

/// The function name in a log line that reports a missing function or method, if it is one. The line may hold
/// bytes that are not UTF-8 (the script's file name, say): only the name has to be text.
fn called_name(text: &[u8]) -> Option<&str> {
    let name = match after(text, NOT_FOUND) {
        Some(name) => name,
        None => after(text, UNKNOWN_COMMAND)?.strip_suffix(b".")?,
    };
    let name = std::str::from_utf8(name).ok()?;
    is_function_name(name).then_some(name)
}
