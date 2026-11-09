//! A Source game's files as one tree: its search paths (VPK archives, loose folders, a map's pak lump), the first that has a path
//! answering, as the engine's file system does. [`SourceFiles`] is a [`FileHost`], so the engine's asset registry loads from it.
//!
//! [`SourceFiles::from_gameinfo`] reads a game folder's `gameinfo.txt` (`GameInfo` / `FileSystem` / `SearchPaths`): every path
//! whose key has `game` in it, `|gameinfo_path|` the game folder and `|all_source_engine_paths|` the folder above it; a
//! `.vpk` names the archive `<name>_dir.vpk`; paths with wildcards (`custom/*`) are not mounted.
//!
//! Every lookup is a table lookup: a loose folder is listed once when it is mounted (at most [`MAX_FOLDER_FILES`] files), so a
//! path never reaches the disk unless the listing found it there. A loose file is opened once and read by the length of the open
//! file, at most the length the listing gave and [`MAX_LOOSE_FILE_BYTES`] (a file that grew since is refused); a link is not
//! followed (the same for gameinfo, `_dir.vpk` and the `_NNN.vpk` data files); `.vpk` files are never listed as loose files.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use torque_resource::zips::ZipMount;
use torque_ts_vm::host::{FileHost, FileRead, FileStat, Refusal};

use crate::kv;
use crate::read::{Error, Result, normalize_path, read_plain_file};
use crate::vpk::VpkArchive;

/// The most files one loose folder lists.
pub const MAX_FOLDER_FILES: usize = 1 << 20;
/// The deepest folder listed.
const MAX_FOLDER_DEPTH: usize = 24;
/// The largest gameinfo file read.
const MAX_GAMEINFO_BYTES: u64 = 1 << 20;
/// The largest loose file one read returns (the engine's biggest maps are a few hundred MiB).
pub const MAX_LOOSE_FILE_BYTES: u64 = 512 << 20;

/// One search path.
enum Mount {
    Vpk(VpkArchive),
    Folder(HashMap<String, (PathBuf, u64)>),
    Zip(ZipMount),
    Memory(HashMap<String, Vec<u8>>),
}

/// A game's files (module docs).
#[derive(Default)]
pub struct SourceFiles {
    mounts: Vec<(String, Mount)>,
    /// What did not mount, and why.
    pub skipped: Vec<String>,
    reads: RefCell<u64>,
}

impl std::fmt::Debug for SourceFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceFiles")
            .field(
                "mounts",
                &self.mounts.iter().map(|m| &m.0).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl SourceFiles {
    pub fn new() -> SourceFiles {
        SourceFiles::default()
    }

    /// The search paths of the game folder `dir` (holding `gameinfo.txt`), in its order.
    pub fn from_gameinfo(dir: &Path) -> Result<SourceFiles> {
        let mut files = SourceFiles::new();
        files.add_gameinfo(dir)?;
        Ok(files)
    }

    /// Mounts the search paths of the game folder `dir` after the ones mounted already (a path mounted already is skipped).
    pub fn add_gameinfo(&mut self, dir: &Path) -> Result<()> {
        let info = dir.join("gameinfo.txt");
        let text = read_plain_file(&info, MAX_GAMEINFO_BYTES, None)?;
        let kv = kv::parse(&text)?;
        let paths = kv
            .first()
            .and_then(|g| g.block())
            .and_then(|g| kv::find_block(g, "FileSystem"))
            .and_then(|fs| kv::find_block(fs, "SearchPaths"))
            .ok_or_else(|| Error::Invalid("gameinfo.txt has no FileSystem/SearchPaths".into()))?;
        let base = dir.parent().unwrap_or(dir).to_path_buf();
        for p in paths {
            let Some(value) = p.str() else { continue };
            if !p.key.to_ascii_lowercase().split('+').any(|k| k == "game") {
                continue;
            }
            if value.contains('*') {
                continue;
            }
            let (root, rest) = if let Some(rest) = strip_prefix_ci(value, "|gameinfo_path|") {
                (dir, rest)
            } else if let Some(rest) = strip_prefix_ci(value, "|all_source_engine_paths|") {
                (base.as_path(), rest)
            } else {
                (base.as_path(), value)
            };
            let path = match search_path(root, rest) {
                Ok(p) => p,
                Err(e) => {
                    self.skipped.push(format!("{value}: {e}"));
                    continue;
                }
            };
            let name = path.display().to_string();
            if self.mounts.iter().any(|m| m.0 == name) {
                continue;
            }
            self.mount_path(&path);
        }
        Ok(())
    }

    /// Mounts a `.vpk` archive or a loose folder at the end of the search paths (what did not mount goes to `skipped`).
    pub fn mount_path(&mut self, path: &Path) {
        let is_vpk = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vpk"));
        let r = if is_vpk {
            self.mount_vpk(path)
        } else {
            self.mount_folder(path)
        };
        if let Err(e) = r {
            self.skipped.push(format!("{}: {e}", path.display()));
        }
    }

    /// Mounts a VPK archive (`<name>.vpk` or `<name>_dir.vpk`) at the end of the search paths.
    pub fn mount_vpk(&mut self, path: &Path) -> Result<()> {
        let a = VpkArchive::open(path)?;
        self.mounts
            .push((path.display().to_string(), Mount::Vpk(a)));
        Ok(())
    }

    /// Mounts a loose folder at the end of the search paths (listed once, now).
    pub fn mount_folder(&mut self, dir: &Path) -> Result<()> {
        if !dir.is_dir() {
            return Err(Error::Missing(dir.display().to_string()));
        }
        let mut index = HashMap::new();
        let mut stack = vec![(dir.to_path_buf(), String::new(), 0usize)];
        while let Some((d, prefix, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                let Ok(ft) = e.file_type() else { continue };
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                if ft.is_dir() {
                    if depth < MAX_FOLDER_DEPTH {
                        stack.push((e.path(), rel, depth + 1));
                    }
                } else if ft.is_file() {
                    // A `.vpk` is an archive: its pieces are read through the archive, never as files of this folder.
                    if name.ends_with(".vpk") {
                        continue;
                    }
                    if index.len() >= MAX_FOLDER_FILES {
                        return Err(Error::TooLarge {
                            what: "a folder's files",
                            asked: index.len() as u64 + 1,
                            cap: MAX_FOLDER_FILES as u64,
                        });
                    }
                    let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                    if let Some(rel) = normalize_path(&rel) {
                        index.insert(rel, (e.path(), len));
                    }
                }
            }
        }
        self.mounts
            .push((dir.display().to_string(), Mount::Folder(index)));
        Ok(())
    }

    /// Mounts a zip's entries (a map's pak lump) before every other search path, as the engine searches a map's own files first.
    pub fn mount_zip_first(&mut self, name: &str, zip: ZipMount) {
        self.mounts.insert(0, (name.to_string(), Mount::Zip(zip)));
    }

    /// Mounts files held in memory before every other search path (tests, a tool's own files).
    pub fn mount_memory_first(
        &mut self,
        name: &str,
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) {
        let map = files
            .into_iter()
            .filter_map(|(p, b)| Some((normalize_path(&p)?, b)))
            .collect();
        self.mounts
            .insert(0, (name.to_string(), Mount::Memory(map)));
    }

    /// The search paths' names, in their order.
    pub fn mounts(&self) -> Vec<&str> {
        self.mounts.iter().map(|m| m.0.as_str()).collect()
    }

    /// Unmounts the first search path named `name` (a map's pak lump when the map is left).
    pub fn unmount(&mut self, name: &str) {
        if let Some(i) = self.mounts.iter().position(|m| m.0 == name) {
            self.mounts.remove(i);
        }
    }

    /// The bytes of `path` from the first search path that has it.
    pub fn read_path(&self, path: &str) -> Option<Result<Vec<u8>>> {
        let p = normalize_path(path)?;
        *self.reads.borrow_mut() += 1;
        for (_, m) in &self.mounts {
            let found = match m {
                Mount::Vpk(a) => a.read(&p),
                Mount::Folder(index) => index.get(&p).map(|(real, listed)| {
                    read_plain_file(real, MAX_LOOSE_FILE_BYTES, Some(*listed))
                }),
                Mount::Zip(z) => z.read(p.as_bytes()).map(Ok),
                Mount::Memory(files) => files.get(&p).cloned().map(Ok),
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// The length of `path` from the first search path that has it.
    pub fn len_of(&self, path: &str) -> Option<u64> {
        let p = normalize_path(path)?;
        for (_, m) in &self.mounts {
            let found = match m {
                Mount::Vpk(a) => a.dir.entry(&p).map(|e| e.total_len()),
                Mount::Folder(index) => index.get(&p).map(|(_, len)| *len),
                Mount::Zip(z) => z.entry(p.as_bytes()).map(|e| e.size),
                Mount::Memory(files) => files.get(&p).map(|b| b.len() as u64),
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// Every path the search paths hold that starts with `prefix` and ends with `suffix` (lower case), sorted, without repeats.
    pub fn list(&self, prefix: &str, suffix: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, m) in &self.mounts {
            match m {
                Mount::Vpk(a) => out.extend(
                    a.dir
                        .paths()
                        .filter(|p| p.starts_with(prefix) && p.ends_with(suffix))
                        .map(str::to_string),
                ),
                Mount::Folder(index) => out.extend(
                    index
                        .keys()
                        .filter(|p| p.starts_with(prefix) && p.ends_with(suffix))
                        .cloned(),
                ),
                Mount::Zip(z) => out.extend(
                    z.entries()
                        .iter()
                        .map(|e| String::from_utf8_lossy(&e.name).to_ascii_lowercase())
                        .filter(|p| p.starts_with(prefix) && p.ends_with(suffix)),
                ),
                Mount::Memory(files) => out.extend(
                    files
                        .keys()
                        .filter(|p| p.starts_with(prefix) && p.ends_with(suffix))
                        .cloned(),
                ),
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// A search path's value made a path under `root`: relative, `/` or `\` separated, no drive, no `..`; and once resolved (links
/// followed) still under `root`. A gameinfo file can come with a mod, so its values are not trusted to stay in the game's folders.
fn search_path(root: &Path, value: &str) -> Result<PathBuf> {
    let v = value.trim().replace('\\', "/");
    if v.starts_with('/') || v.contains(':') {
        return Err(Error::Invalid(format!("{value} is not a relative path")));
    }
    let mut path = root.to_path_buf();
    for part in v.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(Error::Invalid(format!(
                    "{value} climbs out of the game's folders"
                )));
            }
            _ => path.push(part),
        }
    }
    // A `.vpk` names `<name>_dir.vpk` beside it: its folder must exist and stay under the root.
    let is_vpk = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vpk"));
    let existing = if is_vpk {
        path.parent().unwrap_or(root)
    } else {
        path.as_path()
    };
    let real = std::fs::canonicalize(existing)
        .map_err(|e| Error::Missing(format!("{}: {e}", existing.display())))?;
    let real_root = std::fs::canonicalize(root)
        .map_err(|e| Error::Missing(format!("{}: {e}", root.display())))?;
    if !real.starts_with(&real_root) {
        return Err(Error::Invalid(format!(
            "{value} leads out of the game's folders"
        )));
    }
    Ok(path)
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

impl FileHost for SourceFiles {
    fn read(&self, path: &[u8]) -> FileRead {
        let Ok(p) = std::str::from_utf8(path) else {
            return FileRead::Missing;
        };
        match self.read_path(p) {
            Some(Ok(b)) => FileRead::Found(b),
            Some(Err(_)) => FileRead::Refused(Refusal::CannotOpen),
            None => FileRead::Missing,
        }
    }

    fn stat(&self, path: &[u8]) -> FileStat {
        let Ok(p) = std::str::from_utf8(path) else {
            return FileStat::Missing;
        };
        match self.len_of(p) {
            Some(len) => FileStat::Found { len },
            None => FileStat::Missing,
        }
    }

    fn is_archive_entry(&self, path: &[u8]) -> bool {
        std::str::from_utf8(path).is_ok_and(|p| self.len_of(p).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_paths_stay_in_the_game() {
        let root = std::env::temp_dir();
        assert!(search_path(&root, "../x").is_err());
        assert!(search_path(&root, "a/../../x").is_err());
        assert!(search_path(&root, "/etc").is_err());
        assert!(search_path(&root, "\\\\server\\share").is_err());
        assert!(search_path(&root, "C:/Windows").is_err());
        assert!(search_path(&root, "c:x").is_err());
        assert!(search_path(&root, ".").is_ok());
    }

    #[test]
    fn memory_mounts_answer_first() {
        let mut f = SourceFiles::new();
        f.mount_memory_first("a", [("materials/x.vmt".to_string(), b"one".to_vec())]);
        f.mount_memory_first("b", [("Materials\\X.vmt".to_string(), b"two".to_vec())]);
        assert_eq!(f.read_path("materials/x.vmt").unwrap().unwrap(), b"two");
        assert_eq!(f.len_of("MATERIALS/X.VMT"), Some(3));
        assert!(f.read_path("../x").is_none());
        f.unmount("b");
        assert_eq!(f.read_path("materials/x.vmt").unwrap().unwrap(), b"one");
        assert_eq!(f.list("materials/", ".vmt"), vec!["materials/x.vmt"]);
        assert_eq!(f.read(b"materials/x.vmt"), FileRead::Found(b"one".to_vec()));
        assert_eq!(f.read(b"\xff"), FileRead::Missing);
    }
}
