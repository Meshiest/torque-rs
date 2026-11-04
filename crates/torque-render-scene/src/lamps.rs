//! Lamps: point lights a host adds where a static shape is one of its lamp shapes ([`Lamp`]), and the map surfaces that glow
//! under them. A shape's own light is drawn only in its lightmaps and a self-illuminating material; a renderer that lights every
//! pixel from the frame's point lights (and leaves the lightmaps out) gets the light back here:
//!
//! - **the bulb's light** ([`lights`]): a point light near the top of the shape ([`Lamp::lift`] over its centre), which may
//!   cast shadows;
//! - **the shade's light** ([`shade_lights`]): a dimmer, wider light in the middle that casts none (the light through a shade);
//! - **glowing surfaces** ([`glow_of`]): a map draw whose texture is one of the host's glowing ones draws unlit in a tint.
//!
//! The tables are the host's: this crate has no lamps of its own.

use crate::mounts::pose_matrix;
use crate::world::{ShapeKind, ShapeWorld};

/// A lamp: the shape file it is the lamp of (its path's end, any case), its colour (gamma), the bulb light's brightness, radius
/// and height over the shape's centre (units), and the shade light's brightness, radius and height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lamp {
    pub shape: &'static str,
    pub color: [f32; 3],
    pub brightness: f32,
    pub radius: f32,
    pub lift: f32,
    pub shade_brightness: f32,
    pub shade_radius: f32,
    pub shade_lift: f32,
}

/// A lamp's light: where it is, its colour times its brightness, its radius and the ghost it belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LampLight {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub radius: f32,
    pub ghost: u32,
}

/// The world's lamps: each static shape whose shape is one of `lamps`, its ghost index, its lamp and its centre, in ghost order.
pub fn bulbs<'l, W: ShapeWorld + ?Sized>(
    w: &W,
    lamps: &'l [Lamp],
) -> Vec<(u32, &'l Lamp, [f32; 3])> {
    let mut out = Vec::new();
    if lamps.is_empty() {
        return out;
    }
    let world = w.records();
    let statics = w
        .shape_classes()
        .into_iter()
        .filter(|c| w.kind(*c) == ShapeKind::Static);
    for (index, ghost) in statics.flat_map(|c| world.of_class(c)) {
        let Some(db) = ghost.datablock() else {
            continue;
        };
        let Some(file) = world.datablock_text(db, "shapeFile") else {
            continue;
        };
        let file = String::from_utf8_lossy(file)
            .replace('\\', "/")
            .to_ascii_lowercase();
        let Some(lamp) = lamps.iter().find(|l| file.ends_with(l.shape)) else {
            continue;
        };
        let Some((m, _)) = ghost.pose().and_then(|p| pose_matrix(&p, ghost.scale())) else {
            continue;
        };
        out.push((index, lamp, m.mul_p([0.0; 3])));
    }
    out
}

/// The bulbs' lights (they may cast shadows), in ghost order.
pub fn lights<W: ShapeWorld + ?Sized>(w: &W, lamps: &[Lamp]) -> Vec<LampLight> {
    bulbs(w, lamps)
        .into_iter()
        .map(|(ghost, lamp, c)| LampLight {
            position: [c[0], c[1], c[2] + lamp.lift],
            color: lamp.color.map(|v| v * lamp.brightness),
            radius: lamp.radius,
            ghost,
        })
        .collect()
}

/// The shades' lights (they cast no shadows), in ghost order.
pub fn shade_lights<W: ShapeWorld + ?Sized>(w: &W, lamps: &[Lamp]) -> Vec<LampLight> {
    bulbs(w, lamps)
        .into_iter()
        .map(|(ghost, lamp, c)| LampLight {
            position: [c[0], c[1], c[2] + lamp.shade_lift],
            color: lamp.color.map(|v| v * lamp.shade_brightness),
            radius: lamp.shade_radius,
            ghost,
        })
        .collect()
}

/// The tint a map draw of label `label` glows in, when its texture (the file name's stem, any case) is one of `glowing`.
pub fn glow_of(label: &str, glowing: &[(&str, [u8; 4])]) -> Option<[u8; 4]> {
    let name = label.strip_suffix(" lit").unwrap_or(label);
    let file = name.rsplit(['/', ' ']).next().unwrap_or(name);
    let stem = file.split('.').next().unwrap_or(file);
    glowing
        .iter()
        .find(|(t, _)| stem.eq_ignore_ascii_case(t))
        .map(|(_, c)| *c)
}

#[cfg(test)]
#[path = "lamps_tests.rs"]
mod tests;
