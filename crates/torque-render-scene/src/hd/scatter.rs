//! The day's light from the physical sky (Torque3D's `ScatterSky` mapped onto [`crate::hd::atmosphere`]): for a sun toward any
//! direction, the sun's light, the ambient light and the fog's color as the frame's [`HdSun`] and [`HdFog`] take them (gamma
//! 0..1), so a sun a [`TimeOfDay`](https://docs.rs/torque-scene) moves lights the world through sunrise, day, sunset and night.
//! The physical sky itself is the renderer's (drawn for the same sun); this is the light it casts.
//!
//! - **The sun's light** is the atmosphere's transmittance from the camera's height toward the sun (reddened and dimmed near the
//!   horizon; [`SUN_DEPTH`] of its optical depth), faded out as the sun's disk sinks under it, scaled so the sun overhead gives [`NOON_SUN`]; a high sun's cut to
//!   [`HIGH_SUN_SHARE`] of it.
//! - **The ambient** is the sky's light on a level surface (single scattering, integrated over the sky's hemisphere for the
//!   sun's elevation: blue by day, warm at sunset, deep blue at dusk), scaled so the sun overhead gives [`NOON_AMBIENT`]; at
//!   night never darker than the night color.
//! - **The fog** is the horizon's color (single scattering, averaged about the horizon) scaled to [`NOON_FOG`]'s brightness at
//!   noon; at night never darker than the night fog color.
//! - **The moon** (when on) takes over the light once the sun is [`MOON_FULL`] degrees under the horizon (from [`SUN_GONE`]):
//!   the light then comes from the moon's azimuth and elevation, its color the moon's (so the shadows follow it).
//!
//! The scales are ScatterSky's `sunScale`, `ambientScale` and `fogScale` (linear multipliers), `brightness` the light's, and
//! `nightColor`, `nightFogColor`, `moonEnabled`, `moonAzimuth`, `moonElevation` and `moonLightColor` its night fields. The
//! shadows let through [`ScatterLook::shadow`] of the light (the Sun's shadow color).
//!
//! Credit: the fields follow Torque3D's MIT-licensed `environment/scatterSky.cpp` (Copyright (c) 2012 GarageGames, LLC); the
//! light is this crate's own, from the atmosphere of [`crate::hd::atmosphere`].

use std::sync::OnceLock;

use glam::DVec3;

use crate::hd::atmosphere::{
    Atmosphere, CAMERA_HEIGHT, EARTH, Medium, Rgb, mie_phase, rayleigh_phase,
};
use crate::hd::clouds::CloudSky;
use crate::hd::{HdFog, HdSun, linear_to_srgb, srgb_to_linear};

/// The sun's light overhead (linear).
pub const NOON_SUN: [f32; 3] = [1.35, 1.35, 1.35];
/// A high sun's light is cut to [`HIGH_SUN_SHARE`] of it, from [`HIGH_SUN_START`] to [`HIGH_SUN_FULL`] degrees up (a white
/// surface at noon stays clear of the tonemap's shoulder; a low sun keeps its whole light).
pub const HIGH_SUN_START: f32 = 25.0;
pub const HIGH_SUN_FULL: f32 = 70.0;
pub const HIGH_SUN_SHARE: f32 = 0.7;
/// The ambient light with the sun overhead (linear, its brightness: the hue is the sky's).
pub const NOON_AMBIENT: f32 = 0.15;
/// The share of the sky's color the ambient keeps (the single scattering alone is bluer than the sky's light).
pub const AMBIENT_CHROMA: f32 = 0.5;
/// The fog's color with the sun overhead (gamma; its brightness: the hue is the horizon's).
pub const NOON_FOG: [f32; 3] = [0.72, 0.8, 0.9];
/// The sun's elevation (degrees) at which its light is gone (its disk under the horizon), and where it is whole.
pub const SUN_GONE: f32 = -1.0;
pub const SUN_WHOLE: f32 = 3.0;
/// The lowest elevation (degrees) the sun's transmittance is taken at: just over the horizon seen from the camera's height (the
/// fade hides the sun's light below it).
const SUN_LOWEST: f32 = -0.4;
/// The share of the atmosphere's optical depth the sun's light crosses: a low sun dimmed and reddened a little less than through
/// the whole of Earth's haze (a brighter golden hour and sunset).
pub const SUN_DEPTH: f32 = 0.6;
/// How far under the horizon (degrees) the sun is when the moon's light is whole (it starts at [`SUN_GONE`]).
pub const MOON_FULL: f32 = -10.0;

/// The table's lowest sun elevation and its step (degrees); it reaches 90.
const TABLE_LOW: f32 = -20.0;
const TABLE_STEP: f32 = 1.0;
/// The sky's hemisphere's bands of elevation and azimuths, the steps along a view ray and toward the sun.
const BANDS: usize = 6;
const AZIMUTHS: usize = 12;
const VIEW_STEPS: usize = 16;
const SUN_STEPS: usize = 16;
/// The elevation (degrees) of the horizon's color.
const HORIZON_VIEW: f64 = 2.0;

/// ScatterSky's look (its defaults but the night's, which are ours).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScatterLook {
    pub sun_scale: [f32; 3],
    pub ambient_scale: [f32; 3],
    pub fog_scale: [f32; 3],
    pub brightness: f32,
    /// What the shadows let through of the light (gamma 0..1).
    pub shadow: [f32; 3],
    /// The least ambient (gamma) and fog color at night.
    pub night_color: [f32; 3],
    pub night_fog_color: [f32; 3],
    pub moon_enabled: bool,
    /// Degrees.
    pub moon_azimuth: f32,
    pub moon_elevation: f32,
    /// The moon's light (linear).
    pub moon_light_color: [f32; 3],
}

impl Default for ScatterLook {
    fn default() -> ScatterLook {
        ScatterLook {
            sun_scale: [1.0; 3],
            ambient_scale: [1.0; 3],
            fog_scale: [1.0; 3],
            brightness: 1.0,
            shadow: [0.0; 3],
            night_color: [0.0; 3],
            night_fog_color: [0.0; 3],
            moon_enabled: true,
            moon_azimuth: 0.0,
            moon_elevation: 45.0,
            moon_light_color: [0.192, 0.192, 0.192],
        }
    }
}

/// The light of one moment: toward the light (the sun, or at night the moon), its color and the ambient's (gamma, as
/// [`HdSun`]), the shadows' color, the fog's, and how far into the night it is (0 day, 1 the moon's light whole).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyLight {
    pub towards: [f32; 3],
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    pub shadow: [f32; 3],
    pub fog: [f32; 3],
    pub night: f32,
}

impl SkyLight {
    /// The frame's sun.
    pub fn sun(&self) -> HdSun {
        HdSun {
            towards: self.towards,
            diffuse: self.diffuse,
            ambient: self.ambient,
            shadow: self.shadow,
        }
    }

    /// Volumetric clouds lit by this light: the default layers, drifting with `wind`, the sky's light on them the ambient's
    /// (above) and the fog's (toward the horizon).
    pub fn clouds(&self, wind: [f32; 3]) -> CloudSky {
        CloudSky::from_layers(
            Vec::new(),
            wind,
            self.ambient.map(srgb_to_linear),
            self.fog.map(srgb_to_linear),
        )
    }

    /// `fog` with this light's color.
    pub fn fog(&self, fog: HdFog) -> HdFog {
        HdFog {
            color: self.fog,
            ..fog
        }
    }
}

/// The sky's light on a level surface and the horizon's color for each sun elevation (linear, for a sun of 1).
#[derive(Debug, Clone, PartialEq)]
pub struct ScatterTable {
    ambient: Vec<[f32; 3]>,
    horizon: Vec<[f32; 3]>,
    sun_noon: [f32; 3],
    atmosphere: Atmosphere,
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The transmittance from radius `r` along `mu` (cosine from the zenith) out of the atmosphere; none through the ground.
fn transmittance(a: &Atmosphere, r: f64, mu: f64, steps: usize) -> Rgb {
    if a.hits_ground(r, mu) {
        return DVec3::ZERO;
    }
    a.transmittance_direct(r, mu, steps)
}

/// The sky's single scattering toward the eye at radius `r` from `view` with the sun toward `sun` (unit, z up), for a sun of 1.
fn single_scattering(a: &Atmosphere, r: f64, view: DVec3, sun: DVec3) -> Rgb {
    let mu = view.z;
    let d = if a.hits_ground(r, mu) {
        a.distance_to_bottom(r, mu)
    } else {
        a.distance_to_top(r, mu)
    };
    let nu = view.dot(sun);
    let (pr, pm) = (rayleigh_phase(nu), mie_phase(nu, a.mie_g));
    let dt = d / VIEW_STEPS as f64;
    let mut path = DVec3::ONE;
    let mut l = DVec3::ZERO;
    for i in 0..VIEW_STEPS {
        let t = (i as f64 + 0.5) * dt;
        let p = DVec3::new(view.x * t, view.y * t, r + view.z * t);
        let rx = p.length();
        let mu_s = p.dot(sun) / rx;
        let m: Medium = a.medium(rx - a.bottom);
        let tsun = transmittance(a, rx, mu_s, SUN_STEPS);
        let step = m.extinction.map(|e| (-e * dt).exp());
        let s = (m.rayleigh * pr + m.mie * pm) * tsun;
        l += path * (s - s * step) / m.extinction.max(DVec3::splat(1e-12));
        path *= step;
    }
    l
}

impl ScatterTable {
    /// The table of `atmosphere` (some milliseconds: [`ScatterTable::earth`] keeps the Earth's).
    pub fn new(atmosphere: Atmosphere) -> ScatterTable {
        let a = &atmosphere;
        let r = a.bottom + CAMERA_HEIGHT;
        let n = ((90.0 - TABLE_LOW) / TABLE_STEP) as usize + 1;
        let mut ambient = Vec::with_capacity(n);
        let mut horizon = Vec::with_capacity(n);
        for i in 0..n {
            let e = f64::from(TABLE_LOW + i as f32 * TABLE_STEP).to_radians();
            let sun = DVec3::new(0.0, e.cos(), e.sin());
            let mut irradiance = DVec3::ZERO;
            let mut edge = DVec3::ZERO;
            for j in 0..AZIMUTHS {
                let phi = (j as f64 + 0.5) / AZIMUTHS as f64 * std::f64::consts::TAU;
                let (sp, cp) = phi.sin_cos();
                for b in 0..BANDS {
                    let el = (b as f64 + 0.5) / BANDS as f64 * std::f64::consts::FRAC_PI_2;
                    let (se, ce) = el.sin_cos();
                    let l = single_scattering(a, r, DVec3::new(sp * ce, cp * ce, se), sun);
                    // cos(zenith) x the band's solid angle.
                    let w = se
                        * ce
                        * (std::f64::consts::FRAC_PI_2 / BANDS as f64)
                        * (std::f64::consts::TAU / AZIMUTHS as f64);
                    irradiance += l * w;
                }
                let h = HORIZON_VIEW.to_radians();
                let l =
                    single_scattering(a, r, DVec3::new(sp * h.cos(), cp * h.cos(), h.sin()), sun);
                edge += l / AZIMUTHS as f64;
            }
            ambient.push(irradiance.as_vec3().to_array());
            horizon.push(edge.as_vec3().to_array());
        }
        let sun_noon = transmittance(a, r, 1.0, 64).as_vec3().to_array();
        ScatterTable {
            ambient,
            horizon,
            sun_noon,
            atmosphere,
        }
    }

    /// The Earth's table, made once.
    pub fn earth() -> &'static ScatterTable {
        static EARTH_TABLE: OnceLock<ScatterTable> = OnceLock::new();
        EARTH_TABLE.get_or_init(|| ScatterTable::new(EARTH))
    }

    /// The ambient and horizon of a sun at `elevation` degrees (linear between the table's; clamped to its ends).
    fn at(&self, elevation: f32) -> ([f32; 3], [f32; 3]) {
        let x = ((elevation - TABLE_LOW) / TABLE_STEP).clamp(0.0, (self.ambient.len() - 1) as f32);
        let i = (x.floor() as usize).min(self.ambient.len() - 2);
        let f = x - i as f32;
        let mix = |t: &[[f32; 3]]| [0, 1, 2].map(|k| t[i][k] * (1.0 - f) + t[i + 1][k] * f);
        (mix(&self.ambient), mix(&self.horizon))
    }

    /// The sun's light (linear, for a sun of 1 over the noon's) at `elevation` degrees.
    fn sun(&self, elevation: f32) -> [f32; 3] {
        let a = &self.atmosphere;
        let mu = f64::from(elevation.max(SUN_LOWEST).to_radians().sin());
        let t = transmittance(a, a.bottom + CAMERA_HEIGHT, mu, 64);
        [0, 1, 2].map(|k| (t[k] as f32 / self.sun_noon[k].max(1e-6)).powf(SUN_DEPTH))
    }

    /// The light of a sun toward `towards` (unit, z up) under `look`.
    pub fn light(&self, towards: [f32; 3], look: &ScatterLook) -> SkyLight {
        let elevation = towards[2].clamp(-1.0, 1.0).asin().to_degrees();
        let (amb, hor) = self.at(elevation);
        let (amb_noon, hor_noon) = self.at(90.0);
        // The sun: its transmittance, faded as its disk sinks.
        let share = smoothstep(SUN_GONE, SUN_WHOLE, elevation);
        let sun = self.sun(elevation);
        let high =
            1.0 - (1.0 - HIGH_SUN_SHARE) * smoothstep(HIGH_SUN_START, HIGH_SUN_FULL, elevation);
        let sun_lin: [f32; 3] = [0, 1, 2]
            .map(|k| NOON_SUN[k] * high * sun[k] * share * look.sun_scale[k] * look.brightness);
        // The ambient: the sky's irradiance, its noon's brightness NOON_AMBIENT.
        let a_scale = NOON_AMBIENT / luminance(amb_noon).max(1e-9);
        let night_amb = look.night_color.map(srgb_to_linear);
        let gray = luminance(amb);
        let amb_lin: [f32; 3] = [0, 1, 2].map(|k| {
            let sky = gray + (amb[k] - gray) * AMBIENT_CHROMA;
            (sky * a_scale * look.ambient_scale[k] * look.brightness).max(night_amb[k])
        });
        // The fog: the horizon, its noon's brightness NOON_FOG's.
        let noon_fog = NOON_FOG.map(srgb_to_linear);
        let f_scale = luminance(noon_fog) / luminance(hor_noon).max(1e-9);
        let night_fog = look.night_fog_color.map(srgb_to_linear);
        let fog_lin: [f32; 3] =
            [0, 1, 2].map(|k| (hor[k] * f_scale * look.fog_scale[k]).max(night_fog[k]));
        // The moon past the sun's light.
        let night = if look.moon_enabled {
            smoothstep(SUN_GONE, MOON_FULL, elevation)
        } else {
            0.0
        };
        let (towards, light_lin) = if share <= 0.0 && night > 0.0 {
            let (sa, ca) = look.moon_azimuth.to_radians().sin_cos();
            let (se, ce) = look.moon_elevation.to_radians().sin_cos();
            (
                [sa * ce, ca * ce, se],
                look.moon_light_color.map(|c| c * night * look.brightness),
            )
        } else {
            (towards, sun_lin)
        };
        let ambient = amb_lin.map(linear_to_srgb);
        let diffuse =
            [0, 1, 2].map(|k| (linear_to_srgb(amb_lin[k] + light_lin[k]) - ambient[k]).max(0.0));
        SkyLight {
            towards,
            diffuse,
            ambient,
            shadow: look.shadow,
            fog: fog_lin.map(|v| linear_to_srgb(v).clamp(0.0, 1.0)),
            night,
        }
    }
}

#[cfg(test)]
#[path = "scatter_tests.rs"]
mod tests;
