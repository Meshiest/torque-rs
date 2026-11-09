//! The seams to the world outside the interpreter: the files scripts load (`FileHost`, TGE's resource
//! manager), the clocks scripts read (`Clock`) and the compiler that turns TorqueScript source
//! into bytecode (`Compiler`: the dialect's, which a game installs with its executor; [`NoCompiler`], the
//! default, reports that there is none).
//!
//! Paths are bytes with `/` separators, relative to the game folder, and match case-insensitively, as on
//! Windows (`exec("CONFIG/Mod/A.CS")` finds `config/mod/a.cs`).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use web_time::{Instant, SystemTime, UNIX_EPOCH};

use torque_ts_bytecode::DsoFile;
/// How a compile is used (TGE's `CodeBlock::compile` or `compileExec`), see [`Compiler`].
pub use torque_ts_compiler::Purpose;

use crate::text::fold;
use crate::vm::{Level, LogLine};

/// Why a game's resource manager refuses a path ([`ResourceRules::refusal`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The path is outside the folders the rules allow: its line is followed by a BackTrace when the console
    /// rules ask for one.
    NotDefaultDirectory,
    /// The path cannot be opened for another reason: its line alone.
    CannotOpen,
}

/// What a lookup finds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRead {
    Found(Vec<u8>),
    Missing,
    Refused(Refusal),
}

/// What a lookup finds without reading the file (`isFile`, `getFileLength`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStat {
    Found { len: u64 },
    Missing,
    Refused(Refusal),
}

/// The files scripts see. Implementations decide where they come from (a folder, zips, memory); the
/// interpreter prints the resource manager's messages from what they answer.
pub trait FileHost {
    /// The whole file at `path`.
    ///
    /// `path` is whatever a script wrote: any bytes (not UTF-8, NUL, `..`, a drive letter, backslashes,
    /// 100 000 of them). The interpreter passes it on unchanged; keeping the answer inside the host's own
    /// folders is the host's job. An implementation over real folders must apply its [`ResourceRules`] and
    /// then [`safe_path`], match the components it returns against the names `read_dir` lists, and never
    /// `join` a component of the script's path onto a folder (on Windows a component such as `C:\x` or
    /// `\x` replaces the base); the in-memory hosts here only ever look the path up in a table.
    fn read(&self, path: &[u8]) -> FileRead;

    /// Whether the file exists and how long it is, with the refusals of [`read`](Self::read). The
    /// default reads the file. An implementation that looks at the file itself has the same duty as
    /// `read` to keep the answer inside its folders (`safe_path`, listing match, no `join`).
    fn stat(&self, path: &[u8]) -> FileStat {
        match self.read(path) {
            FileRead::Found(contents) => FileStat::Found {
                len: contents.len() as u64,
            },
            FileRead::Missing => FileStat::Missing,
            FileRead::Refused(refusal) => FileStat::Refused(refusal),
        }
    }

    /// The same as [`stat`](Self::stat) for a name the resource manager does NOT look up: the source of a `fileCopy`
    /// (the OS copies it, and the source does not become a resource: a file that left the
    /// list for a refused delete is not listed again by being copied from). A host that lists a file at its first
    /// look-up (`stat`, `read`, `modified`) must not list it for this. The default is `stat`.
    fn stat_without_look_up(&self, path: &[u8]) -> FileStat {
        self.stat(path)
    }

    /// Whether `path` names an entry of a zip the host mounts (a file the OS has no copy of). An EMPTY zip entry
    /// opens for reading where an empty file of the disk does not. The default is false.
    fn is_archive_entry(&self, path: &[u8]) -> bool {
        let _ = path;
        false
    }

    /// The directory of the resource `path` names, as the resource manager stores it (a `ResourceObject`'s path: the
    /// on-disk spelling of the folders, which a script's string need not match). The default is None: the host does not know
    /// the spelling and the caller keeps the one it has.
    fn real_path(&self, path: &[u8]) -> Option<Vec<u8>> {
        let _ = path;
        None
    }

    /// When the file was last changed, in nanoseconds since the Unix epoch; None when unknown. `exec`
    /// compares a `.dso` with its source by it; `getFileModifiedTime` prints it as local time.
    fn modified(&self, path: &[u8]) -> Option<u64> {
        let _ = path;
        None
    }

    /// Every file the searches (`findFirstFile`, `getFileCount`) go through, as game paths in the order
    /// they find them. The resource manager's order: newest first; the files it found at start-up come in reverse
    /// scan order (the mod paths, each depth-first in case-insensitive name order), so the last mod path comes
    /// first and each folder lists its names descending. The default lists none.
    /// The interpreter charges every name it looks at, but not the call itself: a host that builds the
    /// list on each call should cache it (an `Rc` clone is all a search should cost to get it). A host that
    /// puts its list up to date when it is asked for does that work in this call and reports it through
    /// [`work_done`](Self::work_done): the interpreter reads the counter around the call, like the others.
    fn files(&self) -> Rc<Vec<Vec<u8>>> {
        Rc::new(Vec::new())
    }

    /// Deletes a file (`fileDelete`; the interpreter has checked the name with the resource manager's rules, which
    /// are NOT a sandbox, and with [`safe_path`]); false when it cannot. A host over real folders applies
    /// [`safe_path`] to `path` itself as well, as for `write`, and never joins script text onto a folder.
    /// The default deletes nothing.
    fn remove(&self, path: &[u8]) -> bool {
        let _ = path;
        false
    }

    /// A `fileDelete` of a file a script holds open for writing, which the interpreter refuses without asking
    /// [`remove`](Self::remove) (0, and the file stays). The resource manager still takes the
    /// file's resource out of its list for that, until the next look-up of the path, which lists it again
    /// (`findFirstFile` and `getFileCount` stop showing it, and `isFile` after the file is closed lists it at the
    /// head); a host that keeps the searches' list does the same. Nothing is deleted. The default does nothing.
    fn unlist(&self, path: &[u8]) {
        let _ = path;
    }

    /// Copies a file over another (`fileCopy`; names checked by the resource manager's rules and by [`safe_path`]).
    /// The default reads `from` and writes `to` through [`write`](Self::write), so the destination gets the
    /// same [`safe_path`] check as every write (`Vm::check_write_path` is the resource manager's rule, the host's
    /// `write` the sandbox); a host that copies by itself must apply [`safe_path`] to BOTH names.
    fn copy(&self, from: &[u8], to: &[u8]) -> bool {
        match self.read(from) {
            FileRead::Found(contents) => self.write(to, &contents, false),
            _ => false,
        }
    }

    /// Writes a file (appending when `append`, else replacing it); false when it cannot be written. The
    /// interpreter has applied the resource manager's checks on the name (`Vm::check_write_path`: inside one of
    /// the write directories of the [`ResourceRules`], no `..`, none of `: | > < *`), which are NOT a sandbox: they split only
    /// on `/` and pass names such as `config/p\..\..\x.cs`, `config/nul.cs` or `config//x.cs`. A host
    /// over real folders must apply [`safe_path`] to `path` and never `join` text from a script onto a
    /// folder, exactly as for [`read`](Self::read); a name it refuses answers false. The default writes
    /// nothing.
    fn write(&self, path: &[u8], contents: &[u8], append: bool) -> bool {
        let _ = (path, contents, append);
        false
    }

    /// Work the host has done so far that returned no bytes, in the VM's work units (one per byte, see
    /// `WORK_PER_STEP`): a zip entry inflated for a read that then failed, the file an append started from.
    /// Work whose bytes come back (a read that succeeds) is NOT counted: the VM charges those itself, and
    /// would charge them twice. A monotonic counter, 0 for a host that does no such work (the default).
    ///
    /// The VM reads it before and after every call that can do some (`read`, `stat`, `copy`, `write`,
    /// `remove`, `files`) and charges the difference to the step budget, on top of the bytes a read returns: a script
    /// cannot make the host inflate or read for nothing without paying for it. A counter that goes down (a
    /// replaced host) charges nothing.
    fn work_done(&self) -> u64 {
        0
    }

    /// `setModPaths` named the folder `path` (relative, a mod path the script gave): the resource manager walks every mod path
    /// again and a file it did not list yet becomes a resource at the head of the searches' list, the last one walked
    /// first; what it listed keeps its place. A host whose list is the start-up scan's lists the folder's new files so; returns
    /// how many it listed. The work is reported through [`work_done`](Self::work_done). The default lists nothing.
    fn add_mod_path(&self, path: &[u8]) -> usize {
        let _ = path;
        0
    }
}

/// What a game's resource manager allows, for the hosts and the `Vm` (a `FileHost` built with [`MemFiles::with_rules`]
/// or [`DirHost::with_rules`], the `Vm`'s through [`FileRules::resources`](crate::policy::FileRules::resources)). The
/// default allows every path, refuses nothing and scans no folder.
#[derive(Clone, Copy)]
pub struct ResourceRules {
    /// Whether a path is refused before any file is looked at (None: it is not).
    pub refusal: fn(&[u8]) -> Option<Refusal>,
    /// The line a refusal prints (error level).
    pub refusal_text: fn(&[u8], Refusal) -> String,
    /// The folders a written file must lie in (lower case, without the slash); empty: any folder.
    pub write_directories: &'static [&'static [u8]],
    /// The folders a [`DirHost`]'s start-up scan walks, in its order.
    pub scanned: &'static [&'static str],
}

impl Default for ResourceRules {
    fn default() -> ResourceRules {
        ResourceRules {
            refusal: |_| None,
            refusal_text: |path, _| format!("cannot open \"{}\"", String::from_utf8_lossy(path)),
            write_directories: &[],
            scanned: &[],
        }
    }
}

impl std::fmt::Debug for ResourceRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceRules")
            .field("write_directories", &self.write_directories)
            .field("scanned", &self.scanned)
            .finish_non_exhaustive()
    }
}

/// Whether one component of a path is acceptable to a host that maps names onto a real folder: not empty,
/// not `.` or `..`, no `/`, `\`, `:`, NUL or other control character, none of the characters Windows
/// forbids in a name (`*?"<>|`), no trailing dot or space (Windows drops them, so `x.` and `x` are one
/// file), and not a Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9` and `LPT1`-`LPT9`, the
/// latter two also with the superscript digits U+00B9, U+00B2, U+00B3, `CONIN$` and `CONOUT$`; in any case,
/// with or without an extension), which would open the device instead of a file.
pub fn sanitize_component(part: &[u8]) -> bool {
    if part.is_empty() || part == b"." || part == b".." {
        return false;
    }
    if part
        .iter()
        .any(|&b| b < 0x20 || b == 0x7f || b"/\\:*?\"<>|".contains(&b))
    {
        return false;
    }
    if part.ends_with(b".") || part.ends_with(b" ") {
        return false;
    }
    // The device name is what comes before the first dot ("nul.cs" is the NUL device on Windows).
    let stem = part.split(|&b| b == b'.').next().unwrap_or(part);
    let stem = fold(stem);
    let stem = stem.trim_ascii_end();
    const DEVICES: [&[u8]; 6] = [b"con", b"prn", b"aux", b"nul", b"conin$", b"conout$"];
    // COM1-COM9 and LPT1-LPT9, and COM and LPT with the superscript digits U+00B9, U+00B2 and U+00B3 (UTF-8
    // C2 B9, C2 B2, C2 B3), which Windows also treats as ports.
    let numbered = |prefix: &[u8]| {
        stem.starts_with(prefix)
            && matches!(
                stem.get(3..),
                Some([b'1'..=b'9'] | [0xc2, 0xb9 | 0xb2 | 0xb3])
            )
    };
    !(DEVICES.contains(&stem) || numbered(b"com") || numbered(b"lpt"))
}

/// The components of a script's path for a host that maps names onto a real folder, or None when any of
/// them fails [`sanitize_component`] or the path is not UTF-8. The path is split on `/` only; a `\` or a
/// `:` anywhere is a refusal rather than a separator or a drive, so the result can be matched against the
/// names a directory lists without ever joining untrusted text onto a folder.
pub fn safe_path(path: &[u8]) -> Option<Vec<&str>> {
    let text = std::str::from_utf8(path).ok()?;
    let parts: Vec<&str> = text.split('/').collect();
    parts
        .iter()
        .all(|part| sanitize_component(part.as_bytes()))
        .then_some(parts)
}

/// Like [`safe_path`] for a name a script WRITES, deletes or copies: the LAST part may end in one dot (`x.`), which
/// Windows drops, so that `openForWrite("trailing.")` is 1. A dot inside the path
/// (`a./b.txt` would make the folder `a.`), a part of dots only, two trailing dots, a trailing space and the rest
/// of [`sanitize_component`] stay refused. `x.` and `x` are one file on Windows ([`os_name`]).
///
/// **A host over real folders must not join a name that passes this onto a path in the `\\?\` form**, where a
/// trailing dot is NOT dropped and names a file Windows' own tools cannot open; it must refuse such a name or
/// strip the dot itself. (The hosts here keep written files in memory.)
pub fn safe_write_path(path: &[u8]) -> Option<Vec<&str>> {
    let text = std::str::from_utf8(path).ok()?;
    let parts: Vec<&str> = text.split('/').collect();
    let (last, folders) = parts.split_last()?;
    if !folders
        .iter()
        .all(|part| sanitize_component(part.as_bytes()))
    {
        return None;
    }
    let last_bytes = last.as_bytes();
    let accepted = match last_bytes.strip_suffix(b".") {
        Some(rest) if rest.iter().any(|&b| b != b'.') => sanitize_component(rest),
        _ => sanitize_component(last_bytes),
    };
    accepted.then_some(parts)
}

/// The name the OS knows a file by: a last part that ends in one dot is the file without it (`x.` is `x`: Windows drops
/// the dot). The identity of a file in the overlay, of its lock and of a copy onto itself; the
/// resource list keeps the names as they were looked up. A name [`safe_write_path`] would refuse stays as it
/// is.
pub fn os_name(path: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let last = path.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    match path.get(last..).and_then(|part| part.strip_suffix(b".")) {
        Some(rest) if rest.iter().any(|&b| b != b'.') && sanitize_component(rest) => {
            std::borrow::Cow::Borrowed(path.get(..path.len() - 1).unwrap_or(path))
        }
        _ => std::borrow::Cow::Borrowed(path),
    }
}

/// The path the OS resolves a name to, for the parts that mean nothing there: an interior `.` or empty part
/// (`config/a/./b.txt`, `config//b.txt`) is dropped; the first part, the last (a trailing slash makes an empty
/// one) and everything else stay. A zip's entries are not files of the OS: a host looks them up by the name as
/// written, and uses this for the files of the disk (`config/x/./a.txt` reads `config/x/a.txt`).
pub fn collapse_dots(path: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let last = path.iter().filter(|&&b| b == b'/').count();
    let meaningless =
        |i: usize, part: &[u8]| i != 0 && i != last && (part.is_empty() || part == b".");
    if !path
        .split(|&b| b == b'/')
        .enumerate()
        .any(|(i, part)| meaningless(i, part))
    {
        return std::borrow::Cow::Borrowed(path);
    }
    let mut out = Vec::with_capacity(path.len());
    let mut first = true;
    for (i, part) in path.split(|&b| b == b'/').enumerate() {
        if meaningless(i, part) {
            continue;
        }
        if !first {
            out.push(b'/');
        }
        first = false;
        out.extend_from_slice(part);
    }
    std::borrow::Cow::Owned(out)
}

/// No files at all: every lookup is missing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoFiles;

impl FileHost for NoFiles {
    fn read(&self, _path: &[u8]) -> FileRead {
        FileRead::Missing
    }
}

/// One file of [`MemFiles`].
#[derive(Debug, Clone)]
struct MemFile {
    /// The path as it was first written (searches return it).
    name: Vec<u8>,
    contents: Vec<u8>,
    modified: u64,
    /// When the file was created: the searches list the newest first.
    created: u64,
}

/// Folded path -> file.
type MemTable = BTreeMap<Vec<u8>, MemFile>;

/// What [`MemFiles`] clones share: the files, the creation counter and the searches' list (made when a
/// search asks for it, dropped when a file is created or removed).
#[derive(Debug, Default)]
struct MemState {
    files: RefCell<MemTable>,
    created: Cell<u64>,
    listing: RefCell<Option<Rc<Vec<Vec<u8>>>>>,
}

/// Files held in memory, for tests and tools. Clones share the same files, so a test can keep one and
/// look at what scripts did. Lookups apply its [`ResourceRules`]; the searches list the newest file first.
#[derive(Debug, Clone, Default)]
pub struct MemFiles {
    state: Rc<MemState>,
    rules: ResourceRules,
}

impl MemFiles {
    pub fn new() -> MemFiles {
        MemFiles::default()
    }

    /// The same files under a game's resource rules.
    pub fn with_rules(mut self, rules: ResourceRules) -> MemFiles {
        self.rules = rules;
        self
    }

    /// Adds or replaces a file, with modification time 0.
    pub fn insert(&self, path: &str, contents: &[u8]) {
        self.insert_at(path, contents, 0);
    }

    /// Adds or replaces a file with a modification time (nanoseconds since the Unix epoch).
    pub fn insert_at(&self, path: &str, contents: &[u8], modified: u64) {
        let key = self.create(path.as_bytes());
        if let Some(file) = self.state.files.borrow_mut().get_mut(&key) {
            file.contents = contents.to_vec();
            file.modified = modified;
        }
    }

    /// Sets a file's modification time (nanoseconds since the Unix epoch); false when there is no such file.
    pub fn set_modified(&self, path: &[u8], modified: u64) -> bool {
        match self.state.files.borrow_mut().get_mut(&fold(&os_name(path))) {
            Some(file) => {
                file.modified = modified;
                true
            }
            None => false,
        }
    }

    /// The contents of a file, if it exists.
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.state
            .files
            .borrow()
            .get(&fold(&os_name(path.as_bytes())))
            .map(|f| f.contents.clone())
    }

    /// The key of the file at `path`, which is created empty (the newest) when it does not exist.
    fn create(&self, path: &[u8]) -> Vec<u8> {
        let key = fold(&os_name(path));
        let mut files = self.state.files.borrow_mut();
        if !files.contains_key(&key) {
            let created = self.state.created.get() + 1;
            self.state.created.set(created);
            files.insert(
                key.clone(),
                MemFile {
                    name: path.to_vec(),
                    contents: Vec::new(),
                    modified: 0,
                    created,
                },
            );
            self.state.listing.take();
        }
        key
    }
}

impl FileHost for MemFiles {
    fn read(&self, path: &[u8]) -> FileRead {
        if let Some(refusal) = (self.rules.refusal)(path) {
            return FileRead::Refused(refusal);
        }
        match self.state.files.borrow().get(&fold(&os_name(path))) {
            Some(file) => FileRead::Found(file.contents.clone()),
            None => FileRead::Missing,
        }
    }

    /// The length without copying the contents (the default would clone a file to count its bytes).
    fn stat(&self, path: &[u8]) -> FileStat {
        if let Some(refusal) = (self.rules.refusal)(path) {
            return FileStat::Refused(refusal);
        }
        match self.state.files.borrow().get(&fold(&os_name(path))) {
            Some(file) => FileStat::Found {
                len: file.contents.len() as u64,
            },
            None => FileStat::Missing,
        }
    }

    fn modified(&self, path: &[u8]) -> Option<u64> {
        self.state
            .files
            .borrow()
            .get(&fold(&os_name(path)))
            .map(|f| f.modified)
    }

    /// Applies [`safe_path`] as a host over real folders must (the resource manager's checks
    /// pass `config/\x.cs`, `config/nul.cs`, `config//x.cs`), then stores the bytes under the folded name.
    /// A replaced file keeps its modification time and its place in the creation order.
    fn write(&self, path: &[u8], contents: &[u8], append: bool) -> bool {
        if safe_write_path(path).is_none() {
            return false;
        }
        let key = self.create(path);
        let mut files = self.state.files.borrow_mut();
        let Some(file) = files.get_mut(&key) else {
            return false;
        };
        if !append {
            file.contents.clear();
        }
        file.contents.extend_from_slice(contents);
        true
    }

    fn files(&self) -> Rc<Vec<Vec<u8>>> {
        let mut listing = self.state.listing.borrow_mut();
        Rc::clone(listing.get_or_insert_with(|| {
            let files = self.state.files.borrow();
            let mut all: Vec<&MemFile> = files.values().collect();
            all.sort_by_key(|f| std::cmp::Reverse(f.created));
            Rc::new(all.into_iter().map(|f| f.name.clone()).collect())
        }))
    }

    /// Like `write`, a name [`safe_path`] refuses names no file: nothing a script can make is held under
    /// one, and a file a test inserted under one is not reachable through `fileDelete`.
    fn remove(&self, path: &[u8]) -> bool {
        if safe_write_path(path).is_none() {
            return false;
        }
        let removed = self
            .state
            .files
            .borrow_mut()
            .remove(&fold(&os_name(path)))
            .is_some();
        if removed {
            self.state.listing.take();
        }
        removed
    }
}

/// The files of a game folder on disk, read-only (writes, deletes and copies fail: nothing a script does
/// changes a game folder). Names resolve case-insensitively, one path component at a time, as on the
/// Windows; lookups apply its [`ResourceRules`] and then [`safe_path`] (no `\`, `:`, NUL,
/// `.`/`..`, device names...), and components are matched against the names a directory lists, never
/// joined. Symbolic links and junctions are not followed, and a file read is the one found
/// ([`open_found`]). Zip archives are not opened: their files are neither found nor listed. Files
/// directly in the root folder are missing too.
#[derive(Debug, Clone)]
pub struct DirHost {
    root: PathBuf,
    rules: ResourceRules,
    /// The start-up scan, made at the first search or listing (the resource manager makes its own at start-up).
    scanned: RefCell<Option<Rc<Scan>>>,
}

/// What the start-up scan found.
#[derive(Debug)]
struct Scan {
    /// The files, in the resource manager's order (the scan reversed).
    files: Rc<Vec<Vec<u8>>>,
    /// The folders it went through, in scan order.
    folders: Rc<Vec<Vec<u8>>>,
    /// Something was left out: the scan went past [`MAX_SCAN_DEPTH`] or [`MAX_LISTED_FILES`].
    cut_off: bool,
}

/// The scan while it is made.
#[derive(Default)]
struct ScanOut {
    files: Vec<Vec<u8>>,
    folders: Vec<Vec<u8>>,
    cut_off: bool,
}

/// One folder of the start-up scan's walk (see `DirHost::scan`): its entries in upper-case name order, files and
/// folders together, symbolic links and junctions left out, folders walked where they stand; at most `max` files (and
/// folders), [`MAX_SCAN_DEPTH`] deep.
fn walk_folder(dir: &Path, prefix: &str, depth: usize, max: usize, out: &mut ScanOut) {
    if depth > MAX_SCAN_DEPTH {
        out.cut_off = true;
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(String, PathBuf, bool)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let kind = e.file_type().ok()?;
            if kind.is_symlink() {
                return None;
            }
            Some((name, e.path(), kind.is_dir()))
        })
        .collect();
    entries.sort_by_key(|(name, _, _)| name.to_ascii_uppercase());
    for (name, path, is_dir) in entries {
        if out.files.len() >= max || out.folders.len() >= max {
            out.cut_off = true;
            return;
        }
        let game_path = format!("{prefix}/{name}");
        if is_dir {
            out.folders.push(game_path.clone().into_bytes());
            walk_folder(&path, &game_path, depth + 1, max, out);
        } else {
            out.files.push(game_path.into_bytes());
        }
    }
}

/// How deep the start-up scan goes into folders and how many files (and, separately, folders) it lists at
/// most: the host's own folders are not a script's to make, but a game folder holding a cycle the platform
/// does not mark as a link, or millions of files, must not make the first search take for ever or fill the
/// memory (the resource manager's own list has no bound).
const MAX_SCAN_DEPTH: usize = 64;
const MAX_LISTED_FILES: usize = 1 << 20;

impl DirHost {
    pub fn new(root: impl Into<PathBuf>) -> DirHost {
        DirHost {
            root: root.into(),
            rules: ResourceRules::default(),
            scanned: RefCell::new(None),
        }
    }

    /// The same folder under a game's resource rules (its refusals and its start-up scan's folders).
    pub fn with_rules(mut self, rules: ResourceRules) -> DirHost {
        self.rules = rules;
        self
    }

    /// The start-up scan: depth-first, each folder's entries in the order Windows lists them (names
    /// compared in upper case), files and folders together, symbolic links and junctions left out;
    /// the files returned reversed, as the resource manager's list is. Names come from `read_dir` and are never
    /// joined from script text.
    fn scan(&self) -> Scan {
        let walk = |dir: &Path, prefix: &str, depth: usize, out: &mut ScanOut| {
            walk_folder(dir, prefix, depth, MAX_LISTED_FILES, out);
        };
        let mut out = ScanOut::default();
        for folder in self.rules.scanned {
            if let Some(dir) = find_entry(&self.root, folder).filter(|d| d.is_dir()) {
                let shown = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(folder)
                    .to_string();
                out.folders.push(shown.clone().into_bytes());
                walk(&dir, &shown, 0, &mut out);
            }
        }
        out.files.reverse();
        Scan {
            files: Rc::new(out.files),
            folders: Rc::new(out.folders),
            cut_off: out.cut_off,
        }
    }

    /// The files below the game folder `path` (a folder of the disk, each component matched ignoring ASCII case, no link
    /// followed), walked as the start-up scan walks its folders (depth first, names compared in upper case, at most
    /// `max` files, `MAX_SCAN_DEPTH` deep), in the walk's order (not reversed), as game paths spelled as the disk
    /// spells them; and whether a cap cut the walk. None when `path` is not a folder. What `setModPaths` reads for a
    /// mod path the start-up did not scan (`Platform::dumpPath` of every mod path).
    pub fn folder_files(&self, path: &[u8], max: usize) -> Option<(Vec<Vec<u8>>, bool)> {
        let parts = safe_path(path)?;
        let mut at = self.root.clone();
        let mut spelled: Vec<String> = Vec::new();
        for part in parts {
            let next = find_entry(&at, part)?;
            spelled.push(next.file_name()?.to_str()?.to_string());
            at = next;
        }
        if spelled.is_empty() || !std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
            return None;
        }
        let mut out = ScanOut::default();
        walk_folder(&at, &spelled.join("/"), 0, max, &mut out);
        Some((out.files, out.cut_off))
    }

    fn scanned(&self) -> Rc<Scan> {
        let mut scanned = self.scanned.borrow_mut();
        Rc::clone(scanned.get_or_insert_with(|| Rc::new(self.scan())))
    }

    /// The folders the start-up scan went through (the rules' scanned folders as the disk spells
    /// them, and every folder below, the empty ones too), as game paths in scan order. Made with the list of
    /// files, once.
    pub fn folders(&self) -> Rc<Vec<Vec<u8>>> {
        Rc::clone(&self.scanned().folders)
    }

    /// Whether the start-up scan left something out (it went deeper than 64 folders, or past a million
    /// files or folders): then neither list is the whole of what the disk has.
    pub fn scan_cut_off(&self) -> bool {
        self.scanned().cut_off
    }

    /// Whether a game path names a folder of the disk (each component matched ignoring ASCII case, by the
    /// rules of [`safe_path`], symbolic links and junctions not followed). Unlike a file, a folder may be a
    /// single component (`config`).
    pub fn is_dir(&self, path: &[u8]) -> bool {
        let Some(parts) = safe_path(path) else {
            return false;
        };
        let mut at = self.root.clone();
        for part in parts {
            let Some(next) = find_entry(&at, part) else {
                return false;
            };
            at = next;
        }
        std::fs::symlink_metadata(&at).is_ok_and(|meta| meta.is_dir())
    }

    /// The file a game path names, if it exists (each component matched ignoring ASCII case). A path
    /// without a folder names no file: the resource list never holds files of the game folder's
    /// root (`isFile("b.txt")` is 0), so none is found here either.
    fn resolve(&self, path: &[u8]) -> Option<PathBuf> {
        if !path.contains(&b'/') {
            return None;
        }
        let parts = safe_path(path)?;
        let mut at = self.root.clone();
        for part in parts {
            at = find_entry(&at, part)?;
        }
        std::fs::symlink_metadata(&at).ok()?.is_file().then_some(at)
    }

    /// Like [`FileHost::read`], for a file of at most `max` bytes: a longer one is missing. The length is checked on the
    /// opened file before anything is read, and the read stops after `max + 1` bytes, so a file that grows meanwhile is
    /// missing too.
    pub fn read_at_most(&self, path: &[u8], max: u64) -> FileRead {
        use std::io::Read;
        if let Some(refusal) = (self.rules.refusal)(path) {
            return FileRead::Refused(refusal);
        }
        let Some((file, meta)) = open_found(|| self.resolve(path)) else {
            return FileRead::Missing;
        };
        if meta.len() > max {
            return FileRead::Missing;
        }
        let len = meta.len();
        let mut out = Vec::new();
        if usize::try_from(len).map_or(true, |n| out.try_reserve_exact(n).is_err()) {
            return FileRead::Missing;
        }
        match file.take(max.saturating_add(1)).read_to_end(&mut out) {
            Ok(_) if out.len() as u64 <= max => FileRead::Found(out),
            _ => FileRead::Missing,
        }
    }
}

/// What tells one file from another: the volume and file index on Windows, the device and inode
/// elsewhere.
type FileId = (u64, u64);

/// The identity of an open file.
fn identity(file: &std::fs::File) -> Option<FileId> {
    #[cfg(windows)]
    {
        let info = winapi_util::file::information(file).ok()?;
        Some((info.volume_serial_number(), info.file_index()))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata().ok()?;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = file;
        None
    }
}

/// The identity of what `at` names, its last component not followed.
fn identity_at(at: &Path) -> Option<FileId> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        // Asks for no access: the attributes only.
        let file = std::fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(at)
            .ok()?;
        identity(&file)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(at).ok()?;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = at;
        None
    }
}

/// Opens the plain file `find` names (`find` looks at each component without following a link),
/// then asks `find` again: what it names then, its last component not followed, must be the very
/// file opened (the same identity). A folder on the way swapped for a link around the open is no
/// file: the open went through the link to another file, or the second look meets the link. Where
/// a file's identity is not known (neither Windows nor Unix), the file is opened without that check.
pub fn open_found(
    mut find: impl FnMut() -> Option<PathBuf>,
) -> Option<(std::fs::File, std::fs::Metadata)> {
    let at = find()?;
    let file = std::fs::File::open(at).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let again = find()?;
    if cfg!(any(windows, unix)) && identity_at(&again)? != identity(&file)? {
        return None;
    }
    Some((file, meta))
}

/// The entry of `dir` called `name`: see [`pick_name`]. `name` is only compared with what `read_dir`
/// lists; it is never joined onto `dir`. A symbolic link or a junction (a reparse point that names another
/// place) is not an entry.
fn find_entry(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        match entry.file_type() {
            Ok(kind) if !kind.is_symlink() => {}
            _ => continue,
        }
        if let Ok(found) = entry.file_name().into_string() {
            entries.push((found, entry.path()));
        }
    }
    let picked = pick_name(entries.iter().map(|(found, _)| found.as_str()), name)?;
    let at = entries.iter().position(|(found, _)| found == picked)?;
    Some(entries.swap_remove(at).1)
}

/// Which of the names a folder lists is `name`: an exact match, else the first that equals it ignoring
/// ASCII case (a case-sensitive file system can hold both `Ok.cs` and `ok.cs`; the exact spelling wins).
fn pick_name<'a>(listed: impl IntoIterator<Item = &'a str>, name: &str) -> Option<&'a str> {
    let mut folded = None;
    for found in listed {
        if found == name {
            return Some(found);
        }
        if folded.is_none() && found.eq_ignore_ascii_case(name) {
            folded = Some(found);
        }
    }
    folded
}

impl FileHost for DirHost {
    fn read(&self, path: &[u8]) -> FileRead {
        if let Some(refusal) = (self.rules.refusal)(path) {
            return FileRead::Refused(refusal);
        }
        let Some((mut file, meta)) = open_found(|| self.resolve(path)) else {
            return FileRead::Missing;
        };
        let mut contents = Vec::new();
        if usize::try_from(meta.len()).map_or(true, |n| contents.try_reserve_exact(n).is_err()) {
            return FileRead::Missing;
        }
        match std::io::Read::read_to_end(&mut file, &mut contents) {
            Ok(_) => FileRead::Found(contents),
            Err(_) => FileRead::Missing,
        }
    }

    fn stat(&self, path: &[u8]) -> FileStat {
        if let Some(refusal) = (self.rules.refusal)(path) {
            return FileStat::Refused(refusal);
        }
        match self
            .resolve(path)
            .and_then(|p| std::fs::symlink_metadata(p).ok())
            .filter(std::fs::Metadata::is_file)
        {
            Some(meta) => FileStat::Found { len: meta.len() },
            None => FileStat::Missing,
        }
    }

    fn modified(&self, path: &[u8]) -> Option<u64> {
        let modified = std::fs::symlink_metadata(self.resolve(path)?)
            .ok()
            .filter(std::fs::Metadata::is_file)?
            .modified()
            .ok()?;
        let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
        u64::try_from(since.as_nanos()).ok()
    }

    fn files(&self) -> Rc<Vec<Vec<u8>>> {
        Rc::clone(&self.scanned().files)
    }
}

/// A calendar date and time (no time zone: the caller has added the offset it wants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilTime {
    pub year: i64,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// The calendar time `seconds` after 1970-01-01 00:00:00 (proleptic Gregorian; Howard Hinnant's
/// `civil_from_days`). Total over every `i64`: the day count is at most about 1.1e14, far from overflow.
pub fn civil_time(seconds: i64) -> CivilTime {
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    CivilTime {
        year,
        month: month as u32,
        day: day as u32,
        hour: (secs / 3600) as u32,
        minute: (secs % 3600 / 60) as u32,
        second: (secs % 60) as u32,
    }
}

/// What compiling TorqueScript source gives.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Compiled {
    /// The compiler's messages, as the console prints them (its syntax error report, the code generator's
    /// warnings such as `<file> (<line>): string always evaluates to 0.`), in order.
    pub report: Vec<LogLine>,
    /// The source had a syntax error (`eval` then prints `eval error >> <text>`, `compile` gives 0), or
    /// could not be compiled at all (too large).
    pub syntax_error: bool,
    /// The code; None when there is no statement, or after a syntax error for [`Purpose::Check`]. For
    /// [`Purpose::Run`] it holds the statements the parser read before an error: `exec` runs those
    /// (a file with an error on line 2 runs line 1).
    pub code: Option<DsoFile>,
    /// The work the compile did, in the VM's work units (for a compiler one per source byte scanned,
    /// per parser step and per tree node the code generator visited): the `Vm` charges it to the step budget
    /// like any other work (`WORK_PER_STEP` units a step), on top of the bytes of the file or text it
    /// charged when it read them.
    pub work: u64,
    /// A [`Purpose::Run`] compile of the same text would give exactly this result (the same messages and code),
    /// though this one was made for [`Purpose::Check`]: `exec` then uses it for its second pass instead of compiling
    /// again. A compiler leaves it false (the default) unless it knows: a syntax error or a message the purposes word
    /// differently make the two differ.
    pub same_as_run: bool,
}

/// Turns source text into bytecode. `name` is the file name the messages name (None for `eval`'d text);
/// `purpose` is TGE's entry point: [`Purpose::Check`] for `compile()` and the first of `exec`'s two
/// compiles of a source, [`Purpose::Run`] for the second and for `eval`.
pub trait Compiler {
    fn compile(&self, source: &[u8], name: Option<&[u8]>, purpose: Purpose) -> Compiled;
}

/// No compiler: every source is a syntax error with one explanation line (the default of a [`Vm`](crate::vm::Vm);
/// hosts and tests that run only compiled files keep it).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCompiler;

impl Compiler for NoCompiler {
    fn compile(&self, _source: &[u8], name: Option<&[u8]>, _purpose: Purpose) -> Compiled {
        let shown = name.map_or_else(
            || "<input>".to_string(),
            |n| String::from_utf8_lossy(n).into_owned(),
        );
        Compiled {
            report: vec![LogLine {
                level: Level::Error,
                text: format!(
                    "{shown}: cannot compile TorqueScript source: no compiler is installed"
                )
                .into_bytes(),
            }],
            syntax_error: true,
            code: None,
            work: 0,
            same_as_run: false,
        }
    }
}

/// The clocks scripts read besides the simulation clock (which the host drives with `Vm::advance_time`):
/// a millisecond counter, the wall clock and the local time zone.
pub trait Clock {
    /// A millisecond counter that moves forward (as Windows' `GetTickCount`, the milliseconds since the computer
    /// started; `getRealTime`, the seed of `setRandomSeed()`). It may wrap.
    fn real_time_ms(&self) -> u32;

    /// The wall clock: seconds since the Unix epoch.
    fn unix_time(&self) -> i64;

    /// How many seconds local time is ahead of UTC at `unix_time` (the natives print local times).
    fn utc_offset(&self, unix_time: i64) -> i32 {
        let _ = unix_time;
        0
    }
}

/// The computer's clocks. TGE's counter on Windows is `GetTickCount`, the milliseconds since the computer started, which no
/// portable library reads: this one starts at the wall clock's milliseconds (their low 32 bits) when the clock is made
/// and moves with the wall from there, so that it is, like `GetTickCount`, never 0 in practice (a start-up script may
/// seed the random generator from it, and a seed of 0 would make every `getRandom` 0). Local time is UTC: the standard
/// library cannot read the time zone, so a host that wants local times gives the `Vm` its own `Clock`. Only
/// `setRandomSeed()` and the clock natives read it; nothing else a script can do depends on the time, so tests that do
/// not set a clock stay deterministic (they use [`FixedClock`] to read one).
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
    /// The counter's value when the clock was made.
    base: u32,
}

impl SystemClock {
    pub fn new() -> SystemClock {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u32);
        SystemClock {
            start: Instant::now(),
            base: wall.max(1),
        }
    }
}

impl Default for SystemClock {
    fn default() -> SystemClock {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn real_time_ms(&self) -> u32 {
        // The counter wraps like a 32-bit one (the low 32 bits of the milliseconds).
        self.base
            .wrapping_add(self.start.elapsed().as_millis() as u32)
    }

    fn unix_time(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}

/// A tick counter that follows the work scripts do, not the wall: every read moves it on by one millisecond, so a busy wait
/// (`while (getRealTime() - %t < 150) {}`) ends after that many reads whatever the speed of the machine
/// and of the build, and costs the same steps in a debug and a release build. The wall clock it reports is a fixed instant. For
/// a host that runs the simulation faster than real time and must give the same answers every time; a real-time server keeps
/// [`SystemClock`]. The counter starts at [`VIRTUAL_UPTIME_MS`], or where [`VirtualClock::starting_at`] says.
#[derive(Debug, Clone)]
pub struct VirtualClock {
    reads: std::cell::Cell<u32>,
}

/// The instant a [`VirtualClock`] reports as the wall clock (2023-11-14 22:13:20 UTC).
pub const VIRTUAL_UNIX_TIME: i64 = 1_700_000_000;
/// The first value a [`VirtualClock`]'s counter reads (ms) by default: one second.
pub const VIRTUAL_UPTIME_MS: u32 = 1_000;

impl VirtualClock {
    pub fn new() -> VirtualClock {
        VirtualClock::default()
    }

    /// A counter whose first read is `ms`.
    pub fn starting_at(ms: u32) -> VirtualClock {
        VirtualClock {
            reads: std::cell::Cell::new(ms),
        }
    }
}

impl Default for VirtualClock {
    fn default() -> VirtualClock {
        VirtualClock::starting_at(VIRTUAL_UPTIME_MS)
    }
}

impl Clock for VirtualClock {
    fn real_time_ms(&self) -> u32 {
        let now = self.reads.get();
        self.reads.set(now.wrapping_add(1));
        now
    }

    fn unix_time(&self) -> i64 {
        VIRTUAL_UNIX_TIME
    }
}

/// Clocks that read what they are set to, for tests and replays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FixedClock {
    pub real_time_ms: u32,
    pub unix_time: i64,
    pub utc_offset: i32,
}

impl Clock for FixedClock {
    fn real_time_ms(&self) -> u32 {
        self.real_time_ms
    }

    fn unix_time(&self) -> i64 {
        self.unix_time
    }

    fn utc_offset(&self, _unix_time: i64) -> i32 {
        self.utc_offset
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
