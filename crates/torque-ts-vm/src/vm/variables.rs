//! `export` and `deleteVariables` (`Dictionary::exportVariables`, `Dictionary::deleteVariables`), and the checks on a
//! file name a script asks to write (`ResManager::isValidWriteFileName`).
//!
//! Follows Torque3D's MIT-licensed `console/consoleInternal.cpp` (`Dictionary::exportVariables`, `deleteVariables`)
//! (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use super::{Native, NativeCall, Vm, VmError, lossy};
use crate::text::{atof, expand_escape, fold, is_match, match_cost, to_f32};
use crate::vars::Entry;

/// TGE's line buffer (`char buffer[1024]` with `dSprintf`): longer lines are cut.
const MAX_LINE: usize = 1023;

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&b""[..], Vec::as_slice)
}

/// `dAtob`: "true" in any case, or a number that is not 0 once `atof`'s double has been converted to a float
/// (NaN counts as true: `export(..., "1e-50")` does not append, `"1e-45"` does).
fn atob(text: &[u8]) -> bool {
    text.eq_ignore_ascii_case(b"true") || to_f32(atof(text)) != 0.0
}

/// `_stricmp` on two names.
fn cmp_ignoring_case(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.iter()
        .map(u8::to_ascii_lowercase)
        .cmp(b.iter().map(u8::to_ascii_lowercase))
}

pub(super) fn register(vm: &mut Vm) {
    vm.register(
        "export",
        2,
        4,
        "export(pattern [, file [, append]]) - writes the matching global variables as script",
        |vm, c| {
            let append = c.argv.len() == 4 && atob(arg(c, 3));
            // A name that does not expand (its error is printed) sends the lines to the console.
            let file = if c.argv.len() >= 3 {
                vm.expand_or_console(arg(c, 2))
            } else {
                None
            };
            match vm.export_variables(arg(c, 1), file.as_deref(), append) {
                Ok(()) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        },
    );
    vm.register(
        "deleteVariables",
        2,
        2,
        "deleteVariables(pattern) - deletes the global variables whose names match",
        |vm, c| match vm.delete_variables(arg(c, 1)) {
            Ok(()) => Native::Void,
            Err(e) => Native::Fail(e),
        },
    );
}

/// Which globals an export writes: those whose name matches a pattern, or those named in a list.
#[derive(Clone, Copy)]
enum Select<'a> {
    Pattern(&'a [u8]),
    Names(&'a [&'a [u8]]),
}

impl Select<'_> {
    /// An upper bound of the work deciding on a name of `len` bytes takes, charged before it is decided.
    fn cost(&self, len: usize) -> u64 {
        match self {
            Select::Pattern(pattern) => match_cost(pattern, len),
            Select::Names(names) => names.iter().map(|n| n.len().min(len) as u64 + 1).sum(),
        }
    }

    fn keeps(&self, name: &[u8]) -> bool {
        match self {
            Select::Pattern(pattern) => is_match(pattern, name),
            Select::Names(names) => names.iter().any(|n| n.eq_ignore_ascii_case(name)),
        }
    }
}

/// What `export_lines` found: the lines to print or write, and the names it skipped.
type ExportLines = (Vec<Vec<u8>>, Vec<Vec<u8>>);

/// Whether `export` writes a variable of this name: letters, digits, `_`, `:` and `$` only. TGE
/// writes the name raw (the VALUE is escaped, the name is not), and a name is script-controlled text (`$x[%s]`
/// with a string a client sent): `$Pref::xy = 1; $hacked = 1; $z` would come back as three statements when
/// the next start `exec`s the exported prefs, with the whole server's authority. Deliberately unlike TGE.
pub fn name_is_exportable(name: &[u8]) -> bool {
    name.iter()
        .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'$'))
}

impl Vm {
    /// `Con::expandScriptFilename` for `export`: the expanded name, or None after printing why it failed.
    fn expand_or_console(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        match self.expand_script_filename(name) {
            Ok(path) => Some(path),
            Err(message) => {
                self.error(message);
                None
            }
        }
    }

    /// `Dictionary::exportVariables`: the global variables whose name (with its `$`) matches `pattern`
    /// ([`is_match`]), sorted ignoring case, one line each: `name = 5;` for an integer or a float (as `%d` /
    /// `%g` of the stored value), `name = "text";` with `expandEscape` for a string. Printed one line per
    /// variable, or written to `file` with CRLF line ends (appending when `append`). Each line is cut at 1023
    /// bytes, as TGE's buffer is (its `expandEscape` buffer is 1024 bytes too and overflows on a longer
    /// string; this cuts). A variable whose name holds a character other than letters,
    /// digits, `_`, `:` and `$` is skipped with a warning naming it (`name_is_exportable`). Nothing
    /// to write: nothing happens (not even the file is opened).
    ///
    /// The scan, the match (its cost bound is charged before it runs), the sort and the lines are charged to
    /// the step budget, and a native loop checks the budget itself (`ensure_budget`).
    pub fn export_variables(
        &mut self,
        pattern: &[u8],
        file: Option<&[u8]>,
        append: bool,
    ) -> Result<(), VmError> {
        self.export_selected(Select::Pattern(pattern), file, append)
    }

    /// [`export_variables`](Self::export_variables) for the globals named in `names` (compared ignoring case) instead of
    /// a pattern: one scan, one sort, the same lines in the same order as `export` of a pattern matching exactly those
    /// names, and one write (the file is replaced unless `append`).
    pub fn export_named_variables(
        &mut self,
        names: &[&[u8]],
        file: Option<&[u8]>,
        append: bool,
    ) -> Result<(), VmError> {
        self.export_selected(Select::Names(names), file, append)
    }

    fn export_selected(
        &mut self,
        select: Select,
        file: Option<&[u8]>,
        append: bool,
    ) -> Result<(), VmError> {
        let (lines, skipped) = self.export_lines(select, file.is_some())?;
        for name in skipped {
            let shown = lossy(name.get(..MAX_LINE).unwrap_or(&name));
            self.warn(format!(
                "export: skipping variable \"{shown}\": its name holds a character other than letters, digits, '_', ':' and '$'"
            ));
        }
        if lines.is_empty() {
            return Ok(());
        }
        let Some(file) = file else {
            for line in lines {
                self.print(&line);
            }
            return Ok(());
        };
        let contents = lines.concat();
        self.objects.add_work(contents.len() as u64);
        self.write_export(file, &contents, append)
    }

    /// The lines `export` prints or writes, in order, and the names of the matching variables it leaves
    /// out: a name with a byte outside `[A-Za-z0-9_:$]` (`name_is_exportable`). Does not run script code
    /// and holds no borrow of the variables afterwards.
    #[inline(never)]
    fn export_lines(&self, select: Select, to_file: bool) -> Result<ExportLines, VmError> {
        let found = self.matching_globals(select)?;
        let end: &[u8] = if to_file { b"\r\n" } else { b"" };
        let mut lines = Vec::with_capacity(found.len());
        let mut skipped = Vec::new();
        for entry in found {
            let name = entry.name();
            if !name_is_exportable(name) {
                skipped.push(name.to_vec());
                continue;
            }
            let mut line = name.get(..MAX_LINE).unwrap_or(name).to_vec();
            line.extend_from_slice(b" = ");
            if entry.is_string() {
                line.push(b'"');
                // Only the first 1023 bytes can survive the cut, and escaping never shortens a byte.
                line.extend_from_slice(&expand_escape(&entry.string_head(MAX_LINE)));
                line.push(b'"');
            } else {
                line.extend_from_slice(&entry.get_string());
            }
            line.push(b';');
            line.extend_from_slice(end);
            line.truncate(MAX_LINE);
            self.objects.add_work(line.len() as u64);
            lines.push(line);
        }
        Ok((lines, skipped))
    }

    /// The global variables whose name matches `pattern`, sorted ignoring case. Every variable scanned
    /// costs a unit and the bound of its match ([`match_cost`]), charged before the match, and the sort
    /// costs the bytes of the names it compares (about one pass per level of the merge): a hostile pattern
    /// or a crowd of long names ends the run at its step limit.
    fn matching_globals(&self, select: Select) -> Result<Vec<&Entry>, VmError> {
        let mut found: Vec<&Entry> = Vec::new();
        for entry in self.globals.iter() {
            self.objects
                .add_work(1u64.saturating_add(select.cost(entry.name().len())));
            self.ensure_budget()?;
            if select.keeps(entry.name()) {
                found.push(entry);
            }
        }
        let name_bytes: u64 = found.iter().map(|e| e.name().len() as u64).sum();
        let levels = u64::from(usize::BITS - found.len().leading_zeros());
        self.objects
            .add_work(name_bytes.saturating_mul(levels.max(1)));
        self.ensure_budget()?;
        found.sort_by(|a, b| cmp_ignoring_case(a.name(), b.name()));
        Ok(found)
    }

    /// `ResManager::openFileForWrite` for `export` and the write itself: the name checks, then the host. Each
    /// refusal ends with `Unable to open file '<name> for writing.` (`Con::errorf` without the closing quote, as
    /// TGE prints it).
    #[inline(never)]
    fn write_export(&mut self, file: &[u8], contents: &[u8], append: bool) -> Result<(), VmError> {
        let shown = lossy(file);
        if !self.check_write_path(file)? {
            let line = self.messages().invalid_write_name(&shown);
            self.error(line);
        } else if self.export_to_host(file, contents, append) {
            return Ok(());
        } else {
            // The host cannot (a name its folders do not allow, a full disk): the message for a
            // stream that does not open.
            let line = self.messages().write_open_failed(&shown);
            self.error(line);
        }
        self.error(format!("Unable to open file '{shown} for writing."));
        Ok(())
    }

    /// What an object that saves itself does (`ResManager::openFileForWrite` and a write, as `export` does it): the
    /// checks on the name (`ResManager::openFileForWrite - invalid write filename "<name>"`), the host's write (`...
    /// failed to open stream for "<name>"`). True when written; the caller prints its own failure line. For hosts'
    /// natives (`ActionMap::save`).
    pub fn write_engine_file(
        &mut self,
        file: &[u8],
        contents: &[u8],
        append: bool,
    ) -> Result<bool, VmError> {
        let shown = lossy(file);
        if !self.check_write_path(file)? {
            let line = self.messages().invalid_write_name(&shown);
            self.error(line);
            return Ok(false);
        }
        self.objects.add_work(contents.len() as u64);
        if self.export_to_host(file, contents, append) {
            return Ok(true);
        }
        let line = self.messages().write_open_failed(&shown);
        self.error(line);
        Ok(false)
    }

    /// The write of an `export`, if the file can be written: not while another object has it open, and the
    /// host must take it. Like every `openFileForWrite` it tells the file rules the file was opened for writing.
    /// Out of line: `write_export` is live while a refusal's BackTrace runs.
    #[inline(never)]
    fn export_to_host(&mut self, file: &[u8], contents: &[u8], append: bool) -> bool {
        self.note_path(file);
        if self.file_objects.is_locked(file) || !self.host_write(file, contents, append) {
            return false;
        }
        self.policies.files.zero(file);
        true
    }

    /// `Dictionary::deleteVariables`: removes the global variables whose name matches
    /// `pattern`. Charged like [`export_variables`](Self::export_variables); on a spent budget nothing is
    /// removed.
    pub fn delete_variables(&mut self, pattern: &[u8]) -> Result<(), VmError> {
        let doomed: Vec<Vec<u8>> = {
            let mut doomed = Vec::new();
            for entry in self.globals.iter() {
                self.objects
                    .add_work(1u64.saturating_add(match_cost(pattern, entry.name().len())));
                self.ensure_budget()?;
                if is_match(pattern, entry.name()) {
                    doomed.push(entry.name().to_vec());
                }
            }
            doomed
        };
        for name in doomed {
            self.objects
                .add_work(1u64.saturating_add(name.len() as u64));
            self.globals.remove(&name);
        }
        Ok(())
    }

    /// `ResManager::isValidWriteFileName`: a name starting with `/` or holding one of
    /// `: | > < *` is refused silently; `..` anywhere (in a name of two bytes or more) prints
    /// `ERROR: Invalid file path "<name>"` and a BackTrace; a name that does not start with a default
    /// directory and `/` (compared ignoring case) prints `ERROR: File "<name>" is not in one of the default
    /// directories` and a BackTrace. The list of writable extensions is empty by default and not modelled.
    /// True when the name passes.
    pub(crate) fn check_write_path(&mut self, path: &[u8]) -> Result<bool, VmError> {
        match self.write_name_verdict(path) {
            Verdict::Fine => Ok(true),
            Verdict::Silent => Ok(false),
            Verdict::NeedsBacktrace => self.backtrace_after_message().map(|()| false),
        }
    }

    /// The checks and their messages; out of line so that the frame alive during the BackTrace (a host's
    /// replacement of `backtrace` can run a script, which can export again) holds nothing of them.
    #[inline(never)]
    fn write_name_verdict(&mut self, path: &[u8]) -> Verdict {
        if path.first() == Some(&b'/') || path.iter().any(|b| b":|><*".contains(b)) {
            return Verdict::Silent;
        }
        if path.windows(2).any(|w| w == b"..") {
            self.error(format!("ERROR: Invalid file path \"{}\"", lossy(path)));
            return Verdict::NeedsBacktrace;
        }
        let rules = self.policies.files.resources();
        let folded = fold(path);
        let allowed = rules.write_directories.is_empty()
            || rules
                .write_directories
                .iter()
                .any(|d| folded.starts_with(d) && folded.get(d.len()) == Some(&b'/'));
        if allowed {
            return Verdict::Fine;
        }
        let line = (rules.refusal_text)(path, crate::host::Refusal::NotDefaultDirectory);
        self.error(line);
        Verdict::NeedsBacktrace
    }
}

/// What the checks of a file name to write say.
enum Verdict {
    Fine,
    /// Refused without a message.
    Silent,
    /// Refused; the message is printed and a BackTrace follows.
    NeedsBacktrace,
}

#[cfg(test)]
#[path = "variables_tests.rs"]
mod tests;
