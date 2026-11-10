//! Finding the folders and zips of a root and reading each into a [`ModPackage`], under the limits. This is below every
//! layout: a layout names the roots and recognizes mods in the packages, it never reads a file itself.

use std::collections::BTreeMap;

use torque_resource::zips::entry_name_ok;
use torque_ts_vm::host::{FileHost, FileRead, FileStat};
use torque_ts_vm::text::fold;

use crate::layout::{CandidateKind, ModNames, ModRoot, NameInfo, RootPlace};
use crate::package::{ModError, ModLimits, ModPackage, check_listing};

/// What the mods found so far may still hold.
pub(crate) struct Budget {
    pub(crate) mods: usize,
    pub(crate) bytes: u64,
}

impl Budget {
    pub(crate) fn new(limits: &ModLimits) -> Budget {
        Budget {
            mods: limits.mods,
            bytes: limits.total_bytes,
        }
    }
}

/// A folder or zip of a root, read.
pub(crate) struct Found {
    pub(crate) raw: String,
    pub(crate) kind: CandidateKind,
    pub(crate) name: NameInfo,
    pub(crate) source: String,
    pub(crate) package: Result<ModPackage, ModError>,
}

/// What a root's scan has to say besides what it found.
#[derive(Default)]
pub(crate) struct Notes {
    pub(crate) warnings: Vec<String>,
}

/// The warnings one scan keeps; past them one line says more were left out.
pub(crate) const MAX_WARNINGS: usize = 64;

impl Notes {
    /// Keeps `line`, unless [`MAX_WARNINGS`] are kept already: then one last line says the rest were left out.
    pub(crate) fn warn(&mut self, line: String) {
        match self.warnings.len() {
            n if n < MAX_WARNINGS => self.warnings.push(line),
            MAX_WARNINGS => self
                .warnings
                .push("more warnings about the mods folder were left out".into()),
            _ => {}
        }
    }
}

/// A candidate before it is read: where its files are.
pub(crate) struct Pending {
    pub(crate) raw: String,
    pub(crate) kind: CandidateKind,
    /// For a folder: `(path inside the folder, size, where to read it, identity of the file on the disk)`; for a zip: one entry, the zip itself.
    pub(crate) files: Vec<(String, u64, String, u64)>,
    /// Whether "where to read it" is a path on the disk (else a path in the game's files).
    pub(crate) disk: bool,
}

pub(crate) fn scan(
    root: &ModRoot,
    host: &dyn FileHost,
    names: &dyn ModNames,
    limits: &ModLimits,
    budget: &mut Budget,
    notes: &mut Notes,
) -> Vec<Found> {
    let (label, pending) = match &root.place {
        RootPlace::Files(path) => match list_files_root(path, host, limits, notes) {
            Some(listing) => (path.clone(), listing),
            None => return Vec::new(),
        },
        #[cfg(not(target_family = "wasm"))]
        RootPlace::Disk(path) => (
            path.display().to_string(),
            crate::disk::list_root(path, limits, notes),
        ),
        #[cfg(target_family = "wasm")]
        RootPlace::Disk(_) => {
            notes
                .warnings
                .push("a mods folder on the disk is not available on the web".into());
            return Vec::new();
        }
    };
    let mut found = Vec::new();
    for pending in pending {
        match pending.kind {
            CandidateKind::Folder if !root.folders => continue,
            CandidateKind::Zip if !root.zips => continue,
            _ => {}
        }
        let stem = match pending.kind {
            CandidateKind::Zip => pending
                .raw
                .get(..pending.raw.len().saturating_sub(4))
                .unwrap_or(&pending.raw),
            CandidateKind::Folder => &pending.raw,
        };
        let Some(name) = names.map(stem, pending.kind) else {
            continue;
        };
        if budget.mods == 0 {
            notes.warnings.push(format!(
                "{label}: more than {} mods; the rest are left out",
                limits.mods
            ));
            break;
        }
        budget.mods -= 1;
        let source = format!("{label}/{}", pending.raw);
        let package = read_candidate(&pending, host, limits, budget);
        found.push(Found {
            raw: pending.raw,
            kind: pending.kind,
            name,
            source,
            package,
        });
    }
    found
}

fn read_candidate(
    pending: &Pending,
    host: &dyn FileHost,
    limits: &ModLimits,
    budget: &mut Budget,
) -> Result<ModPackage, ModError> {
    let cap = limits.file_bytes.max(limits.zip_bytes as u64);
    let read = |from: &str, size: u64, id: u64| -> Option<Vec<u8>> {
        if pending.disk {
            #[cfg(not(target_family = "wasm"))]
            return crate::disk::read_file(from, cap, size, id);
            #[cfg(target_family = "wasm")]
            return None;
        }
        let _ = (cap, size, id);
        read_one(host, from)
    };
    let package = match pending.kind {
        CandidateKind::Zip => {
            let Some((_, size, from, id)) = pending.files.first() else {
                return Err(ModError::Archive("empty".into()));
            };
            if *size > limits.zip_bytes as u64 {
                return Err(ModError::ZipTooLarge {
                    bytes: usize::try_from(*size).unwrap_or(usize::MAX),
                    max: limits.zip_bytes,
                });
            }
            if *size > budget.bytes {
                return Err(ModError::TooManyBytes {
                    bytes: *size,
                    max: budget.bytes,
                });
            }
            let bytes = read(from, *size, *id)
                .ok_or_else(|| ModError::Archive("it could not be read".into()))?;
            ModPackage::from_zip(bytes, limits)?
        }
        CandidateKind::Folder => {
            let listing: Vec<(String, u64)> = pending
                .files
                .iter()
                .map(|(path, size, _, _)| (path.clone(), *size))
                .collect();
            let total = check_listing(&listing, limits)?;
            if total > budget.bytes {
                return Err(ModError::TooManyBytes {
                    bytes: total,
                    max: budget.bytes,
                });
            }
            let mut files = Vec::with_capacity(pending.files.len());
            for (path, size, from, id) in &pending.files {
                let bytes = read(from, *size, *id).ok_or_else(|| ModError::Entry {
                    path: path.chars().take(80).collect(),
                    why: "could not be read",
                })?;
                files.push((path.clone(), bytes));
            }
            ModPackage::from_files(files, limits)?
        }
    };
    if package.held_bytes() > budget.bytes {
        return Err(ModError::TooManyBytes {
            bytes: package.held_bytes(),
            max: budget.bytes,
        });
    }
    budget.bytes -= package.held_bytes();
    Ok(package)
}

fn read_one(host: &dyn FileHost, path: &str) -> Option<Vec<u8>> {
    match host.read(path.as_bytes()) {
        FileRead::Found(bytes) => Some(bytes),
        _ => None,
    }
}

/// The folders and zips directly in the folder `root` of the game's files.
fn list_files_root(
    root: &str,
    host: &dyn FileHost,
    limits: &ModLimits,
    notes: &mut Notes,
) -> Option<Vec<Pending>> {
    let root = root.replace('\\', "/");
    let root = root.trim_matches('/');
    if root.is_empty() || !entry_name_ok(root.as_bytes()) {
        notes.warnings.push(format!(
            "the mods folder `{}` is not a path inside the game's files",
            root.chars().take(80).collect::<String>()
        ));
        return None;
    }
    let prefix = fold(format!("{root}/").as_bytes());
    let listing = host.files();
    // Folded child name -> (spelled name, kind, its files).
    let mut children: BTreeMap<Vec<u8>, Pending> = BTreeMap::new();
    let mut seen = 0usize;
    for path in listing.iter() {
        if !fold(path).starts_with(&prefix) {
            continue;
        }
        let Ok(path) = std::str::from_utf8(path) else {
            continue;
        };
        let Some(rest) = path.get(prefix.len()..) else {
            continue;
        };
        let (child, tail) = match rest.split_once('/') {
            Some((child, tail)) => (child, Some(tail)),
            None => (rest, None),
        };
        if child.is_empty() {
            continue;
        }
        let kind = match tail {
            Some(_) => CandidateKind::Folder,
            None if child.len() > 4 && child.to_ascii_lowercase().ends_with(".zip") => {
                CandidateKind::Zip
            }
            None => continue,
        };
        // The listing is the game's whole file list: bound what is walked for the mods.
        seen += 1;
        if seen > limits.files.saturating_mul(limits.mods.max(1)) {
            notes.warnings.push(format!(
                "{root}: more files than the mods' limits allow; the rest are not looked at"
            ));
            break;
        }
        let FileStat::Found { len } = host.stat(path.as_bytes()) else {
            continue;
        };
        let entry = children
            .entry(fold(child.as_bytes()))
            .or_insert_with(|| Pending {
                raw: child.to_string(),
                kind,
                files: Vec::new(),
                disk: false,
            });
        if entry.kind != kind {
            continue;
        }
        match tail {
            Some(tail) => entry
                .files
                .push((tail.to_string(), len, path.to_string(), 0)),
            None => entry.files.push((String::new(), len, path.to_string(), 0)),
        }
    }
    Some(children.into_values().collect())
}
