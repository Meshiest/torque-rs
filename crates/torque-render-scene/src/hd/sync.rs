//! An enhanced frame brought up to a world's sky, sun, planes, lights and coronas ([`HdSource`], a [`SceneSource`] with the
//! points where a world draws other than TGE): the fixed-function frame's eye-dependent draws are made for the eye
//! ([`eye_draws`]: the Sky's box, clouds and bans, the sun's two flares, the world's planes, the lights' coronas), and then turned
//! into the enhanced frame's own ([`HdScene::set_eye_draws`]): the sky's irradiance as SH, the volumetric clouds' settings, the sun
//! the box paints, the sky's draws, the planes and water, the coronas and the flare as billboards. The static geometry and the
//! moving shapes are synced apart (the provider's cells, [`crate::hd::objects`]).
//!
//! Credit: the flares and coronas follow TGE 1.4's `game/fx/fxSunLight.cc` and `game/fx/fxLight.cc`, the sky
//! `terrain/sky.cc`, Copyright (C) GarageGames.com, Inc.; the code here is this crate's own.

use crate::camera::Camera;
use crate::hd::{
    HdDraw, HdLight, HdScene, HdVertex, clouds, material, oct_encode, sh, sky, srgb_to_linear,
};
use crate::lights::{CoronaRules, Light};
use crate::sight::LineOfSight;
use crate::sky::{Multipliers, SkyProfile};
use crate::source::SceneSource;
use crate::textures::TextureSet;
use crate::{Assets, Blend, Cull, Draw, Texture};

/// A sun a world moves itself (a cycle of the day's colours): its light, the Sky's multipliers and the flare's colour now, and its
/// elevation (degrees) in place of the Sun's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovingSun {
    pub direct: [f32; 3],
    pub ambient: [f32; 3],
    pub shadow: [f32; 3],
    pub fog: [f32; 3],
    pub sky: [f32; 3],
    pub flare: [f32; 3],
    pub elevation: f32,
}

/// The world an enhanced frame's sky, sun, planes, lights and coronas are made from: its [`SceneSource`] and the points where a
/// world draws other than TGE. Every default is TGE's.
pub trait HdSource: SceneSource {
    /// How the Sky is drawn ([`SkyProfile::TGE`]).
    fn sky_profile(&self) -> SkyProfile {
        SkyProfile::TGE
    }

    /// The sun when the world moves it itself (None: the Sun's own colours and elevation).
    fn moving_sun(&self) -> Option<MovingSun> {
        None
    }

    /// Towards the sun from the Sun's angles, at `elevation` in place of its own when given (TGE's `getVectorFromAngles`:
    /// [`crate::flare::towards`]).
    fn sun_towards(&self, elevation: Option<f32>) -> Option<[f32; 3]> {
        self.sun()
            .map(|body| crate::flare::towards(&body, elevation))
    }

    /// The planes the frame draws for `camera` out to `visible` (each with whether it is blended: water), their textures from
    /// `textures`. TGE has none.
    fn planes(
        &self,
        _camera: &Camera,
        _visible: f32,
        _textures: &mut TextureSet<'_>,
        _notes: &mut Vec<String>,
    ) -> Vec<(bool, Draw)> {
        Vec::new()
    }
}

/// The fixed-function frame's eye-dependent draws an enhanced sync turns into its own ([`eye_draws`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EyeDraws {
    /// The eye they were made for.
    pub eye: [f32; 3],
    /// The Sky's clear (its fog-colour quad), None without a Sky.
    pub clear: Option<[f32; 3]>,
    /// The sky in TGE's order: the box, the remote flare, the clouds and the bans.
    pub sky: Vec<Draw>,
    /// The sun's local flare (None while its line of sight is blocked), and whether its size follows its colour.
    pub local_flare: Option<Draw>,
    pub flare_linked: bool,
    /// The planes, each with whether it is blended (water).
    pub planes: Vec<(bool, Draw)>,
    /// The coronas, each at its eye distance.
    pub coronas: Vec<(f32, Draw)>,
}

/// What a look decides of an enhanced frame's sky ([`HdScene::set_eye_draws`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SkyLook {
    /// The look draws the engine's own sky where the frame's [`sky::SkyChoice`] allows it.
    pub own_sky: bool,
    /// The Sky's box is the one the world counts as its default (an [`sky::SkyChoice::Auto`] choice draws the engine's own for it).
    pub default_sky: bool,
    /// The world moves its sun itself ([`HdSource::moving_sun`]): no painted sun is searched for on a box that stays put.
    pub sun_moves: bool,
}

/// The eye-dependent draws for `camera` (the fixed-function frame's): the Sky's out to `sky_distance` (under the multipliers of
/// the sun when the world moves it, [`HdSource::moving_sun`]), the sun's flares at `visible` (facing the camera whole: the enhanced
/// frame dims nothing), the world's planes and the coronas of `lights`; the lines of sight through `sight`, the coronas under
/// `rules`.
#[allow(clippy::too_many_arguments)]
pub fn eye_draws<S: HdSource>(
    src: &S,
    assets: &dyn Assets,
    camera: &Camera,
    width: u32,
    visible: f32,
    sky_distance: f32,
    lights: &[Light],
    sight: &mut dyn LineOfSight,
    rules: &dyn CoronaRules,
    textures: &mut TextureSet<'_>,
    notes: &mut Vec<String>,
) -> EyeDraws {
    let sun = src.moving_sun();
    let mul = match &sun {
        Some(d) => Multipliers {
            fog: d.fog,
            sky: d.sky,
        },
        None => Multipliers {
            fog: [1.0; 3],
            sky: crate::sky::sky_color(src),
        },
    };
    let eye = camera.eye();
    let sky = crate::sky::draws(
        &src.sky_profile(),
        src,
        assets,
        eye,
        sky_distance,
        mul,
        textures,
        notes,
    );
    let clear = sky.as_ref().map(|s| s.clear);
    let flares = match src.sun_towards(sun.map(|d| d.elevation)) {
        Some(dir) => crate::flare::draws(
            src,
            camera,
            width,
            visible,
            dir,
            sun.map(|d| d.flare),
            sight,
            textures,
            1.0,
        ),
        None => crate::flare::Flares {
            remote: None,
            local: None,
        },
    };
    let mut sky_draws = Vec::new();
    if let Some(s) = sky {
        sky_draws.extend(s.boxes);
        sky_draws.extend(flares.remote);
        sky_draws.extend(s.after_flare);
    }
    let planes = src.planes(camera, visible, textures, notes);
    let coronas = crate::lights::coronas(lights, camera, sight, rules, textures);
    EyeDraws {
        eye,
        clear,
        sky: sky_draws,
        local_flare: flares.local,
        flare_linked: crate::flare::linked(src),
        planes,
        coronas,
    }
}

/// The coronas as anchored billboards ([`anchored_billboard`]), each at its eye distance, their textures through `local`.
pub fn anchored_coronas(coronas: &[(f32, Draw)], local: &[usize]) -> Vec<(f32, HdDraw)> {
    coronas
        .iter()
        .map(|(dist, d)| {
            (
                *dist,
                anchored_billboard(
                    HdDraw::from_fixed(d, material::KIND_SKY, local)
                        .with_material(material::EMITTER),
                ),
            )
        })
        .collect()
}

/// A flare quad as a [`material::BILLBOARD`] (built for the eye at `eye` facing its camera, its colour at full facing; a
/// window syncs its scene when the world changes, not as the camera turns). `linked`: its size follows its colour. `towards`: the
/// direction it is moved to at the same distance from the eye (the sun the box paints, [`HdScene::painted_sun`]).
pub fn billboard(mut d: HdDraw, eye: [f32; 3], linked: bool, towards: Option<[f32; 3]>) -> HdDraw {
    if d.vertices.is_empty() {
        return d;
    }
    let n = d.vertices.len() as f32;
    let c = [0, 1, 2].map(|k| d.vertices.iter().map(|v| v.position[k]).sum::<f32>() / n);
    let o = [0, 1, 2].map(|k| d.vertices[0].position[k] - c[k]);
    let half = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt() / std::f32::consts::SQRT_2;
    let half = if linked { -half } else { half };
    let mut at = [0, 1, 2].map(|k| c[k] - eye[k]);
    if let Some(t) = towards {
        let far = (at[0] * at[0] + at[1] * at[1] + at[2] * at[2]).sqrt();
        at = t.map(|v| v * far);
    }
    for v in &mut d.vertices {
        v.position = at;
        v.extra = half.to_bits();
        v.material |= material::BILLBOARD;
    }
    d
}

/// A corona as [`HdScene::coronas`] holds one: an additive, untextured emitter quad of half side `half` at `at`, tinted `tint`
/// (a scene made without a world's lights).
pub fn corona_draw(at: [f32; 3], half: f32, tint: [u8; 3]) -> HdDraw {
    let corner = |x: f32, z: f32, uv: [f32; 2]| HdVertex {
        position: [at[0] + x * half, at[1], at[2] + z * half],
        normal: oct_encode([0.0, -1.0, 0.0]),
        uv,
        color: [tint[0], tint[1], tint[2], 255],
        material: (material::KIND_SKY << material::KIND_SHIFT)
            | material::UNLIT
            | material::EMITTER,
        extra: 0,
    };
    anchored_billboard(HdDraw {
        label: "corona".into(),
        vertices: vec![
            corner(-1.0, 1.0, [0.0, 0.0]),
            corner(1.0, 1.0, [1.0, 0.0]),
            corner(1.0, -1.0, [1.0, 1.0]),
            corner(-1.0, -1.0, [0.0, 1.0]),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        texture: None,
        blend: Blend::Additive,
        cull: Cull::None,
        depth_test: false,
        depth_write: false,
    })
}

/// A light's corona as an anchored billboard ([`material::ANCHORED`]): faces the camera of whatever frame draws it, at
/// the light, its size the quad's.
pub fn anchored_billboard(mut d: HdDraw) -> HdDraw {
    if d.vertices.is_empty() {
        return d;
    }
    let n = d.vertices.len() as f32;
    let c = [0, 1, 2].map(|k| d.vertices.iter().map(|v| v.position[k]).sum::<f32>() / n);
    let o = [0, 1, 2].map(|k| d.vertices[0].position[k] - c[k]);
    let half = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt() / std::f32::consts::SQRT_2;
    for v in &mut d.vertices {
        v.position = c;
        v.extra = half.to_bits();
        v.material |= material::BILLBOARD | material::ANCHORED;
    }
    d
}

impl<G> HdScene<G> {
    /// The eye-dependent draws ([`eye_draws`], their textures interned: `local` maps the draws' texture indices into
    /// [`HdScene::textures`]) as the frame's own: the sky's irradiance and the clouds' settings, the sun the box paints, the sky,
    /// the planes and water (each plane's material bits from `plane_bits`, given its texture and whether it is blended), the
    /// coronas and the flare. `look` decides the sky.
    pub fn set_eye_draws(
        &mut self,
        src: &dyn SceneSource,
        draws: &EyeDraws,
        local: &[usize],
        look: SkyLook,
        plane_bits: &mut dyn FnMut(&Texture, bool) -> u32,
    ) {
        let (sky_fixed, sky_eye) = (&draws.sky, draws.eye);
        self.sky_sh = sh::sky_shape(sky_fixed, &self.textures, local, sky_eye, self.clear);
        // The volumetric clouds' settings (their ambient: the server's sky as it draws it, its flat layers too).
        let (table, clear) = (&self.textures, self.clear);
        let ambient = |d: [f32; 3]| sh::radiance(sky_fixed, table, local, sky_eye, clear, d);
        let hd_sky: Vec<HdDraw> = sky_fixed
            .iter()
            .filter(|d| sky::SkyPart::of(&d.label) == sky::SkyPart::Cloud)
            .map(|d| HdDraw::from_fixed(d, material::KIND_SKY, local))
            .collect();
        self.clouds = clouds::CloudSky::of_world(src, &hd_sky, table, &ambient);
        self.sky_eye = sky_eye;
        // The engine's own sky in place of the server's box, bans and flares (their light still lights the scene: the SH
        // above).
        self.atmosphere = look.own_sky && self.sky_choice.atmosphere(look.default_sky);
        let own = self.atmosphere;
        // A sun the server's box paints: searched again only when the box's textures or the Sun's direction change (about 6 ms);
        // none while the world moves the Sun across a box that stays put.
        let boxes: Vec<Draw> = if own || !self.follow_painted_sun || look.sun_moves {
            Vec::new()
        } else {
            sky_fixed
                .iter()
                .filter(|d| sky::SkyPart::of(&d.label) == sky::SkyPart::Box)
                .cloned()
                .collect()
        };
        let key = (
            boxes
                .iter()
                .map(|d| {
                    d.texture
                        .and_then(|(i, _)| local.get(i).copied())
                        .unwrap_or(usize::MAX)
                })
                .collect::<Vec<_>>(),
            self.sun.towards.map(f32::to_bits),
        );
        if self.painted_key.as_ref() != Some(&key) {
            self.painted_sun = if boxes.is_empty() {
                None
            } else {
                let table = &self.textures;
                let radiance = |d: [f32; 3], width: u32| {
                    sh::radiance_at(&boxes, table, local, sky_eye, [0.0; 3], d, width)
                };
                sky::painted_sun(&radiance, self.sun.towards, sky::PAINTED_SUN)
            };
            self.painted_key = Some(key);
        }
        let drawn_sun = self.painted_sun.and_then(|p| p.drawn());
        self.sky = sky_fixed
            .iter()
            .filter(|d| !own || sky::SkyPart::of(&d.label) == sky::SkyPart::Cloud)
            .map(|d| {
                let hd = HdDraw::from_fixed(d, material::KIND_SKY, local);
                match sky::SkyPart::of(&d.label) {
                    sky::SkyPart::Flare => billboard(
                        hd.with_material(material::EMITTER),
                        sky_eye,
                        draws.flare_linked,
                        drawn_sun,
                    ),
                    sky::SkyPart::Cloud => hd.with_material(material::CLOUD | material::SKY_EYE),
                    _ => hd.with_material(material::SKY_EYE),
                }
            })
            .collect();
        self.planes.clear();
        self.water.clear();
        self.planes_generation = self.planes_generation.wrapping_add(1);
        for (blended, d) in &draws.planes {
            let mut hd = HdDraw::from_fixed(d, material::KIND_PLANE, local);
            if let Some(t) = hd.texture.and_then(|(i, _)| self.textures.textures.get(i)) {
                let bits = plane_bits(t, *blended);
                if bits != 0 {
                    hd = hd.with_material(bits);
                }
            }
            if *blended {
                self.water.push(hd);
            } else {
                self.planes.push(hd);
            }
        }
        // The coronas are billboards anchored at their lights, facing every frame's camera.
        self.coronas = anchored_coronas(&draws.coronas, local);
        self.flare = draws.local_flare.as_ref().filter(|_| !own).map(|d| {
            billboard(
                HdDraw::from_fixed(d, material::KIND_SKY, local).with_material(material::EMITTER),
                sky_eye,
                draws.flare_linked,
                drawn_sun,
            )
        });
    }
}

impl crate::hd::HdSun {
    /// The fixed-function frame's sun as the enhanced frame's, with the Sun's shadow colour `shadow`.
    pub fn of(sun: &crate::Sun, shadow: [f32; 3]) -> crate::hd::HdSun {
        crate::hd::HdSun {
            towards: sun.towards,
            diffuse: sun.diffuse,
            ambient: sun.ambient,
            shadow,
        }
    }
}

impl HdLight {
    /// A light as a point light: its colour (gamma bytes times the brightness) as a linear colour of the same brightness, the
    /// object it is attached to its carrier.
    pub fn of_light(l: &Light) -> HdLight {
        let m = l.color[0].max(l.color[1]).max(l.color[2]);
        let color = if m > 0.0 && m.is_finite() {
            l.color.map(|c| srgb_to_linear((c / m).clamp(0.0, 1.0)) * m)
        } else {
            [0.0; 3]
        };
        HdLight {
            position: l.position,
            color,
            radius: l.radius.max(0.0),
            carrier: l.attached,
        }
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
