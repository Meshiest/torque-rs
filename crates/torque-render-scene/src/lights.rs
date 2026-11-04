//! Point lights and their coronas: TGE's `fxLight` as a frame draws its flare.
//!
//! **The light** (`fxLight::registerLights` over `AnimateLight`): at its position (an attached light at its object's), its colour
//! times its brightness, its radius. Which lights a world has, and where, is the world's: it hands them over as [`Light`]s.
//!
//! **The corona** (`fxLight::renderObject`, with `FlareOn` and the light enabled): a square facing the camera at the light, half
//! a side `size` across: `ConstantSize`, else `NearSize` within `NearDistance` of the eye, `FarSize` beyond `FarDistance`, the line
//! between them in between; times the fade (1 once the flare has been visible for `FadeTime` seconds; 0 once it was hidden as
//! long: [`crate::sight`], the line of sight), times the colour's luminance when `LinkFlareSize` is set. The flare texture
//! (`FlareBitmap`) in `GL_MODULATE` with `glColor3f` of the light's colour when `LinkFlare` is set, else `FlareColor`; lighting
//! off, depth test off, blended by `BlendMode` (0: `SRC_ALPHA, ONE`; 1: `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`; 2: `ONE, ONE`). Drawn
//! with the translucent objects. A world may draw them under its own limits ([`CoronaRules`]; [`Tge`] is TGE's).
//!
//! Credit: follows TGE 1.4's `game/fx/fxLight.cc`, Copyright (C) GarageGames.com, Inc.; the code here is this crate's own.

use crate::camera::Camera;
use crate::sight::{LineOfSight, Sight};
use crate::textures::TextureSet;
use crate::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

/// `LinkFlareSize`'s luminance weights.
const LUMA: [f32; 3] = [0.212_671, 0.715_16, 0.072_169];

/// A light as `registerLights` leaves it.
#[derive(Debug, Clone, PartialEq)]
pub struct Light {
    pub position: [f32; 3],
    /// The colour times the brightness.
    pub color: [f32; 3],
    pub radius: f32,
    pub flare: Option<Flare>,
    /// An object the corona's line of sight passes through (its ghost index: the one the light sits in; [`crate::sight`]).
    pub through: Option<u32>,
    /// The object the light is attached to (its ghost index; `attachToObject`).
    pub attached: Option<u32>,
}

/// A corona's datablock fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Flare {
    pub bitmap: String,
    pub color: [f32; 3],
    pub constant_size: Option<f32>,
    pub near_size: f32,
    pub far_size: f32,
    pub near_distance: f32,
    pub far_distance: f32,
    pub blend_mode: i64,
    pub link_flare: bool,
    pub link_flare_size: bool,
    /// The light's flare scale as its record sent it.
    pub scale: f32,
}

/// A corona's size before its fade (`ConstantSize`, else `NearSize` to `FarSize` over the distances).
fn corona_size(f: &Flare, dist: f32) -> f32 {
    match f.constant_size {
        Some(s) => s,
        None if dist <= f.near_distance => f.near_size,
        None if dist >= f.far_distance => f.far_size,
        None => {
            (f.far_size - f.near_size) / (f.far_distance - f.near_distance)
                * (dist - f.near_distance)
                + f.near_size
        }
    }
}

/// The limits a world draws its coronas under; each default is TGE's (a corona of any size at any distance, every line of sight
/// tested, a linked flare in the light's own colour).
pub trait CoronaRules {
    /// A corona this far from the eye or farther is not drawn.
    fn reach(&self) -> f32 {
        f32::INFINITY
    }

    /// A corona this size or smaller is not drawn.
    fn min_size(&self) -> f32 {
        0.0
    }

    /// The most lines of sight a frame tests (the nearest coronas'); one past it is not drawn.
    fn max_tests(&self) -> usize {
        usize::MAX
    }

    /// The tint of a `LinkFlare` corona of a light of colour `color` (TGE: the colour).
    fn linked_tint(&self, color: [f32; 3]) -> [f32; 3] {
        color
    }
}

/// TGE's coronas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tge;

impl CoronaRules for Tge {}

/// Whether a frame tests the corona of `l` from `eye` (one larger than [`CoronaRules::min_size`] within
/// [`CoronaRules::reach`]).
pub fn corona_tested(l: &Light, eye: [f32; 3], rules: &dyn CoronaRules) -> bool {
    let Some(f) = &l.flare else { return false };
    let d = [0, 1, 2].map(|k| l.position[k] - eye[k]);
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    corona_size(f, dist) > rules.min_size() && dist < rules.reach()
}

/// The lights whose coronas a frame tests, by index in `lights`: of those within reach ([`corona_tested`]), the nearest
/// [`CoronaRules::max_tests`]. Only a tested corona whose line of sight is clear is drawn: one past the cap, or whose test the
/// world did not make, is not.
pub fn tested_coronas(lights: &[Light], eye: [f32; 3], rules: &dyn CoronaRules) -> Vec<usize> {
    let mut near: Vec<(f32, usize)> = lights
        .iter()
        .enumerate()
        .filter(|(_, l)| corona_tested(l, eye, rules))
        .map(|(i, l)| {
            let d = [0, 1, 2].map(|k| l.position[k] - eye[k]);
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2], i)
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    near.truncate(rules.max_tests());
    let mut out: Vec<usize> = near.into_iter().map(|(_, i)| i).collect();
    out.sort_unstable();
    out
}

/// The coronas of `lights`, each a draw at its eye distance (for the back-to-front pass). A corona whose line of sight from the eye
/// ([`Sight::Corona`], through [`Light::through`]) is blocked has faded out and is not drawn (`fxLight::TestLOS`).
pub fn coronas(
    lights: &[Light],
    camera: &Camera,
    sight: &mut dyn LineOfSight,
    rules: &dyn CoronaRules,
    textures: &mut TextureSet<'_>,
) -> Vec<(f32, Draw)> {
    let eye = camera.eye();
    let right = camera.transform.column(0);
    let up = camera.transform.column(2);
    let mut out = Vec::new();
    let tested = tested_coronas(lights, eye, rules);
    let (min_size, reach) = (rules.min_size(), rules.reach());
    for (i, l) in lights.iter().enumerate() {
        let Some(f) = &l.flare else {
            continue;
        };
        let d = [0, 1, 2].map(|k| l.position[k] - eye[k]);
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let mut size = corona_size(f, dist);
        if size <= min_size || dist >= reach {
            continue;
        }
        // The nearest `max_tests` lines of sight a frame; an untested corona is not drawn.
        if tested.binary_search(&i).is_err()
            || sight.blocked(eye, l.position, Sight::Corona, l.through) != Some(false)
        {
            continue;
        }
        if f.link_flare_size {
            let c = l.color;
            size *= (f64::from(c[0]) * f64::from(LUMA[0])
                + f64::from(c[1]) * f64::from(LUMA[1])
                + f64::from(c[2]) * f64::from(LUMA[2])) as f32;
        }
        let tint = if f.link_flare {
            rules.linked_tint(l.color)
        } else {
            f.color
        };
        let Some(texture) = textures.get(&f.bitmap) else {
            continue;
        };
        let blend = match f.blend_mode {
            1 => Blend::Alpha,
            2 => Blend::One,
            _ => Blend::Additive,
        };
        let corner = |x: f32, z: f32, uv: [f32; 2]| Vertex {
            position: [0, 1, 2].map(|k| l.position[k] + right[k] * x * size + up[k] * z * size),
            normal: [0.0, 0.0, 1.0],
            color: [
                (tint[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (tint[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (tint[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                255,
            ],
            uv,
        };
        let q = [
            corner(-1.0, 1.0, [0.0, 0.0]),
            corner(1.0, 1.0, [1.0, 0.0]),
            corner(1.0, -1.0, [1.0, 1.0]),
            corner(-1.0, -1.0, [0.0, 1.0]),
        ];
        out.push((
            dist,
            Draw {
                label: format!("corona {}", f.bitmap),
                vertices: vec![q[0], q[1], q[2], q[0], q[2], q[3]],
                lighting: false,
                tex_env: TexEnv::Modulate,
                texture: Some((
                    texture,
                    Sampler {
                        repeat: false,
                        mag: Filter::Linear,
                        min: Filter::Linear,
                        mip: Filter::Linear,
                        mipmaps: true,
                    },
                )),
                blend,
                cull: Cull::None,
                depth_test: false,
                depth_write: false,
                fog: false,
                lights: Vec::new(),
                vertex_key: None,
            },
        ));
    }
    out
}

#[cfg(test)]
#[path = "lights_tests.rs"]
pub(crate) mod tests;
