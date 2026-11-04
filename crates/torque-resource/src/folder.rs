//! The game folder as the resource manager's dictionary holds it at start-up: the folder scanned once and indexed, with its zips
//! mounted as folders ([`ZipMount`]: `<dir>/<Name>.zip` reads as the folder `<dir>/<Name>/`) and the searches' list made of both
//! ([`GameFolder`]). A host's own files layer goes over it ([`crate::files::ResourceFiles`] is TGE's).
//!
//! **The folder is indexed once.** TGE's `ResManager::setModPaths` walks the mod paths ([`ResourceRules::scanned`]) once and
//! makes a resource of every file it finds; a lookup asks that dictionary. Here every file and folder the start-up scan
//! (`DirHost::files` and `folders`) names goes into a hash by its folded path, and a lookup of one of them reads or measures that
//! one file by the name the scan gave it (never joined from script text), each component looked at without following it: a link
//! on the way, there at the scan or put there since, is no file (`DirHost` follows none either). A name the dictionary lacks is looked for on the disk
//! (`DirHost`), as TGE's `ResManager::find` does in a build that is not `TORQUE_SHIPPING`; with
//! [`MountPolicy::dictionary_only`] (the shipping build) a name below a mod path that the dictionary lacks is missing, so a file
//! made on the disk after the scan is not seen there. Everything is looked for on the disk when the scan was cut short
//! (`DirHost::scan_cut_off`).
//!
//! **Which zips mount** ([`MountPolicy::zip_folder`]): every zip the scan finds (TGE's `searchPath`, which scans each zip it
//! meets), or only those directly in the one folder the host names. They are mounted in the scan's order, so a cap leaves out
//! the last ones the scan finds. A host may give more archives ([`GameFolder::open`]'s `archives`), mounted as if they were
//! files of the zip folder, and more start-up files (`startup`), presented as files the scan found.
//!
//! **The searches' list** ([`GameFolder::list`]): TGE's `createResource` puts each resource at the head of its list, so the
//! list is the scan reversed, each mounted zip's entries (in reverse archive order) right before the zip file; a loose file at
//! the path of a zip entry is the same resource, listed once at the entry's place.
//!
//! **Caps** (what the folder holds and what a host gives are untrusted): the mounted zips' ([`Limits`]: a zip is charged its
//! own bytes plus [`DIRECTORY_FACTOR`] times its directory's, and refused before it is read when its length is over
//! [`MAX_ZIP_BYTES`] or the bytes left, the read stopping after that many bytes); a host's archive is charged its bytes twice
//! (the bytes a read of its file returns are kept); a zip read or given that does not open is charged its bytes, and every zip
//! read or given counts against [`Limits::mounted_zips`], and the bytes of every zip read or given, mounted or not, stay within
//! [`Limits::mounted_bytes`] ([`MountReport::read`]), so bad archives cannot make the mount read without end;
//! [`MAX_STARTUP_FILES`] and [`MAX_STARTUP_BYTES`] start-up files; a read of a file of the disk returns at most
//! [`MAX_READ_BYTES`]; the report keeps [`MAX_REPORTED_REFUSALS`] refusals and counts the rest, each name cut to
//! [`MAX_REPORTED_NAME`] characters.
//!
//! **Work.** The bytes the zips inflated for reads that failed ([`GameFolder::wasted`]) return nothing to the caller, so a host
//! that charges per byte read charges them through `FileHost::work_done`; [`GameFolder::inflated`] counts every byte inflated.
//!
//! The behaviour follows TGE's `core/resManager.cc` (`Copyright (C) GarageGames.com, Inc.`), MIT-licensed as Torque3D's
//! predecessor; the code here is this crate's own.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use torque_ts_vm::host::{
    DirHost, FileHost, FileRead, FileStat, ResourceRules, collapse_dots, open_found, safe_path,
    sanitize_component,
};
use torque_ts_vm::text::fold;

use crate::files::{DIRECTORY_FACTOR, Limits, MAX_READ_BYTES, MAX_WRITTEN_PATH};
use crate::zips::{MAX_ZIP_BYTES, MountError, ZipEntry, ZipMount};

/// The most start-up files [`GameFolder::open`] presents, and the most bytes of them in all.
pub const MAX_STARTUP_FILES: usize = 1024;
/// See [`MAX_STARTUP_FILES`].
pub const MAX_STARTUP_BYTES: usize = 64 << 20;
/// The most refusals [`MountReport::refused`] keeps; [`MountReport::refused_more`] counts the others.
pub const MAX_REPORTED_REFUSALS: usize = 1024;
/// The most characters of a name [`MountReport::refused`] keeps.
pub const MAX_REPORTED_NAME: usize = 80;

/// What mounts where, and the resource manager's rules ([`GameFolder::open`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct MountPolicy {
    /// The one folder (a top folder of the game folder, any case) whose zips mount, directly in it; `None` mounts every zip the
    /// start-up scan finds (TGE's rule).
    pub zip_folder: Option<&'static [u8]>,
    /// How the zip folder is spelled when the game folder does not have it: where a host's archives are mounted (with no zip
    /// folder and this empty, a host's archives are refused).
    pub zip_folder_spelled: &'static str,
    /// TGE's shipping build (`TORQUE_SHIPPING`): a name below a mod path that the dictionary lacks is missing, not looked for on
    /// the disk. Off: it is looked for on the disk (TGE's other builds).
    pub dictionary_only: bool,
    /// The resource manager's rules: the paths it refuses, the folders a write may go to, the folders its scan walks (the mod
    /// paths, in their order).
    pub rules: ResourceRules,
}

impl MountPolicy {
    /// Where the scanned file `path` mounts, if it is a zip this policy mounts: its stem (the path without `.zip`) and its file
    /// name.
    fn mounts<'a>(&self, path: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
        let stem = zip_stem(path)?;
        let name = path.rsplit(|&b| b == b'/').next().unwrap_or(path);
        match self.zip_folder {
            None => Some((stem, name)),
            Some(folder) => folder_rest(folder, path)
                .filter(|rest| !rest.contains(&b'/'))
                .map(|_| (stem, name)),
        }
    }

    /// The folder a host's archives go in (any case): the zip folder, else its spelling when there is one.
    fn archive_folder(&self) -> Option<&'static [u8]> {
        self.zip_folder.or_else(|| {
            (!self.zip_folder_spelled.is_empty()).then_some(self.zip_folder_spelled.as_bytes())
        })
    }

    /// Whether the first folder of a game path is a mod path the start-up scan goes through (or the path is one of them).
    pub fn is_scanned(&self, path: &[u8]) -> bool {
        let end = path.iter().position(|&b| b == b'/').unwrap_or(path.len());
        let first = path.get(..end).unwrap_or_default();
        self.rules
            .scanned
            .iter()
            .any(|s| first.eq_ignore_ascii_case(s.as_bytes()))
    }

    /// The rank of a game folder in the start-up scan: [`ResourceRules::scanned`]'s order, anything else after them.
    fn scan_rank(&self, top: &[u8]) -> usize {
        self.rules
            .scanned
            .iter()
            .position(|s| top.eq_ignore_ascii_case(s.as_bytes()))
            .unwrap_or(self.rules.scanned.len())
    }

    /// Which of two game paths the start-up scan finds first: depth first, the first folders in [`ResourceRules::scanned`]'s
    /// order, the entries of every folder by their upper-case names, a folder before what is in it.
    fn scan_cmp(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let mut a = a.split(|&c| c == b'/');
        let mut b = b.split(|&c| c == b'/');
        let mut top = true;
        loop {
            let order = match (a.next(), b.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(x), Some(y)) if top => self
                    .scan_rank(x)
                    .cmp(&self.scan_rank(y))
                    .then_with(|| cmp_upper(x, y)),
                (Some(x), Some(y)) => cmp_upper(x, y),
            };
            top = false;
            if order != Ordering::Equal {
                return order;
            }
        }
    }
}

/// What follows `<folder>/` in a game path (any case), if it starts with that.
fn folder_rest<'a>(folder: &[u8], path: &'a [u8]) -> Option<&'a [u8]> {
    let n = folder.len();
    let head = path.get(..n)?;
    (head.eq_ignore_ascii_case(folder) && path.get(n) == Some(&b'/'))
        .then(|| path.get(n + 1..))
        .flatten()
}

/// `path` without a trailing `.zip` (any case); None when it has none or nothing before it.
fn zip_stem(path: &[u8]) -> Option<&[u8]> {
    let n = path.len().checked_sub(4)?;
    let (stem, ext) = path.split_at(n);
    (ext.eq_ignore_ascii_case(b".zip") && !stem.is_empty() && !stem.ends_with(b"/")).then_some(stem)
}

/// The start-up files the host gave: folded path -> contents.
type StartupFiles = HashMap<Vec<u8>, Rc<Vec<u8>>>;
/// What mounting found (for a host's report; nothing is printed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountReport {
    /// Zips mounted.
    pub mounted: usize,
    /// Zips and start-up files not taken, as (name, why): at most [`MAX_REPORTED_REFUSALS`] of them, each name cut to
    /// [`MAX_REPORTED_NAME`] characters.
    pub refused: Vec<(String, String)>,
    /// Refusals beyond those kept in `refused`.
    pub refused_more: usize,
    /// Entries left out of mounted zips (unsafe names, methods nobody reads, a later twin of a name, ...).
    pub skipped_entries: usize,
    /// Entries mounted.
    pub entries: usize,
    /// What the zips were charged against [`Limits::mounted_bytes`]: the mounted ones, and the bytes read or given of those
    /// that failed to open.
    pub bytes: usize,
    /// The bytes of zips read or given, mounted or not: never more than [`Limits::mounted_bytes`].
    pub read: usize,
    /// Start-up files presented ([`GameFolder::open`]); the ones left out are in `refused`.
    pub startup_files: usize,
}

/// One mounted zip.
struct Mounted {
    /// `<dir>/<stem>.zip` as the folder spells it.
    zip_path: Vec<u8>,
    /// `<dir>/<stem>/` as spelled: the prefix of its entries' paths.
    folder: Vec<u8>,
    /// `<STEM>.ZIP` in upper case: what the folder's scan sorts it by.
    top: Vec<u8>,
    mount: ZipMount,
    /// A host's archive, not a file of the folder: listed and read as if it were one (its bytes).
    extra: Option<Rc<Vec<u8>>>,
}

fn describe(e: &MountError) -> String {
    match e {
        MountError::TooLarge(n) => format!("too large ({n} bytes)"),
        MountError::TooManyEntries(n) => format!("claims {n} entries"),
        MountError::TooManyEndRecords(n) => format!("{n} end-of-directory signatures"),
        MountError::DirectoryTooLarge(n) => format!("a central directory of {n} bytes"),
        MountError::NotAZip(why) => format!("not a zip: {why}"),
    }
}

/// A file's time as nanoseconds since the Unix epoch.
fn nanos(time: std::time::SystemTime) -> Option<u64> {
    let since = time.duration_since(std::time::UNIX_EPOCH).ok()?;
    u64::try_from(since.as_nanos()).ok()
}

/// Whether `meta` (of a path not followed, `symlink_metadata`) is a link of any kind: a symbolic link, or on Windows any
/// reparse point (a junction too).
fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// The file the scan named at `rel` below `root` and its length and time, when no part of the way is a link (each component
/// is looked at without following it, as `DirHost` resolves a name) and the last is a plain file. A name the scan gave that
/// has since become a link, or a link below a link, is no file.
fn plain_file(root: &Path, rel: &[u8]) -> Option<(PathBuf, std::fs::Metadata)> {
    let rel = std::str::from_utf8(rel).ok()?;
    let mut at = root.to_path_buf();
    let mut last = None;
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        at.push(part);
        let meta = std::fs::symlink_metadata(&at).ok()?;
        if is_link(&meta) {
            return None;
        }
        last = Some(meta);
    }
    let meta = last.filter(std::fs::Metadata::is_file)?;
    Some((at, meta))
}

/// The file the scan named at `rel` below `root`, opened, when [`plain_file`] finds it before the open and again after it and
/// the file opened is the one found then ([`open_found`]): a folder on the way swapped for a link around the open is no file.
fn open_plain(root: &Path, rel: &[u8]) -> Option<(std::fs::File, std::fs::Metadata)> {
    open_found(|| plain_file(root, rel).map(|(at, _)| at))
}

/// The file the scan named at `rel` below `root` ([`open_plain`]), whole, when it is at most `max` bytes long (the read stops
/// after `max` and one more).
fn read_file(root: &Path, rel: &[u8], max: u64) -> Option<Vec<u8>> {
    let (file, meta) = open_plain(root, rel)?;
    if meta.len() > max {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(usize::try_from(meta.len()).ok()?)
        .ok()?;
    file.take(max.saturating_add(1))
        .read_to_end(&mut out)
        .ok()?;
    (out.len() as u64 <= max).then_some(out)
}

/// The zips being mounted: the caps and the books.
struct Mounting {
    limits: Limits,
    zips: Vec<Mounted>,
    /// Folded stem path (`<dir>/<stem>`) -> index in `zips`.
    by_stem: HashMap<Vec<u8>, usize>,
    /// Folded game path of an entry -> (index in `zips`, index in its entries). The first zip to mount a path keeps it.
    index: HashMap<Vec<u8>, (usize, usize)>,
    report: MountReport,
    used: usize,
    entries: usize,
    /// Zips read or given, mounted or not: charged against [`Limits::mounted_zips`] too.
    tried: usize,
    /// [`MountReport::read`]: whatever becomes of each zip, what is read stays within [`Limits::mounted_bytes`].
    read: usize,
}

impl Mounting {
    /// Names a refusal in the report, the name cut to [`MAX_REPORTED_NAME`] characters.
    fn refuse(&mut self, name: String, why: String) {
        if self.report.refused.len() < MAX_REPORTED_REFUSALS {
            let name = if name.chars().count() > MAX_REPORTED_NAME {
                name.chars().take(MAX_REPORTED_NAME).collect()
            } else {
                name
            };
            self.report.refused.push((name, why));
        } else {
            self.report.refused_more = self.report.refused_more.saturating_add(1);
        }
    }

    /// Whether the zip of stem path `stem_path`, whose own stem is `stem`, may be looked at at all: a name a host may serve,
    /// not one already mounted, and room for one more zip. Decided before the file is measured or read.
    fn admit_name(&self, stem_path: &str, stem: &str) -> Result<(), String> {
        if !sanitize_component(stem.as_bytes()) {
            return Err("not a name a host may serve".into());
        }
        if self.by_stem.contains_key(&fold(stem_path.as_bytes())) {
            return Err("a zip of that name is already mounted".into());
        }
        if self.zips.len() >= self.limits.mounted_zips {
            return Err(format!("too many zips mounted ({})", self.zips.len()));
        }
        if self.tried >= self.limits.mounted_zips {
            return Err(format!("too many zips read ({})", self.tried));
        }
        Ok(())
    }

    /// Charges a zip that was read (or given) but did not open: its `len` bytes against the budget and one against the count of
    /// zips, so bad archives cannot make the mount read without end. (One that opened but is over a budget counts only.)
    fn charge_failed(&mut self, len: usize) {
        self.used = self.used.saturating_add(len);
        self.tried = self.tried.saturating_add(1);
    }

    /// The most bytes a read of a zip of the folder may return: the budget left (of what is charged and of what is read), at
    /// most [`MAX_ZIP_BYTES`].
    fn room(&self) -> u64 {
        let budget = self.limits.mounted_bytes;
        budget
            .saturating_sub(self.used)
            .min(budget.saturating_sub(self.read))
            .min(MAX_ZIP_BYTES) as u64
    }

    /// Whether a zip whose bytes are `len` long, held `copies` times, may be read; the reason when not.
    /// Everything that can be decided before reading the file.
    fn admit_size(&self, len: usize, copies: usize) -> Result<(), String> {
        if len > MAX_ZIP_BYTES {
            return Err(format!("too large ({len} bytes), not read"));
        }
        if self.used.saturating_add(len.saturating_mul(copies)) > self.limits.mounted_bytes {
            return Err(format!(
                "over the mount budget before reading ({len} bytes)"
            ));
        }
        if self.read.saturating_add(len) > self.limits.mounted_bytes {
            return Err(format!("over the bytes the mount may read ({len} bytes)"));
        }
        Ok(())
    }

    /// Mounts an archive the admissions let through (or records why not): the file `zip_path`, called `name`, whose entries
    /// are the folder `<stem_path>/`.
    fn add(&mut self, zip_path: &[u8], stem_path: &str, name: &str, bytes: Vec<u8>, extra: bool) {
        let copies = if extra { 2 } else { 1 };
        let held = bytes.len().saturating_mul(copies);
        let len = bytes.len();
        self.read = self.read.saturating_add(len);
        let copy = extra.then(|| Rc::new(bytes.clone()));
        let mount = match ZipMount::open(bytes) {
            Ok(mount) => mount,
            Err(e) => {
                self.charge_failed(len);
                return self.refuse(name.to_string(), describe(&e));
            }
        };
        let cost = held.saturating_add(DIRECTORY_FACTOR.saturating_mul(mount.directory_bytes()));
        if self.used.saturating_add(cost) > self.limits.mounted_bytes {
            self.tried = self.tried.saturating_add(1);
            return self.refuse(
                name.to_string(),
                format!("over the mount budget with its directory ({cost} bytes)"),
            );
        }
        let count = mount.entries().len();
        if self.entries.saturating_add(count) > self.limits.mounted_entries {
            self.tried = self.tried.saturating_add(1);
            return self.refuse(
                name.to_string(),
                format!("over the budget of entries ({count} entries)"),
            );
        }
        self.used = self.used.saturating_add(cost);
        self.entries = self.entries.saturating_add(count);
        self.tried = self.tried.saturating_add(1);
        self.report.mounted = self.report.mounted.saturating_add(1);
        self.report.skipped_entries = self
            .report
            .skipped_entries
            .saturating_add(mount.skipped().len());
        let at = self.zips.len();
        self.by_stem.insert(fold(stem_path.as_bytes()), at);
        let folder = format!("{stem_path}/").into_bytes();
        for (i, entry) in mount.entries().iter().enumerate() {
            let mut game = folder.clone();
            game.extend_from_slice(&entry.name);
            self.index.entry(fold(&game)).or_insert((at, i));
        }
        self.zips.push(Mounted {
            zip_path: zip_path.to_vec(),
            folder,
            top: name.to_ascii_uppercase().into_bytes(),
            mount,
            extra: copy,
        });
    }
}

/// Two names of one folder as the scan sorts them (upper case, byte by byte).
fn cmp_upper(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.iter()
        .map(u8::to_ascii_uppercase)
        .cmp(b.iter().map(u8::to_ascii_uppercase))
}

/// Why a start-up file `path` is not taken, if it is not: see [`GameFolder::open`]. `taken` holds the
/// folded paths accepted so far, `folders` the folded folders of the scan and of what was accepted.
fn startup_refusal(
    policy: &MountPolicy,
    path: &str,
    disk: &HashSet<Vec<u8>>,
    taken: &StartupFiles,
    folders: &HashSet<Vec<u8>>,
    bytes: usize,
    len: usize,
) -> Option<&'static str> {
    let raw = path.as_bytes();
    if raw.len() > MAX_WRITTEN_PATH || !raw.contains(&b'/') || safe_path(raw).is_none() {
        return Some("not a plain path below a folder");
    }
    if !policy.is_scanned(raw) {
        return Some("not below a folder the start-up scan goes through");
    }
    let key = fold(raw);
    if disk.contains(&key) || taken.contains_key(&key) {
        return Some("the folder has a file of that name");
    }
    if folders.contains(&key) {
        return Some("a folder has that name");
    }
    let mut end = raw.len();
    while let Some(slash) = raw
        .get(..end)
        .and_then(|p| p.iter().rposition(|&b| b == b'/'))
    {
        let below = fold(raw.get(..slash).unwrap_or_default());
        if disk.contains(&below) || taken.contains_key(&below) {
            return Some("below a file");
        }
        end = slash;
    }
    if taken.len() >= MAX_STARTUP_FILES {
        return Some("too many start-up files");
    }
    if bytes.saturating_add(len) > MAX_STARTUP_BYTES {
        return Some("over the start-up files' bytes");
    }
    None
}

/// The folder's list (the scan reversed) with the start-up files merged in where the scan would have found
/// them, the start-up files taken by their folded paths, and the folders of `folders` extended by theirs. A
/// file that is left out is refused in `mounting`'s report.
fn merge_startup(
    policy: &MountPolicy,
    base: Rc<Vec<Vec<u8>>>,
    startup: Vec<(String, Vec<u8>)>,
    folders: &mut HashSet<Vec<u8>>,
    mounting: &mut Mounting,
) -> (Rc<Vec<Vec<u8>>>, StartupFiles) {
    if startup.is_empty() {
        return (base, HashMap::new());
    }
    let on_disk: HashSet<Vec<u8>> = base.iter().map(|p| fold(p)).collect();
    let mut taken: StartupFiles = HashMap::new();
    let mut spelled: Vec<Vec<u8>> = Vec::new();
    let mut bytes = 0usize;
    for (path, contents) in startup {
        if let Some(why) = startup_refusal(
            policy,
            &path,
            &on_disk,
            &taken,
            folders,
            bytes,
            contents.len(),
        ) {
            mounting.refuse(path, why.to_string());
            continue;
        }
        bytes = bytes.saturating_add(contents.len());
        let raw = path.into_bytes();
        let mut end = raw.len();
        while let Some(slash) = raw
            .get(..end)
            .and_then(|p| p.iter().rposition(|&b| b == b'/'))
        {
            folders.insert(fold(raw.get(..slash).unwrap_or_default()));
            end = slash;
        }
        taken.insert(fold(&raw), Rc::new(contents));
        spelled.push(raw);
    }
    mounting.report.startup_files = spelled.len();
    spelled.sort_by(|a, b| policy.scan_cmp(a, b));
    // The merge walks the scan's own order, so the folder's list is turned around and back.
    let mut scan_order = base.iter().rev().peekable();
    let mut merged: Vec<Vec<u8>> = Vec::with_capacity(base.len().saturating_add(spelled.len()));
    for new in spelled {
        while let Some(next) = scan_order.peek() {
            if policy.scan_cmp(next, &new) == std::cmp::Ordering::Less {
                merged.extend(scan_order.next().cloned());
            } else {
                break;
            }
        }
        merged.push(new);
    }
    merged.extend(scan_order.cloned());
    merged.reverse();
    (Rc::new(merged), taken)
}
/// The game folder scanned and indexed once, its zips mounted, and the searches' list of both (module docs). Read-only: the
/// layer of files a host writes goes over it.
pub struct GameFolder {
    root: PathBuf,
    /// For what the index does not hold.
    dir: DirHost,
    /// The start-up scan's files (and the host's start-up files) as the searches' list has them (the scan reversed), as the
    /// disk spells them.
    listing: Rc<Vec<Vec<u8>>>,
    /// Folded path -> position in `listing`.
    disk: HashMap<Vec<u8>, usize>,
    /// Folded folders the scan went through, the empty ones too, and those of the start-up files.
    folders: HashSet<Vec<u8>>,
    /// Start-up files the host gave: folded path -> contents. They are in `listing` and `disk` as files the scan found, and
    /// read from here instead of from the folder.
    startup: StartupFiles,
    /// The scan was whole.
    indexed: bool,
    policy: MountPolicy,
    zips: Vec<Mounted>,
    /// Folded stem path -> index in `zips`.
    by_stem: HashMap<Vec<u8>, usize>,
    /// Folded game path of an entry -> (index in `zips`, index in its entries).
    index: HashMap<Vec<u8>, (usize, usize)>,
    /// The searches' list at start-up.
    list: Vec<Vec<u8>>,
    report: MountReport,
    /// [`GameFolder::wasted`].
    wasted: Cell<u64>,
    /// [`GameFolder::inflated`].
    inflated: Cell<u64>,
}

impl GameFolder {
    /// The game folder at `root` with what `policy` mounts, under `limits` (the mounted zips' caps). `archives` (`(stem, archive
    /// bytes)`) are more zips mounted as if they were `<zip folder>/<stem>.zip` in the folder: listed where such a file would be
    /// and read as that file; a stem the folder has a file of (a zip, mounted or not) or another archive has is refused.
    /// `startup` (`(path, contents)`) are presented as files the start-up scan found in the folder: listed where the scan would
    /// put them (depth first, upper-case name order, in the reversed list), read and measured as files of the disk, with a time
    /// of 0; the folder itself is never written. A start-up path is taken when it is below one of the mod paths, a plain name
    /// ([`safe_path`], at most [`MAX_WRITTEN_PATH`] bytes), not in the folder already, not given twice (ignoring case), not the
    /// name of a folder or below a file, and the caps [`MAX_STARTUP_FILES`] and [`MAX_STARTUP_BYTES`] hold; otherwise it is left
    /// out and named in [`MountReport::refused`].
    pub fn open(
        root: impl Into<PathBuf>,
        policy: MountPolicy,
        limits: Limits,
        archives: Vec<(String, Vec<u8>)>,
        startup: Vec<(String, Vec<u8>)>,
    ) -> GameFolder {
        let root: PathBuf = root.into();
        let dir = DirHost::new(root.clone()).with_rules(policy.rules);
        let mut mounting = Mounting {
            limits,
            zips: Vec::new(),
            by_stem: HashMap::new(),
            index: HashMap::new(),
            report: MountReport::default(),
            used: 0,
            entries: 0,
            tried: 0,
            read: 0,
        };
        let mut folders: HashSet<Vec<u8>> = dir.folders().iter().map(|f| fold(f)).collect();
        let (listing, startup) =
            merge_startup(&policy, dir.files(), startup, &mut folders, &mut mounting);
        let mut disk: HashMap<Vec<u8>, usize> = HashMap::with_capacity(listing.len());
        for (at, path) in listing.iter().enumerate() {
            disk.entry(fold(path)).or_insert(at);
        }
        let indexed = !dir.scan_cut_off();
        // The folder's list is its scan reversed; zips are mounted in the scan's order.
        for path in listing.iter().rev() {
            let Some((stem_path, name)) = policy.mounts(path) else {
                continue;
            };
            let (Ok(stem_path), Ok(name)) =
                (std::str::from_utf8(stem_path), std::str::from_utf8(name))
            else {
                continue;
            };
            let stem = name.get(..name.len().saturating_sub(4)).unwrap_or_default();
            if let Err(why) = mounting.admit_name(stem_path, stem) {
                mounting.refuse(name.to_string(), why);
                continue;
            }
            // The file the scan named, never anything joined from script text, and no link on the way.
            let Some((_, meta)) = plain_file(&root, path) else {
                mounting.refuse(name.to_string(), "unreadable".into());
                continue;
            };
            let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
            if let Err(why) = mounting.admit_size(len, 1) {
                mounting.refuse(name.to_string(), why);
                continue;
            }
            // Measured and refused before reading; the read stops at the same bound.
            match read_file(&root, path, mounting.room()) {
                Some(bytes) => mounting.add(path, stem_path, name, bytes, false),
                None => {
                    // What the failed read may have taken counts as read.
                    mounting.read = mounting.read.saturating_add(len);
                    mounting.charge_failed(len);
                    mounting.refuse(name.to_string(), "unreadable".into());
                }
            }
        }
        // How the folder spells the folder a host's archives go in (the first name in the list that is inside it).
        let spelled = policy.archive_folder().map(|folder| {
            listing
                .iter()
                .find_map(|p| {
                    folder_rest(folder, p)?;
                    std::str::from_utf8(p.get(..folder.len())?).ok()
                })
                .unwrap_or(policy.zip_folder_spelled)
                .to_string()
        });
        for (stem, bytes) in archives {
            let file = format!("{stem}.zip");
            let Some(spelled) = spelled.as_deref().filter(|s| !s.is_empty()) else {
                mounting.refuse(file, "no folder for a host's archives".into());
                continue;
            };
            let stem_path = format!("{spelled}/{stem}");
            let at = format!("{stem_path}.zip");
            let why = mounting
                .admit_name(&stem_path, &stem)
                .and_then(|()| mounting.admit_size(bytes.len(), 2))
                .err()
                .or_else(|| {
                    disk.contains_key(&fold(at.as_bytes()))
                        .then(|| "the folder has a file of that name".to_string())
                });
            match why {
                Some(why) => mounting.refuse(file, why),
                None => mounting.add(at.as_bytes(), &stem_path, &file, bytes, true),
            }
        }
        let list = build_list(&listing, &mounting, policy.archive_folder());
        let mut report = mounting.report;
        report.entries = mounting.entries;
        report.bytes = mounting.used;
        report.read = mounting.read;
        GameFolder {
            root,
            dir,
            listing,
            disk,
            folders,
            startup,
            indexed,
            policy,
            zips: mounting.zips,
            by_stem: mounting.by_stem,
            index: mounting.index,
            list,
            report,
            wasted: Cell::new(0),
            inflated: Cell::new(0),
        }
    }
}

/// The searches' list the folder and the zips give: the folder's list (its scan reversed) with each mounted zip's entries put
/// right before the zip file, last entry first. A file of the folder at the path of a zip entry is left out: the path is one
/// resource, listed at the entry's place. A host's archive has no file in the folder's list and goes where its file would be:
/// among the names of `archive_folder`, by its name in upper case, descending (after the list when nothing follows it).
fn build_list(
    listing: &[Vec<u8>],
    mounting: &Mounting,
    archive_folder: Option<&[u8]>,
) -> Vec<Vec<u8>> {
    let by_file: HashMap<&[u8], &Mounted> = mounting
        .zips
        .iter()
        .filter(|z| z.extra.is_none())
        .map(|z| (z.zip_path.as_slice(), z))
        .collect();
    let mut extras: Vec<&Mounted> = mounting.zips.iter().filter(|z| z.extra.is_some()).collect();
    // Ascending, so the last is the next to place.
    extras.sort_by(|a, b| a.top.cmp(&b.top));
    let entries_of = |zip: &Mounted, out: &mut Vec<Vec<u8>>| {
        for entry in zip.mount.entries().iter().rev() {
            let mut path = zip.folder.clone();
            path.extend_from_slice(&entry.name);
            out.push(path);
        }
    };
    let mut out = Vec::with_capacity(listing.len().saturating_add(mounting.entries));
    for path in listing {
        // A mounted zip has an entry at `path`: the loose file there is that entry's twin.
        if mounting.index.contains_key(&fold(path)) {
            continue;
        }
        if let Some(rest) = archive_folder.and_then(|folder| folder_rest(folder, path))
            && !extras.is_empty()
        {
            let end = rest.iter().position(|&b| b == b'/').unwrap_or(rest.len());
            let top = rest.get(..end).unwrap_or_default().to_ascii_uppercase();
            while extras.last().is_some_and(|z| top < z.top) {
                if let Some(zip) = extras.pop() {
                    entries_of(zip, &mut out);
                    out.push(zip.zip_path.clone());
                }
            }
        }
        if let Some(zip) = by_file.get(path.as_slice()) {
            entries_of(zip, &mut out);
        }
        out.push(path.clone());
    }
    while let Some(zip) = extras.pop() {
        entries_of(zip, &mut out);
        out.push(zip.zip_path.clone());
    }
    out
}

impl GameFolder {
    /// The game folder, as it was given.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// What mounts where.
    pub fn policy(&self) -> &MountPolicy {
        &self.policy
    }

    /// What mounting found.
    pub fn report(&self) -> &MountReport {
        &self.report
    }

    /// The folder's own host, for what the index does not hold (a folder outside the scan, `is_dir`, a walk of a mod path).
    pub fn dir(&self) -> &DirHost {
        &self.dir
    }

    /// The searches' list at start-up (module docs).
    pub fn list(&self) -> &[Vec<u8>] {
        &self.list
    }

    /// The zip files mounted, as the scan spells them (a host's archives as their files would be), in the order they mounted.
    pub fn mounted(&self) -> Vec<Vec<u8>> {
        self.zips.iter().map(|z| z.zip_path.clone()).collect()
    }

    /// Every byte the zips' reads inflated so far, failed ones included.
    pub fn inflated(&self) -> u64 {
        self.inflated.get()
    }

    /// The bytes the zips inflated for reads that failed: work that returned nothing.
    pub fn wasted(&self) -> u64 {
        self.wasted.get()
    }

    /// Whether a lookup of `path` asks the index only: [`MountPolicy::dictionary_only`], `path` is under a mod path and the
    /// scan was whole.
    pub fn authoritative(&self, path: &[u8]) -> bool {
        self.policy.dictionary_only && self.indexed && self.policy.is_scanned(path)
    }

    /// Whether the index has a file at `path` (any case): one the scan found or a start-up file.
    pub fn has_file(&self, path: &[u8]) -> bool {
        self.disk.contains_key(&fold(path))
    }

    /// Whether the folder `path` (any case, no trailing slash) is one the scan went through or one a start-up file implies.
    pub fn has_folder(&self, path: &[u8]) -> bool {
        self.folders.contains(&fold(path))
    }

    /// Whether the searches' list has `path` at start-up: a file the scan found, a start-up file, an entry of a zip, or a
    /// host's archive.
    pub fn listed(&self, path: &[u8]) -> bool {
        self.has_file(path) || self.entry_at(path).is_some() || self.archive(path).is_some()
    }

    /// The directory of the resource at `path` as the resource manager stores it (TGE's `ResourceObject::path`): for a zip's
    /// entry the zip's path without `.zip` as the folder spells it, `/`, then the entry's directory as the archive stores it;
    /// for a file of the disk the start-up scan's own spelling. None for anything else (a file the scan did not index).
    pub fn stored_dir(&self, path: &[u8]) -> Option<Vec<u8>> {
        if let Some((zip, entry)) = self.entry_at(path) {
            let name = entry.name.as_slice();
            let dir = match name.iter().rposition(|&b| b == b'/') {
                Some(at) => name.get(..at)?,
                None => &[],
            };
            let mut real = zip.folder.clone();
            if dir.is_empty() {
                real.pop();
            } else {
                real.extend_from_slice(dir);
            }
            return Some(real);
        }
        let listed = self
            .listing
            .get(*self.disk.get(&fold(&collapse_dots(path)))?)?;
        let at = listed.iter().rposition(|&b| b == b'/')?;
        listed.get(..at).map(<[u8]>::to_vec)
    }

    /// The mounted zip and its entry at game path `path` (any case, as written).
    fn entry_at(&self, path: &[u8]) -> Option<(&Mounted, &ZipEntry)> {
        let &(z, e) = self.index.get(&fold(path))?;
        let zip = self.zips.get(z)?;
        Some((zip, zip.mount.entries().get(e)?))
    }

    /// The zip entry at game path `path` (any case, as written), if a mounted zip has one.
    pub fn entry(&self, path: &[u8]) -> Option<&ZipEntry> {
        self.entry_at(path).map(|(_, entry)| entry)
    }

    /// A read of the zip entry at `path`, if a mounted zip has one: its bytes, or missing when it cannot be inflated (counted
    /// in [`GameFolder::wasted`]).
    pub fn read_entry(&self, path: &[u8]) -> Option<FileRead> {
        let (zip, entry) = self.entry_at(path)?;
        let (wasted, inflated) = (zip.mount.wasted(), zip.mount.inflated());
        let read = zip.mount.read(&entry.name);
        self.wasted.set(
            self.wasted
                .get()
                .saturating_add(zip.mount.wasted().saturating_sub(wasted)),
        );
        self.inflated.set(
            self.inflated
                .get()
                .saturating_add(zip.mount.inflated().saturating_sub(inflated)),
        );
        Some(match read {
            Some(bytes) => FileRead::Found(bytes),
            None => FileRead::Missing,
        })
    }

    /// The bytes of the host's archive whose file `path` names (`<zip folder>/<stem>.zip`), if it is one.
    pub fn archive(&self, path: &[u8]) -> Option<&Rc<Vec<u8>>> {
        let stem = zip_stem(path)?;
        let zip = self.zips.get(*self.by_stem.get(&fold(stem))?)?;
        zip.extra
            .as_ref()
            .filter(|_| zip.zip_path.eq_ignore_ascii_case(path))
    }

    /// The start-up file the host gave at `path`, if any.
    fn startup_file(&self, path: &[u8]) -> Option<&Rc<Vec<u8>>> {
        if self.startup.is_empty() {
            return None;
        }
        self.startup.get(&fold(path))
    }

    /// The disk file the index names for `path`: None when it names none, `Some(None)` when the name it has is no longer a
    /// plain file reached without a link ([`plain_file`]).
    fn indexed_file(&self, path: &[u8]) -> Option<Option<(PathBuf, std::fs::Metadata)>> {
        let at = *self.disk.get(&fold(path))?;
        Some(plain_file(&self.root, self.listing.get(at)?))
    }

    /// The length of the file of the disk at `path` (a start-up file's included).
    pub fn disk_stat(&self, path: &[u8]) -> Option<u64> {
        if let Some(bytes) = self.startup_file(path) {
            return Some(bytes.len() as u64);
        }
        if let Some(file) = self.indexed_file(path) {
            return file.map(|(_, meta)| meta.len());
        }
        if self.authoritative(path) {
            return None;
        }
        match self.dir.stat(path) {
            FileStat::Found { len } => Some(len),
            _ => None,
        }
    }

    /// The whole file of the disk at `path` (not one over [`MAX_READ_BYTES`]).
    pub fn disk_read(&self, path: &[u8]) -> Option<Vec<u8>> {
        if let Some(bytes) = self.startup_file(path) {
            return (bytes.len() as u64 <= MAX_READ_BYTES).then(|| bytes.as_ref().clone());
        }
        if let Some(&at) = self.disk.get(&fold(path)) {
            return read_file(&self.root, self.listing.get(at)?, MAX_READ_BYTES);
        }
        if self.authoritative(path) {
            return None;
        }
        match self.dir.read_at_most(path, MAX_READ_BYTES) {
            FileRead::Found(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The time of the file of the disk at `path` (nanoseconds since the Unix epoch); a start-up file's is 0.
    pub fn disk_modified(&self, path: &[u8]) -> Option<u64> {
        if self.startup_file(path).is_some() {
            return Some(0);
        }
        if let Some(file) = self.indexed_file(path) {
            return nanos(file?.1.modified().ok()?);
        }
        if self.authoritative(path) {
            return None;
        }
        self.dir.modified(path)
    }

    /// What the zips or the folder have at `path`, as a stat: a zip's entry first, then a host's archive, then the disk. The
    /// files of the disk are found by the name the OS resolves (`.` and empty parts dropped); a zip's entry by the name as
    /// written.
    pub fn stat(&self, path: &[u8]) -> FileStat {
        if let Some(entry) = self.entry(path) {
            return FileStat::Found { len: entry.size };
        }
        if let Some(bytes) = self.archive(path) {
            return FileStat::Found {
                len: bytes.len() as u64,
            };
        }
        match self.disk_stat(&collapse_dots(path)) {
            Some(len) => FileStat::Found { len },
            None => FileStat::Missing,
        }
    }

    /// The same, as a read. A zip's entry that cannot be inflated leaves the loose file at its path, if there is one.
    pub fn read(&self, path: &[u8]) -> FileRead {
        if let Some(FileRead::Found(bytes)) = self.read_entry(path) {
            return FileRead::Found(bytes);
        }
        if let Some(bytes) = self.archive(path) {
            return FileRead::Found(bytes.as_ref().clone());
        }
        match self.disk_read(&collapse_dots(path)) {
            Some(bytes) => FileRead::Found(bytes),
            None => FileRead::Missing,
        }
    }

    /// The time of what the zips or the folder have at `path`: a zipped file has its zip's, a host's archive and its entries
    /// have none.
    pub fn modified(&self, path: &[u8]) -> Option<u64> {
        if let Some((zip, _)) = self.entry_at(path) {
            return if zip.extra.is_some() {
                None
            } else {
                self.disk_modified(&zip.zip_path)
            };
        }
        if self.archive(path).is_some() {
            return None;
        }
        self.disk_modified(&collapse_dots(path))
    }
}

#[cfg(test)]
#[path = "folder_tests.rs"]
mod tests;
