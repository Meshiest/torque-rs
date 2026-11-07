//! A game folder read once into memory: the scripts, the maps (`.mis` files in `missions/`) and whatever else it holds. The
//! server and every client read their files from it through [`GameFiles::mem_files`].
//!
//! **Where the files come from.** A folder on disk ([`GameFiles::add_dir`]: a native game reads its folder when it starts), or a
//! pack ([`GameFiles::unpack`]): the files of a folder in one block of bytes ([`GameFiles::pack`]), which a game built for the web
//! carries inside itself, as a browser has no disk to read.
//!
//! **A pack** is the bytes `TQPK`, the file count (a little-endian `u32`), then each file: its path's length (`u32`), the path
//! (UTF-8, `/` between folders), its length (`u32`) and its bytes.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io;
use std::path::Path;

use torque_ts_vm::host::MemFiles;

/// A game folder's files: their paths under the folder (with `/`) and their bytes, sorted by path.
#[derive(Debug, Clone, Default)]
pub struct GameFiles {
    files: Vec<(String, Vec<u8>)>,
}

impl GameFiles {
    /// Every file of the folder `dir`.
    pub fn load(dir: &Path) -> io::Result<GameFiles> {
        let mut files = GameFiles::default();
        files.add_dir(dir, "")?;
        Ok(files)
    }

    /// Adds every file of the folder `dir` under `prefix` (`common/`, or empty for the root), replacing files of the same path.
    pub fn add_dir(&mut self, dir: &Path, prefix: &str) -> io::Result<()> {
        let mut found = Vec::new();
        read_tree(dir, dir, &mut found)?;
        self.extend(
            found
                .into_iter()
                .map(|(path, bytes)| (format!("{prefix}{path}"), bytes)),
        );
        Ok(())
    }

    /// Adds (or replaces) the file `path`.
    pub fn insert(&mut self, path: &str, bytes: &[u8]) {
        self.extend([(path.to_string(), bytes.to_vec())]);
    }

    /// Adds (or replaces) many files at once, in time linear in their number: of two files whose paths differ only in case, the
    /// later one stays.
    pub fn extend(&mut self, files: impl IntoIterator<Item = (String, Vec<u8>)>) {
        let mut index: HashMap<String, usize> = self
            .files
            .iter()
            .enumerate()
            .map(|(i, (p, _))| (p.to_ascii_lowercase(), i))
            .collect();
        for (path, bytes) in files {
            match index.entry(path.to_ascii_lowercase()) {
                Entry::Occupied(at) => self.files[*at.get()] = (path, bytes),
                Entry::Vacant(at) => {
                    at.insert(self.files.len());
                    self.files.push((path, bytes));
                }
            }
        }
        self.files.sort();
    }

    /// A file, by its path in any case.
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(p, _)| p.eq_ignore_ascii_case(path))
            .map(|(_, b)| b.as_slice())
    }

    /// The files' paths and bytes, sorted by path.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files.iter().map(|(p, b)| (p.as_str(), b.as_slice()))
    }

    /// The maps: the paths of the `.mis` files in `missions/`, sorted.
    pub fn missions(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|(p, _)| p.starts_with("missions/") && p.ends_with(".mis"))
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// Every file in one block of bytes (module docs), sorted by path.
    pub fn pack(&self) -> Vec<u8> {
        pack(self.iter())
    }

    /// The files of a pack ([`GameFiles::pack`]).
    pub fn unpack(bytes: &[u8]) -> Result<GameFiles, String> {
        let mut files = GameFiles::default();
        files.extend(
            unpack(bytes)?
                .into_iter()
                .map(|(path, bytes)| (path.to_string(), bytes.to_vec())),
        );
        Ok(files)
    }

    /// Puts every file into `mem`.
    pub fn fill(&self, mem: &MemFiles) {
        for (path, bytes) in &self.files {
            mem.insert(path, bytes);
        }
    }

    /// The files as a script engine's file host.
    pub fn mem_files(&self) -> MemFiles {
        let mem = MemFiles::new();
        self.fill(&mem);
        mem
    }
}

/// The start of a pack.
pub const PACK_MAGIC: &[u8; 4] = b"TQPK";

/// `files` (paths and bytes) as a pack (module docs).
pub fn pack<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Vec<u8> {
    let files: Vec<(&str, &[u8])> = files.into_iter().collect();
    let mut out = PACK_MAGIC.to_vec();
    out.extend_from_slice(&len_u32(files.len()).to_le_bytes());
    for (path, bytes) in files {
        out.extend_from_slice(&len_u32(path.len()).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&len_u32(bytes.len()).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    out
}

/// A length a pack records: a pack holds no file or path of 4 GiB.
fn len_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The files of a pack, in its order (Err: not a pack, or cut short).
pub fn unpack(bytes: &[u8]) -> Result<Vec<(&str, &[u8])>, String> {
    let mut rest = bytes
        .strip_prefix(PACK_MAGIC.as_slice())
        .ok_or("not a pack of files")?;
    let count = take_len(&mut rest)?;
    let mut files = Vec::with_capacity(count.min(rest.len() / 8));
    for _ in 0..count {
        let n = take_len(&mut rest)?;
        let path =
            std::str::from_utf8(take(&mut rest, n)?).map_err(|_| "a pack's path is not UTF-8")?;
        let n = take_len(&mut rest)?;
        files.push((path, take(&mut rest, n)?));
    }
    Ok(files)
}

/// The next `n` bytes of `rest`.
fn take<'a>(rest: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
    if n > rest.len() {
        return Err("a pack of files cut short".into());
    }
    let (got, after) = rest.split_at(n);
    *rest = after;
    Ok(got)
}

/// The next length of `rest`.
fn take_len(rest: &mut &[u8]) -> Result<usize, String> {
    let b = take(rest, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn read_tree(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            read_tree(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            let name = rel.to_string_lossy().replace('\\', "/");
            out.push((name, std::fs::read(&path)?));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_read_in_any_case_and_maps_are_the_missions() {
        let mut files = GameFiles::default();
        files.insert("missions/b.mis", b"b");
        files.insert("missions/a.mis", b"a");
        files.insert("server.cs", b"s");
        files.insert("Server.CS", b"S");
        assert_eq!(files.file("SERVER.cs"), Some(&b"S"[..]));
        assert_eq!(files.missions(), ["missions/a.mis", "missions/b.mis"]);
        assert_eq!(files.iter().count(), 3);
    }

    #[test]
    fn a_pack_gives_back_its_files() {
        let mut files = GameFiles::default();
        files.insert("missions/a.mis", b"a map");
        files.insert("server.cs", b"");
        files.insert("kit/cube.dts", &[0, 1, 2, 255]);
        let pack = files.pack();
        let back = GameFiles::unpack(&pack).expect("a pack");
        assert_eq!(
            back.iter().collect::<Vec<_>>(),
            files.iter().collect::<Vec<_>>()
        );
        for cut in 0..pack.len() {
            assert!(GameFiles::unpack(&pack[..cut]).is_err(), "cut at {cut}");
        }
        assert!(GameFiles::unpack(b"TQPK\xff\xff\xff\xff").is_err());
    }

    #[test]
    fn many_files_added_at_once_replace_by_any_case() {
        let mut files = GameFiles::default();
        files.insert("a.cs", b"old");
        files.extend((0..20_000).map(|i| (format!("f/{i:05}.txt"), vec![0u8; 1])));
        files.extend([("A.CS".to_string(), b"new".to_vec())]);
        assert_eq!(files.iter().count(), 20_001);
        assert_eq!(files.file("a.cs"), Some(&b"new"[..]));
        let back = GameFiles::unpack(&files.pack()).expect("a pack");
        assert_eq!(back.iter().count(), 20_001);
    }
}
