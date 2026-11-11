//! The datablock classes' value checks: what `onAdd` and `preload` check, clamp and warn about for the classes whose packers
//! read what they leave behind, and the error texts of a shape constructor's sequence import. [`DatablockRules`] is the seam: a
//! game installs its own ([`set_rules`]); without one they are TGE's ([`TorqueRules`]).
//!
//! TGE's checks follow `DebrisData::onAdd` (`game/debris.cc`), `ExplosionData::onAdd` (`game/fx/explosion.cc`),
//! `ParticleData::loadParameters` and `ParticleEmitterData::loadParameters` (`game/fx/particleEngine.cc`),
//! `AudioDescription::onAdd` and `AudioProfile::onAdd` (`audio/audioDataBlock.cc`), `PlayerData::preload` (`game/player.cc`),
//! `WheeledVehicleTire::preload` (`game/vehicles/wheeledVehicle.cc`) and `TSShapeConstructor::preload`
//! (`ts/tsShapeConstruct.cc`), as Torque3D, the MIT release of that engine, keeps them: `T3D/debris.cpp`,
//! `T3D/fx/explosion.cpp`, `T3D/fx/particle.cpp`, `T3D/fx/particleEmitter.cpp`, `sfx/sfxDescription.cpp` and `T3D/player.cpp`
//! (each `Copyright (c) 2012 GarageGames, LLC` and `Copyright (C) 2015 Faust Logic, Inc.`);
//! `T3D/vehicles/wheeledVehicle.cpp` and `ts/tsShapeConstruct.cpp` (each `Copyright (c) 2012 GarageGames, LLC`). The code here
//! is this crate's own.

use std::rc::Rc;

use ts_vm::classes::EngineClass;
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError};

use super::DbValues;
use super::fields::{f32_of, i32_of, object_of, put, shown_name, text_of, vec3_of};

/// The datablock classes' value checks a host may replace. Every method has TGE's behaviour by default.
pub trait DatablockRules {
    /// `AudioDescription`'s add-time behaviour.
    fn audio_description(&self) -> Rc<dyn EngineClass> {
        Rc::new(AudioDescriptionAdd)
    }

    /// `AudioProfile`'s add-time behaviour.
    fn audio_profile(&self) -> Rc<dyn EngineClass> {
        Rc::new(AudioProfileAdd)
    }

    /// `DebrisData`'s add-time behaviour.
    fn debris(&self) -> Rc<dyn EngineClass> {
        Rc::new(DebrisDataAdd)
    }

    /// `ExplosionData`'s add-time behaviour.
    fn explosion(&self) -> Rc<dyn EngineClass> {
        Rc::new(ExplosionDataAdd)
    }

    /// `ParticleData`'s add-time behaviour.
    fn particle(&self) -> Rc<dyn EngineClass> {
        Rc::new(ParticleDataAdd)
    }

    /// `ParticleEmitterData`'s add-time behaviour, keeping the ids of its particles in `values`.
    fn particle_emitter(&self, values: &DbValues) -> Rc<dyn EngineClass> {
        Rc::new(ParticleEmitterDataAdd {
            values: values.clone(),
        })
    }

    /// `ProjectileData`'s add-time behaviour, if it has one (TGE's only looks its datablocks up).
    fn projectile(&self) -> Option<Rc<dyn EngineClass>> {
        None
    }

    /// `ParticleData::loadParameters` (the add and `reload()`): false refuses the datablock.
    fn particle_checks(&self, vm: &mut Vm, id: ObjId) -> bool {
        torque_particle_checks(vm, id)
    }

    /// `ParticleEmitterData::loadParameters` (the add and `reload()`): whether it succeeded, and the ids of the particles its
    /// `particles` text names when it got that far. `has_ids` is whether the emitter has ids already (a client's, unpacked).
    fn emitter_checks(&self, vm: &mut Vm, id: ObjId, has_ids: bool) -> (bool, Option<Vec<i32>>) {
        torque_emitter_checks(vm, id, has_ids)
    }

    /// The steps of `PlayerData::preload` before its shape is loaded.
    fn player_preload_clamps(&self, vm: &mut Vm, id: ObjId) {
        if f32_of(vm, id, "minJumpEnergy", 0) < f32_of(vm, id, "jumpEnergyDrain", 0) {
            let drain = f32_of(vm, id, "jumpEnergyDrain", 0);
            put(vm, id, "minJumpEnergy", 0, Value::Float(drain));
        }
        if i32_of(vm, id, "jumpDelay", 0) > JUMP_DELAY_MAX {
            put(vm, id, "jumpDelay", 0, Value::Int(JUMP_DELAY_MAX));
            vm.print(
                format!("PlayerData:: Jump delay exceeds range (0-{JUMP_DELAY_MAX})").as_bytes(),
            );
        }
    }

    /// The steps of `PlayerData::preload` once its shape is loaded: `maxJumpSpeed` above `minJumpSpeed`, `pickupRadius` between
    /// the larger horizontal side of the bounding box and twice that.
    fn player_shape_clamps(&self, vm: &mut Vm, id: ObjId) {
        let min = f32_of(vm, id, "minJumpSpeed", 0);
        if f32_of(vm, id, "maxJumpSpeed", 0) <= min {
            put(vm, id, "maxJumpSpeed", 0, Value::Float(min + 0.1));
        }
        let b = vec3_of(vm, id, "boundingBox");
        let dr = if b[0] > b[1] { b[0] } else { b[1] };
        let radius = f32_of(vm, id, "pickupRadius", 0);
        if radius < dr {
            put(vm, id, "pickupRadius", 0, Value::Float(dr));
        } else if radius > 2.0 * dr {
            put(vm, id, "pickupRadius", 0, Value::Float(2.0 * dr));
        }
    }

    /// `WheeledVehicleTire::preload`'s radius from the loaded shape's bounds (`[min x, min y, min z, max x, max y, max z]`):
    /// half their height.
    fn tire_radius(&self, bounds: &[f32; 6]) -> f32 {
        (bounds[5] - bounds[2]) / 2.0
    }

    /// Constructor defaults a host's classes have that the class table lacks (TGE's table has them all).
    fn constructor_defaults(&self, vm: &mut Vm) {
        let _ = vm;
    }

    /// What a shape-carrying datablock does when the shape its preload names does not load (`path` as its field gives it):
    /// `Ok(())` keeps the datablock with no shape (after any line the rules print), `Err(text)` refuses it with `text` in the
    /// error buffer. TGE's refuses it with [`couldnt_load_shape`].
    fn shape_not_loaded(&self, vm: &mut Vm, class: ShapeClass, path: &str) -> Result<(), String> {
        let _ = vm;
        Err(couldnt_load_shape(class, path))
    }

    /// Whether `TSShapeConstructor::preload`'s loop over the `sequence<n>` fields ends at the first one never assigned. TGE's
    /// skips such a field and goes on to the last.
    fn constructor_stops_at_unset(&self) -> bool {
        false
    }

    /// The error text when a shape constructor's sequence file is not there.
    fn missing_sequence(&self, sequence: &[u8], shape: &[u8]) -> String {
        error_text(format!(
            "Missing sequence {} for {}",
            String::from_utf8_lossy(sequence),
            String::from_utf8_lossy(shape)
        ))
    }

    /// The error text when a shape constructor's sequence file does not import.
    fn load_sequence_failed(&self, sequence: &[u8], shape: &[u8]) -> String {
        error_text(format!(
            "Load sequence {} failed for {}",
            String::from_utf8_lossy(sequence),
            String::from_utf8_lossy(shape)
        ))
    }
}

/// The shape-carrying datablock classes whose failed shape load [`DatablockRules::shape_not_loaded`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeClass {
    /// `ShapeBaseData` and its subclasses (`shapeFile`).
    ShapeBase,
    /// `ExplosionData` (`explosionShape`).
    Explosion,
    /// `WheeledVehicleTire` (`shapeFile`).
    Tire,
}

/// TGE's error buffer when a datablock's shape does not load: `ShapeBaseData: Couldn't load shape "<path>"` and
/// `WheeledVehicleTire: ...` in full, and for `ExplosionData` the three characters its `dSprintf` with `sizeof(errorBuffer)`
/// (the size of a pointer) keeps.
pub fn couldnt_load_shape(class: ShapeClass, path: &str) -> String {
    match class {
        ShapeClass::ShapeBase => {
            error_text(format!("ShapeBaseData: Couldn't load shape \"{path}\""))
        }
        ShapeClass::Explosion => "Exp".to_string(),
        ShapeClass::Tire => error_text(format!(
            "WheeledVehicleTire: Couldn't load shape \"{path}\""
        )),
    }
}

/// The largest `jumpDelay` (it is sent in 7 bits).
pub const JUMP_DELAY_MAX: i32 = 127;

/// A preload's error buffer holds 255 bytes and the NUL.
fn error_text(mut text: String) -> String {
    if text.len() > 255 {
        let mut end = 255;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// TGE's rules.
#[derive(Debug, Default, Clone, Copy)]
pub struct TorqueRules;

impl DatablockRules for TorqueRules {}

struct Installed(Rc<dyn DatablockRules>);

/// The host's rules for `vm` from now on (before the shape classes are registered).
pub fn set_rules(vm: &mut Vm, rules: Rc<dyn DatablockRules>) {
    vm.set_extension(Rc::new(Installed(rules)));
}

/// The rules `vm`'s game installed, else TGE's.
pub fn rules(vm: &Vm) -> Rc<dyn DatablockRules> {
    vm.extension::<Installed>().map_or_else(
        || Rc::new(TorqueRules) as Rc<dyn DatablockRules>,
        |i| Rc::clone(&i.0),
    )
}

/// `mClampF(x, lo, hi)`.
fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// `AudioDescription::onAdd`: the volume, loop gaps, distances, cone and environment level clamped into range.
pub struct AudioDescriptionAdd;

impl EngineClass for AudioDescriptionAdd {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let f = |vm: &Vm, field: &str| f32_of(vm, id, field, 0);
        let i = |vm: &Vm, field: &str| i32_of(vm, id, field, 0);
        put(
            vm,
            id,
            "volume",
            0,
            Value::Float(clamp(f(vm, "volume"), 0.0, 1.0)),
        );
        let loops = i(vm, "loopCount");
        put(vm, id, "loopCount", 0, Value::Int(loops.max(-1)));
        let (min_gap, max_gap) = (i(vm, "minLoopGap"), i(vm, "maxLoopGap"));
        let max_gap = max_gap.max(min_gap);
        put(vm, id, "maxLoopGap", 0, Value::Int(max_gap));
        put(
            vm,
            id,
            "minLoopGap",
            0,
            Value::Int(min_gap.clamp(0, max_gap.max(0))),
        );
        if super::fields::bool_of(vm, id, "is3D", 0) {
            let reference = f(vm, "ReferenceDistance").max(0.0);
            put(vm, id, "ReferenceDistance", 0, Value::Float(reference));
            let max_distance = f(vm, "maxDistance");
            let max_distance = if max_distance > reference {
                max_distance
            } else {
                reference + 0.01
            };
            put(vm, id, "maxDistance", 0, Value::Float(max_distance));
            let inside = i(vm, "coneInsideAngle").clamp(0, 360);
            put(vm, id, "coneInsideAngle", 0, Value::Int(inside));
            let outside = i(vm, "coneOutsideAngle").clamp(inside, 360);
            put(vm, id, "coneOutsideAngle", 0, Value::Int(outside));
            let cone = clamp(f(vm, "coneOutsideVolume"), 0.0, 1.0);
            put(vm, id, "coneOutsideVolume", 0, Value::Float(cone));
            let v = vec3_of(vm, id, "coneVector");
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let v = if len == 0.0 {
                [0.0, 0.0, 1.0]
            } else {
                v.map(|c| c / len)
            };
            put(vm, id, "coneVector", 0, Value::Floats3(v));
            let level = clamp(f(vm, "environmentLevel"), 0.0, 1.0);
            put(vm, id, "environmentLevel", 0, Value::Float(level));
        }
        if !(0..AUDIO_TYPES).contains(&i(vm, "type")) {
            put(vm, id, "type", 0, Value::Int(0));
        }
        Ok(true)
    }
}

/// The number of audio types (`Audio::NumAudioTypes`).
pub const AUDIO_TYPES: i32 = 32;

/// The datablock ids (`DataBlockObjectIdFirst ..= DataBlockObjectIdLast`).
pub const DATABLOCK_IDS: std::ops::RangeInclusive<ObjId> = 3..=0x2002;

/// `AudioProfile::onAdd`: a networked profile's description must be networked too.
pub struct AudioProfileAdd;

impl EngineClass for AudioProfileAdd {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        if let Some(description) = object_of(vm, id, "description", 0)
            && DATABLOCK_IDS.contains(&id)
            && !DATABLOCK_IDS.contains(&description)
        {
            vm.error(
                "AudioProfile: data dataBlock not networkable (use datablock to create)."
                    .to_string(),
            );
            return Ok(false);
        }
        Ok(true)
    }
}

/// `DebrisData::onAdd`'s validation: each setting out of range is said and reset.
pub struct DebrisDataAdd;

impl EngineClass for DebrisDataAdd {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let name = shown_name(vm, id);
        let warn =
            |vm: &mut Vm, what: &str| vm.warn(format!("DebrisData({name})::onAdd: {what} invalid"));
        let f = |vm: &Vm, field: &str| f32_of(vm, id, field, 0);
        if f(vm, "velocityVariance") > f(vm, "velocity") {
            warn(vm, "velocityVariance");
            let velocity = f(vm, "velocity");
            put(vm, id, "velocityVariance", 0, Value::Float(velocity));
        }
        for (field, lo, hi, reset) in [
            ("friction", -10.0, 10.0, 0.2),
            ("elasticity", -10.0, 10.0, 0.2),
        ] {
            let x = f(vm, field);
            if x < lo || x > hi {
                warn(vm, field);
                put(vm, id, field, 0, Value::Float(reset));
            }
        }
        let lifetime = f(vm, "lifetime");
        if !(0.0..=1000.0).contains(&lifetime) {
            warn(vm, "lifetime");
            put(vm, id, "lifetime", 0, Value::Float(3.0));
        }
        let variance = f(vm, "lifetimeVariance");
        if variance < 0.0 || variance > f(vm, "lifetime") {
            warn(vm, "lifetimeVariance");
            put(vm, id, "lifetimeVariance", 0, Value::Float(0.0));
        }
        let bounces = i32_of(vm, id, "numBounces", 0);
        if !(0..=10000).contains(&bounces) {
            warn(vm, "numBounces");
            put(vm, id, "numBounces", 0, Value::Int(3));
        }
        let variance = i32_of(vm, id, "bounceVariance", 0);
        if variance < 0 || variance > i32_of(vm, id, "numBounces", 0) {
            warn(vm, "bounceVariance");
            put(vm, id, "bounceVariance", 0, Value::Int(0));
        }
        let (min_spin, max_spin) = (f(vm, "minSpinSpeed"), f(vm, "maxSpinSpeed"));
        if !(-10000.0..=10000.0).contains(&min_spin) || min_spin > max_spin {
            warn(vm, "minSpinSpeed");
            put(vm, id, "minSpinSpeed", 0, Value::Float(max_spin - 1.0));
        }
        if !(-10000.0..=10000.0).contains(&max_spin) {
            warn(vm, "maxSpinSpeed");
            put(vm, id, "maxSpinSpeed", 0, Value::Float(0.0));
        }
        Ok(true)
    }
}

/// `ExplosionData::onAdd`'s validation of the scale, the debris angles, counts and speeds, the play speed, the times and the
/// offset.
pub struct ExplosionDataAdd;

impl EngineClass for ExplosionDataAdd {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let name = shown_name(vm, id);
        let warn = |vm: &mut Vm, what: &str| vm.warn(format!("ExplosionData({name}) {what}"));
        let f = |vm: &Vm, field: &str| f32_of(vm, id, field, 0);
        let i = |vm: &Vm, field: &str| i32_of(vm, id, field, 0);
        let scale = vec3_of(vm, id, "explosionScale");
        if scale.iter().any(|&c| c < 0.01) {
            vm.warn(format!(
                "ExplosionData({name})::onAdd: ExplosionScale components must be >= 0.01"
            ));
            put(
                vm,
                id,
                "explosionScale",
                0,
                Value::Floats3(scale.map(|c| if c < 0.01 { 0.01 } else { c })),
            );
        }
        let floor = |vm: &mut Vm, field: &str, lo: f32, what: &str| {
            if f32_of(vm, id, field, 0) < lo {
                warn(vm, what);
                put(vm, id, field, 0, Value::Float(lo));
            }
        };
        let ceiling = |vm: &mut Vm, field: &str, hi: f32, what: &str| {
            if f32_of(vm, id, field, 0) > hi {
                warn(vm, what);
                put(vm, id, field, 0, Value::Float(hi));
            }
        };
        floor(vm, "debrisThetaMin", 0.0, "debrisThetaMin < 0.0");
        ceiling(vm, "debrisThetaMax", 180.0, "debrisThetaMax > 180.0");
        if f(vm, "debrisThetaMin") > f(vm, "debrisThetaMax") {
            warn(vm, "debrisThetaMin > debrisThetaMax");
            let max = f(vm, "debrisThetaMax");
            put(vm, id, "debrisThetaMin", 0, Value::Float(max));
        }
        floor(vm, "debrisPhiMin", 0.0, "debrisPhiMin < 0.0");
        ceiling(vm, "debrisPhiMax", 360.0, "debrisPhiMax > 360.0");
        if f(vm, "debrisPhiMin") > f(vm, "debrisPhiMax") {
            warn(vm, "debrisPhiMin > debrisPhiMax");
            let max = f(vm, "debrisPhiMax");
            put(vm, id, "debrisPhiMin", 0, Value::Float(max));
        }
        for field in ["debrisNum", "debrisNumVariance"] {
            if i(vm, field) > 1000 {
                warn(vm, &format!("{field} > 1000"));
                put(vm, id, field, 0, Value::Int(1000));
            }
        }
        floor(vm, "debrisVelocity", 0.1, "debrisVelocity < 0.1");
        ceiling(
            vm,
            "debrisVelocityVariance",
            1000.0,
            "debrisVelocityVariance > 1000",
        );
        floor(vm, "playSpeed", 0.05, "playSpeed < 0.05");
        if i(vm, "lifetimeMS") < 1 {
            warn(vm, "lifetimeMS < 1");
            put(vm, id, "lifetimeMS", 0, Value::Int(1));
        }
        if i(vm, "lifetimeVariance") > i(vm, "lifetimeMS") {
            warn(vm, "lifetimeVariance > lifetimeMS");
            let lifetime = i(vm, "lifetimeMS");
            put(vm, id, "lifetimeVariance", 0, Value::Int(lifetime));
        }
        if i(vm, "delayMS") < 0 {
            warn(vm, "delayMS < 0");
            put(vm, id, "delayMS", 0, Value::Int(0));
        }
        if i(vm, "delayVariance") > i(vm, "delayMS") {
            warn(vm, "delayVariance > delayMS");
            let delay = i(vm, "delayMS");
            put(vm, id, "delayVariance", 0, Value::Int(delay));
        }
        floor(vm, "offset", 0.0, "offset < 0.0");
        Ok(true)
    }
}

/// `ParticleData::onAdd`: [`DatablockRules::particle_checks`] of the installed rules.
pub struct ParticleDataAdd;

impl EngineClass for ParticleDataAdd {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        Ok(rules(vm).particle_checks(vm, id))
    }
}

/// TGE's `ParticleData::loadParameters`.
pub fn torque_particle_checks(vm: &mut Vm, id: ObjId) -> bool {
    let name = shown_name(vm, id);
    let warn = |vm: &mut Vm, what: &str| vm.warn(format!("ParticleData({name}) {what}"));
    let f = |vm: &Vm, field: &str| f32_of(vm, id, field, 0);
    if f(vm, "dragCoefficient") < 0.0 {
        warn(vm, "drag coeff less than 0");
        put(vm, id, "dragCoefficient", 0, Value::Float(0.0));
    }
    if i32_of(vm, id, "lifetimeMS", 0) < 1 {
        warn(vm, "lifetime < 1 ms");
        put(vm, id, "lifetimeMS", 0, Value::Int(1));
    }
    let lifetime = i32_of(vm, id, "lifetimeMS", 0);
    if i32_of(vm, id, "lifetimeVarianceMS", 0) >= lifetime {
        warn(vm, "lifetimeVariance >= lifetime");
        put(vm, id, "lifetimeVarianceMS", 0, Value::Int(lifetime - 1));
    }
    if !(-10000.0..=10000.0).contains(&f(vm, "spinSpeed")) {
        warn(vm, "spinSpeed invalid");
        return false;
    }
    let (min, max) = (f(vm, "spinRandomMin"), f(vm, "spinRandomMax"));
    if !(-10000.0..=10000.0).contains(&min) {
        warn(vm, "spinRandomMin invalid");
        put(vm, id, "spinRandomMin", 0, Value::Float(-360.0));
        return false;
    }
    if min > max {
        warn(vm, "spinRandomMin greater than spinRandomMax");
        put(vm, id, "spinRandomMin", 0, Value::Float(max - (min - max)));
        return false;
    }
    if !(-10000.0..=10000.0).contains(&max) {
        warn(vm, "spinRandomMax invalid");
        put(vm, id, "spinRandomMax", 0, Value::Float(360.0));
        return false;
    }
    if i32_of(vm, id, "framesPerSec", 0) > 200 {
        warn(vm, "framesPerSec invalid");
        put(vm, id, "framesPerSec", 0, Value::Int(20));
        return false;
    }
    put(vm, id, "times", 0, Value::Float(0.0));
    for i in 1..4usize {
        let before = f32_of(vm, id, "times", i - 1);
        if f32_of(vm, id, "times", i) < before {
            warn(vm, &format!("times[{i}] < times[{}]", i - 1));
            put(vm, id, "times", i, Value::Float(before));
        }
    }
    true
}

/// `ParticleEmitterData::onAdd`: the installed rules' [`DatablockRules::emitter_checks`]; the ids of its particles are kept in
/// `values`.
pub struct ParticleEmitterDataAdd {
    pub values: DbValues,
}

impl EngineClass for ParticleEmitterDataAdd {
    /// The add's checks, also for a redeclare: the particle string must be there unless the emitter has ids (a client's, unpacked
    /// from the record: it has no string).
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let has_ids = !self.values.emitter_ids(id).is_empty();
        let (ok, ids) = rules(vm).emitter_checks(vm, id, has_ids);
        if let (true, Some(ids)) = (ok, ids) {
            self.values.set_emitter_ids(id, ids);
        }
        Ok(ok)
    }

    fn on_remove(&self, _vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.values.forget(id);
        Ok(())
    }
}

/// TGE's `ParticleEmitterData::loadParameters`.
pub fn torque_emitter_checks(vm: &mut Vm, id: ObjId, has_ids: bool) -> (bool, Option<Vec<i32>>) {
    let name = shown_name(vm, id);
    let warn = |vm: &mut Vm, what: &str| vm.warn(format!("ParticleEmitterData({name}) {what}"));
    let f = |vm: &Vm, field: &str| f32_of(vm, id, field, 0);
    let i = |vm: &Vm, field: &str| i32_of(vm, id, field, 0);
    if i(vm, "ejectionPeriodMS") < 1 {
        warn(vm, "period < 1 ms");
        put(vm, id, "ejectionPeriodMS", 0, Value::Int(1));
    }
    let period = i(vm, "ejectionPeriodMS");
    if i(vm, "periodVarianceMS") >= period {
        warn(vm, "periodVariance >= period");
        put(vm, id, "periodVarianceMS", 0, Value::Int(period - 1));
    }
    if f(vm, "ejectionVelocity") < 0.0 {
        warn(vm, "ejectionVelocity < 0.0f");
        put(vm, id, "ejectionVelocity", 0, Value::Float(0.0));
    }
    let speed = f(vm, "ejectionVelocity");
    if f(vm, "velocityVariance") > speed {
        warn(vm, "velocityVariance > ejectionVelocity");
        put(vm, id, "velocityVariance", 0, Value::Float(speed));
    }
    if f(vm, "ejectionOffset") < 0.0 {
        warn(vm, "ejectionOffset < 0");
        put(vm, id, "ejectionOffset", 0, Value::Float(0.0));
    }
    if f(vm, "thetaMin") < 0.0 {
        warn(vm, "thetaMin < 0.0");
        put(vm, id, "thetaMin", 0, Value::Float(0.0));
    }
    if f(vm, "thetaMax") > 180.0 {
        warn(vm, "thetaMax > 180.0");
        put(vm, id, "thetaMax", 0, Value::Float(180.0));
    }
    if f(vm, "thetaMin") > f(vm, "thetaMax") {
        warn(vm, "thetaMin > thetaMax");
        let max = f(vm, "thetaMax");
        put(vm, id, "thetaMin", 0, Value::Float(max));
    }
    let phi = f(vm, "phiVariance");
    if !(0.0..=360.0).contains(&phi) {
        warn(vm, "invalid phiVariance");
        put(
            vm,
            id,
            "phiVariance",
            0,
            Value::Float(if phi < 0.0 { 0.0 } else { 360.0 }),
        );
    }
    let particles = text_of(vm, id, "particles", 0);
    if particles.is_none() && !has_ids {
        warn(vm, "no particleString, invalid datablock");
        return (false, None);
    }
    let text: Vec<u8> = particles.as_deref().map(<[u8]>::to_vec).unwrap_or_default();
    if particles.is_some() && text.is_empty() {
        warn(vm, "no particleString, invalid datablock");
        return (false, None);
    }
    if text.len() > 255 {
        vm.error(format!(
            "ParticleEmitterData({name}) particle string too long [> 255 chars]"
        ));
        return (false, None);
    }
    if i(vm, "lifetimeMS") < 0 {
        warn(vm, "lifetimeMS < 0.0f");
        put(vm, id, "lifetimeMS", 0, Value::Int(0));
    }
    let lifetime = i(vm, "lifetimeMS");
    if i(vm, "lifetimeVarianceMS") > lifetime {
        warn(vm, "lifetimeVarianceMS >= lifetimeMS");
        put(vm, id, "lifetimeVarianceMS", 0, Value::Int(lifetime));
    }
    if particles.is_none() {
        return (true, None);
    }
    let tokens: Vec<&[u8]> = text
        .split(|&b| b == b' ' || b == b'\t')
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        warn(vm, "invalid particles string.  No datablocks found");
        return (false, None);
    }
    let mut ids = Vec::new();
    for token in tokens {
        let found = {
            let objects = vm.objects();
            let class = objects.classes.find(b"ParticleData");
            objects.find(token).filter(|&found| {
                matches!((class, objects.get(found)), (Some(c), Some(o)) if objects.classes.is_a(o.class(), c))
            })
        };
        match found {
            Some(found) => ids.push(found as i32),
            None => warn(
                vm,
                &format!(
                    "unable to find particle datablock: {}",
                    String::from_utf8_lossy(token)
                ),
            ),
        }
    }
    if ids.is_empty() {
        warn(vm, "unable to find any particle datablocks");
        return (false, Some(Vec::new()));
    }
    (true, Some(ids))
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
