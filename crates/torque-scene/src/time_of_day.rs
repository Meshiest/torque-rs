//! Torque3D's `TimeOfDay` as plain data: the sun's way over a day of `day_length` seconds, its azimuth and elevation from the
//! time of day (0 to 1), the time running while `play` is on (scaled by `day_scale` while the sun is up and `night_scale` while
//! it is down), the time events fired as the sun passes their elevations, and `animate`'s run to a set elevation.
//!
//! - **With an azimuth override** (any nonzero `azimuth_override`, radians; the default is 1): the sun keeps that azimuth and
//!   its elevation is the time times a full turn: 0 rises on the horizon, 0.25 is overhead, 0.5 sets on the far side, 0.75 is
//!   straight down.
//! - **Without** (0): the simple model of a sun at the equator whose declination is the axis tilt: the elevation is
//!   `asin(cos(tilt) cos(2 pi time))` (time 0 is noon, 0.5 midnight), the azimuth `atan2(sin m, -tan(tilt)) + pi` of the meridian
//!   angle `m`; the elevation the events and the day and night scales read is that elevation over the day's highest, in
//!   quarter turns from sunrise.
//!
//! Elevations (the events', [`TimeOfDay::normalized_elevation`]) run 0 to 360 degrees from sunrise: 90 the day's highest, 180
//! sunset, 270 the lowest. Time is wrapped into 0 to 1 (1 itself stays 1). The fields may come from a hostile server: they are
//! held to sane ranges ([`DayFields::sanitized`]) and nothing loops on them.
//!
//! Credit: follows Torque3D's MIT-licensed `environment/timeOfDay.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is
//! this crate's own.

use std::f32::consts::{PI, TAU};

/// A time event: fired when the sun passes `elevation` (degrees from sunrise, as [`TimeOfDay::normalized_elevation`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TimeEvent {
    pub elevation: f32,
    pub identifier: String,
}

/// What a step of the day did, in order: the events it passed, and the end of an `animate` run.
#[derive(Debug, Clone, PartialEq)]
pub enum DayNote {
    /// `onTimeEvent(identifier, time, elevation)`: the time and elevation (degrees, 0 to 360) after the step.
    Event {
        identifier: String,
        time: f32,
        elevation: f32,
    },
    /// `onAnimateDone`.
    AnimateDone,
}

/// Torque3D's `TimeOfDay` fields, with its defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayFields {
    /// Degrees.
    pub axis_tilt: f32,
    /// Seconds a day lasts (at a scale of 1).
    pub day_length: f32,
    /// The time a new day starts at.
    pub start_time: f32,
    pub play: bool,
    /// Radians; 0 for the axis tilt's model.
    pub azimuth_override: f32,
    pub day_scale: f32,
    pub night_scale: f32,
}

impl Default for DayFields {
    fn default() -> DayFields {
        DayFields {
            axis_tilt: 23.44,
            day_length: 120.0,
            start_time: 0.5,
            play: true,
            azimuth_override: 1.0,
            day_scale: 1.0,
            night_scale: 1.5,
        }
    }
}

/// The sun's day: its fields, the time and where the sun is.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeOfDay {
    pub fields: DayFields,
    /// 0 to 1.
    pub time: f32,
    azimuth: f32,
    elevation: f32,
    prev_elevation: f32,
    next_elevation: f32,
    /// Ordered by elevation.
    events: Vec<TimeEvent>,
    /// `animate`'s target (degrees of the time's turn) and speed (degrees a second).
    animating: Option<(f32, f32)>,
}

impl Default for TimeOfDay {
    fn default() -> TimeOfDay {
        TimeOfDay::new(DayFields::default())
    }
}

/// The shortest and longest day (seconds), the largest day and night scales and the fastest `animate` (degrees a second): a
/// day's fields are held to them ([`DayFields::sanitized`]; they may come from a hostile server).
pub const MIN_DAY_LENGTH: f32 = 1.0;
pub const MAX_DAY_LENGTH: f32 = 1.0e7;
pub const MAX_DAY_SCALE: f32 = 1000.0;
pub const MAX_ANIMATE_SPEED: f32 = 36_000.0;
/// The longest step [`TimeOfDay::advance`] takes (seconds).
pub const MAX_STEP: f32 = 10.0;

/// `v` when finite (held to `lo..=hi`), else `default`.
fn held(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    crate::math::finite_clamp(v, lo, hi).unwrap_or(default)
}

/// `t` wrapped into 0 to 1 (a whole number of days off; 1 stays 1); 0 for a value that is not finite.
fn wrap(t: f32) -> f32 {
    if !t.is_finite() {
        return 0.0;
    }
    if (0.0..=1.0).contains(&t) {
        return t;
    }
    let w = t.rem_euclid(1.0);
    if w.is_finite() {
        w.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

impl DayFields {
    /// The fields held to finite, sane ranges: a day of [`MIN_DAY_LENGTH`] to [`MAX_DAY_LENGTH`] seconds, scales of 0 to
    /// [`MAX_DAY_SCALE`], a tilt short of a quarter turn, the start time wrapped into the day; a value that is not finite takes
    /// the default's.
    pub fn sanitized(self) -> DayFields {
        let d = DayFields::default();
        DayFields {
            axis_tilt: held(self.axis_tilt, -89.0, 89.0, d.axis_tilt),
            day_length: held(
                self.day_length,
                MIN_DAY_LENGTH,
                MAX_DAY_LENGTH,
                d.day_length,
            ),
            start_time: wrap(self.start_time),
            play: self.play,
            azimuth_override: held(self.azimuth_override, -1.0e4, 1.0e4, d.azimuth_override),
            day_scale: held(self.day_scale, 0.0, MAX_DAY_SCALE, d.day_scale),
            night_scale: held(self.night_scale, 0.0, MAX_DAY_SCALE, d.night_scale),
        }
    }
}

impl TimeOfDay {
    /// A day at the start time (a new object's: `onAdd` sets the time to it).
    pub fn new(fields: DayFields) -> TimeOfDay {
        let fields = fields.sanitized();
        let mut d = TimeOfDay {
            fields,
            time: wrap(fields.start_time),
            azimuth: 0.0,
            elevation: 0.0,
            prev_elevation: 0.0,
            next_elevation: 0.0,
            events: Vec::new(),
            animating: None,
        };
        d.update_position();
        d.prev_elevation = d.next_elevation;
        d
    }

    /// The sun's azimuth (radians, from +y toward +x).
    pub fn azimuth(&self) -> f32 {
        self.azimuth
    }

    /// The sun's elevation above the horizon (radians; past a quarter turn with an azimuth override: over the top).
    pub fn elevation(&self) -> f32 {
        self.elevation
    }

    /// The elevation the events read (degrees from sunrise, 0 to 360).
    pub fn normalized_elevation(&self) -> f32 {
        self.next_elevation.to_degrees().rem_euclid(360.0)
    }

    /// Toward the sun (unit; z up): the vector of the azimuth and elevation, as Torque's `getVectorFromAngles`.
    pub fn towards(&self) -> [f32; 3] {
        let (sa, ca) = self.azimuth.sin_cos();
        let (se, ce) = self.elevation.sin_cos();
        [sa * ce, ca * ce, se]
    }

    /// Whether the sun is up as the day and night scales read it (from just before sunrise to just past sunset).
    pub fn sun_up(&self) -> bool {
        let e = self.next_elevation.to_degrees();
        e > 350.0 || (0.0..190.0).contains(&e)
    }

    /// The azimuth and elevation of the time now (`_updatePosition`).
    pub fn update_position(&mut self) {
        self.fields = self.fields.sanitized();
        self.time = wrap(self.time);
        self.prev_elevation = self.next_elevation;
        if self.fields.azimuth_override.abs() > 0.0 {
            self.elevation = self.time * TAU;
            self.azimuth = self.fields.azimuth_override;
            self.next_elevation = self.elevation;
        } else {
            let decline = self.fields.axis_tilt.to_radians();
            let meridian = self.time * TAU;
            let elevation_of = |m: f32| (decline.cos() * m.cos()).clamp(-1.0, 1.0).asin();
            self.elevation = elevation_of(meridian);
            self.azimuth = meridian.sin().atan2(-decline.tan()) + PI;
            let top = elevation_of(0.0).max(1e-6);
            let mut normal = PI * self.elevation / (2.0 * top);
            if self.azimuth > PI {
                normal = PI - normal;
            } else if self.elevation < 0.0 {
                normal += TAU;
            }
            self.next_elevation = normal;
        }
    }

    /// Sets the time (wrapped into the day) and fires the events it passed.
    pub fn set_time(&mut self, time: f32) -> Vec<DayNote> {
        self.time = wrap(time);
        self.update_position();
        self.events_passed()
    }

    /// Adds an event at `elevation` degrees (after those at the same elevation).
    pub fn add_event(&mut self, elevation: f32, identifier: &str) {
        if !elevation.is_finite() {
            return;
        }
        let at = self
            .events
            .iter()
            .position(|e| e.elevation > elevation)
            .unwrap_or(self.events.len());
        self.events.insert(
            at,
            TimeEvent {
                elevation,
                identifier: identifier.to_string(),
            },
        );
    }

    pub fn events(&self) -> &[TimeEvent] {
        &self.events
    }

    /// Runs the time to `elevation` degrees of its turn (0 to 360: the time times 360) at `speed` degrees a second, forward
    /// through midnight when it is behind (the speed held to [`MAX_ANIMATE_SPEED`]); false when the time is there already, or
    /// either number is not finite, or the speed is not above 0 (`onAnimateStart` is called on true).
    pub fn animate(&mut self, elevation: f32, speed: f32) -> bool {
        self.animating = None;
        if !elevation.is_finite() || !speed.is_finite() || speed <= 0.0 {
            return false;
        }
        let speed = speed.min(MAX_ANIMATE_SPEED);
        let target = elevation.clamp(0.0, 360.0);
        let current = self.time * 360.0;
        let ahead = if target < current {
            target + 360.0
        } else {
            target
        };
        if (ahead - current).abs() < 1e-6 {
            return false;
        }
        self.animating = Some((target, speed));
        true
    }

    /// The `animate` run under way: its target (degrees of the time's turn) and speed (degrees a second).
    pub fn animating(&self) -> Option<(f32, f32)> {
        self.animating
    }

    /// One step of `dt` seconds (Torque3D's `processTick` takes a tick's 32 ms): the animation's run, else the day while it
    /// plays; the events passed and the animation's end, in order.
    pub fn advance(&mut self, dt: f32) -> Vec<DayNote> {
        let dt = held(dt, 0.0, MAX_STEP, 0.0);
        self.fields = self.fields.sanitized();
        if let Some((target, speed)) = self.animating {
            let mut next = self.time * 360.0 + speed * dt;
            if next > 360.0 {
                next = held(next.rem_euclid(360.0), 0.0, 360.0, 0.0);
            }
            let done = next >= target;
            if done {
                next = target;
                self.animating = None;
            }
            self.time = next / 360.0;
            self.update_position();
            let mut notes = self.events_passed();
            if done {
                notes.push(DayNote::AnimateDone);
            }
            return notes;
        }
        if !self.fields.play {
            self.update_position();
            return Vec::new();
        }
        let scale = if self.sun_up() {
            self.fields.day_scale
        } else {
            self.fields.night_scale
        };
        self.time = wrap(self.time + dt * scale / self.fields.day_length);
        self.update_position();
        self.events_passed()
    }

    /// The events between the last elevation and this one (forward, through sunrise), in order (`_updateTimeEvents`).
    fn events_passed(&self) -> Vec<DayNote> {
        let prev = self.prev_elevation.to_degrees();
        let mut next = self.next_elevation.to_degrees();
        if next < prev {
            next += 360.0;
        }
        let elevation = self.normalized_elevation();
        let mut notes = Vec::new();
        for day in [0.0, 360.0] {
            for e in &self.events {
                let at = e.elevation + day;
                if at > prev && at <= next {
                    notes.push(DayNote::Event {
                        identifier: e.identifier.clone(),
                        time: self.time,
                        elevation,
                    });
                }
            }
        }
        notes
    }
}

#[cfg(test)]
#[path = "time_of_day_tests.rs"]
mod tests;
