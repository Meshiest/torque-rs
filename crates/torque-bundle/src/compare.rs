//! Two bundles of one game, built from two checkouts in different folders, compared: their archives byte for byte, and when they
//! differ, the files whose manifest entries differ.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use torque_bytes::hex;
use torque_resource::plain::read_plain;

use crate::archive::Format;
use crate::manifest::{self, Entry};
use crate::validate::Limits;

/// The bundle folder `path` names: itself when it holds a manifest, else the one folder of `path` that does (an `--out` folder).
fn bundle_folder(path: &Path) -> Result<PathBuf, String> {
    if path.join(manifest::FILE).is_file() {
        return Ok(path.to_path_buf());
    }
    let mut found = std::fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|dir| dir.join(manifest::FILE).is_file());
    match (found.next(), found.next()) {
        (Some(only), None) => Ok(only),
        (None, _) => Err(format!("{}: no bundle in it", path.display())),
        (Some(_), Some(_)) => Err(format!(
            "{}: more than one bundle in it; name one",
            path.display()
        )),
    }
}

/// The archive that sits beside the bundle folder `dir` (`<dir>.zip` or `<dir>.tar.gz`), the one the other folder has too.
fn archive_beside(dir: &Path) -> Result<PathBuf, String> {
    let name = dir
        .file_name()
        .ok_or_else(|| format!("{}: not a bundle folder", dir.display()))?;
    [Format::Zip, Format::TarGz]
        .into_iter()
        .map(|format| {
            let mut file = name.to_os_string();
            file.push(format!(".{}", format.extension()));
            dir.with_file_name(file)
        })
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "{}: no archive beside it (bundle with --archive)",
                dir.display()
            )
        })
}

fn sha256_of(path: &Path) -> Result<(String, usize), String> {
    let bytes = read_plain(path, Limits::default().total_bytes, None)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((hex(&Sha256::digest(&bytes)), bytes.len()))
}

/// The most a manifest is read as: a bundle's is a few hundred KiB.
const MANIFEST_MAX_BYTES: u64 = 64 << 20;

/// The entries of the manifest of the bundle folder `dir`, by path, read without following a link and within a bound.
fn entries(dir: &Path) -> Result<HashMap<String, Entry>, String> {
    let at = dir.join(manifest::FILE);
    let bytes = manifest::read_below(dir, manifest::FILE, MANIFEST_MAX_BYTES)
        .map_err(|e| format!("{}: {e}", at.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8", at.display()))?;
    Ok(manifest::parse(&text)?
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect())
}

/// The paths whose size or hash differs between two bundle folders' manifests, or that only one has, sorted.
///
/// # Errors
/// When a manifest cannot be read.
pub fn differing_files(one: &Path, two: &Path) -> Result<Vec<String>, String> {
    let (a, b) = (entries(one)?, entries(two)?);
    let mut paths: Vec<String> = a
        .keys()
        .chain(b.keys())
        .filter(|path| a.get(*path) != b.get(*path))
        .cloned()
        .collect();
    paths.sort_unstable();
    paths.dedup();
    Ok(paths)
}

/// What comparing two bundles found.
#[derive(Debug)]
pub struct Comparison {
    /// The SHA-256 and size of each archive.
    pub archives: [(String, usize); 2],
    /// The files whose manifest entries differ; empty when the archives are equal.
    pub differing: Vec<String>,
}

impl Comparison {
    pub fn identical(&self) -> bool {
        self.archives[0] == self.archives[1]
    }
}

/// Compares the archives beside the bundle folders `one` and `two` (each a bundle folder, or an `--out` folder holding one). When they differ, `differing` names the files whose
/// manifest entries do; a difference outside every file (the archive's own headers) shows as an empty list.
pub fn compare(one: &Path, two: &Path) -> Result<Comparison, String> {
    let (one, two) = (&bundle_folder(one)?, &bundle_folder(two)?);
    let a = sha256_of(&archive_beside(one)?)?;
    let b = sha256_of(&archive_beside(two)?)?;
    let differing = if a == b {
        Vec::new()
    } else {
        differing_files(one, two)?
    };
    Ok(Comparison {
        archives: [a, b],
        differing,
    })
}
