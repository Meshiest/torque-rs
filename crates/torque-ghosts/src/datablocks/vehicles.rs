//! TGE's vehicle datablocks: the [`VehicleData`] part (over [`ShapeBaseData`]), [`WheeledVehicleData`],
//! [`FlyingVehicleData`], [`HoverVehicleData`], and the wheel parts [`WheeledVehicleTire`] and [`WheeledVehicleSpring`].
//!
//! ```text
//! VehicleData:          ShapeBaseData; F32 body.restitution, body.friction; db x2 body.sound; F32 minImpactSpeed,
//!                       softImpactSpeed, hardImpactSpeed, minRollSpeed, maxSteeringAngle, maxDrag, minDrag; S32 integration;
//!                       F32 collisionTol, contactTol; F32 x3 massCenter; F32 x3 massBox; F32 jetForce, jetEnergyDrain,
//!                       minJetEnergy; flag cameraRoll; F32 cameraLag, cameraDecay, cameraOffset, triggerDustHeight,
//!                       dustHeight, numDmgEmitterAreas, exitSplashSoundVel, softSplashSoundVel, medSplashSoundVel,
//!                       hardSplashSoundVel; db x5 waterSound; db dustEmitter; db x3 damageEmitterList;
//!                       db x2 splashEmitterList; F32 x3 x2 damageEmitterOffset; F32 x2 damageLevelTolerance;
//!                       F32 splashFreqMod, splashVelEpsilon, collDamageThresholdVel, collDamageMultiplier
//! WheeledVehicleData:   VehicleData; db tireEmitter; db x4 sound; F32 maxWheelSpeed, engineTorque, engineBrake, brakeTorque
//! FlyingVehicleData:    VehicleData; db x2 sound; db x4 jetEmitter; F32 x15 (maneuveringForce .. vertThrustMultiple)
//! HoverVehicleData:     VehicleData; F32 x17 (dragForce .. pitchForce); F32 x3 dustTrailOffset; F32 triggerTrailHeight,
//!                       dustTrailFreqMod; db x3 sound; db x3 jetEmitter; db dustTrailEmitter; F32 floatingGravMag,
//!                       brakingForce, brakingActivationSpeed
//! WheeledVehicleTire:   string shapeName; F32 x11 (mass .. longitudinalRelaxation)
//! WheeledVehicleSpring: F32 length, force, damping, antiSway
//! ```
//!
//! Credit: these follow TGE 1.4's `game/vehicles/vehicle.cc`, `wheeledVehicle.cc`, `flyingVehicle.cc` and `hoverVehicle.cc`
//! (Copyright (C) GarageGames.com, Inc.), whose MIT-licensed successors are Torque3D's `T3D/vehicles/vehicle.cpp`,
//! `wheeledVehicle.cpp`, `flyingVehicle.cpp` and `hoverVehicle.cpp` (each Copyright (c) 2012 GarageGames, LLC). The lists are
//! TGE's; the code is this crate's own.

use super::shapes::{ShapeBaseData, pack_shapebase_data};
use super::{db, db_ref, db_refs, dbs, float};
use crate::codec::{Codec, F32, Layout, NetWidths, R};
use crate::{ObjectView, boolean, raw, raws, text, word};

/// The body sounds of a vehicle (`Body::MaxSounds`).
pub const BODY_SOUNDS: usize = 2;
/// The water sounds of a vehicle (`VehicleData::MaxSounds`).
pub const WATER_SOUNDS: usize = 5;
/// The damage emitters of a vehicle (`VC_NUM_DAMAGE_EMITTERS`).
pub const DAMAGE_EMITTERS: usize = 3;
/// The damage emitter areas of a vehicle (`VC_NUM_DAMAGE_EMITTER_AREAS`).
pub const DAMAGE_EMITTER_AREAS: usize = 2;
/// The damage levels of a vehicle (`VC_NUM_DAMAGE_LEVELS`).
pub const DAMAGE_LEVELS: usize = 2;
/// The splash emitters of a vehicle (`VC_NUM_SPLASH_EMITTERS`).
pub const SPLASH_EMITTERS: usize = 2;
/// The sounds of a WheeledVehicleData.
pub const WHEELED_SOUNDS: usize = 4;
/// The sounds of a FlyingVehicleData.
pub const FLYING_SOUNDS: usize = 2;
/// The jet emitters of a FlyingVehicleData.
pub const FLYING_JETS: usize = 4;
/// The sounds of a HoverVehicleData.
pub const HOVER_SOUNDS: usize = 3;
/// The jet emitters of a HoverVehicleData.
pub const HOVER_JETS: usize = 3;

/// `VehicleData::packData`: the part every vehicle datablock starts with.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VehicleData {
    pub shape: ShapeBaseData,
    pub body_restitution: F32,
    pub body_friction: F32,
    pub body_sound: [Option<u32>; BODY_SOUNDS],
    pub min_impact_speed: F32,
    pub soft_impact_speed: F32,
    pub hard_impact_speed: F32,
    pub min_roll_speed: F32,
    pub max_steering_angle: F32,
    pub max_drag: F32,
    pub min_drag: F32,
    /// An `S32`'s 32 bits.
    pub integration: u32,
    pub collision_tol: F32,
    pub contact_tol: F32,
    pub mass_center: [F32; 3],
    pub mass_box: [F32; 3],
    pub jet_force: F32,
    pub jet_energy_drain: F32,
    pub min_jet_energy: F32,
    pub camera_roll: bool,
    pub camera_lag: F32,
    pub camera_decay: F32,
    pub camera_offset: F32,
    pub trigger_dust_height: F32,
    pub dust_height: F32,
    pub num_dmg_emitter_areas: F32,
    pub exit_splash_sound_vel: F32,
    pub soft_splash_sound_vel: F32,
    pub med_splash_sound_vel: F32,
    pub hard_splash_sound_vel: F32,
    pub water_sound: [Option<u32>; WATER_SOUNDS],
    pub dust_emitter: Option<u32>,
    pub damage_emitter_list: [Option<u32>; DAMAGE_EMITTERS],
    pub splash_emitter_list: [Option<u32>; SPLASH_EMITTERS],
    pub damage_emitter_offset: [[F32; 3]; DAMAGE_EMITTER_AREAS],
    pub damage_level_tolerance: [F32; DAMAGE_LEVELS],
    pub splash_freq_mod: F32,
    pub splash_vel_epsilon: F32,
    pub coll_damage_threshold_vel: F32,
    pub coll_damage_multiplier: F32,
}

impl Layout for VehicleData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        float(c, "body.restitution", &mut self.body_restitution)?;
        float(c, "body.friction", &mut self.body_friction)?;
        db_refs(c, "body.sound", &mut self.body_sound)?;
        float(c, "minImpactSpeed", &mut self.min_impact_speed)?;
        float(c, "softImpactSpeed", &mut self.soft_impact_speed)?;
        float(c, "hardImpactSpeed", &mut self.hard_impact_speed)?;
        float(c, "minRollSpeed", &mut self.min_roll_speed)?;
        float(c, "maxSteeringAngle", &mut self.max_steering_angle)?;
        float(c, "maxDrag", &mut self.max_drag)?;
        float(c, "minDrag", &mut self.min_drag)?;
        c.raw32("integration", &mut self.integration)?;
        float(c, "collisionTol", &mut self.collision_tol)?;
        float(c, "contactTol", &mut self.contact_tol)?;
        c.floats("massCenter", &mut self.mass_center)?;
        c.floats("massBox", &mut self.mass_box)?;
        float(c, "jetForce", &mut self.jet_force)?;
        float(c, "jetEnergyDrain", &mut self.jet_energy_drain)?;
        float(c, "minJetEnergy", &mut self.min_jet_energy)?;
        c.flag("cameraRoll", &mut self.camera_roll)?;
        float(c, "cameraLag", &mut self.camera_lag)?;
        float(c, "cameraDecay", &mut self.camera_decay)?;
        float(c, "cameraOffset", &mut self.camera_offset)?;
        float(c, "triggerDustHeight", &mut self.trigger_dust_height)?;
        float(c, "dustHeight", &mut self.dust_height)?;
        float(c, "numDmgEmitterAreas", &mut self.num_dmg_emitter_areas)?;
        float(c, "exitSplashSoundVel", &mut self.exit_splash_sound_vel)?;
        float(c, "softSplashSoundVel", &mut self.soft_splash_sound_vel)?;
        float(c, "medSplashSoundVel", &mut self.med_splash_sound_vel)?;
        float(c, "hardSplashSoundVel", &mut self.hard_splash_sound_vel)?;
        db_refs(c, "waterSound", &mut self.water_sound)?;
        db_ref(c, "dustEmitter", &mut self.dust_emitter)?;
        db_refs(c, "damageEmitterList", &mut self.damage_emitter_list)?;
        db_refs(c, "splashEmitterList", &mut self.splash_emitter_list)?;
        for offset in self.damage_emitter_offset.iter_mut() {
            c.floats("damageEmitterOffset", offset)?;
        }
        c.floats("damageLevelTolerance", &mut self.damage_level_tolerance)?;
        float(c, "splashFreqMod", &mut self.splash_freq_mod)?;
        float(c, "splashVelEpsilon", &mut self.splash_vel_epsilon)?;
        float(
            c,
            "collDamageThresholdVel",
            &mut self.coll_damage_threshold_vel,
        )?;
        float(c, "collDamageMultiplier", &mut self.coll_damage_multiplier)
    }
}

pub fn pack_vehicle_data(v: &dyn ObjectView, w: NetWidths) -> VehicleData {
    let f = |name: &str| raw(v, name, 0);
    VehicleData {
        shape: pack_shapebase_data(v, w),
        body_restitution: f("body.restitution"),
        body_friction: f("body.friction"),
        body_sound: dbs(v, "body.sound", w),
        min_impact_speed: f("minImpactSpeed"),
        soft_impact_speed: f("softImpactSpeed"),
        hard_impact_speed: f("hardImpactSpeed"),
        min_roll_speed: f("minRollSpeed"),
        max_steering_angle: f("maxSteeringAngle"),
        max_drag: f("maxDrag"),
        min_drag: f("minDrag"),
        integration: word(v, "integration", 0),
        collision_tol: f("collisionTol"),
        contact_tol: f("contactTol"),
        mass_center: raws(v, "massCenter", 0),
        mass_box: raws(v, "massBox", 0),
        jet_force: f("jetForce"),
        jet_energy_drain: f("jetEnergyDrain"),
        min_jet_energy: f("minJetEnergy"),
        camera_roll: boolean(v, "cameraRoll", 0),
        camera_lag: f("cameraLag"),
        camera_decay: f("cameraDecay"),
        camera_offset: f("cameraOffset"),
        trigger_dust_height: f("triggerDustHeight"),
        dust_height: f("dustHeight"),
        num_dmg_emitter_areas: f("numDmgEmitterAreas"),
        exit_splash_sound_vel: f("exitSplashSoundVel"),
        soft_splash_sound_vel: f("softSplashSoundVel"),
        med_splash_sound_vel: f("medSplashSoundVel"),
        hard_splash_sound_vel: f("hardSplashSoundVel"),
        water_sound: dbs(v, "waterSound", w),
        dust_emitter: db(v, "dustEmitter", 0, w),
        damage_emitter_list: dbs(v, "damageEmitterList", w),
        splash_emitter_list: dbs(v, "splashEmitterList", w),
        damage_emitter_offset: std::array::from_fn(|i| raws(v, "damageEmitterOffset", 3 * i)),
        damage_level_tolerance: raws(v, "damageLevelTolerance", 0),
        splash_freq_mod: f("splashFreqMod"),
        splash_vel_epsilon: f("splashVelEpsilon"),
        coll_damage_threshold_vel: f("collDamageThresholdVel"),
        coll_damage_multiplier: f("collDamageMultiplier"),
    }
}

/// `WheeledVehicleData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WheeledVehicleData {
    pub vehicle: VehicleData,
    pub tire_emitter: Option<u32>,
    pub sound: [Option<u32>; WHEELED_SOUNDS],
    pub max_wheel_speed: F32,
    pub engine_torque: F32,
    pub engine_brake: F32,
    pub brake_torque: F32,
}

impl Layout for WheeledVehicleData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.vehicle.layout(c)?;
        db_ref(c, "tireEmitter", &mut self.tire_emitter)?;
        db_refs(c, "sound", &mut self.sound)?;
        float(c, "maxWheelSpeed", &mut self.max_wheel_speed)?;
        float(c, "engineTorque", &mut self.engine_torque)?;
        float(c, "engineBrake", &mut self.engine_brake)?;
        float(c, "brakeTorque", &mut self.brake_torque)
    }
}

pub fn pack_wheeled_vehicle_data(v: &dyn ObjectView, w: NetWidths) -> WheeledVehicleData {
    WheeledVehicleData {
        vehicle: pack_vehicle_data(v, w),
        tire_emitter: db(v, "tireEmitter", 0, w),
        sound: dbs(v, "sound", w),
        max_wheel_speed: raw(v, "maxWheelSpeed", 0),
        engine_torque: raw(v, "engineTorque", 0),
        engine_brake: raw(v, "engineBrake", 0),
        brake_torque: raw(v, "brakeTorque", 0),
    }
}

/// The fifteen forces of a FlyingVehicleData, in wire order.
pub const FLYING_FLOATS: [&str; 15] = [
    "maneuveringForce",
    "horizontalSurfaceForce",
    "verticalSurfaceForce",
    "autoInputDamping",
    "steeringForce",
    "steeringRollForce",
    "rollForce",
    "autoAngularForce",
    "rotationalDrag",
    "autoLinearForce",
    "maxAutoSpeed",
    "hoverHeight",
    "createHoverHeight",
    "minTrailSpeed",
    "vertThrustMultiple",
];

/// `FlyingVehicleData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlyingVehicleData {
    pub vehicle: VehicleData,
    pub sound: [Option<u32>; FLYING_SOUNDS],
    pub jet_emitter: [Option<u32>; FLYING_JETS],
    /// The fields of [`FLYING_FLOATS`], in that order.
    pub forces: [F32; 15],
}

impl Layout for FlyingVehicleData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.vehicle.layout(c)?;
        db_refs(c, "sound", &mut self.sound)?;
        db_refs(c, "jetEmitter", &mut self.jet_emitter)?;
        for (name, x) in FLYING_FLOATS.iter().zip(self.forces.iter_mut()) {
            float(c, name, x)?;
        }
        Ok(())
    }
}

pub fn pack_flying_vehicle_data(v: &dyn ObjectView, w: NetWidths) -> FlyingVehicleData {
    FlyingVehicleData {
        vehicle: pack_vehicle_data(v, w),
        sound: dbs(v, "sound", w),
        jet_emitter: dbs(v, "jetEmitter", w),
        forces: FLYING_FLOATS.map(|name| raw(v, name, 0)),
    }
}

/// The seventeen forces a HoverVehicleData sends first, in wire order.
pub const HOVER_FLOATS: [&str; 17] = [
    "dragForce",
    "vertFactor",
    "floatingThrustFactor",
    "mainThrustForce",
    "reverseThrustForce",
    "strafeThrustForce",
    "turboFactor",
    "stabLenMin",
    "stabLenMax",
    "stabSpringConstant",
    "stabDampingConstant",
    "gyroDrag",
    "normalForce",
    "restorativeForce",
    "steeringForce",
    "rollForce",
    "pitchForce",
];

/// `HoverVehicleData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HoverVehicleData {
    pub vehicle: VehicleData,
    /// The fields of [`HOVER_FLOATS`], in that order.
    pub forces: [F32; 17],
    pub dust_trail_offset: [F32; 3],
    pub trigger_trail_height: F32,
    pub dust_trail_freq_mod: F32,
    pub sound: [Option<u32>; HOVER_SOUNDS],
    pub jet_emitter: [Option<u32>; HOVER_JETS],
    pub dust_trail_emitter: Option<u32>,
    pub floating_grav_mag: F32,
    pub braking_force: F32,
    pub braking_activation_speed: F32,
}

impl Layout for HoverVehicleData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.vehicle.layout(c)?;
        for (name, x) in HOVER_FLOATS.iter().zip(self.forces.iter_mut()) {
            float(c, name, x)?;
        }
        c.floats("dustTrailOffset", &mut self.dust_trail_offset)?;
        float(c, "triggerTrailHeight", &mut self.trigger_trail_height)?;
        float(c, "dustTrailFreqMod", &mut self.dust_trail_freq_mod)?;
        db_refs(c, "sound", &mut self.sound)?;
        db_refs(c, "jetEmitter", &mut self.jet_emitter)?;
        db_ref(c, "dustTrailEmitter", &mut self.dust_trail_emitter)?;
        float(c, "floatingGravMag", &mut self.floating_grav_mag)?;
        float(c, "brakingForce", &mut self.braking_force)?;
        float(
            c,
            "brakingActivationSpeed",
            &mut self.braking_activation_speed,
        )
    }
}

pub fn pack_hover_vehicle_data(v: &dyn ObjectView, w: NetWidths) -> HoverVehicleData {
    HoverVehicleData {
        vehicle: pack_vehicle_data(v, w),
        forces: HOVER_FLOATS.map(|name| raw(v, name, 0)),
        dust_trail_offset: raws(v, "dustTrailOffset", 0),
        trigger_trail_height: raw(v, "triggerTrailHeight", 0),
        dust_trail_freq_mod: raw(v, "dustTrailFreqMod", 0),
        sound: dbs(v, "sound", w),
        jet_emitter: dbs(v, "jetEmitter", w),
        dust_trail_emitter: db(v, "dustTrailEmitter", 0, w),
        floating_grav_mag: raw(v, "floatingGravMag", 0),
        braking_force: raw(v, "brakingForce", 0),
        braking_activation_speed: raw(v, "brakingActivationSpeed", 0),
    }
}

/// The eleven numbers of a WheeledVehicleTire, in wire order.
pub const TIRE_FLOATS: [&str; 11] = [
    "mass",
    "staticFriction",
    "kineticFriction",
    "restitution",
    "radius",
    "lateralForce",
    "lateralDamping",
    "lateralRelaxation",
    "longitudinalForce",
    "longitudinalDamping",
    "longitudinalRelaxation",
];

/// `WheeledVehicleTire::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WheeledVehicleTire {
    pub shape_name: Vec<u8>,
    /// The fields of [`TIRE_FLOATS`], in that order.
    pub values: [F32; 11],
}

impl Layout for WheeledVehicleTire {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.string("shapeName", &mut self.shape_name)?;
        for (name, x) in TIRE_FLOATS.iter().zip(self.values.iter_mut()) {
            float(c, name, x)?;
        }
        Ok(())
    }
}

pub fn pack_tire(v: &dyn ObjectView) -> WheeledVehicleTire {
    WheeledVehicleTire {
        shape_name: text(v, "shapeName", 0),
        values: TIRE_FLOATS.map(|name| raw(v, name, 0)),
    }
}

/// `WheeledVehicleSpring::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WheeledVehicleSpring {
    pub length: F32,
    pub force: F32,
    pub damping: F32,
    pub anti_sway: F32,
}

impl Layout for WheeledVehicleSpring {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        float(c, "length", &mut self.length)?;
        float(c, "force", &mut self.force)?;
        float(c, "damping", &mut self.damping)?;
        float(c, "antiSway", &mut self.anti_sway)
    }
}

pub fn pack_spring(v: &dyn ObjectView) -> WheeledVehicleSpring {
    WheeledVehicleSpring {
        length: raw(v, "length", 0),
        force: raw(v, "force", 0),
        damping: raw(v, "damping", 0),
        anti_sway: raw(v, "antiSway", 0),
    }
}
