//! What the example games share beyond the engine's own game layer ([`torque::app`]).
//!
//! - [`KitAssets`]: the asset kit (`torque-example-assets`) made in memory and read as files under [`KIT_DIR`], the way the
//!   renderer's shape and texture loaders read any game's files.
//! - [`GameFiles`]: the kit, the shared scripts (`game/` here, under `common/`) and an example's own game folder.

use std::collections::HashMap;
use std::io;
use std::marker::PhantomData;
use std::path::Path;

use torque::script::vm::host::MemFiles;
use torque_render_scene::Assets;

/// The folder the kit's files are read from.
pub const KIT_DIR: &str = "kit";

/// The asset kit as files: `kit/cube.dts`, `kit/figure.dts`, `kit/grid.png` and the rest, made once in memory.
pub struct KitAssets {
    files: HashMap<String, Vec<u8>>,
}

impl KitAssets {
    /// Makes every asset of the kit.
    pub fn new() -> io::Result<KitAssets> {
        let files = torque_example_assets::all()?
            .into_iter()
            .map(|a| (kit_path(a.name), a.bytes))
            .collect();
        Ok(KitAssets { files })
    }

    /// The files' paths.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

impl Assets for KitAssets {
    /// A file by its path, ignoring case and with `\` read as `/`.
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files
            .get(&path.replace('\\', "/").to_ascii_lowercase())
            .cloned()
    }
}

/// The path of the kit's file `name` (`figure.dts` is `kit/figure.dts`).
pub fn kit_path(name: &str) -> String {
    format!("{KIT_DIR}/{}", name.to_ascii_lowercase())
}

/// The folder the shared scripts are read from.
pub const COMMON_DIR: &str = "common/";

/// An example's own game folder (`<crate>/game`).
pub trait GameDir {
    /// The folder's path: `concat!(env!("CARGO_MANIFEST_DIR"), "/game")` in the example's crate.
    const DIR: &'static str;
}

/// The kit, the shared scripts and an example's game folder `G` (its scripts, and its maps in `missions/`), read from disk
/// once. The server and every client read them from memory.
pub struct GameFiles<G> {
    pub kit: KitAssets,
    /// The game folder's files, the shared scripts under [`COMMON_DIR`].
    pub folder: torque::app::GameFiles,
    game: PhantomData<G>,
}

impl<G> GameFiles<G> {
    /// The kit, the shared scripts and every file of the game folder `dir`.
    pub fn load(dir: &Path) -> io::Result<GameFiles<G>> {
        let mut folder = torque::app::GameFiles::default();
        folder.add_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("game"),
            COMMON_DIR,
        )?;
        folder.add_dir(dir, "")?;
        Ok(GameFiles {
            kit: KitAssets::new()?,
            folder,
            game: PhantomData,
        })
    }

    /// Adds (or replaces) the game folder's file `path`.
    pub fn insert(&mut self, path: &str, bytes: &[u8]) {
        self.folder.insert(path, bytes);
    }

    /// A file of the game folder.
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.folder.file(path)
    }

    /// The maps: the paths of the `.mis` files in `missions/`, sorted.
    pub fn maps(&self) -> Vec<String> {
        self.folder.missions()
    }

    /// The files as a script engine's file host: the kit under `kit/`, the game folder's at its root.
    pub fn mem_files(&self) -> MemFiles {
        let mem = MemFiles::new();
        for path in self.kit.paths() {
            if let Some(bytes) = self.kit.read(path) {
                mem.insert(path, &bytes);
            }
        }
        self.folder.fill(&mem);
        mem
    }
}

impl<G: GameDir> GameFiles<G> {
    /// The kit, the shared scripts and the example's own game folder.
    pub fn bundled() -> io::Result<GameFiles<G>> {
        GameFiles::load(Path::new(G::DIR))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kit_reads_by_path_in_any_case() {
        let kit = KitAssets::new().unwrap();
        assert!(kit.read("kit/figure.dts").is_some());
        assert_eq!(kit.read("KIT\\Grid.PNG"), kit.read("kit/grid.png"));
        assert!(kit.read("kit/missing.png").is_none());
        assert_eq!(
            kit.paths().count(),
            torque_example_assets::all().unwrap().len()
        );
    }
}
