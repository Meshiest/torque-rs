//! TGE's `Sky` (SceneObject-derived) and `Sun` (a NetObject): both keep their own mask bits and send raw floats.
//!
//! ```text
//! Sky:  flag(Init, bit 0): string (material list), F32 x3 fog colour, raw 32 fog volume count, raw byte (sky textures on),
//!         raw byte (render box bottom), F32 x3 solid fill colour, raw byte (effect precipitation), flag no render bans;
//!         per fog volume: F32 visible distance, F32 min height, F32 max height, F32 x3 colour, raw byte (storm active);
//!         per cloud layer (3): string (texture), F32 height, F32 speed;
//!         F32 x3 wind velocity, raw 32 fog volume;
//!         flag(storm fog under way: F32(now - start) / 1000 < time): F32 fog percentage, F32 time, raw 32 state,
//!         raw 32 (now - start, ms), F32 end percentage
//!       flag(StormCloudsOn, bit 6): raw byte;  flag(StormFogOn, bit 7, && !Init): raw byte
//!       flag(Visibility, bit 1): F32 visible distance, F32 fog distance
//!       flag(StormCloud, bit 2): raw 32 state, F32 time
//!       flag(StormFog, bit 3, && !Init): F32 end percentage, F32 time, raw 32 fog volume
//!       flag(StormRealFog, bit 4): raw 32 real fog, F32 max, F32 min, F32 speed
//!       flag(Wind, bit 5): F32 x3 wind velocity
//! Sun:  flag(Update, bit 0): F32 x3 direction, F32 x4 colour, F32 x4 ambient
//! ```
//! The fog volume count is a 32-bit word: the reader refuses one above [`MAX_FOG_VOLUMES`]. Both packers return 0. After a pack
//! that sent the storm fog block the host sets the storm's start time to the pack time, as `Sky::packUpdate` does. The Sun's
//! direction is the host's: `Sun::packUpdate` derives it from the azimuth and elevation (the negated vector of those angles).
//!
//! **View fields:** Sky: `material_list`, `fog_color` 0..3, `fog_volume_count`, per volume `fog_visible_distance`,
//! `fog_min_height`, `fog_max_height`, `fog_volume_color` (`3 i + k`), `storm_fog_active`; `sky_textures_on`,
//! `render_box_bottom`, `solid_fill_color` 0..3, `effect_precip`, `no_render_bans`; per layer `cloud_text`, `cloud_height`,
//! `cloud_speed`; `wind_velocity` 0..3, `fog_volume`, `storm_fog_start_ms`, `storm_fog_time`, `fog_percentage`,
//! `storm_fog_state`, `storm_fog_end_percentage`, `storm_clouds_on`, `storm_fog_on`, `visible_distance`, `fog_distance`,
//! `storm_cloud_state`, `storm_cloud_time`, `real_fog`, `real_fog_max`, `real_fog_min`, `real_fog_speed`. Sun: `direction` 0..3,
//! `color` 0..4, `ambient` 0..4.
//!
//! Credit: these follow TGE 1.4's `terrain/sky.cc` and `terrain/sun.cc` (Copyright (C) GarageGames.com, Inc.). Torque3D has no
//! Sky class, and its Sun (`environment/sun.cpp`, Copyright (c) 2012 GarageGames, LLC) sends a different record; the layouts
//! here are TGE's. The code is this crate's own.

use torque_net::cap;

use crate::codec::{Codec, F32, Layout, R, opt};
use crate::{ObjectView, PackCtx, boolean, fld, int, raw, raws, text, word};

/// Sky's mask bits.
pub mod mask {
    pub const INIT: u32 = 1 << 0;
    pub const VISIBILITY: u32 = 1 << 1;
    pub const STORM_CLOUD: u32 = 1 << 2;
    pub const STORM_FOG: u32 = 1 << 3;
    pub const STORM_REAL_FOG: u32 = 1 << 4;
    pub const WIND: u32 = 1 << 5;
    pub const STORM_CLOUDS_ON: u32 = 1 << 6;
    pub const STORM_FOG_ON: u32 = 1 << 7;
}

/// Sun's mask bit.
pub const SUN_UPDATE_MASK: u32 = 1 << 0;

/// The most fog volumes (`MaxFogVolumes`).
pub const MAX_FOG_VOLUMES: usize = 3;
/// The cloud layers (`MAX_NUM_LAYERS`).
pub const CLOUD_LAYERS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FogVolume {
    pub visible_distance: F32,
    pub min_height: F32,
    pub max_height: F32,
    pub color: [F32; 3],
    /// The volume takes part in a storm (a byte).
    pub storm_active: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudLayer {
    pub texture: Vec<u8>,
    pub height: F32,
    pub speed: F32,
}

/// A storm fog under way on an initial update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StormFogProgress {
    pub fog_percentage: F32,
    pub time: F32,
    pub state: u32,
    /// Milliseconds since the storm started.
    pub elapsed: u32,
    pub end_percentage: F32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkyInit {
    pub material_list: Vec<u8>,
    pub fog_color: [F32; 3],
    pub textures_on: u8,
    pub render_box_bottom: u8,
    pub solid_fill_color: [F32; 3],
    pub effect_precipitation: u8,
    pub no_render_bans: bool,
    pub fog_volumes: Vec<FogVolume>,
    pub clouds: [CloudLayer; CLOUD_LAYERS],
    pub wind: [F32; 3],
    pub fog_volume: u32,
    pub storm: Option<StormFogProgress>,
}

/// `Sky`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sky {
    pub init: Option<SkyInit>,
    pub storm_clouds_on: Option<u8>,
    pub storm_fog_on: Option<u8>,
    /// Visible distance and fog distance.
    pub visibility: Option<[F32; 2]>,
    /// State and time.
    pub storm_cloud: Option<(u32, F32)>,
    /// End percentage, time, fog volume.
    pub storm_fog: Option<(F32, F32, u32)>,
    /// Real fog, then max, min, speed.
    pub real_fog: Option<(u32, [F32; 3])>,
    pub wind: Option<[F32; 3]>,
}

fn layout_init<C: Codec>(c: &mut C, s: &mut SkyInit) -> R {
    c.string("sky.materialList", &mut s.material_list)?;
    c.floats("sky.fogColor", &mut s.fog_color)?;
    let mut n = s.fog_volumes.len().min(MAX_FOG_VOLUMES) as u32;
    c.raw32("sky.fogVolumeCount", &mut n)?;
    let n = cap::count(
        "sky fog volumes",
        u64::from(n),
        MAX_FOG_VOLUMES,
        usize::MAX,
        0,
    )?;
    s.fog_volumes.resize_with(n, Default::default);
    c.raw8("sky.texturesOn", &mut s.textures_on)?;
    c.raw8("sky.renderBoxBottom", &mut s.render_box_bottom)?;
    c.floats("sky.solidFillColor", &mut s.solid_fill_color)?;
    c.raw8("sky.effectPrecip", &mut s.effect_precipitation)?;
    c.flag("sky.noRenderBans", &mut s.no_render_bans)?;
    for v in s.fog_volumes.iter_mut() {
        c.raw32("sky.fog.visibleDistance", &mut v.visible_distance.0)?;
        c.raw32("sky.fog.minHeight", &mut v.min_height.0)?;
        c.raw32("sky.fog.maxHeight", &mut v.max_height.0)?;
        c.floats("sky.fog.color", &mut v.color)?;
        c.raw8("sky.fog.stormActive", &mut v.storm_active)?;
    }
    for l in s.clouds.iter_mut() {
        c.string("sky.cloud.texture", &mut l.texture)?;
        c.raw32("sky.cloud.height", &mut l.height.0)?;
        c.raw32("sky.cloud.speed", &mut l.speed.0)?;
    }
    c.floats("sky.wind", &mut s.wind)?;
    c.raw32("sky.fogVolume", &mut s.fog_volume)?;
    opt(c, "sky.storm?", &mut s.storm, |c, p| {
        c.raw32("sky.storm.fogPercentage", &mut p.fog_percentage.0)?;
        c.raw32("sky.storm.time", &mut p.time.0)?;
        c.raw32("sky.storm.state", &mut p.state)?;
        c.raw32("sky.storm.elapsed", &mut p.elapsed)?;
        c.raw32("sky.storm.endPercentage", &mut p.end_percentage.0)
    })
}

impl Layout for Sky {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "sky.init?", &mut self.init, layout_init)?;
        opt(
            c,
            "sky.stormCloudsOn?",
            &mut self.storm_clouds_on,
            |c, b| c.raw8("sky.stormCloudsOn", b),
        )?;
        opt(c, "sky.stormFogOn?", &mut self.storm_fog_on, |c, b| {
            c.raw8("sky.stormFogOn", b)
        })?;
        opt(c, "sky.visibility?", &mut self.visibility, |c, v| {
            c.floats("sky.visibility", v)
        })?;
        opt(c, "sky.stormCloud?", &mut self.storm_cloud, |c, s| {
            c.raw32("sky.stormCloud.state", &mut s.0)?;
            c.raw32("sky.stormCloud.time", &mut s.1.0)
        })?;
        opt(c, "sky.stormFog?", &mut self.storm_fog, |c, s| {
            c.raw32("sky.stormFog.endPercentage", &mut s.0.0)?;
            c.raw32("sky.stormFog.time", &mut s.1.0)?;
            c.raw32("sky.stormFog.fogVolume", &mut s.2)
        })?;
        opt(c, "sky.realFog?", &mut self.real_fog, |c, s| {
            c.raw32("sky.realFog", &mut s.0)?;
            c.floats("sky.realFog.maxMinSpeed", &mut s.1)
        })?;
        opt(c, "sky.wind?", &mut self.wind, |c, w| {
            c.floats("sky.wind", w)
        })
    }
}

fn byte(view: &dyn ObjectView, field: &str) -> u8 {
    u8::from(boolean(view, field, 0))
}

pub fn pack_sky(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> Sky {
    let initial = mask & self::mask::INIT != 0;
    let init = initial.then(|| {
        let n = (int(view, "fog_volume_count", 0).max(0) as usize).min(MAX_FOG_VOLUMES);
        let elapsed = ctx.now_ms.wrapping_sub(word(view, "storm_fog_start_ms", 0));
        let storm_time = fld(view, "storm_fog_time", 0);
        SkyInit {
            material_list: text(view, "material_list", 0),
            fog_color: raws::<3>(view, "fog_color", 0),
            textures_on: byte(view, "sky_textures_on"),
            render_box_bottom: byte(view, "render_box_bottom"),
            solid_fill_color: raws::<3>(view, "solid_fill_color", 0),
            effect_precipitation: byte(view, "effect_precip"),
            no_render_bans: boolean(view, "no_render_bans", 0),
            fog_volumes: (0..n)
                .map(|i| FogVolume {
                    visible_distance: raw(view, "fog_visible_distance", i),
                    min_height: raw(view, "fog_min_height", i),
                    max_height: raw(view, "fog_max_height", i),
                    color: raws::<3>(view, "fog_volume_color", 3 * i),
                    storm_active: u8::from(boolean(view, "storm_fog_active", i)),
                })
                .collect(),
            clouds: std::array::from_fn(|i| CloudLayer {
                texture: text(view, "cloud_text", i),
                height: raw(view, "cloud_height", i),
                speed: raw(view, "cloud_speed", i),
            }),
            wind: raws::<3>(view, "wind_velocity", 0),
            fog_volume: word(view, "fog_volume", 0),
            storm: ((elapsed as f32) / 1000.0 < storm_time).then(|| StormFogProgress {
                fog_percentage: raw(view, "fog_percentage", 0),
                time: F32::new(storm_time),
                state: word(view, "storm_fog_state", 0),
                elapsed,
                end_percentage: raw(view, "storm_fog_end_percentage", 0),
            }),
        }
    });
    Sky {
        init,
        storm_clouds_on: (mask & self::mask::STORM_CLOUDS_ON != 0)
            .then(|| byte(view, "storm_clouds_on")),
        storm_fog_on: (mask & self::mask::STORM_FOG_ON != 0 && !initial)
            .then(|| byte(view, "storm_fog_on")),
        visibility: (mask & self::mask::VISIBILITY != 0).then(|| {
            [
                raw(view, "visible_distance", 0),
                raw(view, "fog_distance", 0),
            ]
        }),
        storm_cloud: (mask & self::mask::STORM_CLOUD != 0).then(|| {
            (
                word(view, "storm_cloud_state", 0),
                raw(view, "storm_cloud_time", 0),
            )
        }),
        storm_fog: (mask & self::mask::STORM_FOG != 0 && !initial).then(|| {
            (
                raw(view, "storm_fog_end_percentage", 0),
                raw(view, "storm_fog_time", 0),
                word(view, "fog_volume", 0),
            )
        }),
        real_fog: (mask & self::mask::STORM_REAL_FOG != 0).then(|| {
            (
                word(view, "real_fog", 0),
                [
                    raw(view, "real_fog_max", 0),
                    raw(view, "real_fog_min", 0),
                    raw(view, "real_fog_speed", 0),
                ],
            )
        }),
        wind: (mask & self::mask::WIND != 0).then(|| raws::<3>(view, "wind_velocity", 0)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SunUpdate {
    pub direction: [F32; 3],
    pub color: [F32; 4],
    pub ambient: [F32; 4],
}

/// `Sun`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sun {
    pub update: Option<SunUpdate>,
}

impl Layout for Sun {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "sun.update?", &mut self.update, |c, u| {
            c.floats("sun.direction", &mut u.direction)?;
            c.floats("sun.color", &mut u.color)?;
            c.floats("sun.ambient", &mut u.ambient)
        })
    }
}

pub fn pack_sun(view: &dyn ObjectView, mask: u32) -> Sun {
    Sun {
        update: (mask & SUN_UPDATE_MASK != 0).then(|| SunUpdate {
            direction: raws::<3>(view, "direction", 0),
            color: raws::<4>(view, "color", 0),
            ambient: raws::<4>(view, "ambient", 0),
        }),
    }
}
