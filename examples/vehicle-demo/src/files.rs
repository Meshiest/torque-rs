//! The example's files: the asset kit (made in memory), the examples' shared scripts and the game folder (`game/`: the
//! scripts and the map), read from disk once; and the map's terrain, read from the kit's `.ter` as the server reads it.

use torque::app::terrain::{LayerUv, Terrain};
use torque_example_common::GameDir;
use torque_render_scene::Assets as _;

/// The vehicle demo's game folder.
#[derive(Debug, Clone, Copy)]
pub struct VehicleDemoDir;

impl GameDir for VehicleDemoDir {
    const DIR: &'static str = concat!(env!("CARGO_MANIFEST_DIR"), "/game");
}

/// The kit, the shared scripts and the vehicle demo's game folder.
pub type GameFiles = torque_example_common::GameFiles<VehicleDemoDir>;

/// The map.
pub const MISSION: &str = "missions/hills.mis";
/// The map's terrain file (the `TerrainBlock`'s `terrainFile`) and its square size.
pub const TERRAIN_FILE: &str = "kit/hills.ter";
pub const TERRAIN_SQUARE: i32 = 8;
/// The terrain's first texture and how many units one repeat of it covers (two checker squares a side).
pub const TERRAIN_TEXTURE: &str = "kit/checker.png";
pub const TERRAIN_TILE: f32 = 8.0;
/// The race track's dirt (the terrain's second material).
pub const DIRT_TEXTURE: &str = "kit/dirt.png";

/// The terrain's materials as the view draws them: the checker repeated over the ground, the dirt along the track and across it.
pub fn terrain_layers() -> [(&'static str, LayerUv<'static>); 2] {
    [
        (TERRAIN_TEXTURE, LayerUv::Tiled(TERRAIN_TILE)),
        (
            DIRT_TEXTURE,
            LayerUv::Mapped(&torque_example_assets::track::dirt_uv),
        ),
    ]
}

/// The map's terrain.
pub fn terrain(files: &GameFiles) -> Result<Terrain, String> {
    let bytes = files
        .kit
        .read(TERRAIN_FILE)
        .ok_or_else(|| format!("{TERRAIN_FILE} is not in the kit"))?;
    Terrain::read(&bytes, TERRAIN_SQUARE)
}

/// The files a build for the web fetches ([`GameFiles::pack`]): the kit and the game folder, as they are.
pub fn web_pack(files: &GameFiles) -> Vec<u8> {
    files.pack(|_| true)
}
