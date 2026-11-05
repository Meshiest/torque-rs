//! A game folder read once into memory: the scripts, the maps (`.mis` files in `missions/`) and whatever else it holds. The
//! server and every client read their files from it through [`GameFiles::mem_files`].

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
        for (path, bytes) in found {
            let path = format!("{prefix}{path}");
            self.files.retain(|(p, _)| !p.eq_ignore_ascii_case(&path));
            self.files.push((path, bytes));
        }
        self.files.sort();
        Ok(())
    }

    /// Adds (or replaces) the file `path`.
    pub fn insert(&mut self, path: &str, bytes: &[u8]) {
        self.files.retain(|(p, _)| !p.eq_ignore_ascii_case(path));
        self.files.push((path.to_string(), bytes.to_vec()));
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
}
