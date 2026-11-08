//! Mods on the player's disk (not on the web): the folders and zips of a root read with `std::fs`, never following a link.
//!
//! A symbolic link (or a Windows junction) inside a root or a mod is skipped and reported, so a mod cannot point the loader
//! at a file outside itself. Every file is opened and checked to be a regular file before it is read, and read at most its
//! cap plus one byte.

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::discover::{Notes, Pending};
use crate::layout::CandidateKind;
use crate::package::ModLimits;

/// Whether the entry is neither a link nor anything but a plain file or folder.
fn kind_of(meta: &fs::Metadata) -> Option<bool> {
    let kind = meta.file_type();
    if kind.is_symlink() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // A junction or any other reparse point.
        if meta.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    if kind.is_dir() {
        Some(true)
    } else if kind.is_file() {
        Some(false)
    } else {
        None
    }
}

fn name_of(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(str::to_string)
}

/// The folders and zips directly in `root`, sorted by name, and for each folder its files (sizes from the disk).
pub(crate) fn list_root(root: &Path, limits: &ModLimits, notes: &mut Notes) -> Vec<Pending> {
    let Ok(read) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    let mut out = Vec::new();
    for path in entries {
        let Some(name) = name_of(&path) else {
            notes
                .warnings
                .push("a name in the mods folder is not UTF-8 text; skipped".into());
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        match kind_of(&meta) {
            None => notes
                .warnings
                .push(format!("{name}: links are not followed; skipped")),
            Some(true) => {
                let mut files = Vec::new();
                let mut over = false;
                walk(&path, "", 0, limits, &mut files, &mut over, notes);
                if over {
                    // A listing past the cap stays over it: the package check refuses it with the count.
                    files.truncate(limits.files.saturating_add(1));
                }
                out.push(Pending {
                    raw: name,
                    kind: CandidateKind::Folder,
                    files,
                    disk: true,
                });
            }
            Some(false) => {
                if name.len() > 4 && name.to_ascii_lowercase().ends_with(".zip") {
                    out.push(Pending {
                        raw: name,
                        kind: CandidateKind::Zip,
                        files: vec![(
                            String::new(),
                            meta.len(),
                            path.to_string_lossy().into_owned(),
                            identity(&meta),
                        )],
                        disk: true,
                    });
                }
            }
        }
    }
    out
}

/// Lists the files below `dir` as `(path inside the mod, size, disk path)`, never following a link, stopping at the file cap
/// and the depth cap (a path deeper than the cap is listed, so the package check names it).
fn walk(
    dir: &Path,
    prefix: &str,
    depth: usize,
    limits: &ModLimits,
    files: &mut Vec<(String, u64, String, u64)>,
    over: &mut bool,
    notes: &mut Notes,
) {
    if depth > limits.depth.saturating_add(1) {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if files.len() > limits.files {
            *over = true;
            return;
        }
        let Some(name) = name_of(&path) else {
            // Not UTF-8: list it under a name the path rules refuse, so the whole mod is refused.
            files.push((format!("{prefix}\u{fffd}\u{0}"), 0, String::new(), 0));
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let inner = format!("{prefix}{name}");
        match kind_of(&meta) {
            None => notes
                .warnings
                .push(format!("{inner}: links are not followed; skipped")),
            Some(true) => walk(
                &path,
                &format!("{inner}/"),
                depth + 1,
                limits,
                files,
                over,
                notes,
            ),
            Some(false) => files.push((
                inner,
                meta.len(),
                path.to_string_lossy().into_owned(),
                identity(&meta),
            )),
        }
    }
}

/// Something that tells one file on the disk from another, taken from a listing's metadata and from the opened handle's:
/// the device and inode on Unix, the creation and write times on Windows (the file index is not stable Rust, so there it is a
/// weaker proxy: a link swapped in is caught by the reparse flag, not by this).
fn identity(meta: &fs::Metadata) -> u64 {
    let mix = |a: u64, b: u64| a.rotate_left(23).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ b;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        mix(meta.dev(), meta.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        mix(meta.creation_time(), meta.last_write_time())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (meta, mix);
        0
    }
}

/// A regular file's contents, at most `cap` bytes (a longer file is not returned), for the file that was listed with `size`
/// bytes and `id` ([`identity`]). The file is opened first and the opened handle is what is checked, so a file swapped for a
/// link, a junction or another file after the listing is refused rather than followed: on Windows a link is opened as the
/// reparse point itself (and refused), and on every system the handle must be a plain file with the listed size and identity.
pub(crate) fn read_file(path: &str, cap: u64, size: u64, id: u64) -> Option<Vec<u8>> {
    if path.is_empty() {
        return None;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: a link is opened as itself, not as what it points at.
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path).ok()?;
    let meta = file.metadata().ok()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    if !meta.is_file() || meta.len() != size || identity(&meta) != id {
        return None;
    }
    let mut out = Vec::new();
    file.take(cap.saturating_add(1))
        .read_to_end(&mut out)
        .ok()?;
    (out.len() as u64 == size).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder next to the test executable (inside the build folder).
    fn scratch(name: &str) -> std::path::PathBuf {
        let exe = std::env::current_exe().expect("the test executable");
        let dir = exe
            .parent()
            .expect("its folder")
            .join("mods-disk-unit")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch folder");
        dir
    }

    #[test]
    fn a_file_swapped_after_the_listing_is_refused() {
        let dir = scratch("swap");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, b"aaaa").expect("write");
        fs::write(&b, b"bbbb").expect("write");
        let listed = fs::symlink_metadata(&a).expect("metadata");
        let (size, id) = (listed.len(), identity(&listed));
        let path = a.to_string_lossy().into_owned();
        assert_eq!(
            read_file(&path, 100, size, id).as_deref(),
            Some(&b"aaaa"[..])
        );

        // The same size, another file.
        fs::rename(&b, &a).expect("rename over");
        // (On Windows the identity is a pair of timestamps that two files made at once can share; a swapped link is caught by the
        // reparse flag instead, below.)
        if cfg!(unix) {
            assert_eq!(
                read_file(&path, 100, size, id),
                None,
                "another file of the same size"
            );
        }

        // The same file, grown.
        let listed = fs::symlink_metadata(&a).expect("metadata");
        let (size, id) = (listed.len(), identity(&listed));
        fs::write(&a, b"bbbbbbbb").expect("write");
        assert_eq!(read_file(&path, 100, size, id), None, "a file that grew");

        // A link in its place.
        let target = dir.join("target.txt");
        fs::write(&target, b"bbbbbbbb").expect("write");
        let listed = fs::symlink_metadata(&a).expect("metadata");
        let (size, id) = (listed.len(), identity(&listed));
        fs::remove_file(&a).expect("remove");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &a);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &a);
        if made.is_ok() {
            assert_eq!(
                read_file(&path, 100, size, id),
                None,
                "a link is not followed"
            );
        }
    }
}
