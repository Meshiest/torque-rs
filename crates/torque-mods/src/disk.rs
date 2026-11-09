//! Mods on the player's disk (not on the web): the folders and zips of a root read with `std::fs`, never following a link.
//!
//! A symbolic link (or a Windows junction) inside a root or a mod is skipped and reported, so a mod cannot point the loader
//! at a file outside itself. Every file is opened and checked to be a regular file before it is read, and read at most its
//! cap plus one byte. A root's listing and each folder mod's walk visit at most [`visit_budget`] entries of every kind
//! (files, folders, links, names skipped), and a folder past it is skipped whole; the warnings are capped
//! ([`crate::discover::MAX_WARNINGS`]).

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::discover::{Notes, Pending};

/// The entries of every kind one listing may visit: a root's, or one folder mod's walk. A folder of empty folders or of links
/// costs as much as one of files, so a huge one cannot stall the start.
pub(crate) fn visit_budget(limits: &ModLimits) -> usize {
    limits.files.saturating_mul(4).saturating_add(1_024)
}

/// Reads `dir`'s entries, each one charged to `left`; `None` (and nothing more read) once it runs out.
fn listing(dir: &Path, left: &mut usize) -> Option<Vec<std::path::PathBuf>> {
    let Ok(read) = fs::read_dir(dir) else {
        return Some(Vec::new());
    };
    let mut entries = Vec::new();
    for entry in read {
        *left = left.checked_sub(1)?;
        if let Ok(entry) = entry {
            entries.push(entry.path());
        }
    }
    entries.sort();
    Some(entries)
}
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
    let mut left = visit_budget(limits);
    let Some(entries) = listing(root, &mut left) else {
        notes.warn(format!(
            "the mods folder has more than {} entries; it was not read",
            visit_budget(limits)
        ));
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in entries {
        let Some(name) = name_of(&path) else {
            notes.warn("a name in the mods folder is not UTF-8 text; skipped".into());
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        match kind_of(&meta) {
            None => notes.warn(format!("{name}: links are not followed; skipped")),
            Some(true) => {
                let mut files = Vec::new();
                let mut over = false;
                let mut left = visit_budget(limits);
                let mut walked = Walk {
                    limits,
                    files: &mut files,
                    over: &mut over,
                    left: &mut left,
                    notes: &mut *notes,
                };
                if walked.walk(&path, "", 0).is_none() {
                    notes.warn(format!(
                        "{name}: more than {} files, folders and links; skipped",
                        visit_budget(limits)
                    ));
                    continue;
                }
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

/// One folder mod's walk: what it lists and what it may still visit.
struct Walk<'a> {
    limits: &'a ModLimits,
    files: &'a mut Vec<(String, u64, String, u64)>,
    over: &'a mut bool,
    left: &'a mut usize,
    notes: &'a mut Notes,
}

impl Walk<'_> {
    /// Lists the files below `dir` as `(path inside the mod, size, disk path)`, never following a link, stopping at the file
    /// cap and the depth cap (a path deeper than the cap is listed, so the package check names it). `None` once the visit
    /// budget is spent.
    fn walk(&mut self, dir: &Path, prefix: &str, depth: usize) -> Option<()> {
        if depth > self.limits.depth.saturating_add(1) {
            return Some(());
        }
        for path in listing(dir, self.left)? {
            if self.files.len() > self.limits.files {
                *self.over = true;
                return Some(());
            }
            let Some(name) = name_of(&path) else {
                // Not UTF-8: list it under a name the path rules refuse, so the whole mod is refused.
                self.files
                    .push((format!("{prefix}\u{fffd}\u{0}"), 0, String::new(), 0));
                continue;
            };
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            let inner = format!("{prefix}{name}");
            match kind_of(&meta) {
                None => self
                    .notes
                    .warn(format!("{inner}: links are not followed; skipped")),
                Some(true) => self.walk(&path, &format!("{inner}/"), depth + 1)?,
                Some(false) => self.files.push((
                    inner,
                    meta.len(),
                    path.to_string_lossy().into_owned(),
                    identity(&meta),
                )),
            }
        }
        Some(())
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
/// Opens `path` with `options` only if it is a plain file, never through a link: it is looked at first, opened without
/// following a link where the system can (Windows' reparse flag), and the handle must be the file that was looked at, so a
/// link or another file swapped in between is refused.
pub(crate) fn open_plain(path: &Path, options: &mut fs::OpenOptions) -> Option<fs::File> {
    let before = fs::symlink_metadata(path).ok()?;
    if kind_of(&before) != Some(false) {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
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
    (meta.is_file() && identity(&meta) == identity(&before)).then_some(file)
}

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
    fn a_mod_of_many_empty_folders_is_skipped_quickly_with_one_warning() {
        let root = scratch("folders");
        let limits = ModLimits {
            files: 10,
            ..ModLimits::default()
        };
        let heavy = root.join("heavy");
        for n in 0..visit_budget(&limits) + 50 {
            fs::create_dir_all(heavy.join(format!("d{n}"))).expect("a folder");
        }
        fs::create_dir_all(root.join("light").join("art")).expect("a folder");
        fs::write(root.join("light").join("art").join("a.txt"), b"a").expect("a file");
        let mut notes = Notes::default();
        let found = list_root(&root, &limits, &mut notes);
        assert_eq!(
            found.iter().map(|p| p.raw.as_str()).collect::<Vec<_>>(),
            ["light"]
        );
        assert_eq!(notes.warnings.len(), 1, "{:?}", notes.warnings);
        assert!(notes.warnings[0].starts_with("heavy: more than"));
    }

    #[test]
    fn the_warnings_of_a_scan_are_capped() {
        let mut notes = Notes::default();
        for n in 0..1_000 {
            notes.warn(format!("warning {n}"));
        }
        assert_eq!(notes.warnings.len(), crate::discover::MAX_WARNINGS + 1);
        assert!(
            notes
                .warnings
                .last()
                .is_some_and(|l| l.contains("left out"))
        );
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
