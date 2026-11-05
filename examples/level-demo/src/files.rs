//! The example's files: the asset kit (made in memory), the examples' shared scripts and the game folder (`game/`: the
//! scripts, and the maps in `game/missions/`), read from disk once.
//!
//! **A map** is a `.mis` script that builds `MissionGroup`. Its first two lines are comments: the map's title and one line
//! about it, which the menu shows.

use torque_example_common::GameDir;

/// The level demo's game folder.
#[derive(Debug, Clone, Copy)]
pub struct LevelDemoDir;

impl GameDir for LevelDemoDir {
    const DIR: &'static str = concat!(env!("CARGO_MANIFEST_DIR"), "/game");
}

/// The kit, the shared scripts and the level demo's game folder.
pub type GameFiles = torque_example_common::GameFiles<LevelDemoDir>;

/// A map the menu offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEntry {
    /// The script's path in the game folder (`missions/plaza.mis`).
    pub path: String,
    pub title: String,
    pub about: String,
}

/// The maps in `missions/`, by path.
pub fn maps(files: &GameFiles) -> Vec<MapEntry> {
    files
        .maps()
        .into_iter()
        .map(|path| {
            let text = String::from_utf8_lossy(files.file(&path).unwrap_or_default());
            let mut lines = text
                .lines()
                .map(|l| l.trim().strip_prefix("//").map(str::trim));
            let title = lines.next().flatten().unwrap_or(&path).to_string();
            let about = lines.next().flatten().unwrap_or("").to_string();
            MapEntry { path, title, about }
        })
        .collect()
}

/// The files a build for the web carries ([`GameFiles::pack`]): the kit, the game folder, and the levels already built into
/// the kit's interiors and their maps (their models and templates left out), so the browser builds nothing when it starts.
/// The kit's forests (the vehicle demo's, which no map here places) are left out too.
pub fn web_pack(mut files: GameFiles) -> Result<Vec<u8>, String> {
    crate::levels::add_levels(&mut files)?;
    Ok(files.pack(|path| !path.starts_with("levels/") && !path.starts_with("kit/forest")))
}
