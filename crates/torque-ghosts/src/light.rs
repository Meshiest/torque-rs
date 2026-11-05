//! The engine's own classes after Torque3D's lights and sky: `SpotLight`, `TimeOfDay` and `ScatterSky`. They are not in TGE 1.4's
//! class list ([`crate::tge::TGE_OBJECT_CLASSES`]): a server ghosts them only to a client of the engine's own, with the class ids
//! past the end of the TGE table ([`ENGINE_OBJECT_CLASSES`], in order), so TGE's ids and records are unchanged.
//!
//! ```text
//! SpotLight:  flag(Transform, bit 0): affine transform (193)
//!             flag(Update, bit 1): F32 x3 color, F32 brightness, flag cast shadows, F32 range, F32 inner angle, F32 outer angle
//!             flag(Enabled, bit 2): flag enabled
//!             flag(Mount, bit 3): flag(mounted): ranged(index, 0, MaxGhostCount) (GB + 1 bits), raw byte node
//! TimeOfDay:  flag(Orbit, bit 0): F32 start time, F32 day length, F32 time, F32 axis tilt, F32 azimuth override, F32 day scale,
//!             F32 night scale, flag play
//!             flag(Animate, bit 1): F32 target (degrees of the time's turn), F32 speed (degrees a second)
//! ScatterSky: flag(Update, bit 0): F32 x3 sun scale, F32 x3 ambient scale, F32 x3 fog scale, F32 brightness, F32 x3 night
//!             color, F32 x3 night fog color, flag moon enabled, F32 moon azimuth, F32 moon elevation, F32 x3 moon light color,
//!             flag cast shadows
//! ```
//! A mounted object with no ghost index yet goes out as `U32(-1)` cut to the field (all ones), as fxLight's attachment, and
//! the mount is sent again once it has one (the pack's return mask keeps [`SPOT_MOUNT_MASK`]).
//!
//! **View fields:** SpotLight: `transform`, `color` 0..3, `brightness`, `castShadows`, `range`, `innerAngle`, `outerAngle`,
//! `isEnabled`, the link `mount_object` and `mount_node`; TimeOfDay: `startTime`, `dayLength`, `time`, `axisTilt`,
//! `azimuthOverride`, `dayScale`, `nightScale`, `play`, `animate_time`, `animate_speed`; ScatterSky: `sunScale`, `ambientScale`,
//! `fogScale` (0..3), `brightness`, `nightColor`, `nightFogColor` (0..3), `moonEnabled`, `moonAzimuth`, `moonElevation`,
//! `moonLightColor` (0..3), `castShadows`.
//!
//! Credit: the classes and their fields follow Torque3D's MIT-licensed `T3D/lightBase.cpp`, `T3D/spotLight.cpp`,
//! `environment/timeOfDay.cpp` and `environment/scatterSky.cpp` (each Copyright (c) 2012 GarageGames, LLC; `lightBase.cpp` also
//! Copyright (C) 2015 Faust Logic, Inc.); the records are this crate's own.

use crate::codec::{Codec, F32, Layout, R, Xf, ghost_ranged, opt, wrap_ranged};
use crate::{Link, ObjectView, PackCtx, boolean, int, raw, raws, xform};

/// The engine's own object classes, in the order of their class ids past the TGE table's.
pub const ENGINE_OBJECT_CLASSES: [&str; 3] = ["SpotLight", "TimeOfDay", "ScatterSky"];

/// SpotLight's mask bits.
pub const SPOT_TRANSFORM_MASK: u32 = 1 << 0;
pub const SPOT_UPDATE_MASK: u32 = 1 << 1;
pub const SPOT_ENABLED_MASK: u32 = 1 << 2;
pub const SPOT_MOUNT_MASK: u32 = 1 << 3;
/// TimeOfDay's mask bits.
pub const TIME_ORBIT_MASK: u32 = 1 << 0;
pub const TIME_ANIMATE_MASK: u32 = 1 << 1;
/// ScatterSky's mask bit.
pub const SCATTER_UPDATE_MASK: u32 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpotUpdate {
    pub color: [F32; 3],
    pub brightness: F32,
    pub cast_shadows: bool,
    pub range: F32,
    pub inner_angle: F32,
    pub outer_angle: F32,
}

/// Where a spot light is mounted: the object's ghost index and the node (its mount slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpotMount {
    pub object: u32,
    pub node: u8,
}

/// `SpotLight`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpotLight {
    pub transform: Option<Xf>,
    pub update: Option<SpotUpdate>,
    pub enabled: Option<bool>,
    /// `None` no mount block, `Some(None)` unmounted.
    pub mount: Option<Option<SpotMount>>,
}

impl Layout for SpotLight {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "spot.transform?", &mut self.transform, |c, t| {
            c.xform("spot.transform", t)
        })?;
        opt(c, "spot.update?", &mut self.update, |c, u| {
            c.floats("spot.color", &mut u.color)?;
            c.raw32("spot.brightness", &mut u.brightness.0)?;
            c.flag("spot.castShadows", &mut u.cast_shadows)?;
            c.raw32("spot.range", &mut u.range.0)?;
            c.raw32("spot.innerAngle", &mut u.inner_angle.0)?;
            c.raw32("spot.outerAngle", &mut u.outer_angle.0)
        })?;
        opt(c, "spot.enabled?", &mut self.enabled, |c, e| {
            c.flag("spot.enabled", e)
        })?;
        opt(c, "spot.mount?", &mut self.mount, |c, m| {
            opt(c, "spot.mounted", m, |c, m| {
                ghost_ranged(c, "spot.object", &mut m.object)?;
                c.raw8("spot.node", &mut m.node)
            })
        })
    }
}

pub fn pack_spot_light(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> SpotLight {
    let hi = 1u32
        .checked_shl(u32::from(ctx.ghost_bits()))
        .unwrap_or(u32::MAX);
    let node = int(view, "mount_node", 0).clamp(0, 255) as u8;
    SpotLight {
        transform: (mask & SPOT_TRANSFORM_MASK != 0).then(|| xform(view, "transform")),
        update: (mask & SPOT_UPDATE_MASK != 0).then(|| SpotUpdate {
            color: raws::<3>(view, "color", 0),
            brightness: raw(view, "brightness", 0),
            cast_shadows: boolean(view, "castShadows", 0),
            range: raw(view, "range", 0),
            inner_angle: raw(view, "innerAngle", 0),
            outer_angle: raw(view, "outerAngle", 0),
        }),
        enabled: (mask & SPOT_ENABLED_MASK != 0).then(|| boolean(view, "isEnabled", 0)),
        mount: (mask & SPOT_MOUNT_MASK != 0).then(|| match view.link("mount_object") {
            Link::Null => None,
            Link::Ghost(g) => Some(SpotMount {
                object: wrap_ranged(g, 0, hi),
                node,
            }),
            Link::NotGhosted => Some(SpotMount {
                object: wrap_ranged(u32::MAX, 0, hi),
                node,
            }),
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimeOrbit {
    pub start_time: F32,
    pub day_length: F32,
    pub time: F32,
    pub axis_tilt: F32,
    pub azimuth_override: F32,
    pub day_scale: F32,
    pub night_scale: F32,
    pub play: bool,
}

/// `TimeOfDay`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeOfDay {
    pub orbit: Option<TimeOrbit>,
    /// `animate`'s target and speed.
    pub animate: Option<[F32; 2]>,
}

impl Layout for TimeOfDay {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "time.orbit?", &mut self.orbit, |c, o| {
            c.raw32("time.startTime", &mut o.start_time.0)?;
            c.raw32("time.dayLength", &mut o.day_length.0)?;
            c.raw32("time.time", &mut o.time.0)?;
            c.raw32("time.axisTilt", &mut o.axis_tilt.0)?;
            c.raw32("time.azimuthOverride", &mut o.azimuth_override.0)?;
            c.raw32("time.dayScale", &mut o.day_scale.0)?;
            c.raw32("time.nightScale", &mut o.night_scale.0)?;
            c.flag("time.play", &mut o.play)
        })?;
        opt(c, "time.animate?", &mut self.animate, |c, a| {
            c.floats("time.animate", a)
        })
    }
}

pub fn pack_time_of_day(view: &dyn ObjectView, mask: u32) -> TimeOfDay {
    TimeOfDay {
        orbit: (mask & TIME_ORBIT_MASK != 0).then(|| TimeOrbit {
            start_time: raw(view, "startTime", 0),
            day_length: raw(view, "dayLength", 0),
            time: raw(view, "time", 0),
            axis_tilt: raw(view, "axisTilt", 0),
            azimuth_override: raw(view, "azimuthOverride", 0),
            day_scale: raw(view, "dayScale", 0),
            night_scale: raw(view, "nightScale", 0),
            play: boolean(view, "play", 0),
        }),
        animate: (mask & TIME_ANIMATE_MASK != 0)
            .then(|| [raw(view, "animate_time", 0), raw(view, "animate_speed", 0)]),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScatterUpdate {
    pub sun_scale: [F32; 3],
    pub ambient_scale: [F32; 3],
    pub fog_scale: [F32; 3],
    pub brightness: F32,
    pub night_color: [F32; 3],
    pub night_fog_color: [F32; 3],
    pub moon_enabled: bool,
    pub moon_azimuth: F32,
    pub moon_elevation: F32,
    pub moon_light_color: [F32; 3],
    pub cast_shadows: bool,
}

/// `ScatterSky`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScatterSky {
    pub update: Option<ScatterUpdate>,
}

impl Layout for ScatterSky {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "scatter.update?", &mut self.update, |c, u| {
            c.floats("scatter.sunScale", &mut u.sun_scale)?;
            c.floats("scatter.ambientScale", &mut u.ambient_scale)?;
            c.floats("scatter.fogScale", &mut u.fog_scale)?;
            c.raw32("scatter.brightness", &mut u.brightness.0)?;
            c.floats("scatter.nightColor", &mut u.night_color)?;
            c.floats("scatter.nightFogColor", &mut u.night_fog_color)?;
            c.flag("scatter.moonEnabled", &mut u.moon_enabled)?;
            c.raw32("scatter.moonAzimuth", &mut u.moon_azimuth.0)?;
            c.raw32("scatter.moonElevation", &mut u.moon_elevation.0)?;
            c.floats("scatter.moonLightColor", &mut u.moon_light_color)?;
            c.flag("scatter.castShadows", &mut u.cast_shadows)
        })
    }
}

pub fn pack_scatter_sky(view: &dyn ObjectView, mask: u32) -> ScatterSky {
    ScatterSky {
        update: (mask & SCATTER_UPDATE_MASK != 0).then(|| ScatterUpdate {
            sun_scale: raws::<3>(view, "sunScale", 0),
            ambient_scale: raws::<3>(view, "ambientScale", 0),
            fog_scale: raws::<3>(view, "fogScale", 0),
            brightness: raw(view, "brightness", 0),
            night_color: raws::<3>(view, "nightColor", 0),
            night_fog_color: raws::<3>(view, "nightFogColor", 0),
            moon_enabled: boolean(view, "moonEnabled", 0),
            moon_azimuth: raw(view, "moonAzimuth", 0),
            moon_elevation: raw(view, "moonElevation", 0),
            moon_light_color: raws::<3>(view, "moonLightColor", 0),
            cast_shadows: boolean(view, "castShadows", 0),
        }),
    }
}
