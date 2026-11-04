//! The sun's flare: TGE's `fxSunLight`, drawn twice.
//!
//! - **Where:** `fxSunLight::AnimateSun` takes the azimuth and elevation, turns them into a direction
//!   (`MathUtils::getVectorFromAngles`) and puts the flare at the control camera's position plus that direction times 0.999 of the
//!   scene's visible distance. Without animation the colour is the flare colour times its brightness, the size the flare size.
//! - **`renderObject`:** the billboard's half size is `distance / pixelScale / 2 * size` (`pixelScale` = viewport width / 640:
//!   1.25 at 800 x 600; the distance from the control object's eye), the colour `glColor3f(colour)` (a caller may dim it:
//!   [`draws`]' `facing`), a quad facing the camera with texture coordinates (0, 0) at its top left, `GL_MODULATE`, the blend
//!   mode's function (0: `SRC_ALPHA, ONE`; 1: `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`; 2: `ONE, ONE`), depth writes off.
//! - **The remote flare** (the light's `RemoteFlareBitmap`) is drawn with the depth test on: the opaque geometry drawn after it
//!   covers it. **The local flare** (`LocalFlareBitmap`, sorted last) is drawn with the depth test off, scaled by its fade
//!   (`TestLOS` from the control camera to the flare: grows by `elapsed / fadeTime` to 1 while nothing is in the way, falls as fast
//!   while something is). The fade is taken as settled ([`crate::sight`]): 1 when the ray is clear, 0 (no local flare) when it is
//!   blocked.
//!
//! Credit: follows TGE 1.4's `game/fx/fxSunLight.cc` and `math/mathUtils.cc`, Copyright (C) GarageGames.com, Inc.; as Torque3D
//! carries `getVectorFromAngles` under the MIT licence: `math/mathUtils.cpp`, Copyright (c) 2012 GarageGames, LLC. The code here
//! is this crate's own.

use crate::camera::Camera;
use crate::sight::{LineOfSight, Sight};
use crate::source::{FlareBlend, SceneSource, SunDesc};
use crate::textures::TextureSet;
use crate::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

/// `dglGetPixelScale`'s divisor (`viewport width / 640`), and the flare's distance as a share of the visible distance.
const PIXEL_SCALE_WIDTH: f32 = 640.0;
const DISTANCE: f32 = 0.999;

/// The two flares of the frame.
pub struct Flares {
    pub remote: Option<Draw>,
    pub local: Option<Draw>,
}

/// The sun's direction (towards it) from the Sun's azimuth (held to 0..359, as `AnimateSun` holds it) and elevation (degrees;
/// `elevation_override` in its place when a world moves its sun), as `getVectorFromAngles`.
pub fn sun_direction(src: &dyn SceneSource, elevation_override: Option<f32>) -> Option<[f32; 3]> {
    src.sun().map(|body| towards(&body, elevation_override))
}

/// [`sun_direction`] of the Sun `body`.
pub fn towards(body: &SunDesc, elevation_override: Option<f32>) -> [f32; 3] {
    let az = body.azimuth.clamp(0.0, 359.0);
    let el = elevation_override.unwrap_or(body.elevation);
    let (az, el) = (az.to_radians(), el.to_radians());
    [az.sin() * el.cos(), az.cos() * el.cos(), el.sin()]
}

/// The Sun's azimuth and elevation (degrees) as sent, None without a Sun.
pub fn sun_angles(src: &dyn SceneSource) -> Option<(f32, f32)> {
    src.sun().map(|b| (b.azimuth, b.elevation))
}

/// Whether the world has a sun flare to draw (an enabled `fxSunLight`).
pub fn has_flare(src: &dyn SceneSource) -> bool {
    src.sun_flare().is_some_and(|b| b.enabled)
}

/// Where the flares are drawn for an eye at `eye` towards the sun along `dir` (`AnimateSun`: the eye plus the sun's direction
/// times 0.999 of the visible distance).
pub fn flare_point(dir: [f32; 3], eye: [f32; 3], visible: f32) -> [f32; 3] {
    let reach = (f64::from(visible) * f64::from(DISTANCE)) as f32;
    [0, 1, 2].map(|k| dir[k] * reach + eye[k])
}

/// Whether the flare's size follows its colour's luminance (the light's link flag).
pub fn linked(src: &dyn SceneSource) -> bool {
    src.sun_flare().is_some_and(|b| b.linked)
}

/// The frame's flares, for the camera, the scene's visible distance, the sun's direction `dir` (towards it, [`sun_direction`])
/// and a flare colour a world sets over the light's own (`colour_override`, when its sun moves). `facing`: the share of the
/// colour drawn (TGE draws it whole: 1; a world may dim the flare as the view turns from it). The local flare is drawn only while
/// its line of sight (`sight`, [`Sight::Sun`]) is clear.
#[allow(clippy::too_many_arguments)]
pub fn draws(
    src: &dyn SceneSource,
    camera: &Camera,
    width: u32,
    visible: f32,
    dir: [f32; 3],
    colour_override: Option<[f32; 3]>,
    sight: &mut dyn LineOfSight,
    textures: &mut TextureSet<'_>,
    facing: f32,
) -> Flares {
    let none = Flares {
        remote: None,
        local: None,
    };
    let Some(body) = src.sun_flare() else {
        return none;
    };
    if !body.enabled {
        return none;
    }
    let eye = camera.eye();
    let pos = flare_point(dir, eye, visible);
    let d = [0, 1, 2].map(|k| pos[k] - eye[k]);
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let pixel_scale = width as f32 / PIXEL_SCALE_WIDTH;
    let (brightness, size) = (body.brightness, body.size);
    let half = (f64::from(dist / pixel_scale) * 0.5) as f32 * size;
    // A world's moving sun may write the flare colour itself; the brightness multiplies either.
    let flare =
        colour_override.unwrap_or_else(|| [0, 1, 2].map(|k| f32::from(body.color[k]) / 255.0));
    let base = flare.map(|c| c * brightness);
    let colour = base.map(|c| c * facing);
    let link = body.linked;
    let mut radius = half;
    if link {
        radius *= colour[0] * 0.212_671 + colour[1] * 0.715_16 + colour[2] * 0.072_169;
    }
    if radius <= 0.0 {
        return none;
    }
    let blend = match body.blend {
        FlareBlend::Alpha => Blend::Alpha,
        FlareBlend::One => Blend::One,
        FlareBlend::Additive => Blend::Additive,
    };
    let right = camera.transform.column(0);
    let up = camera.transform.column(2);
    let rgb = colour.map(|c| (f64::from(c.clamp(0.0, 1.0)) * 255.0).round() as u8);
    let quad = |r: f32| {
        let corner = |x: f32, z: f32, uv: [f32; 2]| Vertex {
            position: [0, 1, 2].map(|k| pos[k] + right[k] * x * r + up[k] * z * r),
            normal: [0.0, 0.0, 1.0],
            color: [rgb[0], rgb[1], rgb[2], 255],
            uv,
        };
        let q = [
            corner(-1.0, 1.0, [0.0, 0.0]),
            corner(1.0, 1.0, [1.0, 0.0]),
            corner(1.0, -1.0, [1.0, 1.0]),
            corner(-1.0, -1.0, [0.0, 1.0]),
        ];
        vec![q[0], q[1], q[2], q[0], q[2], q[3]]
    };
    let draw = |name: &str, label: &str, r: f32, textures: &mut TextureSet<'_>| -> Option<Draw> {
        let name = name.replace('\\', "/");
        let t = textures.get_modulated(&name)?;
        Some(Draw {
            label: format!("{label} {name}"),
            vertices: quad(r),
            lighting: false,
            tex_env: TexEnv::Modulate,
            texture: Some((
                t,
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
        })
    };
    let hidden = sight.blocked(eye, pos, Sight::Sun, None) == Some(true);
    Flares {
        remote: draw(&body.textures[1], "sun flare (remote)", radius, textures),
        local: if hidden {
            None
        } else {
            draw(&body.textures[0], "sun flare (local)", radius, textures)
        },
    }
}
