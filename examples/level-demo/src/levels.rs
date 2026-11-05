//! Maps authored as Blockbench models: each `levels/<name>.bbmodel` of the game folder is built into interiors when the
//! example starts (`torque-import`'s reader, `torque-interior-builder`'s mesh input), written into the kit as
//! `kit/<name>.dif` (or `kit/<name>_1.dif`, ... when it is cut into pieces) with its textures, and its map,
//! `missions/<name>.mis`, made from `levels/<name>.mis` with the line `%interiors%` replaced by the interiors' instances.
//!
//! The model's textures are drawn as Blockbench draws them: sampled nearest, a texture with partly see-through texels
//! see-through (the builder's material flags, [`torque_interior_builder::materials`]). A texture whose name ends in `_glow`
//! is drawn unlit ([`LevelStyle`]: a ceiling light's panel).

use torque::app::ghosts::GhostStyle;
use torque_import::{Options, bbmodel};
use torque_interior_builder::materials;
use torque_interior_builder::mesh::{MeshInput, MeshOptions, build_mesh};
use torque_interior_builder::mission::{Placement, fragment};
use torque_render_scene::Draw;

use crate::files::GameFiles;

/// The line of a level's map the interiors take the place of.
pub const INTERIORS: &str = "%interiors%";

/// The end of the name of a texture drawn unlit.
pub const GLOW: &str = "_glow";

/// The levels' touch on the interiors' draws: a `_glow` material is drawn unlit (module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LevelStyle;

impl GhostStyle for LevelStyle {
    fn interior_draw(&mut self, material: &str, draw: &mut Draw) {
        if material.ends_with(GLOW) {
            draw.lighting = false;
        }
    }
}

/// Builds every level of `files` (module docs).
pub fn add_levels(files: &mut GameFiles) -> Result<(), String> {
    let levels: Vec<(String, Vec<u8>)> = files
        .folder
        .iter()
        .filter_map(|(p, b)| {
            let name = p.strip_prefix("levels/")?.strip_suffix(".bbmodel")?;
            Some((name.to_string(), b.to_vec()))
        })
        .collect();
    for (name, bytes) in levels {
        let Some(template) = files.file(&format!("levels/{name}.mis")) else {
            return Err(format!("levels/{name}.bbmodel has no levels/{name}.mis"));
        };
        let template = String::from_utf8_lossy(template).into_owned();
        let read = Options {
            box_hull: false,
            ..Options::default()
        };
        let model =
            bbmodel::read(&bytes, &read, &mut |_| true).map_err(|e| format!("{name}: {e}"))?;
        let input = MeshInput::from_model(&model).map_err(|e| format!("{name}: {e}"))?;
        let mut built =
            build_mesh(&input, &MeshOptions::default()).map_err(|e| format!("{name}: {e}"))?;
        let flags = materials::surface_flags(&model);
        for piece in &mut built.pieces {
            materials::mark(&mut piece.built.dif, &flags);
        }
        let mut instances = String::new();
        let count = built.pieces.len();
        for (k, piece) in built.pieces.iter().enumerate() {
            let file = if count == 1 {
                format!("{name}.dif")
            } else {
                format!("{name}_{}.dif", k + 1)
            };
            let bytes = piece.built.bytes().map_err(|e| format!("{name}: {e}"))?;
            files.kit.insert(&file, bytes);
            let objects = if k == 0 { &input.objects[..] } else { &[] };
            instances.push_str(&fragment(
                "",
                &format!("kit/{file}"),
                &Placement::default(),
                objects,
            ));
        }
        for image in &model.images {
            if let Some(data) = &image.data {
                files
                    .kit
                    .insert(&format!("{}.png", image.name), data.clone());
            }
        }
        files.insert(
            &format!("missions/{name}.mis"),
            template.replace(INTERIORS, &instances).as_bytes(),
        );
    }
    Ok(())
}
