//! `FileObject`, following Torque3D's MIT-licensed `core/fileObject.cpp` and `core/stream/fileStream.cpp` (Copyright
//! (c) 2012 GarageGames, LLC):
//! a script's line reader and writer over the [`FileHost`](crate::host::FileHost).
//!
//! - The three opens expand `./` and `~/` against the executing file, as `exec` does, unless the file rules keep the
//!   name as written ([`FileRules::expand_file_object_names`](crate::policy::FileRules::expand_file_object_names)):
//!   the resource manager then refuses it (`ERROR: File "./Foo.cs" is not in one of the default directories`, a
//!   BackTrace; for a write, then `ResManager::openFileForWrite - invalid write filename "./w.txt"`; 0 and
//!   nothing made).
//! - `openForRead(name)`: the name is looked up as `exec` does (the resource manager's
//!   refusals print their message and a BackTrace); the whole file is read at once and held (`readMemory`), and
//!   charged one work unit per byte; 1, or 0 when it is missing, refused, an empty file of the disk, or open for
//!   writing by another object. A file opened before is closed first, also when this open fails (`close()` comes
//!   first). A read stream refreshes the recorded length of the file (below). A zip's folder entry, a name the host
//!   serves with its trailing `/` as a 0-byte file, does open: 1, a stream that is at its end (`isEOF()` 1,
//!   `readLine()` "") and over which `writeLine` is silent.
//! - `readLine()`: the bytes up to the next CR, LF or CR LF (a lone CR ends a line too), cut at a NUL (the
//!   result is a C string, and the read goes on to the line end all the same); the last line needs no line end;
//!   "" at the end, after `close` and while writing. Charged one unit per byte it scans.
//! - `isEOF()`: 1 at the end of what was read, while writing, and with nothing open.
//! - `openForWrite(name)` / `openForAppend(name)`: the write-name checks (`Vm::check_write_path`:
//!   messages and BackTrace as `export`'s), then `ResManager::openFileForWrite - invalid write filename
//!   "<name>"` on a refusal; the file is created (or emptied, for `openForWrite`) at once (`isFile`
//!   is 1 while it is open); a host that cannot write, a file another object has open for writing and
//!   [`MAX_OPEN_WRITERS`] open files print `... failed to open stream for "<name>"`. 1 or 0.
//! - `writeLine(text)`: `text` and CR LF go into the object's stream, which holds them (see below). Charged a
//!   step, the name's bytes and the line's. On an object that holds a read stream with unread data (`isEOF()`
//!   false) it prints the [`Messages`](crate::policy::Messages) line for it (and a BackTrace when the console rules
//!   ask for one); on any other object that is not writing it writes nothing, silently.
//! - `close()`: forgets the open file; a file open for writing gets its lines then. Opening another file with
//!   the object closes the first.
//!
//! **A file open for writing is buffered and locked** (`FileStream`): the lines are
//! in the stream, an 8 KiB block buffer aligned to multiples of 8192 of the file's offset (`Stream`: a write
//! fills the rest of the block and the block goes to the file; what does not fit starts a new block, or, when
//! it spans past the end of a block into an empty buffer, is written whole; `writeLine` is two writes, the line
//! and then CR LF, so 10000 bytes at offset 0 are on disk at once with their CR LF still in the buffer; an
//! appender starts at its file's size), and the rest reaches the file at `close`. The file is open exclusively:
//! it reads as an empty file (`openForRead` 0, `exec` "invalid script file", `getFileCRC` -1); `getFileLength`
//! is the recorded length (below); a copy of it copies what is on disk (what the blocks flushed). No other
//! object can open it for writing or appending, `export` cannot write it, `fileDelete` returns 0 (and leaves
//! the lines coming; it also takes the file out of the searches' list until its next look-up: the
//! host is told with `FileHost::unlist`) and a copy onto it fails (Windows' `CopyFile` cannot open
//! it). The stream lives in `FileObjects`, which appends what it flushes to the file through the host; so at most
//! [`MAX_OPEN_WRITERS`] x [`WRITE_BUFFER_BYTES`] of lines are held. The lock is a set of folded paths in the
//! `Vm` (`FileObjects::is_locked`) that `read_one`, `fileDelete`, copies, `export` and `open_for_write`
//! consult.
//!
//! **A deleted writer leaks its stream**: deleting an object (also with its group, and at
//! `Vm::shutdown`, which deletes everything) does not close a file it has open for writing. The lock stays for
//! the rest of the session and the lines the stream still held are never written (what had been flushed is in
//! the file). That holds for up to [`MAX_LEAKED_LOCKS`] such files, with no buffer kept for them;
//! past that a deleted writer is closed like a live one (its lines land, the lock goes), which only a script
//! that leaks that many files can tell. A `Vm` dropped without `shutdown` loses the lines of its open streams
//! too.
//!
//! **The recorded length** ([`FileRules`](crate::policy::FileRules)): every `openFileForWrite` (`openForWrite`,
//! `openForAppend`, `export`) tells the rules, and so does every read stream (`exec`, `openForRead`) and
//! `fileDelete`; a game's rules can report a length the resource manager recorded instead of the file's.
//!
//! **Where the open files live**: in the [`Vm`] (`FileObjects`, by object id), not in a table the natives
//! share: a borrow of it cannot be held across anything that prints or runs script code, because the next
//! call on the `Vm` would not compile. Registering twice changes nothing (no state is in the closures).
//!
//! **Caps**: the open files hold at most [`MAX_OPEN_READ_BYTES`] between them (a script that opens a 64 MiB
//! file in a thousand objects would otherwise hold 64 GiB); an open past it is refused with a message and
//! reads 0. The methods act only on objects of the class `FileObject` (a `ScriptObject` with
//! `superClass = FileObject` finds the methods, where a C++ cast would be blind): on any other they do
//! what a closed file does, so no state is kept for an object that has no `on_remove` here.

use std::collections::HashMap;
use std::rc::Rc;

use super::EngineClass;
use crate::sim::{ClassId, ObjId};
use crate::text::{fold, until_nul};
use crate::vm::{Native, NativeCall, Vm, VmError};

/// The most bytes the open files of all `FileObject`s hold together.
pub const MAX_OPEN_READ_BYTES: usize = 256 << 20;

/// The bytes a file open for writing buffers before they go to the host (`FileStream`'s buffer: 8 KiB).
pub const WRITE_BUFFER_BYTES: usize = 8 * 1024;

/// The most files open for writing at once.
pub const MAX_OPEN_WRITERS: usize = 4096;

/// The most locks deleted writers leave for the session (see [`FileObjectClass`]): each is a folded path of at
/// most 1023 bytes. Past it a deleted writer closes its file as a live one does.
pub const MAX_LEAKED_LOCKS: usize = 1024;

/// What one `FileObject` has open.
enum Open {
    /// The whole file (`readMemory`) and where the next line starts.
    Read { bytes: Vec<u8>, at: usize },
    /// The file being written, by its (expanded) game path, and the stream that holds its lines.
    Write { path: Rc<[u8]>, stream: Stream },
}

/// What a file open for writing hands to the host (`Vm::flush_file`): `bytes` are to be appended to the file at
/// `path`.
pub(crate) struct Flush {
    pub(crate) path: Rc<[u8]>,
    pub(crate) bytes: Vec<u8>,
}

/// The `FileStream` of a file open for writing (its `_write`): one
/// buffer of [`WRITE_BUFFER_BYTES`] aligned to multiples of that size of the file's offset.
/// - A write copies what fits into the rest of the block the buffer holds.
/// - What does not fit: the buffer is flushed (the file now ends where the buffer did); then, if the rest fits
///   in the block the file's end is in, it starts a new buffer there; if it spans past the end of that block
///   it is written to the file whole, and the buffer is empty.
///
/// `writeLine` is two writes: the line, then CR LF. So 10000 bytes at offset 0 are on disk at once with their
/// CR LF still in the buffer, and 8100 bytes of short lines are all still in the buffer.
struct Stream {
    /// How far the host has the file (the file's position in the stream).
    file_pos: u64,
    /// Where the next byte copied into the buffer goes.
    pos: u64,
    /// The first offset of the block the buffer holds; None while there is no buffer.
    head: Option<u64>,
    /// The bytes copied into the buffer that the host has not been given, from `file_pos`.
    pending: Vec<u8>,
    /// Opened by `openForAppend`, not `openForWrite`: a copy of its file with bytes on the disk fails.
    appends: bool,
}

impl Stream {
    /// A stream whose file already has `start` bytes (an append starts at the file's size).
    fn new(start: u64) -> Stream {
        Stream {
            file_pos: start,
            pos: start,
            head: None,
            pending: Vec::new(),
            appends: false,
        }
    }

    /// One `_write`: the bytes the host is given by it (the flushed buffer, then bytes written whole) go to `out`.
    fn write(&mut self, src: &[u8], out: &mut Vec<u8>) {
        let block = WRITE_BUFFER_BYTES as u64;
        let mut rest = src;
        if let Some(head) = self.head {
            // The rest of the block the buffer is in.
            let tail = (head / block * block).saturating_add(block - 1);
            let room = if self.pos > tail {
                0
            } else {
                tail - self.pos + 1
            };
            let n = usize::try_from(room).map_or(rest.len(), |room| room.min(rest.len()));
            let (copied, left) = rest.split_at(n);
            self.pending.extend_from_slice(copied);
            self.pos = self.pos.saturating_add(n as u64);
            rest = left;
        }
        if rest.is_empty() {
            return;
        }
        self.flush(out);
        self.pos = self.file_pos;
        let tail = (self.pos / block * block).saturating_add(block - 1);
        if self.pos.saturating_add(rest.len() as u64) <= tail {
            self.head = Some(self.pos);
            self.pending.extend_from_slice(rest);
            self.pos = self.pos.saturating_add(rest.len() as u64);
        } else {
            self.head = None;
            out.extend_from_slice(rest);
            self.file_pos = self.file_pos.saturating_add(rest.len() as u64);
            self.pos = self.file_pos;
        }
    }

    /// Gives the host what the buffer holds.
    fn flush(&mut self, out: &mut Vec<u8>) {
        self.file_pos = self.file_pos.saturating_add(self.pending.len() as u64);
        out.append(&mut self.pending);
    }
}

/// The key of a file in the table of locks: the OS's name for it, folded (`x.` and `x` are one file).
fn lock_key(path: &[u8]) -> Vec<u8> {
    fold(&crate::host::os_name(path))
}

/// What a lock on a file open for writing belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lock {
    /// An object that has the file open.
    Open(ObjId),
    /// The stream of a deleted object, which is never closed: it holds the file for the rest of the session.
    Leaked,
}

/// The open files of a `Vm`'s `FileObject`s, by object id.
pub(crate) struct FileObjects {
    open: HashMap<ObjId, Open>,
    /// The bytes the `Read`s hold.
    read_bytes: usize,
    limit: usize,
    /// The files open for writing: folded path -> who has it. A file is open for writing by one stream at a time
    /// (a second writer, `export` and `fileDelete` are refused, reads see an empty file).
    locked: HashMap<Vec<u8>, Lock>,
    /// The locks a deleted object left ([`Lock::Leaked`]).
    leaked: usize,
}

impl Default for FileObjects {
    fn default() -> FileObjects {
        FileObjects::with_limit(MAX_OPEN_READ_BYTES)
    }
}

impl FileObjects {
    pub(crate) fn with_limit(limit: usize) -> FileObjects {
        FileObjects {
            open: HashMap::new(),
            read_bytes: 0,
            limit,
            locked: HashMap::new(),
            leaked: 0,
        }
    }

    /// The bytes held by files open for reading.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.read_bytes
    }

    /// Forgets what `id` has open. A file open for writing is unlocked, and what its stream still held is
    /// returned: the caller appends it to the file.
    #[must_use]
    pub(crate) fn close(&mut self, id: ObjId) -> Option<Flush> {
        match self.open.remove(&id)? {
            Open::Read { bytes, .. } => {
                self.read_bytes = self.read_bytes.saturating_sub(bytes.len());
                None
            }
            Open::Write { path, mut stream } => {
                self.unlock(&path, id);
                let mut bytes = Vec::new();
                stream.flush(&mut bytes);
                (!bytes.is_empty()).then_some(Flush { path, bytes })
            }
        }
    }

    /// What deleting the object `id` does to what it has open. A read is given up as by `close`. A file open for
    /// writing is NOT closed: the stream leaks, the file stays locked for the rest of the
    /// session and the lines the stream held are lost; nothing is kept for it but the lock. Up to
    /// [`MAX_LEAKED_LOCKS`] locks are kept so; past it a deleted writer is closed as a live one is (its
    /// lines land and the lock goes), which a script can only tell by leaking that many files.
    #[must_use]
    pub(crate) fn abandon(&mut self, id: ObjId) -> Option<Flush> {
        let leak = match self.open.get(&id) {
            Some(Open::Write { path, .. }) if self.leaked < MAX_LEAKED_LOCKS => {
                let key = lock_key(path);
                (self.locked.get(&key) == Some(&Lock::Open(id))).then_some(key)
            }
            _ => None,
        };
        match leak {
            Some(key) => {
                // The stream, and the lines in it, go; the lock stays.
                self.open.remove(&id);
                self.locked.insert(key, Lock::Leaked);
                self.leaked += 1;
                None
            }
            None => self.close(id),
        }
    }

    fn unlock(&mut self, path: &[u8], id: ObjId) {
        let key = lock_key(path);
        if self.locked.get(&key) == Some(&Lock::Open(id)) {
            self.locked.remove(&key);
        }
    }

    /// Whether a `fileCopy` of a file with bytes on the disk fails for who holds it: the stream of a deleted object,
    /// which was never closed, or an appender, live or not (a live writer's copy gives
    /// the bytes on the disk).
    pub(crate) fn blocks_copy(&self, path: &[u8]) -> bool {
        if self.locked.is_empty() {
            return false;
        }
        match self.locked.get(&lock_key(path)) {
            Some(Lock::Leaked) => true,
            Some(Lock::Open(id)) => {
                matches!(self.open.get(id), Some(Open::Write { stream, .. }) if stream.appends)
            }
            None => false,
        }
    }

    /// Notes that the stream `id` just opened with [`open_write`](Self::open_write) appends.
    pub(crate) fn mark_appender(&mut self, id: ObjId) {
        if let Some(Open::Write { stream, .. }) = self.open.get_mut(&id) {
            stream.appends = true;
        }
    }

    /// Holds `bytes` as the file `id` reads. Refused, with the file's length, when the open files would hold
    /// more than the limit (nothing changes then). `id` has nothing open: the caller closed it first, as the
    /// `openForRead` closes before it looks at the file.
    pub(crate) fn open_read(&mut self, id: ObjId, bytes: Vec<u8>) -> Result<(), usize> {
        let len = bytes.len();
        match self.read_bytes.checked_add(len) {
            Some(total) if total <= self.limit => {
                self.read_bytes = total;
                self.open.insert(id, Open::Read { bytes, at: 0 });
                Ok(())
            }
            _ => Err(len),
        }
    }

    /// Whether a file can be opened for writing now: nobody has it open for writing (ignoring case), and fewer
    /// than [`MAX_OPEN_WRITERS`] files are (leaked locks count).
    pub(crate) fn can_open_write(&self, path: &[u8]) -> bool {
        self.locked.len() < MAX_OPEN_WRITERS && !self.is_locked(path)
    }

    /// Whether some stream has the file open for writing (ignoring case).
    #[inline(never)]
    pub(crate) fn is_locked(&self, path: &[u8]) -> bool {
        !self.locked.is_empty() && self.locked.contains_key(&lock_key(path))
    }

    /// Records that `id` writes the file at `path`, which the caller has created and which has `start` bytes
    /// (0 unless it appends): false, and nothing changes, when
    /// [`can_open_write`](Self::can_open_write) no longer says so (a script that ran meanwhile took the file).
    /// `id` has nothing open: the caller closed it first.
    pub(crate) fn open_write(&mut self, id: ObjId, path: Vec<u8>, start: u64) -> bool {
        if !self.can_open_write(&path) {
            return false;
        }
        self.locked.insert(lock_key(&path), Lock::Open(id));
        self.open.insert(
            id,
            Open::Write {
                path: Rc::from(path),
                stream: Stream::new(start),
            },
        );
        true
    }

    /// The path `id` writes and the size its file will have with what it wrote so far, if it is open for writing.
    pub(crate) fn write_target(&self, id: ObjId) -> Option<(&[u8], u64)> {
        match self.open.get(&id) {
            Some(Open::Write { path, stream }) => Some((path, stream.pos)),
            _ => None,
        }
    }

    /// Whether `id` has a file open for reading and has not read it all (`isEOF()` is false): `writeLine` complains when
    /// a line is written then (`Vm::write_line`).
    pub(crate) fn has_unread(&self, id: ObjId) -> bool {
        matches!(self.open.get(&id), Some(Open::Read { bytes, at }) if *at < bytes.len())
    }

    /// Writes `line` and its CR LF to the stream of the file `id` writes (two writes, as `FileObject::writeLine`'s
    /// `writeLine` makes them); what goes to the host by it is returned, to be appended to the file.
    #[must_use]
    pub(crate) fn buffer_line(&mut self, id: ObjId, line: &[u8]) -> Option<Flush> {
        let Some(Open::Write { path, stream }) = self.open.get_mut(&id) else {
            return None;
        };
        let mut bytes = Vec::new();
        stream.write(line, &mut bytes);
        stream.write(b"\r\n", &mut bytes);
        (!bytes.is_empty()).then(|| Flush {
            path: Rc::clone(path),
            bytes,
        })
    }

    /// Whether `id` is at the end: of what it reads, and always while writing or with nothing open.
    pub(crate) fn is_eof(&self, id: ObjId) -> bool {
        match self.open.get(&id) {
            Some(Open::Read { bytes, at }) => *at >= bytes.len(),
            Some(Open::Write { .. }) | None => true,
        }
    }

    /// Where the file `id` reads is (the offset of its next line); None when it reads nothing.
    pub(crate) fn read_position(&self, id: ObjId) -> Option<usize> {
        match self.open.get(&id) {
            Some(Open::Read { at, .. }) => Some(*at),
            _ => None,
        }
    }

    /// Moves the file `id` reads back to `at` (an offset [`read_position`](Self::read_position) gave, at most where it is now).
    pub(crate) fn rewind_to(&mut self, id: ObjId, to: usize) -> bool {
        match self.open.get_mut(&id) {
            Some(Open::Read { at, .. }) if to <= *at => {
                *at = to;
                true
            }
            _ => false,
        }
    }

    /// The next line of the file `id` reads and the number of bytes it passed over (its length and its line
    /// end); None when it reads nothing.
    pub(crate) fn read_line(&mut self, id: ObjId) -> Option<(Vec<u8>, u64)> {
        let Some(Open::Read { bytes, at }) = self.open.get_mut(&id) else {
            return None;
        };
        let (line, next) = next_line(bytes, *at);
        let scanned = next.saturating_sub(*at) as u64;
        *at = next;
        Some((line, scanned))
    }
}

/// One line from `bytes` at `at`: the line (cut at a NUL) and where the next one starts.
fn next_line(bytes: &[u8], at: usize) -> (Vec<u8>, usize) {
    let rest = bytes.get(at..).unwrap_or_default();
    let end = rest
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(rest.len());
    let mut next = at.saturating_add(end);
    match rest.get(end) {
        Some(b'\r') => {
            next = next.saturating_add(1);
            if rest.get(end.saturating_add(1)) == Some(&b'\n') {
                next = next.saturating_add(1);
            }
        }
        Some(_) => next = next.saturating_add(1),
        None => {}
    }
    (
        until_nul(rest.get(..end).unwrap_or_default()).to_vec(),
        next,
    )
}

/// The behaviour `install` gives the class `FileObject`: a deleted object gives up the file it reads (the bytes
/// held against [`MAX_OPEN_READ_BYTES`] are given back), but a file it has open for WRITING is not closed, as in
/// leaked (the stream is never closed, the file stays locked for the rest of the session and the lines the
/// stream still held are never written; also when the object goes with its group). The lock is kept and
/// drops the lines, for up to [`MAX_LEAKED_LOCKS`] such files; past that a deleted writer closes its file as a
/// live one does. `Vm::shutdown` deletes every object, so an object still writing at the end loses its lines the
/// same way. A host that replaces the behaviour must call this one from its own, or deleted objects leak what
/// they held.
pub struct FileObjectClass;

impl EngineClass for FileObjectClass {
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        vm.abandon_file_object(id);
        Ok(())
    }
}

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    until_nul(c.argv.get(i).map_or(&b""[..], Vec::as_slice))
}

/// The object a method runs on, when it is a `FileObject` (of the class or a subclass).
fn file_object(vm: &Vm, c: &NativeCall, class: ClassId) -> Option<ObjId> {
    let id = c.this_object?;
    let of = vm.objects().get(id)?.class();
    vm.objects().classes.is_a(of, class).then_some(id)
}

/// `FileObject`'s methods on a `Vm` that has the class (`install` calls this after the class table, which
/// made the class and put stubs where these go): the natives replace the stubs in place, and the class gets
/// its behaviour if it has none.
pub(crate) fn register(vm: &mut Vm) {
    // Reading opens a file by a name the caller gives: as much a file effect as writing.
    for method in ["openForRead", "openForWrite", "openForAppend", "writeLine"] {
        vm.add_native_effects("FileObject", method, crate::effects::Effects::FILES);
    }
    let Some(class) = vm.objects().classes.find(b"FileObject") else {
        return;
    };
    if vm
        .objects()
        .classes
        .get(class)
        .is_some_and(|def| def.behaviour.is_none())
    {
        vm.set_class_behaviour("FileObject", Rc::new(FileObjectClass));
    }
    vm.register_in(
        "FileObject",
        "openForRead",
        3,
        3,
        "file.openForRead(path) - opens a file for reading",
        move |vm, c| {
            let Some(id) = file_object(vm, c, class) else {
                return Native::Bool(false);
            };
            match open_for_read(vm, id, arg(c, 2)) {
                Ok(opened) => Native::Bool(opened),
                Err(e) => Native::Fail(e),
            }
        },
    );
    for (method, append) in [("openForWrite", false), ("openForAppend", true)] {
        vm.register_in(
            "FileObject",
            method,
            3,
            3,
            "file.openForWrite(path) - opens a file for writing (openForAppend: at its end)",
            move |vm, c| {
                let Some(id) = file_object(vm, c, class) else {
                    return Native::Bool(false);
                };
                match open_for_write(vm, id, arg(c, 2), append) {
                    Ok(opened) => Native::Bool(opened),
                    Err(e) => Native::Fail(e),
                }
            },
        );
    }
    vm.register_in(
        "FileObject",
        "isEOF",
        2,
        2,
        "file.isEOF() - whether the reading has reached the end",
        move |vm, c| {
            let eof = file_object(vm, c, class).is_none_or(|id| vm.file_objects.is_eof(id));
            Native::Bool(eof)
        },
    );
    vm.register_in(
        "FileObject",
        "readLine",
        2,
        2,
        "file.readLine() - the next line",
        move |vm, c| {
            let read = file_object(vm, c, class).and_then(|id| vm.file_objects.read_line(id));
            let Some((line, scanned)) = read else {
                return Native::Str(Vec::new());
            };
            if !vm.charge_work(scanned) {
                return Native::Fail(VmError::StepLimit);
            }
            Native::Str(line)
        },
    );
    vm.register_in(
        "FileObject",
        "writeLine",
        3,
        3,
        "file.writeLine(text) - writes a line",
        move |vm, c| {
            let Some(id) = file_object(vm, c, class) else {
                return Native::Void;
            };
            match vm.write_line(id, arg(c, 2)) {
                Ok(()) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        },
    );
    vm.register_in(
        "FileObject",
        "close",
        2,
        2,
        "file.close() - closes the file",
        move |vm, c| {
            if let Some(id) = file_object(vm, c, class) {
                vm.close_file_object(id);
            }
            Native::Void
        },
    );
}

/// `FileObject::openForRead`: closes, expands the name, looks the file up (the resource manager's refusals print
/// as `exec`'s do) and holds it. An empty file of the disk does not open (0, silently; an empty zip entry
/// opens), and a file another object
/// has open for writing reads as empty (`Vm::find_file`), so it does not open either. A zip's folder entry (a name
/// the host serves with its trailing `/`, as a 0-byte file) is the exception: it opens as a stream already at its
/// end (`openForRead` 1, `isEOF` 1, `readLine` "").
#[inline(never)]
fn open_for_read(vm: &mut Vm, id: ObjId, name: &[u8]) -> Result<bool, VmError> {
    vm.close_file_object(id);
    let path = match vm.file_object_name(name) {
        Ok(path) => path,
        Err(message) => {
            vm.error(message);
            return Ok(false);
        }
    };
    if !vm.charge_work(path.len() as u64) {
        return Err(VmError::StepLimit);
    }
    // The exec table takes the spelling a read opens a file under, a missing file's too; `isFile` does not, and
    // neither does a refused name (`exec` of another spelling of it still says `Missing file: <its own>`).
    if (vm.file_rules().resources().refusal)(&path).is_none() {
        vm.note_exec_name(&path);
    }
    let Some(bytes) = vm.find_file(&path)? else {
        return Ok(false);
    };
    // An empty file opens unless the file rules say an empty file of the disk does not (an empty archive entry, and
    // a folder entry a host serves with its slash, still do).
    if bytes.is_empty()
        && !vm.file_rules().opens_empty_files()
        && !path.ends_with(b"/")
        && !vm.file_host().is_archive_entry(&path)
    {
        return Ok(false);
    }
    // The lookup can print a BackTrace, which can run a script that opens this object again.
    vm.close_file_object(id);
    match vm.file_objects.open_read(id, bytes) {
        Ok(()) => Ok(true),
        Err(len) => Ok(report_read_cap(vm, len, &path)),
    }
}

/// The complaint of an open that would hold more than [`MAX_OPEN_READ_BYTES`]: an error line; false.
/// Out of line: `open_for_read`'s frame is alive while a refusal's BackTrace runs, and holds none of it.
#[cold]
#[inline(never)]
fn report_read_cap(vm: &mut Vm, len: usize, path: &[u8]) -> bool {
    vm.error(format!(
        "ERROR: FileObject::openForRead - too many bytes are open (limit {MAX_OPEN_READ_BYTES}), cannot hold {len} more: \"{}\"",
        String::from_utf8_lossy(path)
    ));
    false
}

/// `FileObject::openForWrite` / `openForAppend`: closes (its lines land), then `Vm::open_for_write` checks the
/// name and the lock and has the host make the file.
#[inline(never)]
fn open_for_write(vm: &mut Vm, id: ObjId, name: &[u8], append: bool) -> Result<bool, VmError> {
    vm.close_file_object(id);
    let path = match vm.file_object_name(name) {
        Ok(path) => path,
        Err(message) => {
            vm.error(message);
            return Ok(false);
        }
    };
    let Some(start) = vm.open_for_write(&path, append)? else {
        return Ok(false);
    };
    Ok(keep_stream(vm, id, path, start, append))
}

/// Gives `id` the stream of the file `Vm::open_for_write` made. Out of line: `open_for_write`'s frame is alive
/// while a refusal's BackTrace runs, and holds none of this. False when a script that ran meanwhile (a
/// BackTrace) took the file.
#[inline(never)]
fn keep_stream(vm: &mut Vm, id: ObjId, path: Vec<u8>, start: u64, append: bool) -> bool {
    vm.close_file_object(id);
    let kept = vm.file_objects.open_write(id, path, start);
    if kept && append {
        vm.file_objects.mark_appender(id);
    }
    kept
}

#[cfg(test)]
#[path = "file_object_tests.rs"]
mod tests;
