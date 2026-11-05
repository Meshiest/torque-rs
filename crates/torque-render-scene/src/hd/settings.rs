//! The enhanced renderer's image settings: the numbers that set its image formation (exposure, tonemap, grade, bloom, emissive
//! gains), its sun disk and volumetrics, when its volumetric clouds run, its ambient occlusion and sharpen strengths, and the
//! switches of its lighting, sky, fog, translucency and water. The renderer reads every such number from an [`ImageSettings`];
//! [`ImageSettings::DEFAULT`] is the engine's, and a caller may set its own as data.

use crate::hd::Quality;
use crate::hd::clouds::CloudQuality;
use crate::hd::grade::{BloomSettings, Emissive, ExposureMode, Tonemap};
use crate::hd::lut::{Grade, LutAxis};
use crate::hd::sky::{SunDisk, Volumetric};

/// The image settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageSettings {
    pub exposure: ExposureMode,
    pub tonemap: Tonemap,
    pub grade: Grade,
    pub bloom: BloomSettings,
    pub emissive: Emissive,
    pub sun_disk: SunDisk,
    pub volumetric: Volumetric,
    /// The volumetrics run only where the fog starts short of the visible distance (else always).
    pub volumetrics_need_fog: bool,
    /// The volumetric clouds run only at Ultra and only when asked (else from Medium up).
    pub clouds_opt_in: bool,
    /// The share of the ambient occlusion applied.
    pub ao_strength: f32,
    /// The sharpen's strength (0 to 1) where the quality has it.
    pub sharpen: f32,
    /// The water is clear: absorbing toward its colour along the path to the floor, scattering, foam where it is shallow, its
    /// reflection by Fresnel whole; else its texture stays its face (a blend over the floor, at most three quarters reflection).
    pub clear_water: bool,
    /// Surfaces are lit in linear light (albedo x (ambient + N.L sun)) and the sky's cloud layers are lit by the sun; else the
    /// fixed-function response: the light factor ambient + N.L diffuse (as GL light 0) applied to the albedo in gamma space, so a
    /// lit face lands on the fixed-function colour.
    pub linear_light: bool,
    /// The physical sky: the atmosphere's sky view table shades the server's sky box and, where the scene asks
    /// ([`crate::hd::HdScene::atmosphere`]), draws its own sky in the box's place.
    pub physical_sky: bool,
    /// An exponential height fog ([`crate::hd::sky::DEFAULT_HEIGHT_FOG`]) under the distance fog, and a volumetric medium that thins
    /// with height as it does (else a constant one).
    pub height_fog: bool,
    /// Translucent surfaces blend in linear light (glass: its reflections not scaled by its alpha); else in display (gamma) space, as
    /// the fixed-function blend does (a translucent colour over a light background stays far more saturated).
    pub linear_blend: bool,
    /// The scene's smooth occlusion whatever the scene says ([`crate::hd::HdScene::smooth_occlusion`]; every frame has it now:
    /// kept for the callers that set it).
    pub smooth_occlusion: bool,
}

impl ImageSettings {
    /// The engine's settings: fixed exposure, the neutral tonemap and grade, the whole frame mixed into a low bloom, a small bright sun
    /// disk with strong limb darkening, volumetrics and volumetric clouds always on, full AO, a light sharpen, clear water, linear
    /// light and blending, the physical sky and the height fog; the occlusion smooth only where the scene asks.
    pub const DEFAULT: ImageSettings = ImageSettings {
        exposure: ExposureMode::Fixed(1.0),
        tonemap: Tonemap::Neutral,
        grade: Grade::NEUTRAL,
        bloom: BloomSettings {
            threshold: 0.0,
            knee: 0.0,
            strength: crate::hd::grade::DEFAULT_BLOOM_MIX,
            mix: true,
        },
        emissive: Emissive {
            glow: 8.0,
            shape: 3.0,
            emitter: 4.0,
        },
        sun_disk: SunDisk {
            radius: 0.0093,
            radiance: 160.0,
            limb: [0.50, 0.58, 0.68],
        },
        volumetric: Volumetric {
            scattering: 0.0009,
            anisotropy: 0.6,
            strength: 1.0,
            reach: 220.0,
        },
        volumetrics_need_fog: false,
        clouds_opt_in: false,
        ao_strength: 1.0,
        sharpen: 0.2,
        clear_water: true,
        linear_light: true,
        physical_sky: true,
        height_fog: true,
        linear_blend: true,
        smooth_occlusion: false,
    };
    pub fn exposure(self) -> ExposureMode {
        self.exposure
    }

    pub fn tonemap(self) -> Tonemap {
        self.tonemap
    }

    pub fn grade(self) -> Grade {
        self.grade
    }

    pub fn bloom(self) -> BloomSettings {
        self.bloom
    }

    pub fn emissive(self) -> Emissive {
        self.emissive
    }

    pub fn sun_disk(self) -> SunDisk {
        self.sun_disk
    }

    pub fn volumetric(self) -> Volumetric {
        self.volumetric
    }

    /// The share of the ambient occlusion applied.
    pub fn ao_strength(self) -> f32 {
        self.ao_strength
    }

    /// The sharpen's strength (0 to 1) where the quality has it.
    pub fn sharpen(self) -> f32 {
        self.sharpen
    }

    /// The table's axis of the tonemap.
    pub fn lut_axis(self) -> LutAxis {
        match self.tonemap {
            Tonemap::Neutral => LutAxis::Srgb,
            Tonemap::AgxStyle => LutAxis::AgxSigmoid,
        }
    }

    /// Whether the volumetrics run with the fog starting at `fog_start` and the visible distance `visible`.
    pub fn volumetrics_on(self, fog_start: f32, visible: f32) -> bool {
        !self.volumetrics_need_fog || fog_start < visible
    }

    /// The volumetric clouds at `quality` ([`CloudQuality::of`]); with `clouds_opt_in` only at Ultra and only when `opted_in`.
    pub fn clouds(self, quality: Quality, opted_in: bool) -> Option<CloudQuality> {
        let q = CloudQuality::of(quality)?;
        (!self.clouds_opt_in || (quality == Quality::Ultra && opted_in)).then_some(q)
    }
}

/// How the enhanced renderer draws a frame: its image settings at one of the quality steps (Low, Medium, High, Ultra), by default
/// [`ImageSettings::DEFAULT`] ([`HdMode::new`]); a caller with other renderers beside this one keeps an `Option<HdMode>` (None:
/// another renderer draws).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdMode {
    pub image: ImageSettings,
    pub quality: Quality,
}

impl HdMode {
    /// The engine's settings at `quality`.
    pub const fn new(quality: Quality) -> HdMode {
        HdMode {
            image: ImageSettings::DEFAULT,
            quality,
        }
    }
}

impl Default for HdMode {
    /// The engine's settings at High.
    fn default() -> HdMode {
        HdMode::new(Quality::High)
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
