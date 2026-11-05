//! What the example games share beyond the engine's own game layer ([`torque::app`]).
//!
//! - [`KitAssets`]: the asset kit (`torque-example-assets`) made in memory and read as files under [`KIT_DIR`], the way the
//!   renderer's shape and texture loaders read any game's files.
//! - [`GameFiles`]: the kit, the shared scripts (`game/` here, under `common/`) and an example's own game folder.
//! - [`prediction`]: what the client's prediction of its own figure asks a game of kit cubes (the cubes it collides with, smooth
//!   aim).
//! - [`ray_box`]: a ray against a box, for a camera behind a figure kept in front of the cubes.
//! - [`camera_log`]: the camera's motion frame by frame, for the examples' camera clips.

pub mod camera_log;
pub mod prediction;

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

    /// Adds (or replaces) the file `name` under the kit's folder (`office.dif` is `kit/office.dif`): files an example
    /// builds when it starts.
    pub fn insert(&mut self, name: &str, bytes: Vec<u8>) {
        self.files.insert(kit_path(name), bytes);
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

/// The ghost scene's options the examples share: TGE's, with the first-person view held steady over the figure's bobbing
/// gaits ([`torque::app::ghosts::GhostOptions::steady_eye`], the engine's own).
pub fn ghost_options() -> torque::app::ghosts::GhostOptions {
    torque::app::ghosts::GhostOptions {
        steady_eye: true,
        ..Default::default()
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

    /// The kit and the game folder as one pack ([`torque::app::files`]): the kit's files under [`KIT_DIR`], the folder's at the
    /// root, those `keep` turns down left out.
    pub fn pack(&self, keep: impl Fn(&str) -> bool) -> Vec<u8> {
        let mut kit: Vec<(&str, &[u8])> = self
            .kit
            .files
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        kit.sort();
        let all = kit.into_iter().chain(self.folder.iter());
        torque::app::files::pack(all.filter(|(p, _)| keep(p)))
    }

    /// The kit and the game folder of a pack ([`GameFiles::pack`]): a game built for the web carries its files so.
    pub fn unpack(bytes: &[u8]) -> Result<GameFiles<G>, String> {
        Ok(GameFiles::of(
            torque::app::files::unpack(bytes)?
                .into_iter()
                .map(|(path, bytes)| (path.to_string(), bytes.to_vec())),
        ))
    }

    /// The kit and the game folder of the files a web build fetched by its list ([`torque::app::manifest`]): the files of each
    /// pack (a path ending in `.pack`), and every other file at its path.
    pub fn from_fetched(fetched: Vec<(String, Vec<u8>)>) -> Result<GameFiles<G>, String> {
        let mut all = Vec::new();
        for (path, bytes) in fetched {
            if path.to_ascii_lowercase().ends_with(".pack") {
                all.extend(
                    torque::app::files::unpack(&bytes)?
                        .into_iter()
                        .map(|(path, bytes)| (path.to_string(), bytes.to_vec())),
                );
            } else {
                all.push((path, bytes));
            }
        }
        Ok(GameFiles::of(all))
    }

    /// The kit (the files under [`KIT_DIR`]) and the game folder (the others) of `files`.
    fn of(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> GameFiles<G> {
        let mut kit = HashMap::new();
        let mut rest = Vec::new();
        let prefix = format!("{KIT_DIR}/");
        for (path, bytes) in files {
            if path.starts_with(&prefix) {
                kit.insert(path.to_ascii_lowercase(), bytes);
            } else {
                rest.push((path, bytes));
            }
        }
        let mut folder = torque::app::GameFiles::default();
        folder.extend(rest);
        GameFiles {
            kit: KitAssets { files: kit },
            folder,
            game: PhantomData,
        }
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

/// Where the ray from `from` along the unit `dir` enters the box `min`..`max` (its distance), if it does ahead.
pub fn ray_box(from: [f32; 3], dir: [f32; 3], min: [f32; 3], max: [f32; 3]) -> Option<f32> {
    let (mut near, mut far) = (0.0f32, f32::MAX);
    for i in 0..3 {
        if dir[i].abs() < 1e-6 {
            if from[i] < min[i] || from[i] > max[i] {
                return None;
            }
            continue;
        }
        let a = (min[i] - from[i]) / dir[i];
        let b = (max[i] - from[i]) / dir[i];
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return None;
        }
    }
    // A ray that starts inside a box (the figure's own floor) does not count.
    (near > 0.0).then_some(near)
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

    #[test]
    fn a_pack_holds_the_kit_and_the_folder_apart() {
        struct Dir;
        impl GameDir for Dir {
            const DIR: &'static str = concat!(env!("CARGO_MANIFEST_DIR"), "/game");
        }
        let mut files = GameFiles::<Dir> {
            kit: KitAssets {
                files: HashMap::new(),
            },
            folder: torque::app::GameFiles::default(),
            game: PhantomData,
        };
        files.kit.insert("Cube.dts", vec![1, 2, 3]);
        files.kit.insert("big.dif", vec![9]);
        files.insert("missions/a.mis", b"// A");
        let pack = files.pack(|p| p != "kit/big.dif");
        let back = GameFiles::<Dir>::unpack(&pack).unwrap();
        assert_eq!(back.kit.read("kit/cube.dts"), Some(vec![1, 2, 3]));
        assert_eq!(back.kit.read("kit/big.dif"), None);
        assert_eq!(back.file("missions/a.mis"), Some(&b"// A"[..]));
        assert_eq!(back.maps(), ["missions/a.mis"]);

        let fetched = GameFiles::<Dir>::from_fetched(vec![
            ("game.pack".to_string(), pack),
            ("kit/extra.png".to_string(), vec![7]),
            ("missions/b.mis".to_string(), b"// B".to_vec()),
        ])
        .unwrap();
        assert_eq!(fetched.kit.read("kit/cube.dts"), Some(vec![1, 2, 3]));
        assert_eq!(fetched.kit.read("kit/extra.png"), Some(vec![7]));
        assert_eq!(fetched.maps(), ["missions/a.mis", "missions/b.mis"]);
        assert!(GameFiles::<Dir>::from_fetched(vec![("bad.pack".to_string(), vec![1])]).is_err());
    }
}
