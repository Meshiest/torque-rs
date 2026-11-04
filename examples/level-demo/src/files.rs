//! The example's files: the asset kit (made in memory) and the game folder (`game/`: the scripts, and the maps in
//! `game/missions/`), read from disk once.
//!
//! **A map** is a `.mis` script that builds `MissionGroup`. Its first two lines are comments: the map's title and one line
//! about it, which the menu shows.

use std::io;
use std::path::{Path, PathBuf};

use torque::script::vm::host::MemFiles;
use torque_example_common::KitAssets;

/// The example's own game folder.
pub fn game_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("game")
}

/// A map the menu offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEntry {
    /// The script's path in the game folder (`missions/plaza.mis`).
    pub path: String,
    pub title: String,
    pub about: String,
}

/// The kit and the game folder's files.
pub struct GameFiles {
    pub kit: KitAssets,
    /// The game folder's files: their paths under the folder (with `/`) and their bytes.
    pub files: Vec<(String, Vec<u8>)>,
}

impl GameFiles {
    /// The kit and every file of the game folder `dir`.
    pub fn load(dir: &Path) -> io::Result<GameFiles> {
        let mut files = Vec::new();
        read_tree(dir, dir, &mut files)?;
        files.sort();
        Ok(GameFiles {
            kit: KitAssets::new()?,
            files,
        })
    }

    /// The kit and the example's own game folder.
    pub fn bundled() -> io::Result<GameFiles> {
        GameFiles::load(&game_dir())
    }

    /// Adds (or replaces) the game folder's file `path`.
    pub fn insert(&mut self, path: &str, bytes: &[u8]) {
        self.files.retain(|(p, _)| !p.eq_ignore_ascii_case(path));
        self.files.push((path.to_string(), bytes.to_vec()));
        self.files.sort();
    }

    /// Puts every file into `mem`: the kit under `kit/`, the game folder's at its root.
    pub fn fill(&self, mem: &MemFiles) {
        for path in self.kit.paths() {
            if let Some(bytes) = torque_render_scene::Assets::read(&self.kit, path) {
                mem.insert(path, &bytes);
            }
        }
        for (path, bytes) in &self.files {
            mem.insert(path, bytes);
        }
    }

    /// A file of the game folder.
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(p, _)| p.eq_ignore_ascii_case(path))
            .map(|(_, b)| b.as_slice())
    }

    /// The maps in `missions/`, by path.
    pub fn maps(&self) -> Vec<MapEntry> {
        self.files
            .iter()
            .filter(|(p, _)| p.starts_with("missions/") && p.ends_with(".mis"))
            .map(|(path, bytes)| {
                let text = String::from_utf8_lossy(bytes);
                let mut lines = text
                    .lines()
                    .map(|l| l.trim().strip_prefix("//").map(str::trim));
                let title = lines.next().flatten().unwrap_or(path).to_string();
                let about = lines.next().flatten().unwrap_or("").to_string();
                MapEntry {
                    path: path.clone(),
                    title,
                    about,
                }
            })
            .collect()
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
