//! A map's ground as the client draws it: its `TerrainBlock` and its `WaterBlock`. The client is sent neither (their classes
//! have no ghost here), so it reads them from the map the server runs, as the server does: the terrain from its
//! `terrainFile` (each of the file's materials drawn with the kit's texture of the same name, `kit/<material>.png`, one repeat
//! every [`GROUND_TILE`] m), the water from the top of its box (TGE's fluid sits at its box's top: `position` plus `scale`'s
//! height), with its `surfaceTexture` at its `surfaceOpacity` and the demo's own tint ([`WATER_TINT`]).
//!
//! A map whose `MapInfo` says `groundVariation = 1` has its ground's textures varied against visible tiling (the renderer's
//! [`WorldView::set_ground_variation`]); the others are drawn as before. One whose `MapInfo` gives a `cloudTimeScale` has its
//! volumetric clouds run that many times as fast as the clock (a timelapse: their drift, change of shape and shadows,
//! [`WorldView::set_cloud_time`]).
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
/// The steps a line is looked along for the ground ([`MapGround::line_share`]).
pub const LINE_STEPS: usize = 32;
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
    /// Whether the ground's textures are varied against visible tiling.
    pub variation: bool,
    /// How many times as fast as the clock the clouds run (0: on the effect time, as every other map's).
    pub cloud_time_scale: f32,
}

impl MapGround {
    /// The ground of the map whose script is `text`, its files read from `files`.
    pub fn read(files: &GameFiles, text: &str) -> Result<MapGround, String> {
        let mut ground = MapGround {
            variation: blocks(text, "ScriptObject").iter().any(|b| {
                field(b, "groundVariation").is_some_and(|v| v.trim() != "0" && !v.trim().is_empty())
            }),
            cloud_time_scale: cloud_time_scale(text),
            water: water_of(text),
            ..MapGround::default()
        };
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
        Ok(ground)
    }

    /// Whether the map has a terrain or water.
    pub fn is_empty(&self) -> bool {
        self.terrain.is_none() && self.water.is_none()
    }

    /// The ground drawn in `view` (or taken away when there is none).
    pub fn apply(&self, view: &mut WorldView, kit: &KitAssets) {
        view.clear_ground();
        view.set_ground_variation(self.variation);
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

    /// The height a camera keeps above at (x, y): the terrain's or the water's surface over it, whichever is higher.
    pub fn surface(&self, x: f32, y: f32) -> Option<f32> {
        let water = self
            .water
            .as_ref()
            .filter(|w| (w.min[0]..=w.max[0]).contains(&x) && (w.min[1]..=w.max[1]).contains(&y))
            .map(|w| w.level);
        match (self.height(x, y), water) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    /// The share of the way from `from` to `to` at which the line first goes under the [`Self::surface`], if it does: found
    /// in [`LINE_STEPS`] steps and narrowed between the last two. None for a line that starts under it (the figure lying on
    /// the ground) until it comes out.
    pub fn line_share(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
        if self.is_empty() {
            return None;
        }
        let at = |t: f32| [0, 1, 2].map(|k| from[k] + (to[k] - from[k]) * t);
        let under = |t: f32| {
            let p = at(t);
            self.surface(p[0], p[1]).is_some_and(|h| p[2] < h)
        };
        if under(0.0) {
            return None;
        }
        let mut last = 0.0;
        for i in 1..=LINE_STEPS {
            let t = i as f32 / LINE_STEPS as f32;
            if under(t) {
                let (mut lo, mut hi) = (last, t);
                for _ in 0..8 {
                    let mid = (lo + hi) * 0.5;
                    if under(mid) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                return Some(lo);
            }
            last = t;
        }
        None
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

/// The numbers of a field (`"1 2 3"`); None when one is not a number or not finite (a map file is not trusted: `nan` and `inf`
/// parse as numbers).
fn numbers(text: &str) -> Option<Vec<f32>> {
    text.split_whitespace()
        .map(|w| w.parse::<f32>().ok().filter(|v| v.is_finite()))
        .collect()
}

/// One finite number of a field.
fn number(text: &str) -> Option<f32> {
    text.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

/// The most times as fast as the clock a map's clouds may run.
pub const MAX_CLOUD_TIME_SCALE: f32 = 1_000.0;

/// The map's `MapInfo` `cloudTimeScale`, held to `0..=`[`MAX_CLOUD_TIME_SCALE`] (0: none, or not a finite number).
pub fn cloud_time_scale(text: &str) -> f32 {
    blocks(text, "ScriptObject")
        .iter()
        .find_map(|b| number(&field(b, "cloudTimeScale")?))
        .filter(|s| *s > 0.0)
        .map_or(0.0, |s| s.min(MAX_CLOUD_TIME_SCALE))
}

/// The map's first `WaterBlock` as a sheet of water: the top of its box. None without one, or when its position, scale or
/// opacity is not finite numbers (a map file is not trusted).
pub fn water_of(text: &str) -> Option<WaterSheet> {
    let block = blocks(text, "WaterBlock").into_iter().next()?;
    let p = numbers(&field(&block, "position").unwrap_or_default())?;
    let s = numbers(&field(&block, "scale").unwrap_or_default())?;
    let (p, s) = (
        [0, 1, 2].map(|k| p.get(k).copied().unwrap_or(0.0)),
        [0, 1, 2].map(|k| s.get(k).copied().unwrap_or(1.0)),
    );
    let opacity = match field(&block, "surfaceOpacity") {
        Some(v) => number(&v)?,
        None => 0.75,
    };
    let sheet = WaterSheet {
        level: p[2] + s[2],
        min: [p[0].min(p[0] + s[0]), p[1].min(p[1] + s[1])],
        max: [p[0].max(p[0] + s[0]), p[1].max(p[1] + s[1])],
        texture: field(&block, "surfaceTexture").unwrap_or_default(),
        opacity: opacity.clamp(0.0, 1.0),
    };
    // Sums of finite numbers can still overflow to infinity.
    let finite = [
        sheet.level,
        sheet.min[0],
        sheet.min[1],
        sheet.max[0],
        sheet.max[1],
    ]
    .iter()
    .all(|v| v.is_finite());
    finite.then_some(sheet)
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
            field(&found[0], "scale").and_then(|s| numbers(&s)),
            Some(vec![40.0, 30.0, 12.0])
        );
        assert!(blocks(text, "TerrainBlock").is_empty());
    }

    /// A hostile valley: not-a-number and infinite fields leave no water and no cloud timelapse, and a huge cloud speed is
    /// held to its cap.
    #[test]
    fn a_hostile_map_s_numbers_are_refused() {
        let map = |water: &str, info: &str| {
            format!(
                "new SimGroup(MissionGroup) {{\n   new ScriptObject(MapInfo) {{ title = \"Valley\"; {info} }};\n   \
                 new WaterBlock(Lake) {{ {water} }};\n}};"
            )
        };
        let good = map(
            "position = \"-10 -20 0\"; scale = \"40 30 12\"; surfaceOpacity = 0.6;",
            "cloudTimeScale = 150;",
        );
        let w = water_of(&good).expect("water");
        assert_eq!(
            (w.level, w.min, w.max, w.opacity),
            (12.0, [-10.0, -20.0], [30.0, 10.0], 0.6)
        );
        assert_eq!(cloud_time_scale(&good), 150.0);
        for water in [
            "position = \"nan 0 0\"; scale = \"40 30 12\";",
            "position = \"0 0 0\"; scale = \"inf 30 12\";",
            "position = \"0 0 -inf\"; scale = \"40 30 12\";",
            "position = \"0 0 0\"; scale = \"40 30 12\"; surfaceOpacity = NaN;",
            "position = \"3e38 0 0\"; scale = \"3e38 30 12\";",
        ] {
            assert_eq!(water_of(&map(water, "")), None, "{water}");
        }
        for (info, scale) in [
            ("cloudTimeScale = nan;", 0.0),
            ("cloudTimeScale = inf;", 0.0),
            ("cloudTimeScale = -5;", 0.0),
            ("cloudTimeScale = 1e30;", MAX_CLOUD_TIME_SCALE),
        ] {
            assert_eq!(cloud_time_scale(&map("", info)), scale, "{info}");
        }
    }
}
