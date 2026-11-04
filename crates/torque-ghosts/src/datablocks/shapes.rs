//! TGE's ShapeBase datablocks: [`ShapeBaseData`] (also the whole record of `CameraData`, `PathCameraData` and
//! `MissionMarkerData`, which add nothing), [`StaticShapeData`], [`ItemData`] and [`PlayerData`].
//!
//! ```text
//! ShapeBaseData:   flag(computeCRC): U32 mCRC; string shapeName, cloakTexName;
//!                  per float of mass, drag, density, maxEnergy, cameraMaxDist, cameraMinDist, cameraDefaultFov, cameraMinFov,
//!                  cameraMaxFov: flag(!= its default 1, 0, 1, 0, 0, 0.2, 90, 5, 120): F32;
//!                  string debrisShapeName; flag observeThroughObject; db debris; flag emap, isInvincible, renderWhenDestroyed;
//!                  db explosion, underwaterExplosion; flag inheritEnergyFromMount, firstPersonOnly, useEyePoint
//! StaticShapeData: ShapeBaseData; flag noIndividualDamage; S32 dynamicTypeField
//! ItemData:        ShapeBaseData; float(10) friction, elasticity; flag sticky; flag(gravityMod != 1): signed float(11);
//!                  flag(maxVelocity != -1): F32; flag(lightType != NoLight): int(2) type, float(7) x4 colour, S32 lightTime,
//!                  F32 lightRadius, flag lightOnlyStatic
//! PlayerData:      ShapeBaseData; flag renderFirstPerson; F32 x15 (look angles .. runSurfaceAngle); S32 recoverDelay;
//!                  F32 x7 (recoverRunForceScale .. jumpSurfaceAngle); int(7) jumpDelay; F32 x16 (horizMaxSpeed ..
//!                  minImpactSpeed); db x18 sound; F32 x3 boxSize; db footPuffEmitter; S32 footPuffNumParts;
//!                  F32 footPuffRadius; db decalData; F32 decalOffset; db dustEmitter, splash; db x3 splashEmitterList;
//!                  F32 groundImpactMinSpeed, x3 shake frequency, x3 shake amplitude, shake duration, shake falloff
//! ```
//! `db` is a datablock pointer ([`db_ref`]); `float(n)` is `writeFloat`, `signed float(n)` `writeSignedFloat`. ShapeBaseData's
//! packer clamps `cameraDefaultFov` into `cameraMinFov..cameraMaxFov` (`mClampF`) before it compares and sends it.
//!
//! Credit: these follow TGE 1.4's `game/shapeBase.cc`, `game/staticShape.cc`, `game/item.cc`, `game/player.cc`,
//! `game/camera.cc`, `game/pathCamera.cc` and `game/missionMarker.cc` (Copyright (C) GarageGames.com, Inc.), whose
//! MIT-licensed successors are Torque3D's `T3D/shapeBase.cpp`, `T3D/staticShape.cpp` and `T3D/player.cpp` (each Copyright (c)
//! 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.) and `T3D/item.cpp`, `T3D/camera.cpp`, `T3D/pathCamera.cpp`
//! and `T3D/missionMarker.cpp` (each Copyright (c) 2012 GarageGames, LLC). The lists are TGE's; the code is this crate's own.

use super::{
    clamp_f, db, db_ref, db_refs, dbs, float, float_ints, float_ints_of, opt_float, unless,
};
use crate::codec::{Codec, F32, Layout, NetWidths, R, mask_bits, opt};
use crate::motion::{float_bits, signed_float_bits};
use crate::{ObjectView, boolean, fld, int, raw, raws, text, word};

/// The sounds of a PlayerData (`PlayerData::MaxSounds`).
pub const PLAYER_SOUNDS: usize = 18;

/// The splash emitters of a PlayerData (`NUM_SPLASH_EMITTERS`).
pub const PLAYER_SPLASH_EMITTERS: usize = 3;

/// `ShapeBaseData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShapeBaseData {
    /// `flag(computeCRC)`: the shape's CRC.
    pub crc: Option<u32>,
    pub shape_name: Vec<u8>,
    pub cloak_tex_name: Vec<u8>,
    pub mass: Option<F32>,
    pub drag: Option<F32>,
    pub density: Option<F32>,
    pub max_energy: Option<F32>,
    pub camera_max_dist: Option<F32>,
    pub camera_min_dist: Option<F32>,
    pub camera_default_fov: Option<F32>,
    pub camera_min_fov: Option<F32>,
    pub camera_max_fov: Option<F32>,
    pub debris_shape_name: Vec<u8>,
    pub observe_through_object: bool,
    pub debris: Option<u32>,
    pub emap: bool,
    pub is_invincible: bool,
    pub render_when_destroyed: bool,
    pub explosion: Option<u32>,
    pub underwater_explosion: Option<u32>,
    pub inherit_energy_from_mount: bool,
    pub first_person_only: bool,
    pub use_eye_point: bool,
}

impl Layout for ShapeBaseData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "computeCRC", &mut self.crc, |c, v| c.raw32("mCRC", v))?;
        c.string("shapeName", &mut self.shape_name)?;
        c.string("cloakTexName", &mut self.cloak_tex_name)?;
        opt_float(c, "mass", &mut self.mass)?;
        opt_float(c, "drag", &mut self.drag)?;
        opt_float(c, "density", &mut self.density)?;
        opt_float(c, "maxEnergy", &mut self.max_energy)?;
        opt_float(c, "cameraMaxDist", &mut self.camera_max_dist)?;
        opt_float(c, "cameraMinDist", &mut self.camera_min_dist)?;
        opt_float(c, "cameraDefaultFov", &mut self.camera_default_fov)?;
        opt_float(c, "cameraMinFov", &mut self.camera_min_fov)?;
        opt_float(c, "cameraMaxFov", &mut self.camera_max_fov)?;
        c.string("debrisShapeName", &mut self.debris_shape_name)?;
        c.flag("observeThroughObject", &mut self.observe_through_object)?;
        db_ref(c, "debris", &mut self.debris)?;
        c.flag("emap", &mut self.emap)?;
        c.flag("isInvincible", &mut self.is_invincible)?;
        c.flag("renderWhenDestroyed", &mut self.render_when_destroyed)?;
        db_ref(c, "explosion", &mut self.explosion)?;
        db_ref(c, "underwaterExplosion", &mut self.underwater_explosion)?;
        c.flag(
            "inheritEnergyFromMount",
            &mut self.inherit_energy_from_mount,
        )?;
        c.flag("firstPersonOnly", &mut self.first_person_only)?;
        c.flag("useEyePoint", &mut self.use_eye_point)
    }
}

/// Packs a ShapeBaseData (the defaults it compares against are its constructor's).
pub fn pack_shapebase_data(v: &dyn ObjectView, w: NetWidths) -> ShapeBaseData {
    let min_fov = fld(v, "cameraMinFov", 0);
    let max_fov = fld(v, "cameraMaxFov", 0);
    let default_fov = clamp_f(fld(v, "cameraDefaultFov", 0), min_fov, max_fov);
    ShapeBaseData {
        crc: boolean(v, "computeCRC", 0).then(|| word(v, "mCRC", 0)),
        shape_name: text(v, "shapeName", 0),
        cloak_tex_name: text(v, "cloakTexName", 0),
        mass: unless(fld(v, "mass", 0), 1.0),
        drag: unless(fld(v, "drag", 0), 0.0),
        density: unless(fld(v, "density", 0), 1.0),
        max_energy: unless(fld(v, "maxEnergy", 0), 0.0),
        camera_max_dist: unless(fld(v, "cameraMaxDist", 0), 0.0),
        camera_min_dist: unless(fld(v, "cameraMinDist", 0), 0.2),
        camera_default_fov: unless(default_fov, 90.0),
        camera_min_fov: unless(min_fov, 5.0),
        camera_max_fov: unless(max_fov, 120.0),
        debris_shape_name: text(v, "debrisShapeName", 0),
        observe_through_object: boolean(v, "observeThroughObject", 0),
        debris: db(v, "debris", 0, w),
        emap: boolean(v, "emap", 0),
        is_invincible: boolean(v, "isInvincible", 0),
        render_when_destroyed: boolean(v, "renderWhenDestroyed", 0),
        explosion: db(v, "explosion", 0, w),
        underwater_explosion: db(v, "underwaterExplosion", 0, w),
        inherit_energy_from_mount: boolean(v, "inheritEnergyFromMount", 0),
        first_person_only: boolean(v, "firstPersonOnly", 0),
        use_eye_point: boolean(v, "useEyePoint", 0),
    }
}

/// `StaticShapeData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StaticShapeData {
    pub shape: ShapeBaseData,
    pub no_individual_damage: bool,
    /// An `S32`'s 32 bits.
    pub dynamic_type_field: u32,
}

impl Layout for StaticShapeData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        c.flag("noIndividualDamage", &mut self.no_individual_damage)?;
        c.raw32("dynamicTypeField", &mut self.dynamic_type_field)
    }
}

pub fn pack_static_shape_data(v: &dyn ObjectView, w: NetWidths) -> StaticShapeData {
    StaticShapeData {
        shape: pack_shapebase_data(v, w),
        no_individual_damage: boolean(v, "noIndividualDamage", 0),
        dynamic_type_field: word(v, "dynamicTypeField", 0),
    }
}

/// An ItemData's light block (sent when `lightType` is not `NoLight`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemLight {
    /// `int(2)`.
    pub light_type: u32,
    /// `writeFloat(channel, 7)` of red, green, blue, alpha.
    pub color: [u32; 4],
    /// An `S32`'s 32 bits.
    pub time: u32,
    pub radius: F32,
    pub only_static: bool,
}

/// `ItemData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemData {
    pub shape: ShapeBaseData,
    /// `writeFloat(friction, 10)`.
    pub friction: u32,
    /// `writeFloat(elasticity, 10)`.
    pub elasticity: u32,
    pub sticky: bool,
    /// `flag(gravityMod != 1)`: `writeSignedFloat(gravityMod, 11)`.
    pub gravity_mod: Option<u32>,
    /// `flag(maxVelocity != -1)`.
    pub max_velocity: Option<F32>,
    pub light: Option<ItemLight>,
}

impl Layout for ItemData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        c.int("friction", &mut self.friction, 10)?;
        c.int("elasticity", &mut self.elasticity, 10)?;
        c.flag("sticky", &mut self.sticky)?;
        opt(c, "gravityMod", &mut self.gravity_mod, |c, g| {
            c.int("gravityMod", g, 11)
        })?;
        opt_float(c, "maxVelocity", &mut self.max_velocity)?;
        opt(c, "lightType", &mut self.light, |c, l| {
            c.int("lightType", &mut l.light_type, 2)?;
            float_ints(c, "lightColor", &mut l.color, 7)?;
            c.raw32("lightTime", &mut l.time)?;
            float(c, "lightRadius", &mut l.radius)?;
            c.flag("lightOnlyStatic", &mut l.only_static)
        })
    }
}

pub fn pack_item_data(v: &dyn ObjectView, w: NetWidths) -> ItemData {
    let gravity = fld(v, "gravityMod", 0);
    let light_type = int(v, "lightType", 0);
    ItemData {
        shape: pack_shapebase_data(v, w),
        friction: float_bits(fld(v, "friction", 0), 10),
        elasticity: float_bits(fld(v, "elasticity", 0), 10),
        sticky: boolean(v, "sticky", 0),
        gravity_mod: unless(gravity, 1.0).map(|_| signed_float_bits(gravity, 11)),
        max_velocity: unless(fld(v, "maxVelocity", 0), -1.0),
        light: (light_type != 0).then(|| ItemLight {
            light_type: mask_bits(light_type as u32, 2),
            color: float_ints_of(v, "lightColor", 0, 7),
            time: word(v, "lightTime", 0),
            radius: raw(v, "lightRadius", 0),
            only_static: boolean(v, "lightOnlyStatic", 0),
        }),
    }
}

/// `PlayerData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayerData {
    pub shape: ShapeBaseData,
    pub render_first_person: bool,
    pub min_look_angle: F32,
    pub max_look_angle: F32,
    pub max_freelook_angle: F32,
    pub max_time_scale: F32,
    pub max_step_height: F32,
    pub run_force: F32,
    pub run_energy_drain: F32,
    pub min_run_energy: F32,
    pub max_forward_speed: F32,
    pub max_backward_speed: F32,
    pub max_side_speed: F32,
    pub max_underwater_forward_speed: F32,
    pub max_underwater_backward_speed: F32,
    pub max_underwater_side_speed: F32,
    pub run_surface_angle: F32,
    /// An `S32`'s 32 bits.
    pub recover_delay: u32,
    pub recover_run_force_scale: F32,
    pub jump_force: F32,
    pub jump_energy_drain: F32,
    pub min_jump_energy: F32,
    pub min_jump_speed: F32,
    pub max_jump_speed: F32,
    pub jump_surface_angle: F32,
    /// `int(7)` (`JumpDelayBits`).
    pub jump_delay: u32,
    pub horiz_max_speed: F32,
    pub horiz_resist_speed: F32,
    pub horiz_resist_factor: F32,
    pub up_max_speed: F32,
    pub up_resist_speed: F32,
    pub up_resist_factor: F32,
    pub splash_velocity: F32,
    pub splash_angle: F32,
    pub splash_freq_mod: F32,
    pub splash_vel_epsilon: F32,
    pub bubble_emit_time: F32,
    pub med_splash_sound_vel: F32,
    pub hard_splash_sound_vel: F32,
    pub exit_splash_sound_vel: F32,
    pub foot_splash_height: F32,
    pub min_impact_speed: F32,
    pub sound: [Option<u32>; PLAYER_SOUNDS],
    pub box_size: [F32; 3],
    pub foot_puff_emitter: Option<u32>,
    /// An `S32`'s 32 bits.
    pub foot_puff_num_parts: u32,
    pub foot_puff_radius: F32,
    pub decal_data: Option<u32>,
    pub decal_offset: F32,
    pub dust_emitter: Option<u32>,
    pub splash: Option<u32>,
    pub splash_emitter_list: [Option<u32>; PLAYER_SPLASH_EMITTERS],
    pub ground_impact_min_speed: F32,
    pub ground_impact_shake_freq: [F32; 3],
    pub ground_impact_shake_amp: [F32; 3],
    pub ground_impact_shake_duration: F32,
    pub ground_impact_shake_falloff: F32,
}

impl Layout for PlayerData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        c.flag("renderFirstPerson", &mut self.render_first_person)?;
        float(c, "minLookAngle", &mut self.min_look_angle)?;
        float(c, "maxLookAngle", &mut self.max_look_angle)?;
        float(c, "maxFreelookAngle", &mut self.max_freelook_angle)?;
        float(c, "maxTimeScale", &mut self.max_time_scale)?;
        float(c, "maxStepHeight", &mut self.max_step_height)?;
        float(c, "runForce", &mut self.run_force)?;
        float(c, "runEnergyDrain", &mut self.run_energy_drain)?;
        float(c, "minRunEnergy", &mut self.min_run_energy)?;
        float(c, "maxForwardSpeed", &mut self.max_forward_speed)?;
        float(c, "maxBackwardSpeed", &mut self.max_backward_speed)?;
        float(c, "maxSideSpeed", &mut self.max_side_speed)?;
        float(
            c,
            "maxUnderwaterForwardSpeed",
            &mut self.max_underwater_forward_speed,
        )?;
        float(
            c,
            "maxUnderwaterBackwardSpeed",
            &mut self.max_underwater_backward_speed,
        )?;
        float(
            c,
            "maxUnderwaterSideSpeed",
            &mut self.max_underwater_side_speed,
        )?;
        float(c, "runSurfaceAngle", &mut self.run_surface_angle)?;
        c.raw32("recoverDelay", &mut self.recover_delay)?;
        float(c, "recoverRunForceScale", &mut self.recover_run_force_scale)?;
        float(c, "jumpForce", &mut self.jump_force)?;
        float(c, "jumpEnergyDrain", &mut self.jump_energy_drain)?;
        float(c, "minJumpEnergy", &mut self.min_jump_energy)?;
        float(c, "minJumpSpeed", &mut self.min_jump_speed)?;
        float(c, "maxJumpSpeed", &mut self.max_jump_speed)?;
        float(c, "jumpSurfaceAngle", &mut self.jump_surface_angle)?;
        c.int("jumpDelay", &mut self.jump_delay, 7)?;
        float(c, "horizMaxSpeed", &mut self.horiz_max_speed)?;
        float(c, "horizResistSpeed", &mut self.horiz_resist_speed)?;
        float(c, "horizResistFactor", &mut self.horiz_resist_factor)?;
        float(c, "upMaxSpeed", &mut self.up_max_speed)?;
        float(c, "upResistSpeed", &mut self.up_resist_speed)?;
        float(c, "upResistFactor", &mut self.up_resist_factor)?;
        float(c, "splashVelocity", &mut self.splash_velocity)?;
        float(c, "splashAngle", &mut self.splash_angle)?;
        float(c, "splashFreqMod", &mut self.splash_freq_mod)?;
        float(c, "splashVelEpsilon", &mut self.splash_vel_epsilon)?;
        float(c, "bubbleEmitTime", &mut self.bubble_emit_time)?;
        float(c, "medSplashSoundVel", &mut self.med_splash_sound_vel)?;
        float(c, "hardSplashSoundVel", &mut self.hard_splash_sound_vel)?;
        float(c, "exitSplashSoundVel", &mut self.exit_splash_sound_vel)?;
        float(c, "footSplashHeight", &mut self.foot_splash_height)?;
        float(c, "minImpactSpeed", &mut self.min_impact_speed)?;
        db_refs(c, "sound", &mut self.sound)?;
        c.floats("boxSize", &mut self.box_size)?;
        db_ref(c, "footPuffEmitter", &mut self.foot_puff_emitter)?;
        c.raw32("footPuffNumParts", &mut self.foot_puff_num_parts)?;
        float(c, "footPuffRadius", &mut self.foot_puff_radius)?;
        db_ref(c, "decalData", &mut self.decal_data)?;
        float(c, "decalOffset", &mut self.decal_offset)?;
        db_ref(c, "dustEmitter", &mut self.dust_emitter)?;
        db_ref(c, "splash", &mut self.splash)?;
        db_refs(c, "splashEmitterList", &mut self.splash_emitter_list)?;
        float(c, "groundImpactMinSpeed", &mut self.ground_impact_min_speed)?;
        c.floats("groundImpactShakeFreq", &mut self.ground_impact_shake_freq)?;
        c.floats("groundImpactShakeAmp", &mut self.ground_impact_shake_amp)?;
        float(
            c,
            "groundImpactShakeDuration",
            &mut self.ground_impact_shake_duration,
        )?;
        float(
            c,
            "groundImpactShakeFalloff",
            &mut self.ground_impact_shake_falloff,
        )
    }
}

pub fn pack_player_data(v: &dyn ObjectView, w: NetWidths) -> PlayerData {
    let f = |name: &str| raw(v, name, 0);
    PlayerData {
        shape: pack_shapebase_data(v, w),
        render_first_person: boolean(v, "renderFirstPerson", 0),
        min_look_angle: f("minLookAngle"),
        max_look_angle: f("maxLookAngle"),
        max_freelook_angle: f("maxFreelookAngle"),
        max_time_scale: f("maxTimeScale"),
        max_step_height: f("maxStepHeight"),
        run_force: f("runForce"),
        run_energy_drain: f("runEnergyDrain"),
        min_run_energy: f("minRunEnergy"),
        max_forward_speed: f("maxForwardSpeed"),
        max_backward_speed: f("maxBackwardSpeed"),
        max_side_speed: f("maxSideSpeed"),
        max_underwater_forward_speed: f("maxUnderwaterForwardSpeed"),
        max_underwater_backward_speed: f("maxUnderwaterBackwardSpeed"),
        max_underwater_side_speed: f("maxUnderwaterSideSpeed"),
        run_surface_angle: f("runSurfaceAngle"),
        recover_delay: word(v, "recoverDelay", 0),
        recover_run_force_scale: f("recoverRunForceScale"),
        jump_force: f("jumpForce"),
        jump_energy_drain: f("jumpEnergyDrain"),
        min_jump_energy: f("minJumpEnergy"),
        min_jump_speed: f("minJumpSpeed"),
        max_jump_speed: f("maxJumpSpeed"),
        jump_surface_angle: f("jumpSurfaceAngle"),
        jump_delay: mask_bits(word(v, "jumpDelay", 0), 7),
        horiz_max_speed: f("horizMaxSpeed"),
        horiz_resist_speed: f("horizResistSpeed"),
        horiz_resist_factor: f("horizResistFactor"),
        up_max_speed: f("upMaxSpeed"),
        up_resist_speed: f("upResistSpeed"),
        up_resist_factor: f("upResistFactor"),
        splash_velocity: f("splashVelocity"),
        splash_angle: f("splashAngle"),
        splash_freq_mod: f("splashFreqMod"),
        splash_vel_epsilon: f("splashVelEpsilon"),
        bubble_emit_time: f("bubbleEmitTime"),
        med_splash_sound_vel: f("medSplashSoundVel"),
        hard_splash_sound_vel: f("hardSplashSoundVel"),
        exit_splash_sound_vel: f("exitSplashSoundVel"),
        foot_splash_height: f("footSplashHeight"),
        min_impact_speed: f("minImpactSpeed"),
        sound: dbs(v, "sound", w),
        box_size: raws(v, "boxSize", 0),
        foot_puff_emitter: db(v, "footPuffEmitter", 0, w),
        foot_puff_num_parts: word(v, "footPuffNumParts", 0),
        foot_puff_radius: f("footPuffRadius"),
        decal_data: db(v, "decalData", 0, w),
        decal_offset: f("decalOffset"),
        dust_emitter: db(v, "dustEmitter", 0, w),
        splash: db(v, "splash", 0, w),
        splash_emitter_list: dbs(v, "splashEmitterList", w),
        ground_impact_min_speed: f("groundImpactMinSpeed"),
        ground_impact_shake_freq: raws(v, "groundImpactShakeFreq", 0),
        ground_impact_shake_amp: raws(v, "groundImpactShakeAmp", 0),
        ground_impact_shake_duration: f("groundImpactShakeDuration"),
        ground_impact_shake_falloff: f("groundImpactShakeFalloff"),
    }
}
