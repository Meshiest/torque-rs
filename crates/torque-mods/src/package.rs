//! A mod's files, held and checked: [`ModPackage`] (a folder's files or a zip's entries), [`ModLimits`] and why a mod is
//! refused ([`ModError`]).
//!
//! Every mod, whatever layout found it, becomes a package before any layout code sees it, so the caps and the path rules
//! hold for every layout: the number of files, the size of a file and of all the mods together, the depth of a path,
//! the compression ratio of a zip, and paths a host may name. A zip is read by `torque_resource::zips::ZipMount`, which
//! trusts nothing in the archive (its directory is bounded, an entry inflates at most its declared size and is checked
//! against its CRC), and nothing here panics on any bytes.

use std::collections::{HashMap, HashSet};
use std::fmt;

use torque_resource::zips::{
    MAX_ENTRY_BYTES, MAX_ZIP_BYTES, MountError, Skip, ZipMount, entry_name_ok,
};
use torque_ts_vm::text::fold;

/// The caps on what mods may hold ([`ModsPlugin::limits`](crate::ModsPlugin::limits)). A mod over one is refused, with the
/// reason, and the others load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ModLimits {
    /// The most mods loaded together; folders and zips found beyond it are left out.
    pub mods: usize,
    /// The most bytes all the mods hold in memory together: a folder's files count as they are, a zip counts its own size.
    /// A mod's unpacked size may not pass it either.
    pub total_bytes: u64,
    /// The most files in one mod.
    pub files: usize,
    /// The largest file of a mod, unpacked (at most the zip reader's own cap of 64 MiB).
    pub file_bytes: u64,
    /// The deepest path, in folders and the file's name together.
    pub depth: usize,
    /// The most a zip may unpack to for each byte it takes (checked once it unpacks to more than [`RATIO_FLOOR`] bytes).
    pub ratio: u64,
    /// The largest zip file (at most the zip reader's own cap of 64 MiB).
    pub zip_bytes: usize,
}

/// A zip that unpacks to no more than this is not held to [`ModLimits::ratio`].
pub const RATIO_FLOOR: u64 = 4 << 20;

impl Default for ModLimits {
    fn default() -> ModLimits {
        ModLimits {
            mods: 64,
            total_bytes: 512 << 20,
            files: 20_000,
            file_bytes: 32 << 20,
            depth: 16,
            ratio: 100,
            zip_bytes: MAX_ZIP_BYTES,
        }
    }
}

/// The longest path of a mod's file (bytes).
pub const MAX_PATH_LEN: usize = 240;

/// Why a mod is refused. Its text says what and where, and is one line.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ModError {
    /// The archive is over [`ModLimits::zip_bytes`].
    ZipTooLarge {
        /// Its length.
        bytes: usize,
        /// The cap.
        max: usize,
    },
    /// The reader refused the archive (not a zip, damaged, too many entries or end records, a directory too large).
    Archive(String),
    /// A file's name is one a host may not serve.
    BadPath {
        /// The name, cleaned for a message.
        path: String,
        /// Which rule it breaks.
        why: &'static str,
    },
    /// Two files whose names differ only in case.
    Collision {
        /// One of the two.
        path: String,
    },
    /// A path with more parts than [`ModLimits::depth`].
    TooDeep {
        /// The path.
        path: String,
        /// The cap.
        max: usize,
    },
    /// A file over [`ModLimits::file_bytes`].
    FileTooLarge {
        /// The path.
        path: String,
        /// Its size.
        bytes: u64,
        /// The cap.
        max: u64,
    },
    /// More files than [`ModLimits::files`].
    TooManyFiles {
        /// How many.
        files: usize,
        /// The cap.
        max: usize,
    },
    /// More bytes than [`ModLimits::total_bytes`] (this mod's, or all the mods').
    TooManyBytes {
        /// How many.
        bytes: u64,
        /// The cap.
        max: u64,
    },
    /// A zip that unpacks to far more than it takes.
    Ratio {
        /// What it unpacks to.
        unpacked: u64,
        /// The archive's length.
        packed: u64,
        /// The cap.
        max: u64,
    },
    /// An entry the reader cannot read: an unsupported compression method, encryption, a damaged header.
    Entry {
        /// The entry.
        path: String,
        /// What is wrong.
        why: &'static str,
    },
}

impl fmt::Display for ModError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModError::ZipTooLarge { bytes, max } => {
                write!(f, "the zip is {bytes} bytes; at most {max} are read")
            }
            ModError::Archive(why) => write!(f, "not a zip the reader accepts: {why}"),
            ModError::BadPath { path, why } => write!(f, "the path `{path}` is refused: {why}"),
            ModError::Collision { path } => {
                write!(f, "two files differ only in case (`{path}`)")
            }
            ModError::TooDeep { path, max } => {
                write!(f, "the path `{path}` is more than {max} folders deep")
            }
            ModError::FileTooLarge { path, bytes, max } => {
                write!(
                    f,
                    "`{path}` is {bytes} bytes; a file may have at most {max}"
                )
            }
            ModError::TooManyFiles { files, max } => {
                write!(f, "{files} files; a mod may have at most {max}")
            }
            ModError::TooManyBytes { bytes, max } => {
                write!(f, "{bytes} bytes; the mods may hold at most {max} together")
            }
            ModError::Ratio {
                unpacked,
                packed,
                max,
            } => write!(
                f,
                "the zip unpacks to {unpacked} bytes from {packed} (more than {max} to 1)"
            ),
            ModError::Entry { path, why } => write!(f, "the entry `{path}` {why}"),
        }
    }
}

impl std::error::Error for ModError {}

/// A path as it is shown in a message: cut short, with no control characters.
fn shown(path: &[u8]) -> String {
    String::from_utf8_lossy(path)
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect()
}

/// One check of a path a mod's file has, shared by folders and zips.
fn check_path(name: &str, limits: &ModLimits) -> Result<(), ModError> {
    let bad = |why| {
        Err(ModError::BadPath {
            path: shown(name.as_bytes()),
            why,
        })
    };
    if name.len() > MAX_PATH_LEN {
        return bad("longer than 240 bytes");
    }
    if name.contains('\\') {
        return bad("it has a backslash");
    }
    if !entry_name_ok(name.as_bytes()) {
        return bad(
            "it is empty, absolute, has `..`, a drive letter, a control character or NUL, or a name Windows reserves",
        );
    }
    if name.split('/').count() > limits.depth {
        return Err(ModError::TooDeep {
            path: shown(name.as_bytes()),
            max: limits.depth,
        });
    }
    Ok(())
}

/// Whether the file is a compiled script: never read from a mod (ship the source).
fn is_compiled(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".dso")
}

/// Checks a listing of `(path, size)` before any file is read: path rules, collisions, sizes and counts. Used for folders by
/// the discovery (so an oversized file is never read) and again when the package is made.
pub(crate) fn check_listing(files: &[(String, u64)], limits: &ModLimits) -> Result<u64, ModError> {
    let counted = files.iter().filter(|(name, _)| !is_compiled(name)).count();
    if counted > limits.files {
        return Err(ModError::TooManyFiles {
            files: counted,
            max: limits.files,
        });
    }
    let mut seen: HashSet<Vec<u8>> = HashSet::with_capacity(counted);
    let mut total: u64 = 0;
    for (name, size) in files {
        check_path(name, limits)?;
        if is_compiled(name) {
            continue;
        }
        if !seen.insert(fold(name.as_bytes())) {
            return Err(ModError::Collision {
                path: shown(name.as_bytes()),
            });
        }
        if *size > limits.file_bytes {
            return Err(ModError::FileTooLarge {
                path: shown(name.as_bytes()),
                bytes: *size,
                max: limits.file_bytes,
            });
        }
        total = total.saturating_add(*size);
        if total > limits.total_bytes {
            return Err(ModError::TooManyBytes {
                bytes: total,
                max: limits.total_bytes,
            });
        }
    }
    Ok(total)
}

enum Backing {
    /// A zip's entries: the entry's full name in the archive, for each file.
    Zip {
        mount: Box<ZipMount>,
        entry: Vec<Vec<u8>>,
    },
    /// A folder's files read into memory.
    Memory(Vec<Vec<u8>>),
}

/// A mod's files: a folder's or a zip's, with the caps and path rules already applied. A path is relative to the mod,
/// `/`-separated, matched without regard to case.
pub struct ModPackage {
    names: Vec<String>,
    sizes: Vec<u64>,
    index: HashMap<Vec<u8>, usize>,
    backing: Backing,
    held: u64,
    unpacked: u64,
    ignored: Vec<String>,
}

impl ModPackage {
    /// The package of a zip's bytes. A zip whose files all lie in one top folder (a folder zipped as it is) is read as that
    /// folder's contents.
    pub fn from_zip(bytes: Vec<u8>, limits: &ModLimits) -> Result<ModPackage, ModError> {
        if bytes.len() > limits.zip_bytes.min(MAX_ZIP_BYTES) {
            return Err(ModError::ZipTooLarge {
                bytes: bytes.len(),
                max: limits.zip_bytes.min(MAX_ZIP_BYTES),
            });
        }
        let packed = bytes.len() as u64;
        let mount = ZipMount::open(bytes).map_err(|e| {
            ModError::Archive(match e {
                MountError::TooLarge(n) => format!("{n} bytes is too large"),
                MountError::TooManyEntries(n) => format!("it claims {n} entries"),
                MountError::TooManyEndRecords(n) => format!("it has {n} end records"),
                MountError::DirectoryTooLarge(n) => format!("its directory spans {n} bytes"),
                MountError::NotAZip(why) => shown(why.as_bytes()),
            })
        })?;
        for (name, skip) in mount.skipped() {
            let path = shown(name);
            match skip {
                Skip::Directory => {}
                Skip::BadName => {
                    return Err(ModError::BadPath {
                        path,
                        why: "it is empty, absolute, has `..`, a drive letter, a control character or NUL, or a name Windows reserves",
                    });
                }
                Skip::Duplicate => return Err(ModError::Collision { path }),
                Skip::TooLarge => {
                    return Err(ModError::FileTooLarge {
                        path,
                        bytes: MAX_ENTRY_BYTES + 1,
                        max: limits.file_bytes,
                    });
                }
                Skip::Unsupported => {
                    return Err(ModError::Entry {
                        path,
                        why: "uses a compression method or encryption that is not read (store or deflate it)",
                    });
                }
                Skip::Unreadable => {
                    return Err(ModError::Entry {
                        path,
                        why: "has a damaged header",
                    });
                }
            }
        }
        let mut listed: Vec<(String, u64, Vec<u8>)> = Vec::new();
        for entry in mount.entries() {
            if entry.folder {
                continue;
            }
            let Ok(name) = String::from_utf8(entry.name.clone()) else {
                return Err(ModError::BadPath {
                    path: shown(&entry.name),
                    why: "it is not UTF-8 text",
                });
            };
            listed.push((name, entry.size, entry.name.clone()));
        }
        let unpacked: u64 = listed.iter().map(|(_, size, _)| *size).sum();
        if unpacked > RATIO_FLOOR && unpacked / packed.max(1) > limits.ratio {
            return Err(ModError::Ratio {
                unpacked,
                packed,
                max: limits.ratio,
            });
        }
        let strip = common_top_folder(listed.iter().map(|(name, ..)| name.as_str()));
        let mut files: Vec<(String, u64)> = Vec::with_capacity(listed.len());
        let mut entry = Vec::with_capacity(listed.len());
        for (name, size, full) in listed {
            let kept = name[strip..].to_string();
            files.push((kept, size));
            entry.push(full);
        }
        let unpacked_total = check_listing(&files, limits)?;
        let kept: Vec<Vec<u8>> = files
            .iter()
            .zip(entry)
            .filter(|((name, _), _)| !is_compiled(name))
            .map(|(_, full)| full)
            .collect();
        let mut package = ModPackage::index(files, packed, limits)?;
        package.unpacked = unpacked_total;
        package.backing = Backing::Zip {
            mount: Box::new(mount),
            entry: kept,
        };
        Ok(package)
    }

    /// The package of a folder's files, `(path, contents)`.
    pub fn from_files(
        files: Vec<(String, Vec<u8>)>,
        limits: &ModLimits,
    ) -> Result<ModPackage, ModError> {
        let listing: Vec<(String, u64)> = files
            .iter()
            .map(|(name, bytes)| (name.clone(), bytes.len() as u64))
            .collect();
        let unpacked = check_listing(&listing, limits)?;
        let mut package = ModPackage::index(listing, unpacked, limits)?;
        let mut contents: Vec<Vec<u8>> = vec![Vec::new(); package.names.len()];
        for (name, bytes) in files {
            if is_compiled(&name) {
                continue;
            }
            if let Some(&slot) = package.index.get(&fold(name.as_bytes()))
                && let Some(place) = contents.get_mut(slot)
            {
                *place = bytes;
            }
        }
        package.unpacked = unpacked;
        package.held = unpacked;
        package.backing = Backing::Memory(contents);
        Ok(package)
    }

    /// The index of `files` (compiled scripts go to [`ignored`](Self::ignored)).
    fn index(
        files: Vec<(String, u64)>,
        held: u64,
        limits: &ModLimits,
    ) -> Result<ModPackage, ModError> {
        if held > limits.total_bytes {
            return Err(ModError::TooManyBytes {
                bytes: held,
                max: limits.total_bytes,
            });
        }
        let mut names = Vec::new();
        let mut sizes = Vec::new();
        let mut ignored = Vec::new();
        let mut index = HashMap::new();
        for (name, size) in files {
            if is_compiled(&name) {
                ignored.push(name);
                continue;
            }
            index.insert(fold(name.as_bytes()), names.len());
            names.push(name);
            sizes.push(size);
        }
        Ok(ModPackage {
            names,
            sizes,
            index,
            backing: Backing::Memory(Vec::new()),
            held,
            unpacked: 0,
            ignored,
        })
    }

    /// The files' paths, in the order of the archive or the folder listing.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// How many files the package has.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the package has no files.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Whether the package has the file `path` (any case).
    pub fn contains(&self, path: &str) -> bool {
        self.position(path.as_bytes()).is_some()
    }

    /// The file's size.
    pub fn size_of(&self, path: &str) -> Option<u64> {
        self.sizes.get(self.position(path.as_bytes())?).copied()
    }

    /// The file's path as the package spells it.
    pub fn spelled(&self, path: &str) -> Option<&str> {
        self.names
            .get(self.position(path.as_bytes())?)
            .map(String::as_str)
    }

    /// The contents of the file `path` (any case); None when there is none, or when a zip entry does not unpack to what its
    /// header says.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_bytes(path.as_bytes())
    }

    pub(crate) fn read_bytes(&self, path: &[u8]) -> Option<Vec<u8>> {
        let slot = self.position(path)?;
        match &self.backing {
            Backing::Memory(files) => files.get(slot).cloned(),
            Backing::Zip { mount, entry } => mount.read(entry.get(slot)?),
        }
    }

    pub(crate) fn position(&self, path: &[u8]) -> Option<usize> {
        self.index.get(&fold(path)).copied()
    }

    /// Whether the files come from a zip.
    pub fn is_zip(&self) -> bool {
        matches!(self.backing, Backing::Zip { .. })
    }

    /// The bytes the package holds in memory (a zip: its own size).
    pub fn held_bytes(&self) -> u64 {
        self.held
    }

    /// The bytes of all the files, unpacked.
    pub fn unpacked_bytes(&self) -> u64 {
        self.unpacked
    }

    /// The compiled scripts left out: nothing is read from a mod but source.
    pub fn ignored(&self) -> &[String] {
        &self.ignored
    }

    /// Bytes zip entries inflated for reads that failed (work that returned nothing).
    pub(crate) fn wasted(&self) -> u64 {
        match &self.backing {
            Backing::Zip { mount, .. } => mount.wasted(),
            Backing::Memory(_) => 0,
        }
    }
}

/// The length (with its `/`) of the one top folder every path of `names` is in, or 0.
fn common_top_folder<'a>(names: impl Iterator<Item = &'a str>) -> usize {
    let mut top: Option<&str> = None;
    for name in names {
        let Some((first, _)) = name.split_once('/') else {
            return 0;
        };
        match top {
            None => top = Some(first),
            Some(seen) if seen.eq_ignore_ascii_case(first) && seen == first => {}
            Some(_) => return 0,
        }
    }
    top.map_or(0, |folder| folder.len() + 1)
}
