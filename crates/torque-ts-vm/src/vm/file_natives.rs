//! The file functions: queries and searches over the files of the [`FileHost`](crate::host::FileHost), deleting,
//! the modification times, and the four name splitters.
//!
//! The answers come from the resource manager's list, which records what it saw. A copy is found by `isFile`
//! but not listed by the searches until something looks it up: this lists what the host lists. The length a
//! file's resource recorded can differ from the file's ([`FileRules`](crate::policy::FileRules)). The searches
//! name a file by the runtime-wide first spelling of its folder and its name (`Vm::intern_path`: the string
//! table, which the host's list seeds in the order of the start-up scan and every look-up adds to). The other
//! file natives expand names against the executing file.
//!
//! Everything that reaches a file goes through the host, which is the only code that touches a folder:
//! the queries and the searches read (`stat`, `read`, `modified`, `files`), and `fileDelete` first applies the
//! file rules' name checks and then [`safe_path`] (no `\`,
//! `:`, `..`, device names ...), so a name such as `config/..\..\x` never reaches `FileHost::remove` or
//! `copy`. The searches are charged for every name
//! they look at.
//!
//! **What a host call costs.** Every call into the host that can do work is bracketed by `FileHost::work_done`
//! (`host_stat`, `host_remove`, `host_write`, `copy_file`, `next_match` for the list, and `read_one` in `files`): what
//! the host did that returned no bytes (a zip inflated for a read that failed, the file an append started from) is
//! charged besides the bytes a read returns. A copy is charged the source's length (a `stat` first) and `FILE_OP_UNITS`
//! before the host reads anything, so a budget that cannot pay copies nothing; opening a file for writing and each line
//! written are charged `FILE_OP_UNITS` and the name's bytes.
//!
//! **Not modelled: the 260-byte path limit** of Windows builds, which make `<working directory>/<name>` and do
//! nothing past 259 bytes. The [`FileHost`](crate::host::FileHost) trait has no working directory, so its length
//! is not known here and the limit is not applied: a host that can delete or copy (`MemFiles`; `DirHost` is
//! read-only) receives a long name and may refuse it itself.
//!
//! Follows Torque3D's MIT-licensed `console/fileSystemFunctions.cpp` (Copyright (c) 2012 GarageGames, LLC); the code
//! here is this crate's own.

use super::{Native, Vm, VmError, WORK_PER_STEP, lossy};
use crate::classes::file_object::Flush;
use crate::host::{CivilTime, FileStat, civil_time, safe_write_path};
use crate::policy::WRITE_WINDOW_MS;
use crate::sim::ObjId;
use crate::text::{crc32, is_match, match_cost};

/// What the host costs when a script opens a file for writing, writes a line, or copies a file, besides the
/// bytes it moves and the name's own length: one step. Making a file or finding its place in the searches'
/// list takes the host time a call's instructions do not show.
pub(crate) const FILE_OP_UNITS: u64 = WORK_PER_STEP;

/// `fileExt`: from the last `.` (wherever it is, even before a `/`), or "".
fn file_ext(s: &[u8]) -> &[u8] {
    s.iter()
        .rposition(|&b| b == b'.')
        .map_or(&b""[..], |i| &s[i..])
}

/// The part after the last `/` (only `/` separates).
fn file_name(s: &[u8]) -> &[u8] {
    s.iter()
        .rposition(|&b| b == b'/')
        .map_or(s, |i| &s[i + 1..])
}

/// `fileBase`: the name without its last extension.
pub(crate) fn file_base(s: &[u8]) -> &[u8] {
    let name = file_name(s);
    name.iter()
        .rposition(|&b| b == b'.')
        .map_or(name, |i| &name[..i])
}

/// `filePath`: the part before the last `/`, or "".
fn file_path(s: &[u8]) -> &[u8] {
    s.iter()
        .rposition(|&b| b == b'/')
        .map_or(&b""[..], |i| &s[..i])
}

impl Vm {
    /// Deletes `path` through the host, for `fileDelete` once its checks passed: the resource goes from the list
    /// whether or not the file does (its recorded length with it), and the next look-up makes it again from the disk
    /// with the real size, as it is THEN (a file a writer holds is not deleted, and keeps the size the first look-up
    /// after the delete finds). True when the host removed the file.
    pub fn delete_file(&mut self, path: &[u8]) -> bool {
        if self.file_objects.is_locked(path) {
            self.policies.files.relist(path);
        } else {
            self.policies.files.forget(path);
        }
        self.host_remove(path)
    }

    /// `ResManager::find` without reading: what the host knows, after printing a refusal as the resource manager
    /// does.
    pub fn stat_file(&mut self, path: &[u8]) -> Result<FileStat, VmError> {
        self.note_lookup(path);
        let stat = self.host_stat(path);
        if let FileStat::Refused(refusal) = stat {
            self.report_refusal(path, refusal)?;
        }
        if let FileStat::Found { len } = stat {
            self.policies.files.listed(path, len);
        }
        Ok(stat)
    }

    /// What `ResManager::find` does with a path before it looks at anything: its folder and its name
    /// go into the runtime-wide string table (`getPaths`: the first spelling of each stays, whatever the case of
    /// the next look-up), and a file a copy made becomes part of the list ([`RecordedLengths::looked_up`]).
    /// Out of line: the callers are live while a refusal's BackTrace runs.
    ///
    /// [`RecordedLengths::looked_up`]: crate::policy::FileRules::looked_up
    #[inline(never)]
    pub(super) fn note_lookup(&mut self, path: &[u8]) {
        self.note_path(path);
        self.policies.files.looked_up(path);
    }

    /// A stat through the host, charged for the work it did (`FileHost::work_done`). Out of line: `stat_file`
    /// is live while a refusal's BackTrace runs (a host-replaced `backtrace` re-enters the interpreter from
    /// there) and holds only the answer.
    #[inline(never)]
    fn host_stat(&mut self, path: &[u8]) -> FileStat {
        let mark = self.host_work_mark();
        let stat = self.files.stat(path);
        self.charge_host_work(mark);
        stat
    }

    /// A delete through the host, charged for the work it did (`FileHost::work_done`). A file open for writing
    /// cannot be deleted (0, isFile 1, every line lands), and the host is not asked to remove it; it is
    /// told to take the file out of its list, as the resource manager does for the refused delete
    /// (until the next look-up of the path, `FileHost::unlist`).
    #[inline(never)]
    pub(super) fn host_remove(&mut self, path: &[u8]) -> bool {
        let mark = self.host_work_mark();
        if self.file_objects.is_locked(path) {
            self.files.unlist(path);
            self.charge_host_work(mark);
            return false;
        }
        let removed = self.files.remove(path);
        self.charge_host_work(mark);
        removed
    }

    /// Whether the [`WriteQuota`](crate::policy::WriteQuota) lets scripts open `files` more files for writing and write `bytes` more, which leave
    /// the file `file_size` bytes long, in the current window of [`WRITE_WINDOW_MS`] of simulation time; they are counted
    /// when it does.
    pub(crate) fn admit_write(&mut self, file_size: u64, bytes: u64, files: u32) -> bool {
        let quota = self.policies.files.write_quota();
        let now = self.sim_time();
        let window = &mut self.writes;
        if now.wrapping_sub(window.start) >= WRITE_WINDOW_MS {
            *window = WriteWindow {
                start: now,
                ..WriteWindow::default()
            };
        }
        let total = window.bytes.saturating_add(bytes);
        let opened = window.files.saturating_add(files);
        if file_size > quota.per_file || total > quota.per_hour || opened > quota.files_per_hour {
            return false;
        }
        window.bytes = total;
        window.files = opened;
        true
    }

    /// The line of a write the quota refused.
    #[cold]
    #[inline(never)]
    pub(crate) fn refuse_write_quota(&mut self, path: &str) {
        let line = self.messages().write_quota_used(path);
        self.error(line);
    }

    /// A write through the host, charged for the work it did beyond the bytes it was given
    /// (`FileHost::work_done`: the file an append starts from).
    #[inline(never)]
    pub(crate) fn host_write(&mut self, path: &[u8], contents: &[u8], append: bool) -> bool {
        let mark = self.host_work_mark();
        let wrote = self.files.write(path, contents, append);
        self.charge_host_work(mark);
        wrote
    }

    /// `ResManager::openFileForWrite` for `FileObject` (`FileObject::openForWrite`): the
    /// name checks (`check_write_path`: messages and BackTrace as `export`'s), then
    /// `ResManager::openFileForWrite - invalid write filename "<name>"` on a refusal; the host creates the file
    /// at once (empty unless `append`: `isFile` is 1 while it is open), or the message
    /// `... failed to open stream for "<name>"` says it cannot: the host cannot, or another object has the file
    /// open for writing, or [`MAX_OPEN_WRITERS`](crate::classes::file_object::MAX_OPEN_WRITERS)
    /// files are. False after printing why not. Charged a step and the name's bytes, and what the host did on
    /// top (an append starts from the file it has); a budget that cannot pay ends the run before the host does
    /// anything.
    #[inline(never)]
    pub(crate) fn open_for_write(
        &mut self,
        path: &[u8],
        append: bool,
    ) -> Result<Option<u64>, VmError> {
        if !self.charge_work(FILE_OP_UNITS.saturating_add(path.len() as u64)) {
            return Err(VmError::StepLimit);
        }
        self.note_path(path);
        // What follows is out of line: this frame is alive while a refusal's BackTrace runs (a host's replacement
        // of `backtrace` can run a script, which can open a file again), and holds none of the messages.
        if !self.check_write_path(path)? {
            return Ok(self.refuse_write_name(path));
        }
        if !self.admit_write(0, 0, 1) {
            self.refuse_write_quota(&lossy(path));
            return Ok(None);
        }
        Ok(self.begin_stream(path, append))
    }

    /// The refusal of a write name (`check_write_path` has said why); None.
    #[cold]
    #[inline(never)]
    fn refuse_write_name(&mut self, path: &[u8]) -> Option<u64> {
        let line = self.messages().invalid_write_name(&lossy(path));
        self.error(line);
        None
    }

    /// The lock and the host: the file is made (or emptied) and the stream starts at `Some(position)`, or the
    /// "failed to open stream" line is printed and None.
    #[inline(never)]
    fn begin_stream(&mut self, path: &[u8], append: bool) -> Option<u64> {
        // The lock comes before the host: a write of the file another object has open would empty it.
        if self.file_objects.can_open_write(path) && self.host_write(path, b"", append) {
            // Every `openFileForWrite` tells the file rules (a recorded size of 0).
            self.policies.files.zero(path);
            return Some(self.stream_start(path, append));
        }
        let line = self.messages().write_open_failed(&lossy(path));
        self.error(line);
        None
    }

    /// Where a stream opened on `path` starts: 0, or the file's size for an append (`FileObject::openForWrite`
    /// sets the position to the stream's size).
    fn stream_start(&mut self, path: &[u8], append: bool) -> u64 {
        if !append {
            return 0;
        }
        match self.host_stat(path) {
            FileStat::Found { len } => len,
            _ => 0,
        }
    }

    /// `FileObject::writeLine` of the file `id` has open for writing: the line and its CR LF go into the
    /// stream (`FileStream`, modelled by `FileObjects`: an 8 KiB block buffer, which gives the host
    /// what it flushes, and the rest at `close`; the file is locked and reads as empty until
    /// then). An object that is not writing writes nothing, silently, except one that holds a read stream with
    /// unread data (`isEOF()` false): that prints the `Messages` line for it, and a BackTrace when the rules ask
    /// then. Charged a step, the name's bytes and the line's, a budget that cannot pay ends the run
    /// before anything is kept.
    #[inline(never)]
    pub(crate) fn write_line(&mut self, id: ObjId, line: &[u8]) -> Result<(), VmError> {
        let Some((path, size)) = self.file_objects.write_target(id) else {
            if self.file_objects.has_unread(id) {
                return self.report_write_to_a_reader();
            }
            return Ok(());
        };
        let units = FILE_OP_UNITS
            .saturating_add(path.len() as u64)
            .saturating_add(line.len() as u64);
        if !self.charge_work(units) {
            return Err(VmError::StepLimit);
        }
        let bytes = line.len() as u64 + 2;
        if !self.admit_write(size.saturating_add(bytes), bytes, 0) {
            let path = self.file_objects.write_target(id).map(|(p, _)| lossy(p));
            self.refuse_write_quota(&path.unwrap_or_default());
            return Ok(());
        }
        if let Some(flush) = self.file_objects.buffer_line(id, line) {
            self.flush_file(flush);
        }
        Ok(())
    }

    /// The complaint about a `writeLine` on an object whose stream is not a write stream (no blank lines,
    /// unlike a native's range error).
    #[cold]
    #[inline(never)]
    fn report_write_to_a_reader(&mut self) -> Result<(), VmError> {
        let line = self.messages().write_line_while_reading();
        self.error(line);
        self.backtrace_after_message()
    }

    /// Deleting the object `id` (the class's behaviour): a read is given up; a file open for writing is NOT
    /// closed: its stream is left open, so its lock stays for the session and its buffered lines are
    /// lost; see `FileObjects::abandon`, which closes it normally past `MAX_LEAKED_LOCKS` of them.
    #[inline(never)]
    /// `readLine()` of the `FileObject` `id` for a host doing a script's work: the line and the bytes it passed over (which
    /// `readLine` charges; the caller charges them). None when the object reads no file.
    pub fn file_object_read_line(&mut self, id: ObjId) -> Option<(Vec<u8>, u64)> {
        self.file_objects.read_line(id)
    }

    /// `isEOF()` of the `FileObject` `id` (true when it reads no file).
    pub fn file_object_is_eof(&self, id: ObjId) -> bool {
        self.file_objects.is_eof(id)
    }

    /// Where the `FileObject` `id` reads (the offset of its next line); None when it reads no file.
    pub fn file_object_position(&self, id: ObjId) -> Option<usize> {
        self.file_objects.read_position(id)
    }

    /// Moves the `FileObject` `id` back to an offset [`file_object_position`](Self::file_object_position) gave, so that
    /// the next `readLine` reads that line again (a host that read a line and leaves it to a script). False when it
    /// reads no file or the offset is past where it is.
    pub fn file_object_rewind(&mut self, id: ObjId, at: usize) -> bool {
        self.file_objects.rewind_to(id, at)
    }

    pub(crate) fn abandon_file_object(&mut self, id: ObjId) {
        if !self.policies.files.leaks_deleted_writers() {
            return self.close_file_object(id);
        }
        if let Some(flush) = self.file_objects.abandon(id) {
            self.flush_file(flush);
        }
    }

    /// Closes what `id` has open: the lines a file open for writing still holds are appended to it through
    /// the host, and its lock goes. Also what deleting the object and `Vm::shutdown` do (the class's behaviour).
    #[inline(never)]
    pub(crate) fn close_file_object(&mut self, id: ObjId) {
        if let Some(flush) = self.file_objects.close(id) {
            self.flush_file(flush);
        }
    }

    /// Appends what a stream held to its file. A host that cannot (a full overlay) loses it, as a full disk
    /// would. Nothing is charged here but the host's own work: the bytes were charged as lines.
    fn flush_file(&mut self, flush: Flush) {
        self.host_write(&flush.path, &flush.bytes, true);
    }

    /// `fileCopy`'s copy, charged for what it moves: the source's length is asked first and paid before
    /// the host reads anything, so a budget that cannot pay for the bytes copies nothing (the host's
    /// `copy` reads a file of up to 256 MiB and writes it); the host's own count of work it did
    /// (`FileHost::work_done`: inflating a zipped source) is paid after. A budget that cannot pay ends the
    /// run (`StepLimit`). The caller has checked the names.
    #[inline(never)]
    pub fn copy_file(&mut self, from: &[u8], to: &[u8]) -> Result<bool, VmError> {
        // A copy onto a file another object writes cannot open it (the OS's sharing violation). A
        // copy FROM one copies what is on disk, which is what the host holds (an empty copy).
        if self.file_objects.is_locked(to) {
            return Ok(false);
        }
        // Neither name is looked up as a resource by the copy itself: the OS copies, so a copy is not
        // listed until something looks it up and a source that left the list stays out of it; the names are
        // interned (the host is asked about the destination as before, and about the source
        // without a look-up: `FileHost::stat_without_look_up`).
        self.note_path(from);
        self.note_path(to);
        let existed = match self.host_stat(to) {
            FileStat::Found { len } => Some(len),
            _ => None,
        };
        let mark = self.host_work_mark();
        let len = match self.files.stat_without_look_up(from) {
            FileStat::Found { len } => len,
            _ => 0,
        };
        self.charge_host_work(mark);
        // A copy FROM a file a live writer holds copies what is on the disk, but not from the stream of a DELETED
        // object, which was never closed, nor from an appender, live or not: with bytes of its file on the disk the
        // answer is 0 and nothing is made. One that has not reached the disk yet copies as an empty file.
        if len > 0 && self.file_objects.blocks_copy(from) {
            return Ok(false);
        }
        if !self.charge_work(FILE_OP_UNITS.saturating_add(len)) {
            return Err(VmError::StepLimit);
        }
        if !self.admit_write(len, len, 1) {
            self.refuse_write_quota(&lossy(to));
            return Ok(false);
        }
        let mark = self.host_work_mark();
        let copied = self.files.copy(from, to);
        self.charge_host_work(mark);
        if copied {
            self.policies.files.copied_onto(to, existed);
        }
        Ok(copied)
    }

    /// The first file of the host's list whose path matches `pattern` (the resource manager's own search,
    /// `ResManager::findMatch`: the scripts' `findNextFile` walk is left as it was), charged as `Vm::next_match`
    /// charges it.
    pub fn first_file_match(&self, pattern: &[u8]) -> Result<Option<Vec<u8>>, VmError> {
        Ok(self.next_match(pattern, 0)?.map(|(_, name)| name))
    }

    /// The next file of the host's list at or after `from` whose path matches `pattern`. Every name looked
    /// at costs one work unit plus the work of matching it ([`match_cost`], the bound of `is_match`),
    /// charged BEFORE the match, and the budget is checked after each, so a long list or a hostile
    /// pattern ends the run at the step limit instead of running to the end of the list. Outside a run, under a host
    /// budget ([`Vm::begin_host_budget`]), that budget pays (lookups a host makes between scripts, where the
    /// run's check would read the last run's leftover steps).
    fn next_match(&self, pattern: &[u8], from: usize) -> Result<Option<(usize, Vec<u8>)>, VmError> {
        // A host that keeps its list lazily does the work of bringing it up to date here (`FileHost::work_done`).
        let mark = self.host_work_mark();
        let files = self.files.files();
        self.charge_host_work(mark);
        let host_paid = self.host_budget_pays();
        for (i, name) in files.iter().enumerate().skip(from) {
            let units = 1u64.saturating_add(match_cost(pattern, name.len()));
            if host_paid {
                if !self.charge_work(units) {
                    return Err(VmError::StepLimit);
                }
            } else {
                self.objects.add_work(units);
                self.ensure_budget()?;
            }
            if is_match(pattern, name) {
                return Ok(Some((i, name.clone())));
            }
        }
        Ok(None)
    }

    /// `findFirstFile`/`findNextFile` (TGE's): one walk for every pattern. The
    /// next search starts after the last match; after the last one, "" and the walk starts again from the
    /// top (the first match comes back).
    fn find_file_from(&mut self, pattern: &[u8], first: bool) -> Result<Vec<u8>, VmError> {
        let from = if first {
            0
        } else {
            self.file_search.map_or(0, |i| i.saturating_add(1))
        };
        let pattern = self.expand_or_report(pattern);
        match self.next_match(&pattern, from) {
            Ok(Some((i, name))) => {
                self.file_search = Some(i);
                // The folder and the name as the string table has them first.
                Ok(self.intern_path(&name))
            }
            Ok(None) => {
                self.file_search = None;
                Ok(Vec::new())
            }
            Err(e) => {
                self.file_search = None;
                Err(e)
            }
        }
    }

    /// `getFileCount`: every match of the pattern as written (it is not expanded); resets the
    /// `findNextFile` walk.
    fn count_files(&mut self, pattern: &[u8]) -> Result<i32, VmError> {
        self.file_search = None;
        let mut count = 0i32;
        let mut from = 0;
        while let Some((i, _)) = self.next_match(pattern, from)? {
            count = count.saturating_add(1);
            from = i + 1;
        }
        Ok(count)
    }

    /// The file's modification time as local calendar time, or None (`getFileModifiedTime`).
    pub fn modified_civil(&mut self, name: &[u8]) -> Option<CivilTime> {
        let path = self.expand_or_report(name);
        let nanos = self.files.modified(&path)?;
        let secs = i64::try_from(nanos / 1_000_000_000).ok()?;
        let local = secs.saturating_add(i64::from(self.clock.utc_offset(secs)));
        Some(civil_time(local))
    }
}

/// A search or a count as a native's result.
fn counted(result: Result<i32, VmError>) -> Native {
    match result {
        Ok(n) => Native::Int(n),
        Err(e) => Native::Fail(e),
    }
}

fn found(result: Result<Vec<u8>, VmError>) -> Native {
    match result {
        Ok(name) => Native::Str(name),
        Err(e) => Native::Fail(e),
    }
}

pub(super) fn register(vm: &mut Vm) {
    vm.add_native_effects("", "fileDelete", crate::effects::Effects::FILES);
    vm.register(
        "isFile",
        2,
        2,
        "isFile(file) - whether the file exists",
        |vm, c| {
            let path = vm.expand_or_report(c.cstr(1));
            match vm.stat_file(&path) {
                Ok(stat) => Native::Bool(matches!(stat, FileStat::Found { .. })),
                Err(e) => Native::Fail(e),
            }
        },
    );
    // The CRC-32 complemented (no final xor), -1 for an empty or missing file, and while the file rules
    // record a length of 0, without looking at the file.
    vm.register(
        "getFileCRC",
        2,
        2,
        "getFileCRC(file) - the file's checksum",
        |vm, c| {
            let path = vm.expand_or_report(c.cstr(1));
            if vm.policies.files.is_zero(&path) {
                vm.note_lookup(&path);
                return Native::Int(-1);
            }
            let found = match vm.find_file(&path) {
                Ok(found) => found,
                Err(e) => return Native::Fail(e),
            };
            match found {
                Some(bytes) if !bytes.is_empty() => {
                    vm.objects.add_work(bytes.len() as u64);
                    Native::Int(!crc32(&bytes) as i32)
                }
                _ => Native::Int(-1),
            }
        },
    );
    vm.register(
        "findFirstFile",
        2,
        2,
        "findFirstFile(pattern) - the first file whose path matches",
        |vm, c| found(vm.find_file_from(c.cstr(1), true)),
    );
    vm.register(
        "findNextFile",
        2,
        2,
        "findNextFile(pattern) - the next file of the search findFirstFile began",
        |vm, c| found(vm.find_file_from(c.cstr(1), false)),
    );
    vm.register(
        "getFileCount",
        2,
        2,
        "getFileCount(pattern) - how many files match",
        |vm, c| counted(vm.count_files(c.cstr(1))),
    );
    // TGE then asks the OS whether it could open the file for writing; this answers from
    // the name alone.
    vm.register(
        "isWriteableFileName",
        2,
        2,
        "isWriteableFileName(file) - whether a script may write that file",
        |vm, c| {
            let path = vm.expand_or_report(c.cstr(1));
            match vm.check_write_path(&path) {
                Ok(ok) => Native::Bool(ok),
                Err(e) => Native::Fail(e),
            }
        },
    );
    vm.register(
        "fileDelete",
        2,
        2,
        "fileDelete(path) - deletes a file",
        |vm, c| {
            let path = vm.expand_or_report(c.cstr(1));
            // The file is looked up first (printing a refusal), then deleted whatever the lookup said.
            if let Err(e) = vm.stat_file(&path) {
                return Native::Fail(e);
            }
            // Only a plain name inside the host's folders reaches the host (`config/..\\..\\x` does not).
            if safe_write_path(&path).is_none() {
                return Native::Bool(false);
            }
            Native::Bool(vm.delete_file(&path))
        },
    );
    vm.register(
        "fileExt",
        2,
        2,
        "fileExt(path) - the extension, from the last dot",
        |_, c| Native::Str(file_ext(c.cstr(1)).to_vec()),
    );
    vm.register(
        "fileBase",
        2,
        2,
        "fileBase(path) - the file name without its folder and extension",
        |_, c| Native::Str(file_base(c.cstr(1)).to_vec()),
    );
    // The name after the last slash, copied whole.
    vm.register(
        "fileName",
        2,
        2,
        "fileName(path) - the file name without its folder",
        |_, c| Native::Str(file_name(c.cstr(1)).to_vec()),
    );
    vm.register(
        "filePath",
        2,
        2,
        "filePath(path) - the folder part of the path",
        |_, c| Native::Str(file_path(c.cstr(1)).to_vec()),
    );
}

/// What scripts wrote in the current window of the [`WriteQuota`](crate::policy::WriteQuota): since `start` (simulation time), `bytes` written and
/// `files` opened for writing.
#[derive(Debug, Default)]
pub(crate) struct WriteWindow {
    start: u32,
    bytes: u64,
    files: u32,
}
