//! What a map places and how it looks: instances, the environment, spawns, entities with their outputs, and its lighting.
//!
//! Legacy maps build these on each side from their mission records (sky, sun and sun-light objects), so they are the contract
//! a BSP importer would fill first.

use crate::collision::Contents;

/// A placed model: its file (as the format names it), its transform (a row-major 4 x 4 matrix, rotation and translation) and
/// scale, its skin, and what it collides as (`None`: not at all).
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub model: String,
    pub matrix: [f32; 16],
    pub scale: [f32; 3],
    pub skin: u32,
    pub solid: Option<Contents>,
}

/// A sky.
#[derive(Debug, Clone, PartialEq)]
pub enum Sky {
    /// Six faces (+x -x +y -y +z -z), each a texture name; empty for a face not drawn.
    Faces([String; 6]),
    /// A material list file's names in order (legacy `.dml`: the faces, then the cloud layers).
    List(Vec<String>),
    /// One colour (RGBA, 0-1).
    Solid([f32; 4]),
    /// A painted gradient: the colours at the top, the horizon and the bottom.
    Painted([[f32; 3]; 3]),
}

/// A region of the map drawn scaled behind the world (a BSP's 3D skybox): its camera's point and how much smaller it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyRegion {
    pub camera: [f32; 3],
    pub scale: f32,
}

/// Distance fog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
    /// The most it covers (0-1).
    pub max: f32,
}

/// The sun: the unit direction towards it, its colour and the ambient light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunLight {
    pub towards: [f32; 3],
    pub color: [f32; 3],
    pub ambient: [f32; 3],
}

/// A body of water: its surface height, the box it fills (min, max) and its surface material.
#[derive(Debug, Clone, PartialEq)]
pub struct Water {
    pub surface: f32,
    pub bounds: ([f32; 3], [f32; 3]),
    pub material: String,
}

/// What falls from the sky.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precipitation {
    Rain,
    Snow,
}

/// A map's environment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Environment {
    pub sky: Option<Sky>,
    pub sky_region: Option<SkyRegion>,
    pub fog: Option<Fog>,
    pub sun: Option<SunLight>,
    pub water: Vec<Water>,
    pub precipitation: Option<Precipitation>,
}

/// A spawn point: where, facing which way (yaw in radians about +z, 0 along +y), and its labels (a team, a game mode's tag).
#[derive(Debug, Clone, PartialEq)]
pub struct Spawn {
    pub position: [f32; 3],
    pub yaw: f32,
    pub tags: Vec<String>,
}

/// One output of an entity: when `output` fires, `input` is sent to the entities `target` names with `parameter`, after
/// `delay` seconds, at most `times` times (`None`: every time).
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub output: String,
    pub target: String,
    pub input: String,
    pub parameter: String,
    pub delay: f32,
    pub times: Option<u32>,
}

/// What moves an entity: a brush model of the map (its index) or a placed model.
#[derive(Debug, Clone, PartialEq)]
pub enum Mover {
    BrushModel(u32),
    Instance(Instance),
}

/// A map entity: its class, its key-values in the file's order (a name may repeat), its origin, what moves it, and its outputs. The I/O
/// system runs on these whatever the format; legacy maps have none.
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub class: String,
    pub keys: Vec<(String, String)>,
    pub origin: [f32; 3],
    pub mover: Option<Mover>,
    pub outputs: Vec<Output>,
}

/// Where a map's lightmaps come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lighting {
    /// The map's own bake, drawn as it is (BSP).
    Baked,
    /// Made by the relight from the sun (legacy: the stored bake was made for a different sun, so it is replaced).
    Relight,
}
