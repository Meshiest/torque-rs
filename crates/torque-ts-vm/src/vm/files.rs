//! Loading scripts: `exec`, `compile`, `eval`, `expandFilename`. Files come from the
//! [`FileHost`](crate::host::FileHost), source
//! text goes through the [`Compiler`](crate::host::Compiler): `CodeBlock::compile` ([`Purpose::Check`]) for
//! `compile()` and the first of `exec`'s two compiles of a source, `compileExec` ([`Purpose::Run`]) for the
//! second, for `eval` and for a host's [`Vm::compile_exec`].
//!
//! Follows Torque3D's MIT-licensed `console/torquescript/runtime.cpp` and `console/console.cpp` (the latter Copyright
//! (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::collections::HashMap;
use std::rc::Rc;

use torque_ts_bytecode::DsoFile;

use super::{COMPILE_BASE_WORK, MAX_LOG_LINE, Mode, Native, Vm, VmError, lossy};
use crate::code::CodeBlock;
use crate::host::{Compiled, FileRead, Purpose, Refusal};
use crate::sim::{ObjId, SimObject};
use crate::text::{fold, until_nul};
use crate::vm::{Level, LogLine};

/// Extra call-depth units charged for the BackTrace of a refused path (see `refusal_backtrace`).
const REFUSAL_BACKTRACE_COST: usize = 1;
/// TGE's file-name buffer (`scriptFilenameBuffer`, 1024 bytes with the terminator).
const MAX_PATH: usize = 1023;

/// The bytes of file names the interning map (`file_name`) holds at most: a script that execs distinct
/// 1023-byte names in every run (each with its own budget) would otherwise grow it for ever. Past the
/// cap a name is not remembered, so a spelling that differs only in case from an earlier one may then show
/// as itself in messages (TGE's string table has no cap).
const MAX_FILE_NAME_BYTES: usize = 16 << 20;
/// The length from which the `eval error >>` line is not printed.
pub const EVAL_ERROR_TEXT_BYTES: usize = 1000;
/// The script files [`Vm::script_file`] remembers at most (a game execs some hundreds). Past the cap a file runs as
/// before but is not remembered, so a watcher of the table cannot see it change.
pub const MAX_TRACKED_SCRIPT_FILES: usize = 4096;
/// The objects one trace ([`Vm::trace_new_objects`]) lists at most; the rest are counted.
pub const MAX_TRACED_OBJECTS: usize = 8192;

/// A script file `exec` ran ([`Vm::script_file`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFile {
    /// The expanded path, as `exec` first spelled it.
    pub path: Rc<[u8]>,
    /// The file's change time when it was last run (`FileHost::modified`; None when the host gives none).
    pub modified: Option<u64>,
    /// A `datablock` declaration in the file's code registered a datablock.
    pub declares_datablocks: bool,
}

/// What `exec` recorded: the files it ran, and the file whose code declared each datablock
/// ([`Vm::datablocks_declared_by`]). Both are cleared by `deleteDataBlocks`. The files are capped
/// ([`MAX_TRACKED_SCRIPT_FILES`]) and the declarations hold one entry per datablock id, so they are bounded by the
/// datablock id range.
#[derive(Default)]
pub(crate) struct ScriptFiles {
    /// By the folded path.
    files: HashMap<Rc<[u8]>, ScriptFile>,
    /// A datablock's id to the folded path of the file whose code last declared it.
    declared_in: HashMap<ObjId, Rc<[u8]>>,
    /// The objects registered while a trace is on, and how many past the cap.
    traced: Option<(Vec<ObjId>, usize)>,
}

/// How `exec` runs a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecWay {
    /// TGE's `exec`: the compiled file when it is at least as new, two compiles of a source, the statements before a
    /// syntax error run.
    Tge,
    /// [`Vm::exec_file_checked`]: the source, compiled once, nothing of one with a syntax error.
    Checked,
}

/// What [`Vm::exec_file_checked`] did with the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecCheck {
    /// The file ran to its end.
    Ran,
    /// The source has a syntax error (printed once): nothing of it ran.
    SyntaxError,
    /// The file is missing, refused, or empty (the message was printed): nothing ran.
    NotRun,
}

/// A replacement of the report of an `eval` with a syntax error (see [`Vm::set_eval_report`]).
pub type EvalReport = Rc<dyn Fn(&mut Vm, &[u8], Option<usize>) -> Result<(), VmError>>;

/// The slot of [`EvalReport`] (with a `Debug` that says whether it is set).
#[derive(Default, Clone)]
pub(crate) struct EvalReportSlot(pub(crate) Option<EvalReport>);

impl std::fmt::Debug for EvalReportSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "EvalReport(set)"
        } else {
            "EvalReport(none)"
        })
    }
}

pub(super) fn register(vm: &mut Vm) {
    // Running a file or a text is code; `compile` writes the compiled file beside the script.
    vm.add_native_effects("", "exec", crate::effects::Effects::CODE);
    vm.add_native_effects("", "eval", crate::effects::Effects::CODE);
    vm.add_native_effects(
        "",
        "compile",
        crate::effects::Effects::CODE | crate::effects::Effects::FILES,
    );
    vm.register(
        "exec",
        2,
        4,
        "exec(file [, noCalls]) - compiles and runs a script file",
        |vm, c| {
            // The third argument (journals) does nothing here.
            let no_calls = c.argc() >= 3 && c.int(2) != 0;
            match vm.exec_file(c.arg(1), no_calls) {
                Ok(done) => Native::Bool(done),
                Err(e) => Native::Fail(e),
            }
        },
    );
    vm.register(
        "compile",
        2,
        2,
        "compile(file) - compiles a script without running it",
        |vm, c| match vm.compile_file(c.arg(1)) {
            Ok(ok) => Native::Bool(ok),
            Err(e) => Native::Fail(e),
        },
    );
    vm.register(
        "eval",
        2,
        2,
        "eval(code) - compiles and runs the code, returns what it returned",
        |vm, c| {
            // Script called it: the static array's first slot; `call("eval", ...)` ran it: the second.
            let via_call = std::mem::take(&mut vm.quirks.eval_via_call);
            let slot = if c.from_bytecode {
                Some(0)
            } else {
                via_call.then_some(1)
            };
            match vm.eval_with(c.arg(1), slot) {
                Ok(result) => Native::Str(result),
                Err(e) => Native::Fail(e),
            }
        },
    );
    vm.register(
        "expandFilename",
        2,
        2,
        "expandFilename(name) - the name with ./ and ~/ resolved against the running script",
        |vm, c| Native::Str(vm.expand_or_report(c.arg(1))),
    );
}

/// The error for a name that does not fit the file-name buffer. The console cuts a line at 4095
/// bytes, so only that much of a (possibly enormous) name is ever built into the message.
fn overflow_message(name: &[u8]) -> String {
    let shown = name.get(..MAX_LOG_LINE).unwrap_or(name);
    format!(
        "Buffer overflow attempting to expand filename: {}",
        lossy(shown)
    )
}

/// What [`Vm::checked_dso`] does with a `.dso` of another version: the first lookup reports it, the one after a check
/// compile (an older `.dso` that would run after all) passes over it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OtherVersion {
    Report,
    Skip,
}

/// The largest `.dso` `exec` loads, as large as the largest source the compiler takes: a loaded block holds several times
/// its file's size, so a bigger one is refused before it is parsed (and the source, if there is one, runs instead).
pub const MAX_DSO_BYTES: usize = 16 << 20;

/// The room for the path of a compiled file (`exec`'s and `compile`'s 512-byte name buffer, as in Torque3D's MIT-licensed
/// `console/torquescript/runtime.cpp`): 511 bytes and the terminator.
const MAX_DSO_PATH: usize = 511;

/// `<path>.dso` in that room: cut at 511 bytes, so a name longer than 507 bytes looks up a path with a
/// partial or no extension.
fn dso_path(path: &[u8]) -> Vec<u8> {
    let mut out = [path, b".dso"].concat();
    out.truncate(MAX_DSO_PATH);
    out
}

/// The path of one lookup: the file, or its compiled `.dso`.
#[inline(never)]
fn lookup_path(path: &[u8], compiled: bool) -> Vec<u8> {
    if compiled {
        dso_path(path)
    } else {
        path.to_vec()
    }
}

/// What `exec` found to do.
enum Prepared {
    /// Nothing to run; the result.
    Done(bool),
    /// Run this block (then the result is true).
    Run(Rc<CodeBlock>, Mode),
    /// The step budget was spent before a compile (see `compile_source`): the run ends with `StepLimit`.
    OutOfBudget,
    /// A checked exec ([`Vm::exec_file_checked`]) found a syntax error: nothing runs.
    SyntaxError,
    /// A file of another language ([`ScriptLanguage`](crate::policy::ScriptLanguage)): boxed, so this stays as small
    /// as the frames on the recursion path need.
    Foreign(Box<Foreign>),
}

/// A file of another language `exec` runs: the language, the path and the source.
struct Foreign {
    language: Rc<dyn crate::policy::ScriptLanguage>,
    path: Vec<u8>,
    source: Vec<u8>,
}

/// How one compile ended. A byte, not a `Result`: the helpers that hold it have frames on the recursion path
/// (an `eval` or `exec` at the bottom of a deep recursion), and a `VmError` in them would cost every level.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Clean,
    SyntaxError,
    /// Not compiled: inside a run, the step budget was already spent.
    OutOfBudget,
}

/// The version word at the start of a DSO (0 when the file is too short).
fn dso_version(bytes: &[u8]) -> u32 {
    bytes
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

impl Vm {
    /// `Con::expandScriptFilename`: `./x` is relative to the folder of the code block that is executing,
    /// `~/x` to its first folder (the mod); other names are kept as they are, and so is everything when no
    /// named block is executing (the console, `eval`'d text). `Err` carries the error message.
    pub fn expand_script_filename(&self, name: &[u8]) -> Result<Vec<u8>, String> {
        let base = self.current_file();
        let relative = name.starts_with(b"./") || name.starts_with(b"~/");
        let Some(base) = base.filter(|_| relative) else {
            // With a named block executing, a name of more than 1024 bytes is an error;
            // anything else is copied with a 1023-byte bound, silently (also with no block at all).
            if base.is_some() && name.len() > MAX_PATH + 1 {
                return Err(overflow_message(name));
            }
            return Ok(name.get(..MAX_PATH).unwrap_or(name).to_vec());
        };
        let cut = if name.starts_with(b"~/") {
            base.iter().position(|&b| b == b'/')
        } else {
            base.iter().rposition(|&b| b == b'/')
        };
        let Some(cut) = cut else {
            return Err(format!(
                "Illegal CodeBlock path detected (no mod directory): {}",
                lossy(base)
            ));
        };
        let mut out = base[..cut].to_vec();
        out.extend_from_slice(&name[1..]);
        if out.len() > MAX_PATH {
            return Err(overflow_message(name));
        }
        Ok(out)
    }

    /// The expansion as `expandFilename` and `exec` use it: an error is printed and the name is "".
    pub fn expand_or_report(&mut self, name: &[u8]) -> Vec<u8> {
        let expanded = self.expand_script_filename(name);
        self.report_name(expanded)
    }

    /// A name taken as written for a native that does not expand it (`FileObject`'s opens under
    /// [`FileRules::expand_file_object_names`](crate::policy::FileRules::expand_file_object_names), a game's `fileCopy`):
    /// a `./` or `~/` name is kept, so the resource manager (or the copy's own check) refuses it as not in a default
    /// directory; any other name is treated as the expanding natives treat it.
    pub(crate) fn unexpanded_filename(&self, name: &[u8]) -> Result<Vec<u8>, String> {
        if name.starts_with(b"./") || name.starts_with(b"~/") {
            return Ok(name.to_vec());
        }
        self.expand_script_filename(name)
    }

    /// The name a `FileObject` open uses: expanded as `exec` expands it, or kept as written when the file rules say
    /// so ([`FileRules::expand_file_object_names`](crate::policy::FileRules::expand_file_object_names)).
    pub(crate) fn file_object_name(&self, name: &[u8]) -> Result<Vec<u8>, String> {
        if self.policies.files.expand_file_object_names() {
            self.expand_script_filename(name)
        } else {
            self.unexpanded_filename(name)
        }
    }

    /// `unexpanded_filename`, printing an error as [`expand_or_report`](Self::expand_or_report) does.
    pub fn unexpanded_or_report(&mut self, name: &[u8]) -> Vec<u8> {
        let kept = self.unexpanded_filename(name);
        self.report_name(kept)
    }

    /// The name of an expansion, or "" after printing its error.
    fn report_name(&mut self, name: Result<Vec<u8>, String>) -> Vec<u8> {
        match name {
            Ok(path) => path,
            Err(message) => {
                self.error(message);
                Vec::new()
            }
        }
    }

    /// One `ResManager::find`: the file's contents, or None after printing a refusal. The BackTrace of a
    /// refusal can run a script (through a host's replacement of `backtrace`) that execs again, so this frame is live
    /// once per level then and holds as little as it can: the lookup and its messages are in
    /// `read_one`, which has returned by then.
    pub fn find_file(&mut self, path: &[u8]) -> Result<Option<Vec<u8>>, VmError> {
        let (found, backtrace) = self.read_one(path);
        if backtrace {
            self.refusal_backtrace()?;
        }
        Ok(found)
    }

    /// The message `ResManager::find` prints when it refuses a path, for the queries that ask
    /// the host without reading the file (`isFile`, `getFileLength`): an error, and a BackTrace for a
    /// folder outside the default directories. `read_one` prints the same message through
    /// `refusal_message`.
    pub fn report_refusal(&mut self, path: &[u8], refusal: Refusal) -> Result<(), VmError> {
        if self.refusal_message(path, refusal) {
            self.refusal_backtrace()?;
        }
        Ok(())
    }

    /// Prints the refusal's message; true when a BackTrace follows.
    #[inline(never)]
    fn refusal_message(&mut self, path: &[u8], refusal: Refusal) -> bool {
        // The message names the path as the string table has it, the first spelling of each part (after a refused
        // `openForRead("nodir/X.CS")` the lookups of `exec("nodir/x.cs")` say `nodir/X.CS`).
        let path = &self.intern_path(path);
        let line = (self.policies.files.resources().refusal_text)(path, refusal);
        self.error(line);
        refusal == Refusal::NotDefaultDirectory
    }

    /// The BackTrace after a refused path. Its frames (this lookup, `exec`'s checks, the native) come on top
    /// of a re-entry's when a host-replaced `backtrace` runs a script that execs a refused path again, so the
    /// level is charged `REFUSAL_BACKTRACE_COST` more depth unit: the recursion stops at 64 levels (three depth
    /// units for the re-entry and the frame, one for the refusal) instead of 85, inside the stack the `Vm`
    /// documents.
    #[inline(never)]
    fn refusal_backtrace(&mut self) -> Result<(), VmError> {
        self.depth = self.depth.saturating_add(REFUSAL_BACKTRACE_COST);
        let result = self.backtrace_after_message();
        self.depth = self.depth.saturating_sub(REFUSAL_BACKTRACE_COST);
        result
    }

    /// `FileHost::work_done` now: the mark `charge_host_work` takes after the host call it brackets.
    #[inline(always)]
    pub(super) fn host_work_mark(&self) -> u64 {
        self.files.work_done()
    }

    /// Charges what the host did since `mark` that no returned byte shows (`FileHost::work_done`: zip entries
    /// inflated for a read that failed, the file an append started from). The bytes a read returns are
    /// charged by the caller, on top. A counter that went down charges nothing.
    #[inline(always)]
    pub(super) fn charge_host_work(&self, mark: u64) {
        self.objects
            .add_work(self.files.work_done().saturating_sub(mark));
    }

    /// The file at `path` and whether a BackTrace follows the message about it.
    #[inline(never)]
    fn read_one(&mut self, path: &[u8]) -> (Option<Vec<u8>>, bool) {
        // A file a `FileObject` has open for writing cannot be opened: it reads as an empty file
        // (`exec`: an invalid script file; the CRC -1; `openForRead` 0), and the host is not asked.
        self.note_path(path);
        if self.file_objects.is_locked(path) {
            return (Some(Vec::new()), false);
        }
        let mark = self.host_work_mark();
        let read = self.files.read(path);
        self.charge_host_work(mark);
        match read {
            FileRead::Found(bytes) => {
                // A read stream refreshes the recorded length (`exec`, `openForRead`).
                self.policies.files.refresh(path);
                // By design the host's own top-level `exec_file` (the boot) is not charged for this: the
                // work is counted before `run_here`'s `enter_run`, which discards the work counted outside
                // a run at an outermost start (a host call has no script step to bill). Inside a run, as
                // from a script's `exec`, it is charged. Do not "fix" it by billing the host.
                // Reading, checksumming and parsing a file costs time in proportion to its size, whatever
                // few instructions asked for it: one work unit per byte, as the string stack's writes.
                self.objects.add_work(bytes.len() as u64);
                (Some(bytes), false)
            }
            FileRead::Missing => (None, false),
            FileRead::Refused(refusal) => (None, self.refusal_message(path, refusal)),
        }
    }

    /// The first spelling `exec` saw of a file name (the string table). NOT the spelling the scan or
    /// another native put in the table for the folder and the name (`intern_path`, which the searches show):
    /// `exec("./weapon.cs")` of a zip entry `Weapon.cs` prints the spelling of the call, so exec's messages keep
    /// to the names `exec` was given.
    fn file_name(&mut self, path: &[u8]) -> Vec<u8> {
        self.objects.add_work(path.len() as u64);
        self.first_spelling(path)
    }

    /// [`file_name`](Self::file_name) without the charge, for a caller that has paid for the name's bytes.
    fn first_spelling(&mut self, path: &[u8]) -> Vec<u8> {
        let key = fold(path);
        if let Some(first) = self.file_names.get(&key) {
            return first.clone();
        }
        let size = key.len().saturating_add(path.len());
        if self.file_name_bytes.saturating_add(size) <= MAX_FILE_NAME_BYTES {
            self.file_name_bytes += size;
            self.file_names.insert(key, path.to_vec());
        }
        path.to_vec()
    }

    /// Enters `path` in the table `exec`'s messages take their spelling from (the first spelling wins), as a
    /// `FileObject` read of the file does.
    pub(crate) fn note_exec_name(&mut self, path: &[u8]) {
        let _ = self.first_spelling(path);
    }

    /// `exec` found `path` (the expanded name) and is about to run it: the table of script files takes it with the
    /// change time the host gives now (an entry past [`MAX_TRACKED_SCRIPT_FILES`] is not made).
    #[inline(never)]
    fn note_executed(&mut self, path: &[u8]) {
        let modified = self.files.modified(path);
        let key = fold(path);
        if let Some(file) = self.script_files.files.get_mut(key.as_slice()) {
            file.modified = modified;
            return;
        }
        if self.script_files.files.len() >= MAX_TRACKED_SCRIPT_FILES {
            return;
        }
        let spelled: Rc<[u8]> = Rc::from(self.first_spelling(path));
        self.script_files.files.insert(
            Rc::from(key),
            ScriptFile {
                path: spelled,
                modified,
                declares_datablocks: false,
            },
        );
    }

    /// ADD_OBJECT registered `id` from the code block named `block`: a datablock (one in DataBlockGroup) is recorded as
    /// declared by that file when `exec` ran it, and a trace in progress ([`trace_new_objects`](Self::trace_new_objects))
    /// takes every object.
    #[inline(never)]
    pub(crate) fn note_registered(&mut self, id: ObjId, block: Option<&[u8]>) {
        if let Some((traced, over)) = &mut self.script_files.traced {
            if traced.len() < MAX_TRACED_OBJECTS {
                traced.push(id);
            } else {
                *over = over.saturating_add(1);
            }
        }
        let Some(block) = block else {
            return;
        };
        let in_group =
            self.objects.get(id).and_then(SimObject::group) == Some(self.objects.datablock_group());
        if !in_group {
            return;
        }
        self.objects.add_work(block.len() as u64);
        let Some((key, file)) = self
            .script_files
            .files
            .get_key_value(fold(block).as_slice())
        else {
            return;
        };
        let key = Rc::clone(key);
        if !file.declares_datablocks
            && let Some(file) = self.script_files.files.get_mut(&key)
        {
            file.declares_datablocks = true;
        }
        self.script_files.declared_in.insert(id, key);
    }

    /// The script file `exec` ran at `path` (an expanded name, compared without case), with the change time it had then.
    pub fn script_file(&self, path: &[u8]) -> Option<&ScriptFile> {
        self.script_files.files.get(fold(path).as_slice())
    }

    /// The script files `exec` ran whose code declared a datablock, by path.
    pub fn script_files_declaring_datablocks(&self) -> Vec<ScriptFile> {
        let mut files: Vec<ScriptFile> = self
            .script_files
            .files
            .values()
            .filter(|f| f.declares_datablocks)
            .cloned()
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }

    /// The live datablocks whose last declaration was in the file at `path` (an expanded name, compared without case),
    /// in id order; the entries of datablocks that are gone are dropped on the way.
    pub fn datablocks_declared_by(&mut self, path: &[u8]) -> Vec<ObjId> {
        let key = fold(path);
        let objects = &self.objects;
        let declared = &mut self.script_files.declared_in;
        declared.retain(|id, _| objects.contains(*id));
        let mut ids: Vec<ObjId> = declared
            .iter()
            .filter(|(_, file)| file.as_ref() == key.as_slice())
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Starts (`on`) or ends a trace of the objects ADD_OBJECT registers: ending one returns the objects it took, in
    /// order, and how many more there were past [`MAX_TRACED_OBJECTS`]. Starting one again empties it.
    pub fn trace_new_objects(&mut self, on: bool) -> (Vec<ObjId>, usize) {
        let taken = self.script_files.traced.take().unwrap_or_default();
        if on {
            self.script_files.traced = Some((Vec::new(), 0));
        }
        taken
    }

    /// Forgets every script file and declaration recorded (`deleteDataBlocks` does this).
    pub fn clear_script_files(&mut self) {
        self.script_files.files.clear();
        self.script_files.declared_in.clear();
    }

    /// The path as the runtime-wide string table has it: the folder (everything before the last `/`) and the name
    /// each in the first spelling the table has of them, ignoring case (`ResManager::find` interns both:
    /// TGE's `getPaths`). Out of line, and only the table's own cap (16 MiB) bounds what it keeps.
    #[inline(never)]
    pub(crate) fn intern_path(&mut self, path: &[u8]) -> Vec<u8> {
        match path.iter().rposition(|&b| b == b'/') {
            None => self.strings.intern(path).to_vec(),
            Some(slash) => {
                let folder = self.strings.intern(&path[..slash]);
                let name = self.strings.intern(&path[slash + 1..]);
                let mut out = Vec::with_capacity(path.len());
                out.extend_from_slice(&folder);
                out.push(b'/');
                out.extend_from_slice(&name);
                out
            }
        }
    }

    /// Puts a path's folder and name in the string table (what every look-up does before it asks for the file).
    #[inline(never)]
    pub(super) fn note_path(&mut self, path: &[u8]) {
        let _ = self.intern_path(path);
    }

    /// Puts the names of the host's list in the string table, in the order the start-up scan found them
    /// (the list is the scan reversed): the first spelling of each folder and name is then the scan's, before
    /// any script has stored a string (a disk file lists under the spelling of a zip entry scanned before it).
    pub(super) fn seed_names(&mut self) {
        let list = self.files.files();
        for name in list.iter().rev() {
            let _ = self.intern_path(name);
        }
    }

    /// The compiler's messages, as log lines (cut at the console's line length like every other).
    fn print_report(&mut self, report: &[LogLine]) {
        for line in report {
            self.log_bytes(line.level, &line.text);
        }
    }

    /// One compile of source text: charges the fixed cost of a compile (`COMPILE_BASE_WORK`: a thread is
    /// started whatever the text) and the compiler's work (the bytes of the text were charged when
    /// the file was read, or by `prepare_text`), prints its messages and gives the code and how the compile
    /// ended ([`Verdict`]). Out of line, and only called from frames that have returned before any
    /// script runs (`choose_exec`, `prepare_text`, `check_source`): the compiler's whole result never sits
    /// in a frame of the recursion path.
    ///
    /// Inside a run a spent step budget stops here, before the compile (`ensure_budget`: the bytes of the
    /// text, or the work of the compile before this one, are counted but not yet charged, because the
    /// interpreter charges at its next instruction): `exec` compiles twice with no instruction between, and
    /// a text that alone costs more than the budget is not compiled at all. Outside a run `steps` is the last
    /// run's and nothing is billed, so there is nothing to check.
    #[inline(never)]
    fn compile_source(
        &mut self,
        text: &[u8],
        name: Option<&[u8]>,
        purpose: Purpose,
    ) -> (Option<DsoFile>, Verdict) {
        let (compiled, verdict) = self.compile_with(text, name, purpose, None);
        (compiled.code, verdict)
    }

    /// [`compile_source`](Self::compile_source), keeping the whole result. With `again` (a [`Purpose::Check`] result of
    /// the same text that is [`same_as_run`](Compiled::same_as_run)) a [`Purpose::Run`] compile does not run the
    /// compiler a second time: it bills, prints and gives what that one did.
    #[inline(never)]
    fn compile_with(
        &mut self,
        text: &[u8],
        name: Option<&[u8]>,
        purpose: Purpose,
        again: Option<Compiled>,
    ) -> (Compiled, Verdict) {
        if self.running > 0 && self.ensure_budget().is_err() {
            return (Compiled::default(), Verdict::OutOfBudget);
        }
        // The fixed cost of starting a compile (a thread with a big stack), whatever the text: see
        // `COMPILE_BASE_STEPS`. After the budget check, so a compile that has the budget for its text still starts
        // (as before) and the next one is the one that cannot.
        self.objects.add_work(COMPILE_BASE_WORK);
        let compiled = match again {
            Some(compiled) => compiled,
            None => self.compiler.compile(text, name, purpose),
        };
        self.objects.add_work(compiled.work);
        self.print_report(&compiled.report);
        let verdict = if compiled.syntax_error {
            Verdict::SyntaxError
        } else {
            Verdict::Clean
        };
        (compiled, verdict)
    }

    /// Runs a loaded block from its start in the current local frame (as `exec` and `eval` do) and
    /// returns the string its `return` left. Shares the budgets of the run in progress; inside one it
    /// costs the re-entry units of `invoke`.
    #[inline(never)]
    fn run_here(&mut self, block: Rc<CodeBlock>, mode: Mode) -> Result<(), VmError> {
        let saved = self.enter_run();
        let result = self.run_block(&block, 0, None, mode);
        self.leave_run(saved, result)
    }

    /// `exec(fileName, nocalls)`: checks the name, prefers the compiled
    /// `<name>.dso` (when it is at least as new as the source, or there is no source; a source that is newer
    /// is compiled first only for its syntax report, then the old `.dso` still runs), else compiles and runs
    /// the source: compiled twice, as TGE does (`CodeBlock::compile`, whose messages come before
    /// `Executing <name>.`, then `compileExec`, whose messages come after it; a `.mis` only once), and the
    /// statements before a syntax error run. The top level runs in the caller's local frame; a compiled
    /// file's top level is trusted (see `protected`). True when something ran (a syntax error does not
    /// change that). No `.dso` is written.
    ///
    /// On the recursion path of a file that execs files: everything but running the block happens in
    /// `prepare_exec`, which has returned by then, so this frame stays small.
    pub fn exec_file(&mut self, name: &[u8], no_calls: bool) -> Result<bool, VmError> {
        match self.prepare_exec(name, no_calls, ExecWay::Tge)? {
            Prepared::Done(ran) => Ok(ran),
            Prepared::OutOfBudget => Err(VmError::StepLimit),
            Prepared::SyntaxError => Ok(true),
            Prepared::Run(block, mode) => {
                self.run_here(block, mode)?;
                Ok(true)
            }
            Prepared::Foreign(foreign) => {
                self.run_foreign(*foreign)?;
                Ok(true)
            }
        }
    }

    /// [`exec_file`](Self::exec_file) for a host that runs a file again because it changed: the source is run when there
    /// is one (a compiled `.dso` beside it is not preferred, however new), it is compiled once, and a source with a
    /// syntax error runs nothing (`exec` runs the statements before the error): the error is printed and the result says
    /// so. The name is expanded and checked as `exec` checks it, so the file comes from the same roots.
    ///
    /// # Errors
    /// The run's budget was spent, or the file's code stopped the run.
    pub fn exec_file_checked(&mut self, name: &[u8]) -> Result<ExecCheck, VmError> {
        match self.prepare_exec(name, false, ExecWay::Checked)? {
            Prepared::Done(true) => Ok(ExecCheck::Ran),
            Prepared::Done(false) => Ok(ExecCheck::NotRun),
            Prepared::OutOfBudget => Err(VmError::StepLimit),
            Prepared::SyntaxError => Ok(ExecCheck::SyntaxError),
            Prepared::Run(block, mode) => {
                self.run_here(block, mode)?;
                Ok(ExecCheck::Ran)
            }
            Prepared::Foreign(foreign) => {
                self.run_foreign(*foreign)?;
                Ok(ExecCheck::Ran)
            }
        }
    }

    /// Runs a file of another language inside a run, as `run_here` runs a block.
    #[inline(never)]
    fn run_foreign(&mut self, foreign: Foreign) -> Result<(), VmError> {
        let Foreign {
            language,
            path,
            source,
        } = foreign;
        self.objects.add_work(source.len() as u64);
        let saved = self.enter_run();
        let result = language.run(self, &path, &source);
        self.leave_run(saved, result)
    }

    /// `exec`'s lookup of a file another language runs: the source, its messages, or nothing to run.
    #[inline(never)]
    fn prepare_foreign(
        &mut self,
        path: &[u8],
        language: Rc<dyn crate::policy::ScriptLanguage>,
    ) -> Result<Prepared, VmError> {
        let name = self.file_name(path);
        let Some(source) = self.find_file(path)? else {
            self.warn(format!("Missing file: {}!", lossy(&name)));
            return Ok(Prepared::Done(false));
        };
        self.note_executed(path);
        self.print(format!("Executing {}.", lossy(&name)).as_bytes());
        Ok(Prepared::Foreign(Box::new(Foreign {
            language,
            path: name,
            source,
        })))
    }

    /// Everything `exec` does before running the file: the checks, the lookups, the messages, the
    /// compiling. A lookup the resource manager refuses runs a BackTrace, which can run a script when the host replaced
    /// `backtrace` and that script can `exec` again: this frame is live then, once per level, so it only
    /// holds what the lookups return; the checks and the choice between source and compiled file are in
    /// helpers that have returned before any lookup that can run a script.
    #[inline(never)]
    fn prepare_exec(
        &mut self,
        name: &[u8],
        no_calls: bool,
        way: ExecWay,
    ) -> Result<Prepared, VmError> {
        let path = self.expand_or_report(name);
        let Some(compiled) = self.check_exec_name(&path) else {
            return Ok(Prepared::Done(false));
        };
        if let Some(policy) = self.policies.exec.clone()
            && !policy.check(self, &path)?
        {
            return Ok(Prepared::Done(false));
        }
        if let Some(language) = self.script_language_of(&path) {
            return self.prepare_foreign(&path, language);
        }
        // The source, then the compiled file. (A loop: one call site, one set of temporaries in this frame.)
        let mut found: [Option<Vec<u8>>; 2] = [None, None];
        for (step, slot) in found.iter_mut().enumerate() {
            if step == 1 && !compiled {
                break;
            }
            *slot = self.find_file(&lookup_path(&path, step == 1))?;
        }
        let [source, dso] = found;
        if source.is_some() || dso.is_some() {
            self.note_executed(&path);
        }
        Ok(self.choose_exec(&path, compiled, (source, dso), no_calls, way))
    }

    /// The checks `exec` makes on the expanded name (printed before it looks at any file). None
    /// when the name is refused, after printing why; otherwise whether a compiled `.dso` may exist.
    #[inline(never)]
    fn check_exec_name(&mut self, path: &[u8]) -> Option<bool> {
        let Some(dot) = path.iter().rposition(|&b| b == b'.') else {
            self.error(format!("exec: invalid script file name {}.", lossy(path)));
            return None;
        };
        // A mission file has no compiled form (as Torque3D's `exec`).
        Some(fold(&path[dot..]) != b".mis")
    }

    /// What `exec` does with what the lookups found (`source`, `.dso`): the compiled file when it is at
    /// least as new as the source or there is no source, else the source; the messages and the compiling.
    /// Nothing here runs script code; a step budget that the bytes read or an earlier compile spent stops it
    /// (`compile_source`). [`ExecWay::Checked`] is [`Vm::exec_file_checked`]'s way: a source is the one run, compiled
    /// once, and a syntax error runs nothing.
    #[inline(never)]
    fn choose_exec(
        &mut self,
        path: &[u8],
        compiled: bool,
        (source, dso): (Option<Vec<u8>>, Option<Vec<u8>>),
        no_calls: bool,
        way: ExecWay,
    ) -> Prepared {
        let checked = way == ExecWay::Checked;
        let name = self.file_name(path);
        let dso_path = dso_path(path);
        let mut loadable = None;
        // The first compile's result, when the second can be that same one.
        let mut again = None;
        let mut dso = dso;
        if dso.is_some()
            && (source.is_none()
                || (!checked && self.files.modified(&dso_path) >= self.files.modified(path)))
        {
            loadable = dso
                .take()
                .and_then(|bytes| self.checked_dso(&dso_path, bytes, OtherVersion::Report));
        }
        if let Some(text) = &source
            && loadable.is_none()
        {
            if text.is_empty() {
                self.error(format!("exec: invalid script file {}.", lossy(&name)));
                return Prepared::Done(false);
            }
            if compiled && !checked {
                // `CodeBlock::compile`: only its messages (nothing is written), and an existing (older) DSO
                // runs after all. A `.mis` has no compiled form and skips this pass.
                let (first, verdict) = self.compile_with(text, Some(&name), Purpose::Check, None);
                if verdict == Verdict::OutOfBudget {
                    return Prepared::OutOfBudget;
                }
                again = Some(first).filter(|first| first.same_as_run);
                loadable =
                    dso.and_then(|bytes| self.checked_dso(&dso_path, bytes, OtherVersion::Skip));
            }
        }
        let mode = Mode::top_level(no_calls);
        if let Some(bytes) = loadable {
            self.print(format!("Loading compiled script {}.", lossy(&name)).as_bytes());
            return match self.load_dso(&dso_path, &bytes, &name) {
                Some(block) => Prepared::Run(block, mode.trusted()),
                None => Prepared::Done(false),
            };
        }
        let Some(text) = source else {
            self.warn(format!("Missing file: {}!", lossy(&name)));
            return Prepared::Done(false);
        };
        self.print(format!("Executing {}.", lossy(&name)).as_bytes());
        // `compileExec`: its messages again, and the code of the statements before any syntax error.
        let (compiled, verdict) = self.compile_with(&text, Some(&name), Purpose::Run, again);
        if verdict == Verdict::OutOfBudget {
            return Prepared::OutOfBudget;
        }
        if checked && compiled.syntax_error {
            return Prepared::SyntaxError;
        }
        match compiled.code {
            Some(code) => match self.load_compiled(code, Some(&name)) {
                Some(block) => Prepared::Run(block, mode),
                // A block that does not load still counts as executed.
                None => Prepared::Done(true),
            },
            None => Prepared::Done(true),
        }
    }

    /// The DSO's bytes when its version is the executor's dialect's and it is at most [`MAX_DSO_BYTES`] (every `.dso`
    /// `exec` loads passes here). A file of another version is refused as `other` says (TGE also shows a
    /// "Script/Executable version mismatch" message box and quits, which this does not); an oversized one with an error.
    fn checked_dso(&mut self, path: &[u8], bytes: Vec<u8>, other: OtherVersion) -> Option<Vec<u8>> {
        let version = dso_version(&bytes);
        let wanted = self.executor.format().version();
        if version != wanted {
            if other == OtherVersion::Report {
                self.error(format!(
                    "ERROR: DSO file \"{}\" is version {version}, expected version {wanted}",
                    lossy(path)
                ));
            }
            return None;
        }
        if bytes.len() > MAX_DSO_BYTES {
            self.error(format!(
                "ERROR: DSO file \"{}\" is larger than {MAX_DSO_BYTES} bytes",
                lossy(path)
            ));
            return None;
        }
        Some(bytes)
    }

    /// Parses and loads a DSO; a damaged one is reported and gives None.
    fn load_dso(&mut self, path: &[u8], bytes: &[u8], name: &[u8]) -> Option<Rc<CodeBlock>> {
        let executor = Rc::clone(&self.executor);
        match executor.format().parse(bytes) {
            Ok(dso) => self.load_compiled(dso, Some(name)),
            Err(e) => {
                self.error(format!(
                    "exec: cannot load compiled script {}: {e}",
                    lossy(path)
                ));
                None
            }
        }
    }

    fn load_compiled(&mut self, dso: DsoFile, name: Option<&[u8]>) -> Option<Rc<CodeBlock>> {
        let executor = Rc::clone(&self.executor);
        match CodeBlock::from_dso_owned(dso, name, executor.format()) {
            Ok(mut block) => {
                // Loading a block interns each identifier (`StringTable->insert`, case-insensitive), so every name of the
                // block is spelled as the string table first saw it anywhere: a function, a variable or a field named by two
                // files in two cases prints the first file's spelling in both.
                block.respell_names(|n| self.strings.intern(n));
                // The code counts in the memory account while anything runs it (an `eval`'s functions keep it alive).
                block.hold(self.objects.memory()).then(|| Rc::new(block))
            }
            Err(e) => {
                let shown = name.map_or_else(|| "<input>".to_string(), lossy);
                self.error(format!("exec: cannot load compiled script {shown}: {e}"));
                None
            }
        }
    }

    /// `eval(text)`: compiles the text without a file name (`<input>`), runs what it got in
    /// the current local frame (the statements before a syntax error too) and returns what its `return`
    /// left ("" without one, and "" after a syntax error whatever the code before it returned). A
    /// syntax error is reported by the compiler, and after the run by `eval error >> <argv[1]>` (under 1000
    /// bytes), a BackTrace and a blank line (an executor may report it its own way, [`Vm::set_eval_report`]).
    /// This host entry prints its text; the native `eval` (`eval_with`) tells the report which
    /// argument slot the call came from.
    pub fn eval(&mut self, text: &[u8]) -> Result<Vec<u8>, VmError> {
        self.eval_with(text, None)
    }

    /// [`eval`](Self::eval) for the native `eval`: `slot` is the argument slot its report reads
    /// (see [`Vm::eval`]'s doc): 0 when script bytecode called it, 1 when `call("eval", ...)` did, None for a host
    /// call, which has an argument vector of its own and prints the text.
    pub(crate) fn eval_with(
        &mut self,
        text: &[u8],
        slot: Option<usize>,
    ) -> Result<Vec<u8>, VmError> {
        self.quirks.syntax_error = false;
        self.quirks.evals = self.quirks.evals.wrapping_add(1);
        let (block, verdict) = self.prepare_text(text, None);
        if verdict == Verdict::OutOfBudget {
            return Err(VmError::StepLimit);
        }
        self.quirks.syntax_error |= verdict == Verdict::SyntaxError;
        let mut result = match block {
            Some(block) => {
                self.run_here(block, Mode::PLAIN)?;
                self.executor.result()
            }
            None => Vec::new(),
        };
        // The report runs when the compile of this text had a syntax error. An `eval` inside the run of a bad text,
        // if it compiles clean, leaves the flag clear, and one with an error of its own leaves it set. The BackTrace
        // the report runs clears nothing. Only `eval` sets and clears it here, an `exec` of a bad file does not.
        if self.quirks.syntax_error {
            self.report_eval_error(text, slot)?;
            // The native runs a second compile for the BackTrace before it returns the first one's result: ""
            // even when the text `return`ed a value before its error. (Cleared in place: this frame is live at every
            // level of a host-replaced BackTrace that evals again, so it gets no new local.)
            result.clear();
        }
        Ok(result)
    }

    /// Compiles `eval`'s or `compile_exec`'s text as `compileExec` does (out of line, see `prepare_exec`): the
    /// block to run (the statements before any syntax error) and how the compile ended. `name` is the name
    /// the compiler's messages and the block carry (None for `eval`'s `<input>`). The text's bytes are
    /// charged here, the compiler's work in `compile_source`.
    #[inline(never)]
    fn prepare_text(
        &mut self,
        text: &[u8],
        name: Option<&[u8]>,
    ) -> (Option<Rc<CodeBlock>>, Verdict) {
        self.objects.add_work(text.len() as u64);
        // The name enters the string table before anything is compiled (after a start-up that ran `main.cs`,
        // `exec("MAIN.CS")` says `Missing file: main.cs!`), so the first spelling of it wins from
        // here on, in the compiler's messages, the block's paths and `exec`'s. An `eval`'s text has no name.
        let name = name.map(|n| self.file_name(n));
        let (code, verdict) = self.compile_source(text, name.as_deref(), Purpose::Run);
        let block = code.and_then(|code| self.load_compiled(code, name.as_deref()));
        (block, verdict)
    }

    /// `CodeBlock::compileExec(name, text)` as a host's start-up calls it for a script it embeds: compiles `text`
    /// under the name `name` (the block's name, which the compiler's messages and the `./` paths of its `exec`s use),
    /// runs the statements before any syntax error as `eval` does (the report first, then the statements)
    /// in the current local frame, and returns what its `return` left (without one, as `eval`: "" from a
    /// script, and from the host whatever string the string stack has current). Unlike `eval` it prints no
    /// `eval error >>`, BackTrace or blank line, and no `Executing` line (that is `exec`'s): a syntax error is
    /// the compiler's report and nothing more. The name and the text are C strings: each
    /// ends at its first NUL. An empty text runs nothing and prints nothing. The name is entered in the
    /// file-name table `exec` keeps (`file_name`) before anything is compiled (a later `exec("MAIN.CS")` of a
    /// start-up's `main.cs` says `Missing file: main.cs!`): its first spelling is the one every later message and
    /// path shows.
    ///
    /// Unlike `eval`'s native, `compile_exec` returns the run's result after a syntax error too: the native
    /// loses it by running a second compile before it returns.
    ///
    /// A host calls it outside any run (its step limit and call depth start fresh, and its compile is not
    /// billed, as `exec_file`'s is not); from a native inside a run it shares that run's budgets, charges the
    /// text's bytes and `COMPILE_BASE_STEPS` for the compile itself, and stops with `VmError::StepLimit` before
    /// compiling when the budget is spent, as `eval` does. `Err` is the run's (`StepLimit`, `CallDepth`, ...);
    /// what the compiler said is in the log.
    pub fn compile_exec(&mut self, name: &[u8], text: &[u8]) -> Result<Vec<u8>, VmError> {
        let (block, verdict) = self.prepare_text(text, Some(until_nul(name)));
        if verdict == Verdict::OutOfBudget {
            return Err(VmError::StepLimit);
        }
        match block {
            Some(block) => {
                self.run_here(block, Mode::PLAIN)?;
                Ok(self.executor.result())
            }
            None => Ok(Vec::new()),
        }
    }

    /// The report of an `eval` whose text had a syntax error: the executor's ([`Vm::set_eval_report`]), else the
    /// `eval error >> <text>` line (none for a text of [`EVAL_ERROR_TEXT_BYTES`] or more), a BackTrace and a blank
    /// line. `slot` is what the native `eval` was told about its caller (see [`Vm::set_eval_report`]).
    #[cold]
    #[inline(never)]
    fn report_eval_error(&mut self, text: &[u8], slot: Option<usize>) -> Result<(), VmError> {
        if let Some(report) = self.quirks.eval_report.0.clone() {
            return report(self, text, slot);
        }
        if self.console_rules().backtraces {
            if text.len() < EVAL_ERROR_TEXT_BYTES {
                self.eval_error_line(Some(text));
            }
            self.print_backtrace()?;
            self.print(b"");
        }
        Ok(())
    }

    /// Prints the `eval error >> <argument>` line (`(null)` for None). The bytes as they are: the log is bytes,
    /// and a script can print any (the line is formatted with `%s` over the raw string). Out of line: the
    /// frames of the report are live at every level of a host-replaced BackTrace that evals again.
    #[inline(never)]
    pub fn eval_error_line(&mut self, argument: Option<&[u8]>) {
        const HEAD: &[u8] = b"eval error >> ";
        let line = [HEAD, argument.unwrap_or(b"(null)")].concat();
        self.log_bytes(Level::Error, &line);
    }

    /// Replaces the report of an `eval` whose text had a syntax error (see `report_eval_error`): `report` is
    /// given the text and the slot the native `eval` was given (`Some(0)` when script bytecode called it,
    /// `Some(1)` through `call("eval", ...)`, None for a host's [`Vm::eval`]).
    pub fn set_eval_report(&mut self, report: Option<EvalReport>) {
        self.quirks.eval_report.0 = report;
    }

    /// `compile(fileName)`: compiles the source as `CodeBlock::compile` does, for its messages;
    /// writes nothing. False when the file cannot be read (`exec: invalid script file ...`) or has a syntax error
    /// (its report only).
    pub fn compile_file(&mut self, name: &[u8]) -> Result<bool, VmError> {
        let path = self.expand_or_report(name);
        // The source and the DSO (unused) are looked up before the source is read.
        self.find_file(&path)?;
        self.find_file(&dso_path(&path))?;
        let source = self.find_file(&path)?.filter(|s| !s.is_empty());
        let Some(text) = source else {
            self.error(format!("exec: invalid script file {}.", lossy(&path)));
            return Ok(false);
        };
        match self.check_source(&text, &path) {
            Verdict::Clean => Ok(true),
            Verdict::SyntaxError => Ok(false),
            Verdict::OutOfBudget => Err(VmError::StepLimit),
        }
    }

    /// `compile()`'s compile, out of line: this native's frame is live at every level of a host-replaced
    /// BackTrace that compiles a refused path again (see `find_file`), and must not hold the compiler's
    /// result. True when the source has no syntax error.
    #[inline(never)]
    fn check_source(&mut self, text: &[u8], path: &[u8]) -> Verdict {
        let (dso, verdict) = self.compile_source(text, Some(path), Purpose::Check);
        // The compiler interns every identifier it lexes (`StringTable->insert`, case-insensitive), so a `compile()` that
        // runs nothing still decides the first spelling of each name: a later script's name reads in the spelling of the
        // first file compiled.
        let executor = Rc::clone(&self.executor);
        if let Some(dso) = dso
            && let Ok(mut block) = CodeBlock::from_dso(&dso, Some(path), executor.format())
        {
            block.respell_names(|n| self.strings.intern(n));
        }
        verdict
    }
}
