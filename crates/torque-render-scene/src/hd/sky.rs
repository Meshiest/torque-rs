//! The enhanced renderer's sky, fog and volumetrics as plain data (the
//! fixed-function sky's data, not how it is drawn).
//!
//! - **The sky** (the server's sky as the fixed-function frame draws it): the `.dml` box, remote flare, clouds and bans in the
//!   environment's colours; with [`ImageSettings::physical_sky`](crate::hd::ImageSettings::physical_sky) the box shaded by the
//!   physical sky of [`crate::hd::atmosphere`]: each direction's brightness relative
//!   to the atmosphere's mean at the same elevation (the sky view table for the frame's sun; brighter toward the sun, darker away
//!   from it), a scalar so the colours stay the server's ([`DEFAULT_SKY_SHADING`]); the sun disk in the sun's colour with limb
//!   darkening (a linear law per channel, stronger in the blue); with linear light the Sky's cloud layers brightened toward the sun
//!   (a forward scattering lobe) and faded toward the horizon; procedural stars where the server's sky is dark. The fog's colour is
//!   the sky drawn behind the pixel, its amount the distance fog's (all fog at the visible distance: the far plane's cut is
//!   hidden), with the height fog setting times an exponential height fog.
//! - **Volumetrics** (the settings may ask for them only when the Sky's fog distance is under its visible distance): the sun's light scattered
//!   by a thin medium toward the eye, shadowed by the sun's cascades, marched per pixel at a reduced size with a fixed per-pixel
//!   dither (temporally stable), upsampled by depth and added.
//!
//! - **The physical sky in the box's place** ([`SkyChoice`]: the caller's setting; `auto` picks it when the caller says the server's Sky is the built-in default box): the
//!   physical sky of [`crate::hd::atmosphere`] in its own colours in place of the server's box, bans and flares (the sky view
//!   table for the server's sun, below the horizon the horizon's colour), with the sun disk, the stars, the volumetric clouds and
//!   the fog toward the sky its own; the server's sun still sets the sun's place and colour.
//!
//! [`SkyPart`] sorts the fixed-function sky's draws (box faces, the remote flare, clouds, bans).

use glam::Vec3;

use crate::hd::{HdDraw, Quality};

/// Which sky the physical sky draws (the caller's setting: `auto`, `atmosphere` or `server`; without the physical sky always the
/// server's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkyChoice {
    /// The physical sky in the box's place where the caller says the server's Sky is the built-in default box, else the server's (none:
    /// none).
    #[default]
    Auto,
    /// Always the physical sky in the box's place.
    Atmosphere,
    /// Always the server's box, shaded by the atmosphere.
    Server,
}

impl SkyChoice {
    pub const ALL: [SkyChoice; 3] = [SkyChoice::Auto, SkyChoice::Atmosphere, SkyChoice::Server];

    pub fn name(self) -> &'static str {
        match self {
            SkyChoice::Auto => "auto",
            SkyChoice::Atmosphere => "atmosphere",
            SkyChoice::Server => "server",
        }
    }

    pub fn named(word: &str) -> Option<SkyChoice> {
        SkyChoice::ALL
            .into_iter()
            .find(|c| c.name().eq_ignore_ascii_case(word.trim()))
    }

    /// Whether the physical sky is drawn in the box's place; `default_box`: the caller says the server's Sky is the built-in default box.
    pub fn atmosphere(self, default_box: bool) -> bool {
        match self {
            SkyChoice::Atmosphere => true,
            SkyChoice::Server => false,
            SkyChoice::Auto => default_box,
        }
    }
}

/// What a draw of the fixed-function sky is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkyPart {
    Box,
    Flare,
    Cloud,
    Bans,
}

impl SkyPart {
    /// The part of a fixed-function sky draw, from its label (`sky face`, `sky cloud`, a flare's, else the bans).
    pub fn of(label: &str) -> SkyPart {
        if label.starts_with("sky face") {
            SkyPart::Box
        } else if label.starts_with("sky cloud") {
            SkyPart::Cloud
        } else if label.contains("flare") {
            SkyPart::Flare
        } else {
            SkyPart::Bans
        }
    }
}

/// Where the sky passes go among the sky's draws (all of them drawn in the Sky's order, the server's sky): the physical sky's
/// shading at `shade` (after the box, before the remote flare, the clouds and the bans), the sun disk at `disk` (before
/// the first cloud or ban), the volumetric clouds at `clouds` (after the server's flat cloud layers, the sky's high layer,
/// and before the bans: the disk, the flare and the flat layers behind them, the horizon's bans over them). Each is an index into the
/// draws (their count: after all of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkyOrder {
    pub shade: usize,
    pub disk: usize,
    pub clouds: usize,
}

/// The [`SkyOrder`] of the sky's draws.
pub fn order(sky: &[HdDraw]) -> SkyOrder {
    let labels: Vec<String> = sky.iter().map(|d| d.label.clone()).collect();
    order_labels(&labels)
}

/// [`order`] from the draws' labels.
pub fn order_labels(labels: &[String]) -> SkyOrder {
    let parts: Vec<SkyPart> = labels.iter().map(|l| SkyPart::of(l)).collect();
    let first =
        |f: &dyn Fn(SkyPart) -> bool| parts.iter().position(|p| f(*p)).unwrap_or(parts.len());
    SkyOrder {
        shade: first(&|p| p != SkyPart::Box),
        disk: first(&|p| matches!(p, SkyPart::Cloud | SkyPart::Bans)),
        clouds: first(&|p| p == SkyPart::Bans),
    }
}

/// The sun disk: its angular radius (radians), its brightness (linear HDR; scaled by the sun's colour), the limb darkening's
/// coefficient per channel (`I(mu) / I(1) = 1 - u (1 - mu)`, mu the cosine across the disk).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunDisk {
    pub radius: f32,
    pub radiance: f32,
    pub limb: [f32; 3],
}

/// The neutral tonemap's shoulder (`tonemap.wgsl`'s `SHOULDER`): where the disk's own path to white starts its channels'
/// curves.
pub const DISK_SHOULDER: f32 = 0.8;
/// The overshoot past white (the disk's largest channel, in units of white) at which its color has gone half way to white.
pub const DISK_WHITE_SPILL: f32 = 16.0;

fn disk_shoulder(v: f32) -> f32 {
    if v <= DISK_SHOULDER {
        return v;
    }
    let span = 1.0 - DISK_SHOULDER;
    DISK_SHOULDER + span * (1.0 - (-(v - DISK_SHOULDER) / span).exp())
}

/// The sun disk's light made white-hot (the CPU form of `sky.wgsl`'s `disk_white_hot`). The neutral tonemap keeps a color's hue
/// however far past white it is, so a saturated disk a hundred times over white showed as the same orange as the sky about it and
/// darker than a paler sky. The disk takes its own path to white instead: its hue goes `q / (1 + q)` of the way (`q` the square of
/// the overshoot over [`DISK_WHITE_SPILL`]) to each channel's own shoulder, so the light the shoulder cannot show raises
/// the other channels in the order they fill (white overhead; cream, peach and orange as the air dims and reddens it), with its
/// largest channel kept. Under white it is unchanged.
pub fn disk_white_hot(d: [f32; 3]) -> [f32; 3] {
    let m = d[0].max(d[1]).max(d[2]);
    if m <= 1.0 {
        return d;
    }
    let x = (m - 1.0) / DISK_WHITE_SPILL;
    let q = x * x;
    let g = q / (1.0 + q);
    let shown = d.map(|v| {
        let h = v / m;
        h + (disk_shoulder(v) - h) * g
    });
    let top = shown[0].max(shown[1]).max(shown[2]).max(1e-6);
    shown.map(|v| v * m / top)
}

/// The physical sky's light for a sun of 1 times this (`π` times the scale that makes a sunny day's sky as bright as the fixed-function
/// sky box beside sunlit surfaces).
pub const DEFAULT_SKY_GAIN: f32 = 16.0;
/// The night sky's floor (linear, before the gain): a moonless sky still has some light.
pub const NIGHT_SKY: [f32; 3] = [0.0006, 0.0009, 0.0018];
/// The stars: the share of directions with one, their brightness (linear), the sky luminance under which they show.
pub const STAR_DENSITY: f32 = 0.0035;
pub const STAR_BRIGHTNESS: f32 = 3.0;
pub const STAR_SKY_LIMIT: f32 = 0.05;
/// The lit cloud layers: their alpha's fade toward the horizon (radians of elevation over which it rises to 1).
pub const CLOUD_HORIZON_FADE: f32 = 0.12;

/// How the physical sky shades the server's box: the atmosphere's brightness toward a direction over its mean at
/// that elevation, raised to `strength` and held within `floor..cap`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyShading {
    pub strength: f32,
    pub floor: f32,
    pub cap: f32,
}

pub const DEFAULT_SKY_SHADING: SkyShading = SkyShading {
    strength: 0.6,
    floor: 0.6,
    cap: 2.5,
};

/// The lit cloud layers: the share of the forward-scattering lobe (Henyey-Greenstein, its mean over the sphere 1) in their
/// brightness, and the most it brightens them.
pub const CLOUD_LOBE_SHARE: f32 = 0.25;
pub const CLOUD_LOBE_CAP: f32 = 2.5;

/// The physical sky's scale for the frame: [`DEFAULT_SKY_GAIN`].
pub fn default_sky_scale() -> f32 {
    DEFAULT_SKY_GAIN
}

/// The exponential height fog: the extinction at the ground (per unit), the height over which it falls by e (units), the
/// ground's height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeightFog {
    pub density: f32,
    pub falloff: f32,
    pub base: f32,
}

pub const DEFAULT_HEIGHT_FOG: HeightFog = HeightFog {
    density: 0.0006,
    falloff: 40.0,
    base: 0.0,
};

/// The volumetrics of a quality: the march's reduced size (1 in this many pixels a side) and its steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumetricQuality {
    pub scale: u32,
    pub steps: u32,
}

impl Quality {
    /// Off at Low; Medium a quarter size, 12 steps; High half, 24; Ultra half, 48.
    pub fn volumetrics(self) -> Option<VolumetricQuality> {
        match self {
            Quality::Low => None,
            Quality::Medium => Some(VolumetricQuality {
                scale: 4,
                steps: 12,
            }),
            Quality::High => Some(VolumetricQuality {
                scale: 2,
                steps: 24,
            }),
            Quality::Ultra => Some(VolumetricQuality {
                scale: 2,
                steps: 48,
            }),
        }
    }
}

/// The volumetric medium: the scattering coefficient (per unit), the Henyey-Greenstein asymmetry, the share of the sun's
/// light it scatters (its strength), and how far the march goes (units; also no farther than the shadows reach).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volumetric {
    pub scattering: f32,
    pub anisotropy: f32,
    pub strength: f32,
    pub reach: f32,
}

/// How [`painted_sun`] looks for a sun the server's box paints: within `reach` (radians) of the Sun's direction, on a grid of
/// directions `step` apart (radians, on the plane facing the Sun), each sampled from the box's textures at their first level no
/// wider than `width`; a direction is hot where every channel is at least `hot` (gamma; a painted sun's core is clipped white);
/// a sun is a group of touching hot directions not reaching the grid's edge (the horizon's white band and a white box do), at
/// most `max_radius` and at least `min_radius` (radians) from their centre to the furthest (a speck of white in a broad glow is
/// not a sun).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintedSunSearch {
    pub reach: f32,
    pub step: f32,
    pub width: u32,
    pub hot: f32,
    pub min_radius: f32,
    pub max_radius: f32,
}

pub const PAINTED_SUN: PaintedSunSearch = PaintedSunSearch {
    reach: 0.52,
    step: 0.006,
    width: 512,
    hot: 0.96,
    min_radius: 0.015,
    max_radius: 0.17,
};

/// A sun painted on the server's box ([`painted_sun`]): its direction (towards it), its hot core's radius and its angle from the
/// Sun's direction (radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintedSun {
    pub towards: [f32; 3],
    pub radius: f32,
    pub offset: f32,
}

impl PaintedSun {
    /// Where the disk, the flares and the glow go: on the painted sun when the Sun's direction is off its hot core (a box
    /// whose painted sun sits 12.7 degrees below the Sun), None (the Sun's direction kept) when it is on it (a Sun between the two
    /// lobes of a painted sun behind a cloud, 1.1 degrees from the larger one's centre).
    pub fn drawn(&self) -> Option<[f32; 3]> {
        (self.offset > self.radius).then_some(self.towards)
    }
}

/// `v` at unit length (a zero vector stays zero).
fn normalized(v: Vec3) -> Vec3 {
    v / v.length().max(1e-12)
}

/// The sun the server's sky box paints near the Sun's direction `sun`, if it paints one ([`PaintedSunSearch`]): `radiance` is the
/// box's colour along a direction (gamma 0..1, its textures sampled from their first level no wider than the width given). The
/// nearest such sun to `sun`.
pub fn painted_sun(
    radiance: &dyn Fn([f32; 3], u32) -> [f32; 3],
    sun: [f32; 3],
    search: PaintedSunSearch,
) -> Option<PaintedSun> {
    let s = normalized(sun.into());
    // The plane facing the Sun: right and up.
    let helper = if s.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let right = normalized(s.cross(helper));
    let up = right.cross(s);
    let half = (search.reach.tan() / search.step).ceil() as i32;
    let n = (2 * half + 1) as usize;
    let at = |i: usize, j: usize| {
        let x = (i as i32 - half) as f32 * search.step;
        let y = (j as i32 - half) as f32 * search.step;
        normalized(s + right * x + up * y)
    };
    let cos_reach = search.reach.cos();
    let mut hot = vec![false; n * n];
    let mut inside = vec![false; n * n];
    for j in 0..n {
        for i in 0..n {
            let d = at(i, j);
            if d.dot(s) < cos_reach {
                continue;
            }
            inside[j * n + i] = true;
            let c = radiance(d.to_array(), search.width);
            hot[j * n + i] = c.iter().all(|&v| v >= search.hot);
        }
    }
    let mut seen = vec![false; n * n];
    let mut best: Option<PaintedSun> = None;
    for start in 0..n * n {
        if !hot[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut cells = Vec::new();
        let mut edge = false;
        while let Some(c) = stack.pop() {
            cells.push(c);
            let (i, j) = ((c % n) as i32, (c / n) as i32);
            for (di, dj) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (a, b) = (i + di, j + dj);
                if a < 0
                    || b < 0
                    || a >= n as i32
                    || b >= n as i32
                    || !inside[b as usize * n + a as usize]
                {
                    edge = true;
                    continue;
                }
                let k = b as usize * n + a as usize;
                if hot[k] && !seen[k] {
                    seen[k] = true;
                    stack.push(k);
                }
            }
        }
        if edge {
            continue;
        }
        let mut sum = Vec3::ZERO;
        for &c in &cells {
            sum += at(c % n, c / n);
        }
        let centre = normalized(sum);
        let radius = cells
            .iter()
            .map(|&c| at(c % n, c / n).dot(centre).clamp(-1.0, 1.0).acos())
            .fold(0.0f32, f32::max);
        if radius > search.max_radius || radius < search.min_radius {
            continue;
        }
        let offset = centre.dot(s).clamp(-1.0, 1.0).acos();
        if best.is_none_or(|b| offset < b.offset) {
            best = Some(PaintedSun {
                towards: centre.to_array(),
                radius,
                offset,
            });
        }
    }
    best
}

#[cfg(test)]
#[path = "sky_tests.rs"]
mod tests;
