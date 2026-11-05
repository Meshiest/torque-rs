//! A map's ground as the client draws it: its `TerrainBlock` and its `WaterBlock`. The client is sent neither (their classes
//! have no ghost here), so it reads them from the map the server runs, as the server does: the terrain from its
//! `terrainFile` (each of the file's materials drawn with the kit's texture of the same name, `kit/<material>.png`, one repeat
//! every [`GROUND_TILE`] m), the water from the top of its box (TGE's fluid sits at its box's top: `position` plus `scale`'s
//! height), with its `surfaceTexture` at its `surfaceOpacity` and the demo's own tint ([`WATER_TINT`]).
//!
//! Only a block's fields written in its `new` statement are read (`new TerrainBlock(Name) { terrainFile = "..."; ... };`), as
//! the maps here write them; a map without either has neither.

use torque::app::terrain::{LayerUv, Terrain};
use torque::app::view::{Water, WorldView};
use torque_example_common::KitAssets;

use crate::files::GameFiles;

/// One repeat of a terrain material's texture (m).
pub const GROUND_TILE: f32 = 12.0;
/// One repeat of the water's texture (m).
pub const WATER_TILE: f32 = 24.0;
/// The water's tint (gamma): a deep blue-green.
pub const WATER_TINT: [u8; 3] = [64, 112, 120];
/// The terrain's squares when its block names none (TGE's default).
pub const DEFAULT_SQUARE: i32 = 8;

/// A map's water: its surface's height, the rectangle it covers, its texture and its opacity.
#[derive(Debug, Clone, PartialEq)]
pub struct WaterSheet {
    pub level: f32,
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub texture: String,
    pub opacity: f32,
}

/// A map's terrain and water.
#[derive(Debug, Clone, Default)]
pub struct MapGround {
    pub terrain: Option<Terrain>,
    /// The terrain's materials' textures, in the file's order.
    pub layers: Vec<String>,
    pub water: Option<WaterSheet>,
}

impl MapGround {
    /// The ground of the map whose script is `text`, its files read from `files`.
    pub fn read(files: &GameFiles, text: &str) -> Result<MapGround, String> {
        let mut ground = MapGround::default();
        if let Some(block) = blocks(text, "TerrainBlock").into_iter().next() {
            let file = field(&block, "terrainFile").ok_or("a TerrainBlock names no terrainFile")?;
            let square = field(&block, "squareSize")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(DEFAULT_SQUARE);
            let bytes = torque_render_scene::Assets::read(&files.kit, &file)
                .ok_or_else(|| format!("{file} is not in the kit"))?;
            let terrain = Terrain::read(&bytes, square)?;
            ground.layers = terrain
                .materials()
                .iter()
                .map(|m| format!("kit/{m}.png"))
                .collect();
            ground.terrain = Some(terrain);
        }
        if let Some(block) = blocks(text, "WaterBlock").into_iter().next() {
            let p = numbers(&field(&block, "position").unwrap_or_default());
            let s = numbers(&field(&block, "scale").unwrap_or_default());
            let (p, s) = (
                [0, 1, 2].map(|k| p.get(k).copied().unwrap_or(0.0)),
                [0, 1, 2].map(|k| s.get(k).copied().unwrap_or(1.0)),
            );
            ground.water = Some(WaterSheet {
                level: p[2] + s[2],
                min: [p[0].min(p[0] + s[0]), p[1].min(p[1] + s[1])],
                max: [p[0].max(p[0] + s[0]), p[1].max(p[1] + s[1])],
                texture: field(&block, "surfaceTexture").unwrap_or_default(),
                opacity: field(&block, "surfaceOpacity")
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0.75),
            });
        }
        Ok(ground)
    }

    /// Whether the map has a terrain or water.
    pub fn is_empty(&self) -> bool {
        self.terrain.is_none() && self.water.is_none()
    }

    /// The ground drawn in `view` (or taken away when there is none).
    pub fn apply(&self, view: &mut WorldView, kit: &KitAssets) {
        view.clear_ground();
        if let Some(terrain) = &self.terrain {
            let layers: Vec<(&str, LayerUv<'_>)> = self
                .layers
                .iter()
                .map(|t| (t.as_str(), LayerUv::Tiled(GROUND_TILE)))
                .collect();
            view.set_terrain_layers(kit, terrain, &layers);
        }
        if let Some(w) = &self.water {
            let alpha = (w.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
            view.set_water(
                kit,
                &Water {
                    texture: &w.texture,
                    tile: WATER_TILE,
                    color: [WATER_TINT[0], WATER_TINT[1], WATER_TINT[2], alpha],
                    level: w.level,
                    min: w.min,
                    max: w.max,
                },
            );
        }
    }

    /// The ground's height at (x, y): the terrain's, if it covers the point.
    pub fn height(&self, x: f32, y: f32) -> Option<f32> {
        self.terrain.as_ref()?.height(x, y)
    }
}

/// The text between the braces of each `new <class>(...) { ... }` in `text`.
fn blocks(text: &str, class: &str) -> Vec<String> {
    let start = format!("new {class}(");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(&start) {
        rest = &rest[at + start.len()..];
        let (Some(open), Some(close)) = (rest.find('{'), rest.find('}')) else {
            break;
        };
        if open < close {
            out.push(rest[open + 1..close].to_string());
            rest = &rest[close + 1..];
        }
    }
    out
}

/// The value of the field `name = "value";` in a block's text.
fn field(block: &str, name: &str) -> Option<String> {
    block.split(';').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().trim_matches('"').to_string())
    })
}

fn numbers(text: &str) -> Vec<f32> {
    text.split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_s_fields_are_read_from_its_statement() {
        let text = "new SimGroup(MissionGroup) {\n   new WaterBlock(Lake) { position = \"-10 -20 0\"; scale = \"40 30 12\"; \
                    surfaceOpacity = 0.6; };\n};";
        let found = blocks(text, "WaterBlock");
        assert_eq!(found.len(), 1);
        assert_eq!(field(&found[0], "position").as_deref(), Some("-10 -20 0"));
        assert_eq!(field(&found[0], "surfaceopacity").as_deref(), Some("0.6"));
        assert_eq!(
            field(&found[0], "scale").map(|s| numbers(&s)),
            Some(vec![40.0, 30.0, 12.0])
        );
        assert!(blocks(text, "TerrainBlock").is_empty());
    }
}
