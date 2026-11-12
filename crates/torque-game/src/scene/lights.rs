//! The engine's own light and sky classes on a server, after Torque3D's (their records are `torque_ghosts::light`'s, with the same
//! mask bits):
//!
//! - **SpotLight:** `setLightEnabled(state)` sets `isEnabled` and sends it; `inspectPostApply()` sends every field again. A
//!   ShapeBase mounts one with `mountObject(light, slot)` ([`mount_spot`]: the light then rides the shape's mount node `slot`,
//!   its own transform the offset from it) and `unmountObject(light)` lets it go; the link is the light's ([`spot_mount`]).
//! - **TimeOfDay:** its day ([`torque_scene::time_of_day::TimeOfDay`]) is made from its fields when it is added ([`add_day`]) and
//!   kept in its ext; `setTimeOfDay`, `setPlay`, `setDayLength`, `addTimeOfDayEvent` and `animate` change it and send the orbit
//!   (or the animation); each tick ([`tick_day`]) advances it, keeps its `time` field current (a client that joins later starts
//!   there) and calls `onTimeEvent(identifier, time, elevation)` and `onAnimateDone()` on the object (`animate` calls
//!   `onAnimateStart()`). The clients run the day themselves from the orbit they were sent.
//! - **ScatterSky:** `inspectPostApply()` sends its fields again.
//!
//! The server's tests run them from script (`torque-server`'s `tests/lights.rs`).
//!
//! Credit: follows Torque3D's MIT-licensed `T3D/lightBase.cpp`, `T3D/spotLight.cpp`, `environment/timeOfDay.cpp` and
//! `environment/scatterSky.cpp` (each Copyright (c) 2012 GarageGames, LLC; `lightBase.cpp` also Copyright (C) 2015 Faust Logic,
//! Inc.); the code here is this crate's own.

use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::text::format_g;
use ts_vm::vm::{Native, Vm, VmError};

use torque_scene::time_of_day::{DayFields, DayNote, TimeOfDay};

use crate::methods::register_method;
use crate::scene::access::{bool_field, set_field, value_of};
use crate::scene::camera::is_class;
use crate::scene::ext;
use crate::scene::ghost as ghosts;

/// SpotLight's record bits.
pub const SPOT_TRANSFORM_MASK: u32 = 1 << 0;
pub const SPOT_UPDATE_MASK: u32 = 1 << 1;
pub const SPOT_ENABLED_MASK: u32 = 1 << 2;
pub const SPOT_MOUNT_MASK: u32 = 1 << 3;
/// TimeOfDay's record bits.
pub const TIME_ORBIT_MASK: u32 = 1 << 0;
pub const TIME_ANIMATE_MASK: u32 = 1 << 1;
/// ScatterSky's record bit.
pub const SCATTER_UPDATE_MASK: u32 = 1 << 0;
/// A tick's seconds.
const TICK_SECONDS: f32 = collide::types::TICK_SECONDS;

fn float(vm: &Vm, id: ObjId, name: &'static [u8], default: f32) -> f32 {
    match value_of(vm, id, name) {
        Some(Value::Float(f)) => f,
        _ => default,
    }
}

/// Whether `id` is a SpotLight.
pub fn is_spot(vm: &Vm, id: ObjId) -> bool {
    is_class(vm, id, "SpotLight")
}

/// The shape a SpotLight rides and its mount node.
pub fn spot_mount(vm: &Vm, id: ObjId) -> Option<(ObjId, u32)> {
    ext::get(vm.objects(), id)?
        .spot_mount
        .filter(|(on, _)| vm.objects().contains(*on))
}

/// Mounts the SpotLight `light` on `shape`'s node `node` (none: unmounts it) and sends it.
pub fn mount_spot(vm: &mut Vm, light: ObjId, mount: Option<(ObjId, u32)>) {
    if let Some(e) = ext::get_mut(vm.objects_mut(), light) {
        e.spot_mount = mount;
    }
    ghosts::mark(vm, light, SPOT_MOUNT_MASK);
}

/// The TimeOfDay's fields as its object has them.
fn day_fields(vm: &Vm, id: ObjId) -> DayFields {
    let d = DayFields::default();
    DayFields {
        axis_tilt: float(vm, id, b"axisTilt", d.axis_tilt),
        day_length: float(vm, id, b"dayLength", d.day_length),
        start_time: float(vm, id, b"startTime", d.start_time),
        play: bool_field(vm, id, b"play"),
        azimuth_override: float(vm, id, b"azimuthOverride", d.azimuth_override),
        day_scale: float(vm, id, b"dayScale", d.day_scale),
        night_scale: float(vm, id, b"nightScale", d.night_scale),
    }
}

/// A TimeOfDay's day made from its fields, at its start time (`onAdd`).
pub fn add_day(vm: &mut Vm, id: ObjId) {
    let day = TimeOfDay::new(day_fields(vm, id));
    set_field(vm, id, b"time", Value::Float(day.time));
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.day = Some(Box::new(day));
    }
}

/// The TimeOfDay's `animate` under way: its target and speed (the record's animation block).
pub fn day_animation(vm: &Vm, id: ObjId) -> Option<(f32, f32)> {
    ext::get(vm.objects(), id)?.day.as_ref()?.animating()
}

/// Runs `f` on the TimeOfDay's day (none for an object without one) and returns what it gave.
fn with_day<R>(vm: &mut Vm, id: ObjId, f: impl FnOnce(&mut TimeOfDay) -> R) -> Option<R> {
    let day = ext::get_mut(vm.objects_mut(), id)?.day.as_mut()?;
    Some(f(day))
}

/// Calls the notes' callbacks on the object.
fn notify(vm: &mut Vm, id: ObjId, notes: Vec<DayNote>) -> Result<(), VmError> {
    for n in notes {
        match n {
            DayNote::Event {
                identifier,
                time,
                elevation,
            } => {
                let args = [
                    identifier.into_bytes(),
                    format_g(f64::from(time)).into_bytes(),
                    format_g(f64::from(elevation)).into_bytes(),
                ];
                vm.call_method(id, b"onTimeEvent", &args)?;
            }
            DayNote::AnimateDone => {
                vm.call_method(id, b"onAnimateDone", &[])?;
            }
        }
    }
    Ok(())
}

/// One tick of a TimeOfDay: the day advanced, its `time` field kept, its callbacks called.
pub fn tick_day(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    let Some((notes, time)) = with_day(vm, id, |d| (d.advance(TICK_SECONDS), d.time)) else {
        return Ok(());
    };
    set_field(vm, id, b"time", Value::Float(time));
    notify(vm, id, notes)
}

/// The changed day's fields written back and its orbit sent.
fn day_changed(vm: &mut Vm, id: ObjId) {
    let Some((fields, time)) = with_day(vm, id, |d| (d.fields, d.time)) else {
        return;
    };
    set_field(vm, id, b"time", Value::Float(time));
    set_field(vm, id, b"play", Value::Bool(fields.play));
    set_field(vm, id, b"dayLength", Value::Float(fields.day_length));
    ghosts::mark(vm, id, TIME_ORBIT_MASK);
}

fn fail(r: Result<(), VmError>) -> Native {
    match r {
        Ok(()) => Native::Void,
        Err(e) => Native::Fail(e),
    }
}

/// The console methods.
pub fn register(vm: &mut Vm) {
    register_method(vm, "SpotLight", "setLightEnabled", |vm, c| {
        if let Some(this) = c.this_object {
            set_field(vm, this, b"isEnabled", Value::Bool(c.boolean(2)));
            ghosts::mark(vm, this, SPOT_ENABLED_MASK);
        }
        Native::Void
    });
    register_method(vm, "SpotLight", "inspectPostApply", |vm, c| {
        if let Some(this) = c.this_object {
            ghosts::mark(
                vm,
                this,
                SPOT_TRANSFORM_MASK | SPOT_UPDATE_MASK | SPOT_ENABLED_MASK,
            );
        }
        Native::Void
    });
    register_method(vm, "ScatterSky", "inspectPostApply", |vm, c| {
        if let Some(this) = c.this_object {
            ghosts::mark(vm, this, SCATTER_UPDATE_MASK);
        }
        Native::Void
    });
    register_method(vm, "TimeOfDay", "setTimeOfDay", |vm, c| {
        let Some(this) = c.this_object else {
            return Native::Void;
        };
        let time = c.float(2);
        let notes = with_day(vm, this, |d| d.set_time(time)).unwrap_or_default();
        day_changed(vm, this);
        fail(notify(vm, this, notes))
    });
    register_method(vm, "TimeOfDay", "setPlay", |vm, c| {
        if let Some(this) = c.this_object {
            let on = c.boolean(2);
            with_day(vm, this, |d| d.fields.play = on);
            day_changed(vm, this);
        }
        Native::Void
    });
    register_method(vm, "TimeOfDay", "setDayLength", |vm, c| {
        if let Some(this) = c.this_object {
            let length = c.float(2);
            if length != 0.0 {
                with_day(vm, this, |d| d.fields.day_length = length);
                day_changed(vm, this);
            }
        }
        Native::Void
    });
    register_method(vm, "TimeOfDay", "addTimeOfDayEvent", |vm, c| {
        if let Some(this) = c.this_object {
            let elevation = c.float(2);
            let name = String::from_utf8_lossy(c.cstr(3)).into_owned();
            with_day(vm, this, |d| d.add_event(elevation, &name));
        }
        Native::Void
    });
    register_method(vm, "TimeOfDay", "animate", |vm, c| {
        let Some(this) = c.this_object else {
            return Native::Void;
        };
        let (elevation, speed) = (c.float(2), c.float(3));
        if with_day(vm, this, |d| d.animate(elevation, speed)) == Some(true) {
            ghosts::mark(vm, this, TIME_ANIMATE_MASK);
            return fail(vm.call_method(this, b"onAnimateStart", &[]).map(|_| ()));
        }
        Native::Void
    });
}
