//! How a model's materials are drawn, carried into the `.dif` as this engine's surface flags
//! ([`torque_maps::dif::surface_flags::TRANSLUCENT`] and [`torque_maps::dif::surface_flags::NEAREST`]): every surface of a
//! marked material carries its bits, so a reader finds them whichever surface it reads.
//!
//! - **Nearest:** a material that asks for nearest-neighbor filtering ([`Material::nearest`]: a Blockbench model's textures,
//!   a glTF sampler's `NEAREST`).
//! - **Translucent:** a material blended with what is behind it ([`Alpha::Blend`]), or one whose PNG texture has partly
//!   see-through texels (alpha above [`CLEAR_ALPHA`] and below [`SOLID_ALPHA`]), as Blockbench blends them: glass. A texture
//!   whose texels are all solid or all clear (a cut-out) stays opaque. Only textures of at most [`MAX_SCAN_SIDE`] texels a side
//!   are looked at; the decoder caps its own work.
//!
//! The flags change only how the surfaces are drawn: the hulls, and so the collision, are the same.

use std::collections::BTreeMap;

use torque_maps::dif::{Dif, surface_flags};
use torque_modelcore::{Alpha, Material, Model};

/// A texel's alpha at or below this is clear, at or above [`SOLID_ALPHA`] solid; between, partly see-through.
pub const CLEAR_ALPHA: u8 = 4;
pub const SOLID_ALPHA: u8 = 251;
/// The widest and tallest texture looked at for partly see-through texels.
pub const MAX_SCAN_SIDE: u32 = 1_024;

/// The surface flags each of `model`'s materials asks for, by its name (the name its surfaces' texture has); materials that
/// ask for none are left out.
pub fn surface_flags(model: &Model) -> BTreeMap<String, u8> {
    let mut out = BTreeMap::new();
    for m in &model.materials {
        let mut flags = 0;
        if m.nearest {
            flags |= surface_flags::NEAREST;
        }
        if translucent(model, m) {
            flags |= surface_flags::TRANSLUCENT;
        }
        if flags != 0 {
            *out.entry(m.name.clone()).or_insert(0) |= flags;
        }
    }
    out
}

/// Sets `flags` (by material name) on every drawn surface of every interior of `dif`.
pub fn mark(dif: &mut Dif, flags: &BTreeMap<String, u8>) {
    if flags.is_empty() {
        return;
    }
    for interior in dif.details.iter_mut().chain(dif.sub_objects.iter_mut()) {
        let bits: Vec<u8> = interior
            .materials
            .iter()
            .map(|name| flags.get(name).copied().unwrap_or(0))
            .collect();
        for s in &mut interior.surfaces {
            s.flags |= bits.get(s.texture as usize).copied().unwrap_or(0);
        }
    }
}

fn translucent(model: &Model, m: &Material) -> bool {
    if m.alpha == Alpha::Blend {
        return true;
    }
    if matches!(m.alpha, Alpha::Mask(_)) {
        return false;
    }
    let Some(data) = m
        .base_texture
        .and_then(|t| model.images.get(t))
        .and_then(|i| i.data.as_deref())
    else {
        return false;
    };
    let Some((w, h)) = torque_images::png::header_size(data) else {
        return false;
    };
    if w > MAX_SCAN_SIDE || h > MAX_SCAN_SIDE {
        return false;
    }
    let Ok(bitmap) = torque_images::png::decode(data) else {
        return false;
    };
    bitmap
        .to_rgba8()
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[3] > CLEAR_ALPHA && p[3] < SOLID_ALPHA)
}

#[cfg(test)]
mod tests {
    use super::*;
    use torque_modelcore::Image;

    /// A 2 x 1 RGBA PNG of `alphas`.
    fn png(alphas: [u8; 2]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 2, 1);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut w = e.write_header().unwrap();
        w.write_image_data(&[200, 220, 230, alphas[0], 200, 220, 230, alphas[1]])
            .unwrap();
        w.finish().unwrap();
        out
    }

    fn model(textures: &[(&str, [u8; 2], bool)]) -> Model {
        let mut m = Model::default();
        for (name, alphas, nearest) in textures {
            m.images.push(Image {
                name: name.to_string(),
                data: Some(png(*alphas)),
                mime: "image/png".into(),
                ..Image::default()
            });
            m.materials.push(Material {
                name: name.to_string(),
                base_texture: Some(m.images.len() - 1),
                nearest: *nearest,
                ..Material::default()
            });
        }
        m
    }

    #[test]
    fn partly_see_through_textures_are_translucent_and_nearest_is_kept() {
        let m = model(&[
            ("glass", [90, 120], true),
            ("cutout", [0, 255], true),
            ("wall", [255, 255], false),
        ]);
        let flags = surface_flags(&m);
        assert_eq!(
            flags.get("glass"),
            Some(&(surface_flags::TRANSLUCENT | surface_flags::NEAREST))
        );
        assert_eq!(flags.get("cutout"), Some(&surface_flags::NEAREST));
        assert_eq!(flags.get("wall"), None);
    }

    #[test]
    fn a_blended_material_is_translucent_without_a_texture() {
        let mut m = Model::default();
        m.materials.push(Material {
            name: "tint".into(),
            alpha: Alpha::Blend,
            ..Material::default()
        });
        assert_eq!(
            surface_flags(&m).get("tint"),
            Some(&surface_flags::TRANSLUCENT)
        );
    }
}
