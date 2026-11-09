//! The files a headless host serves its scripts, as TGE's resource manager has them: a game folder on disk (read-only), the
//! zips it holds mounted as folders ([`crate::zips::ZipMount`]: `<dir>/<Name>.zip` reads as the folder `<dir>/<Name>/`), and the files
//! scripts write, kept in memory (nothing a script does changes the game folder).
//!
//! The game folder and its zips are a [`GameFolder`] (indexed once at start-up, `crate::folder`); the written files go over it.
//!
//! **Lookups.** The policy's refusals first ([`MountPolicy::rules`]), then the written files, then the zips (a zip's entry
//! beats a loose file of its path; an entry that cannot be read leaves the loose file, if there is one), then the folder. Names
//! are matched without regard to case; `\`, `:`, NUL, `..`, device names and the like never reach the disk or a zip
//! (`safe_path`).
//!
//! **The searches' list** (`FileHost::files`): the resource manager adds each resource at the head of its list, so the start-up
//! scan comes reversed, each mounted zip's entries (in reverse archive order) right before the zip file, a loose file at the
//! path of a zip entry listed once; a file a script writes that the list does not have goes to the head, the newest first.
//!
//! **Deleting** (`FileHost::remove`) deletes a written file; the game folder and its zips are not written, so a delete of one
//! of their files answers false. A resource stays listed while its file is there.
//!
//! **Which zips mount** is the host's [`MountPolicy`]: every zip the scan finds (TGE's rule) unless the policy names the one
//! folder whose zips mount.
//!
//! **Caps** ([`Limits`]; what the folder holds and what a script writes are untrusted):
//! - mounted zips: [`MAX_MOUNTED_ZIPS`] of them, [`MAX_MOUNTED_BYTES`] in all and [`MAX_MOUNTED_ENTRIES`] entries. A zip is
//!   charged its own bytes plus [`DIRECTORY_FACTOR`] times its directory's; one longer than [`crate::zips::MAX_ZIP_BYTES`], or than the
//!   bytes left, is refused before it is read, and the read stops after that many bytes. A refused zip is not mounted;
//! - the written files: [`MAX_OVERLAY_BYTES`] of contents and [`MAX_OVERLAY_FILES`] files, at paths of at most
//!   [`MAX_WRITTEN_PATH`] bytes; a write past them fails (as on a full disk) and changes nothing;
//! - a read of a file of the folder returns at most [`MAX_READ_BYTES`].
//!
//! **Work** ([`FileHost::work_done`]) counts what returns no bytes to the caller, so a host that charges per byte read can
//! charge it too: the bytes the zips inflated for reads that failed, and the bytes appends copied from the folder or a zip.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use torque_ts_vm::host::{
    FileHost, FileRead, FileStat, MemFiles, ResourceRules, os_name, safe_path,
};
use torque_ts_vm::text::fold;

use crate::folder::GameFolder;
pub use crate::folder::{MountPolicy, MountReport};

/// The most bytes a read of a file of the folder returns (a bigger file is missing to a read; `stat` still tells its length).
/// The same as a zip entry may hold ([`crate::zips::MAX_ENTRY_BYTES`]) and a script's string may be.
pub const MAX_READ_BYTES: u64 = 64 << 20;
/// The most bytes of zips mounted in all. See [`DIRECTORY_FACTOR`] for what a zip is charged.
pub const MAX_MOUNTED_BYTES: usize = 1 << 30;
/// How many times a zip's directory is charged to [`MAX_MOUNTED_BYTES`] on top of the archive's own bytes: the `zip` crate
/// holds a directory at up to nine times its size ([`crate::zips`]).
pub const DIRECTORY_FACTOR: usize = 9;
/// The most zips mounted. Each costs memory beyond its bytes, even when it is empty.
pub const MAX_MOUNTED_ZIPS: usize = 4096;
/// The most entries mounted from all zips together. The searches' list holds a path for each.
pub const MAX_MOUNTED_ENTRIES: usize = 1 << 20;
/// The most bytes the written files hold in all; a write past it fails.
pub const MAX_OVERLAY_BYTES: usize = 256 << 20;
/// The most written files. Each costs memory even when empty (its name, a table entry, a place in the searches' list), and
/// making one is cheap for a script.
pub const MAX_OVERLAY_FILES: usize = 65_536;
/// The longest path a script may write: a 1024-byte file name buffer with its NUL.
pub const MAX_WRITTEN_PATH: usize = 1023;

/// The caps on what [`ResourceFiles`] holds ([`ResourceFiles::open_with_limits`]). The default is the constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// [`MAX_MOUNTED_BYTES`].
    pub mounted_bytes: usize,
    /// [`MAX_MOUNTED_ZIPS`].
    pub mounted_zips: usize,
    /// [`MAX_MOUNTED_ENTRIES`].
    pub mounted_entries: usize,
    /// [`MAX_OVERLAY_BYTES`].
    pub overlay_bytes: usize,
    /// [`MAX_OVERLAY_FILES`].
    pub overlay_files: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            mounted_bytes: MAX_MOUNTED_BYTES,
            mounted_zips: MAX_MOUNTED_ZIPS,
            mounted_entries: MAX_MOUNTED_ENTRIES,
            overlay_bytes: MAX_OVERLAY_BYTES,
            overlay_files: MAX_OVERLAY_FILES,
        }
    }
}

/// Written files the base list lacks, in the order they were made.
#[derive(Default)]
struct Front {
    next: u64,
    order: BTreeMap<u64, Vec<u8>>,
    /// Folded path -> its key in `order`.
    keys: HashMap<Vec<u8>, u64>,
}

/// What the written files hold.
#[derive(Clone, Copy, Default)]
struct Held {
    bytes: usize,
    files: usize,
}

struct Inner {
    rules: ResourceRules,
    limits: Limits,
    /// The game folder, its zips and the start-up list (the scan reversed, zip entries before their zip).
    folder: GameFolder,
    written: MemFiles,
    front: RefCell<Front>,
    list: RefCell<Option<Rc<Vec<Vec<u8>>>>>,
    held: Cell<Held>,
    /// The bytes appends copied into the written files ([`FileHost::work_done`]).
    copied: Cell<u64>,
}

/// A game folder, its zips and the written files. A clone is another handle on the same files.
#[derive(Clone)]
pub struct ResourceFiles(Rc<Inner>);

impl ResourceFiles {
    /// The game folder at `root` with what `policy` mounts.
    pub fn open(root: impl Into<PathBuf>, policy: MountPolicy) -> ResourceFiles {
        ResourceFiles::open_with_limits(root, policy, Limits::default())
    }

    /// Like [`open`](Self::open), with other caps than the defaults.
    pub fn open_with_limits(
        root: impl Into<PathBuf>,
        policy: MountPolicy,
        limits: Limits,
    ) -> ResourceFiles {
        ResourceFiles::open_with_parts(root, policy, limits, Vec::new(), Vec::new())
    }

    /// Like [`open_with_limits`](Self::open_with_limits), with a host's `archives` (`(stem, archive bytes)`, mounted as if they
    /// were `<zip folder>/<stem>.zip`) and `startup` files (`(path, contents)`, presented as files the scan found): see
    /// [`GameFolder::open`].
    pub fn open_with_parts(
        root: impl Into<PathBuf>,
        policy: MountPolicy,
        limits: Limits,
        archives: Vec<(String, Vec<u8>)>,
        startup: Vec<(String, Vec<u8>)>,
    ) -> ResourceFiles {
        let folder = GameFolder::open(root, policy, limits, archives, startup);
        ResourceFiles(Rc::new(Inner {
            rules: policy.rules,
            limits,
            folder,
            written: MemFiles::new().with_rules(policy.rules),
            front: RefCell::new(Front::default()),
            list: RefCell::new(None),
            held: Cell::new(Held::default()),
            copied: Cell::new(0),
        }))
    }

    /// The game folder these files were opened over.
    pub fn root(&self) -> &Path {
        self.0.folder.root()
    }

    /// The zip files mounted, as the scan spells them.
    pub fn mounted(&self) -> Vec<Vec<u8>> {
        self.0.folder.mounted()
    }

    /// What mounting found.
    pub fn report(&self) -> &MountReport {
        self.0.folder.report()
    }

    /// The game folder and its zips under the written files.
    pub fn folder(&self) -> &GameFolder {
        &self.0.folder
    }

    /// The contents of a file a script wrote, if there is one.
    pub fn written(&self, path: &str) -> Option<Vec<u8>> {
        self.0.written.get(path)
    }

    /// The length of the written file at `path`, if there is one.
    fn written_len(&self, path: &[u8]) -> Option<usize> {
        match self.0.written.stat(path) {
            FileStat::Found { len } => Some(usize::try_from(len).unwrap_or(usize::MAX)),
            _ => None,
        }
    }

    /// Whether the written files may hold `new_len` bytes at a path whose written file is `old_len` long (None: a new file).
    fn room_for(&self, old_len: Option<usize>, new_len: usize) -> bool {
        let held = self.0.held.get();
        let others = held.bytes.saturating_sub(old_len.unwrap_or(0));
        others.saturating_add(new_len) <= self.0.limits.overlay_bytes
            && (old_len.is_some() || held.files < self.0.limits.overlay_files)
    }
}

impl FileHost for ResourceFiles {
    fn read(&self, path: &[u8]) -> FileRead {
        if let Some(refusal) = (self.0.rules.refusal)(path) {
            return FileRead::Refused(refusal);
        }
        if let FileRead::Found(contents) = self.0.written.read(path) {
            return FileRead::Found(contents);
        }
        if safe_path(path).is_none() {
            return FileRead::Missing;
        }
        self.0.folder.read(&os_name(path))
    }

    fn read_prefix(&self, path: &[u8], max: usize) -> FileRead {
        if let Some(refusal) = (self.0.rules.refusal)(path) {
            return FileRead::Refused(refusal);
        }
        if let FileRead::Found(contents) = self.0.written.read_prefix(path, max) {
            return FileRead::Found(contents);
        }
        if safe_path(path).is_none() {
            return FileRead::Missing;
        }
        self.0.folder.read_prefix(&os_name(path), max)
    }

    fn stat(&self, path: &[u8]) -> FileStat {
        if let Some(refusal) = (self.0.rules.refusal)(path) {
            return FileStat::Refused(refusal);
        }
        if let FileStat::Found { len } = self.0.written.stat(path) {
            return FileStat::Found { len };
        }
        if safe_path(path).is_none() {
            return FileStat::Missing;
        }
        self.0.folder.stat(&os_name(path))
    }

    fn is_archive_entry(&self, path: &[u8]) -> bool {
        self.0.folder.entry(&os_name(path)).is_some()
    }

    /// The directory of the resource as the folder or the zip spells it ([`GameFolder::stored_dir`]); none for a written
    /// file.
    fn real_path(&self, path: &[u8]) -> Option<Vec<u8>> {
        if (self.0.rules.refusal)(path).is_some()
            || self.0.written.stat(path) != FileStat::Missing
            || safe_path(path).is_none()
        {
            return None;
        }
        self.0.folder.stored_dir(&os_name(path))
    }

    fn modified(&self, path: &[u8]) -> Option<u64> {
        if let Some(time) = self.0.written.modified(path) {
            return Some(time);
        }
        self.0.folder.modified(&os_name(path))
    }

    fn files(&self) -> Rc<Vec<Vec<u8>>> {
        let mut list = self.0.list.borrow_mut();
        Rc::clone(list.get_or_insert_with(|| {
            let front = self.0.front.borrow();
            let mut all: Vec<Vec<u8>> = front.order.values().rev().cloned().collect();
            all.extend(self.0.folder.list().iter().cloned());
            Rc::new(all)
        }))
    }

    fn remove(&self, path: &[u8]) -> bool {
        let len = self.written_len(path);
        let removed = self.0.written.remove(path);
        if removed {
            let held = self.0.held.get();
            self.0.held.set(Held {
                bytes: held.bytes.saturating_sub(len.unwrap_or(0)),
                files: held.files.saturating_sub(usize::from(len.is_some())),
            });
            let mut front = self.0.front.borrow_mut();
            if let Some(at) = front.keys.remove(&fold(&os_name(path))) {
                front.order.remove(&at);
            }
            self.0.list.take();
        }
        removed
    }

    fn write(&self, path: &[u8], contents: &[u8], append: bool) -> bool {
        if path.len() > MAX_WRITTEN_PATH {
            return false;
        }
        let old_len = self.written_len(path);
        let start = if append && old_len.is_none() {
            // The append copies the folder's or the zip's file first: refused before the read when the copy cannot fit.
            if let FileStat::Found { len } = self.stat(path) {
                let len = usize::try_from(len).unwrap_or(usize::MAX);
                if !self.room_for(None, len.saturating_add(contents.len())) {
                    return false;
                }
            }
            match self.read(path) {
                FileRead::Found(old) => {
                    self.0
                        .copied
                        .set(self.0.copied.get().saturating_add(old.len() as u64));
                    Some(old)
                }
                _ => None,
            }
        } else {
            None
        };
        let new_len = match (&start, append) {
            (Some(old), _) => old.len().saturating_add(contents.len()),
            (None, true) => old_len.unwrap_or(0).saturating_add(contents.len()),
            (None, false) => contents.len(),
        };
        if !self.room_for(old_len, new_len) {
            return false;
        }
        let ok = match start {
            Some(mut old) => {
                old.extend_from_slice(contents);
                self.0.written.write(path, &old, false)
            }
            None => self.0.written.write(path, contents, append),
        };
        if ok {
            let held = self.0.held.get();
            self.0.held.set(Held {
                bytes: held
                    .bytes
                    .saturating_sub(old_len.unwrap_or(0))
                    .saturating_add(new_len),
                files: held.files + usize::from(old_len.is_none()),
            });
            let key = fold(&os_name(path));
            let mut front = self.0.front.borrow_mut();
            if !self.0.folder.listed(&os_name(path)) && !front.keys.contains_key(&key) {
                let at = front.next;
                front.next += 1;
                front.order.insert(at, path.to_vec());
                front.keys.insert(key, at);
                self.0.list.take();
            }
        }
        ok
    }

    fn work_done(&self) -> u64 {
        self.0.folder.wasted().saturating_add(self.0.copied.get())
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
