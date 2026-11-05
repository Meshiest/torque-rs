//! The engine's own light and sky ghosts (`ghosts::light`: SpotLight, TimeOfDay, ScatterSky) as the enhanced frame's: the spot
//! lights where their ghosts are ([`spot_lights`]), the day run on the client from the orbit the server sent ([`ClientDay`]),
//! and the sky's look ([`scatter_look`]); [`ClientDay::light`] puts the day's light into a frame.
//!
//! - **A spot light** points along its transform's y axis; its `innerAngle` and `outerAngle` are whole cone angles (degrees),
//!   its color (gamma) times `brightness` the light at half its `range` (the falloff's inverse square undone there), its
//!   `range` the light's reach. One mounted on a shape rides the shape's `mount<N>` node ([`mounted_transform`]), its own
//!   transform the offset from the node, and its shape is its carrier (the shape casts no shadow of its own light). A light
//!   switched off (`isEnabled` 0) gives none.
//! - **The day** starts from the orbit's fields and time and runs on each frame's seconds; a new orbit (the server set the
//!   time, the length or play) starts it again from what was sent, and an animation the server sent runs once.
//!
//! Credit: follows Torque3D's MIT-licensed `T3D/spotLight.cpp`, `T3D/lightBase.cpp`, `environment/timeOfDay.cpp` and
//! `environment/scatterSky.cpp` (each Copyright (c) 2012 GarageGames, LLC; `lightBase.cpp` also Copyright (C) 2015 Faust Logic,
//! Inc.); the code here is this crate's own.

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::light::{ScatterUpdate, SpotUpdate, TimeOrbit};
use ghosts::{TgeClass, TgeRecord};
use torque_scene::time_of_day::{DayFields, TimeOfDay};

use crate::Assets;
use crate::hd::scatter::{ScatterLook, ScatterTable, SkyLight};
use crate::hd::{HdLight, HdScene, HdSpot, srgb_to_linear};
use crate::math::Mat;
use crate::mounts::{mounted_transform, pose_matrix, render_pose};
use crate::shapes::Models;
use crate::world::ShapeWorld;

/// A spot light's color at half its range for a brightness of 1: the gain that undoes the falloff there.
pub fn spot_gain(range: f32) -> f32 {
    1.0 + 0.25 * (range * 0.5).powi(2)
}

/// A SpotLight's light at `transform` (its world matrix), `carrier` the shape it rides.
pub fn spot_of(u: &SpotUpdate, transform: &Mat, carrier: Option<u32>) -> HdSpot {
    let range = u.range.get().max(0.05);
    let outer = u.outer_angle.get().clamp(0.01, 179.0);
    let inner = u.inner_angle.get().clamp(0.0, outer);
    let gain = u.brightness.get().max(0.0) * spot_gain(range);
    HdSpot {
        light: HdLight {
            position: transform.column(3),
            color: u
                .color
                .map(|c| srgb_to_linear(c.get().clamp(0.0, 1.0)) * gain),
            radius: range,
            carrier,
        },
        direction: transform.column(1),
        inner: (inner * 0.5).to_radians(),
        outer: (outer * 0.5).to_radians(),
        shadows: u.cast_shadows,
    }
}

/// The spot lights of the client's world that are on: each SpotLight ghost where its record puts it, or on the node of the
/// shape it rides (`models` the shapes' cache, its notes into `notes`).
pub fn spot_lights<W: ShapeWorld<Set = TgeClientSet> + ?Sized>(
    w: &W,
    assets: &dyn Assets,
    models: &mut Models,
    notes: &mut Vec<String>,
) -> Vec<HdSpot> {
    let world = w.records();
    let mut out = Vec::new();
    for (index, g) in world.of_class(TgeClass::SpotLight) {
        let Some(TgeRecord::SpotLight(r)) = g.record.as_ref() else {
            continue;
        };
        let (Some(u), Some(true)) = (r.update.as_ref(), r.enabled) else {
            continue;
        };
        let own = render_pose(w, index)
            .and_then(|p| pose_matrix(&p, None))
            .map_or(Mat::IDENTITY, |(m, _)| m);
        let (transform, carrier) = match world.mount_of(index) {
            Some((on, _)) => match mounted_transform(w, assets, models, notes, index, 0) {
                Some(node) => (Mat::mul(&node, &own), Some(on)),
                None => continue,
            },
            None => (own, None),
        };
        out.push(spot_of(u, &transform, carrier));
    }
    out
}

/// The look of the first ScatterSky's record (none before one is ghosted).
pub fn scatter_look(w: &ClientWorld<TgeClientSet>) -> Option<ScatterLook> {
    w.of_class(TgeClass::ScatterSky)
        .find_map(|(_, g)| match g.record.as_ref() {
            Some(TgeRecord::ScatterSky(r)) => r.update.as_ref().map(look_of),
            _ => None,
        })
}

/// A ScatterSky record's look.
pub fn look_of(u: &ScatterUpdate) -> ScatterLook {
    let rgb = |c: [ghosts::F32; 3]| c.map(|v| v.get());
    ScatterLook {
        sun_scale: rgb(u.sun_scale),
        ambient_scale: rgb(u.ambient_scale),
        fog_scale: rgb(u.fog_scale),
        brightness: u.brightness.get(),
        night_color: rgb(u.night_color),
        night_fog_color: rgb(u.night_fog_color),
        moon_enabled: u.moon_enabled,
        moon_azimuth: u.moon_azimuth.get(),
        moon_elevation: u.moon_elevation.get(),
        moon_light_color: rgb(u.moon_light_color).map(srgb_to_linear),
        ..ScatterLook::default()
    }
}

/// The day as a client runs it: the last orbit the server sent and the day run from it.
#[derive(Debug, Clone, Default)]
pub struct ClientDay {
    pub day: Option<TimeOfDay>,
    orbit: Option<TimeOrbit>,
    animate: Option<[ghosts::F32; 2]>,
}

impl ClientDay {
    /// Follows the first TimeOfDay of `w` for `dt` seconds: a new orbit starts the day from it, an animation sent runs once.
    pub fn update(&mut self, w: &ClientWorld<TgeClientSet>, dt: f32) {
        let record = w
            .of_class(TgeClass::TimeOfDay)
            .find_map(|(_, g)| match g.record.as_ref() {
                Some(TgeRecord::TimeOfDay(r)) => Some(r.clone()),
                _ => None,
            });
        let Some(r) = record else {
            self.day = None;
            self.orbit = None;
            return;
        };
        if let Some(o) = r.orbit
            && self.orbit != Some(o)
        {
            self.orbit = Some(o);
            let mut day = TimeOfDay::new(DayFields {
                axis_tilt: o.axis_tilt.get(),
                day_length: o.day_length.get(),
                start_time: o.start_time.get(),
                play: o.play,
                azimuth_override: o.azimuth_override.get(),
                day_scale: o.day_scale.get(),
                night_scale: o.night_scale.get(),
            });
            day.set_time(o.time.get());
            self.day = Some(day);
        }
        if r.animate.is_some() && r.animate != self.animate {
            self.animate = r.animate;
            if let (Some(day), Some([t, s])) = (self.day.as_mut(), r.animate) {
                day.animate(t.get(), s.get());
            }
        }
        if let Some(day) = self.day.as_mut() {
            day.advance(dt.max(0.0));
        }
    }

    /// The light of the day now under `look` (none without a day).
    pub fn light(&self, look: &ScatterLook) -> Option<SkyLight> {
        let day = self.day.as_ref()?;
        Some(ScatterTable::earth().light(day.towards(), look))
    }

    /// Lights `scene` by the day now under `look`: the sun (or the moon), the fog's color and the clear, and the physical sky
    /// made for the sun; false (nothing changed) without a day.
    pub fn apply<G>(&self, scene: &mut HdScene<G>, look: &ScatterLook) -> bool {
        let (Some(day), Some(light)) = (self.day.as_ref(), self.light(look)) else {
            return false;
        };
        scene.sun = light.sun();
        scene.fog = light.fog(scene.fog);
        scene.clear = light.fog;
        scene.atmosphere_sun = Some(day.towards());
        true
    }
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;
