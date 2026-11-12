//! TGE's effect datablocks: [`ProjectileData`], [`ExplosionData`], [`DebrisData`], [`ParticleData`],
//! [`ParticleEmitterData`], [`ParticleEmitterNodeData`], [`SplashData`] and [`DecalData`].
//!
//! ```text
//! ProjectileData:  string projectileShapeName; flag faceViewer; flag(scale != 1,1,1): F32 x3; db particleEmitter,
//!                  particleWaterEmitter, explosion, waterExplosion, splash, sound; db x6 decals;
//!                  flag(hasLight): float(8) lightRadius / 20, float(7) x3 colour; flag(hasWaterLight): float(7) x3 colour;
//!                  ranged(lifetime, armingDelay, fadeDelay; 0, 4095); flag(isBallistic): F32 gravityMod, bounceElasticity,
//!                  bounceFriction
//! ExplosionData:   string dtsFileName; db soundProfile, particleEmitter; int(14) particleDensity; F32 particleRadius;
//!                  flag faceViewer; flag(explosionScale != 1,1,1): int(16) x3 scale * 100; int(14) playSpeed * 20;
//!                  ranged debrisThetaMin, debrisThetaMax (0..180), debrisPhiMin, debrisPhiMax (0..360), debrisNum,
//!                  debrisNumVariance (0..1000); int(14) debrisVelocity * 10; ranged debrisVelocityVariance * 10 (0..10000);
//!                  int(16) delayMS, delayVariance, lifetimeMS, lifetimeVariance (each >> 5); F32 offset; flag shakeCamera;
//!                  F32 x3 camShakeFreq; F32 x3 camShakeAmp; F32 camShakeDuration, camShakeRadius, camShakeFalloff;
//!                  db debrisList; db x4 emitterList; db x5 explosionList; ranged(keys, 0, 4); float(8) per key time;
//!                  ranged x3 per key size * 100 (0..16000); float(8) light start and end radius / 20; float(7) x3 start
//!                  colour; float(7) x3 end colour
//! DebrisData:      F32 elasticity, friction; U32 numBounces, bounceVariance; F32 minSpinSpeed, maxSpinSpeed;
//!                  bool render2D, explodeOnMaxBounce, staticOnMaxBounce, snapOnMaxBounce; F32 lifetime, lifetimeVariance,
//!                  minSpinSpeed, maxSpinSpeed (again), velocity, velocityVariance; bool fade, useRadiusMass;
//!                  F32 baseRadius, gravModifier, terminalVelocity; bool ignoreWater; string textureName, shapeName;
//!                  db x2 emitterList; db explosion
//! ParticleData:    float(10) dragCoefficient / 5; flag(windCoefficient != 1): F32; signed float(12) gravityCoefficient / 10;
//!                  float(9) inheritedVelFactor; flag(constantAcceleration != 0): F32; int(10) lifetimeMS >> 5,
//!                  lifetimeVarianceMS >> 5; flag(spinSpeed != 0): F32; flag(spinRandomMin or Max != 0): int(11) x2
//!                  (each + 1000); flag useInvAlpha; int(2) keys - 1; per key float(7) x4 colour, float(14) size / 50,
//!                  float(8) time; int(6) textures; string per texture
//! ParticleEmitterData: int(10) ejectionPeriodMS, periodVarianceMS; int(16) ejectionVelocity * 100; int(14) velocityVariance
//!                  * 100; flag(ejectionOffset != 0): int(16) * 100; ranged thetaMin, thetaMax (0..180);
//!                  flag(phiReferenceVel != 0): ranged (0..360); flag(phiVariance != 360): ranged (0..360); flag
//!                  overrideAdvance, orientParticles, orientOnVelocity; int(10) lifetimeMS >> 5, lifetimeVarianceMS >> 5;
//!                  flag useEmitterSizes, useEmitterColors; U32 count; U32 per particle datablock id
//! ParticleEmitterNodeData: F32 timeMultiple
//! SplashData:      F32 x3 scale; S32 delayMS, delayVariance, lifetimeMS, lifetimeVariance; F32 width; U32 numSegments;
//!                  F32 velocity, height, acceleration, texWrap, texFactor, ejectionFreq, ejectionAngle, ringLifetime,
//!                  startRadius; db explosion; db x3 emitterList; ColorI x4 colors; F32 x4 times; string x2 textureName
//! DecalData:       F32 sizeX, sizeY; string textureName
//! ```
//!
//! **The engine protocol's extension blocks** (only on a connection whose [`NetWidths::extensions`] is set; Torque's records end
//! where the list above ends):
//!
//! ```text
//! ParticleData:        flag(extension != defaults): F32 turbulence, turbulenceScale; flag collide; F32 bounceElasticity,
//!                      bounceFriction; int(2) lit; F32 softDistance; flag flipbookBlend; F32 sizeByVelocity
//! ParticleEmitterData: flag(extension != defaults): flag trail; int(18) budget
//! ```
//!
//! A reader sanitizes every value of a block as it reads it ([`ParticleExt::sanitize`], [`EmitterExt::sanitize`]): a float that
//! is not finite becomes its default and any other is held to its range, `lit` past 2 is the default and `budget` is held to
//! [`MAX_EMITTER_BUDGET`]. The writer sanitizes the same way, so a record that reaches the wire is one a reader accepts as it is.
//!
//! `bool` is `write(bool)`, one byte. A key count is a list: the decoder refuses more than [`MAX_EXPLOSION_KEYS`] keys,
//! [`MAX_PARTICLE_TEXTURES`] textures or [`MAX_EMITTER_PARTICLES`] particle ids.
//!
//! **Pack choices.** An explosion sends its keys up to and including the first whose time is 1 or more (TGE's loop means this
//! but tests an index left over from the loop before it; the layout is the same). A particle sends its keys the same way,
//! at most four, and its textures up to the first missing one.
//!
//! Credit: these follow TGE 1.4's `game/projectile.cc`, `game/fx/explosion.cc`, `game/debris.cc`, `game/fx/particleEngine.cc`,
//! `game/fx/particleEmitter.cc`, `game/fx/splash.cc` and `sim/decalManager.cc` (Copyright (C) GarageGames.com, Inc.), whose
//! MIT-licensed successors are Torque3D's `T3D/projectile.cpp`, `T3D/fx/explosion.cpp`, `T3D/debris.cpp`, `T3D/fx/particle.cpp`
//! and `T3D/fx/particleEmitter.cpp` (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.) and
//! `T3D/fx/particleEmitterNode.cpp`, `T3D/fx/splash.cpp` and `T3D/decal/decalData.cpp` (each Copyright (c) 2012 GarageGames,
//! LLC). The lists are TGE's; the code is this crate's own.

use super::{
    MAX_EMITTER_PARTICLES, MAX_EXPLOSION_KEYS, MAX_PARTICLE_TEXTURES, byte, cast, color_bytes,
    color_i, db, db_ref, db_refs, dbs, float, float_ints, float_ints_of, opt_float, present, texts,
    unless,
};
use crate::codec::{Codec, F32, Layout, NetWidths, R, mask_bits, opt, wrap_ranged};
use crate::motion::{float_bits, signed_float_bits};
use crate::{ObjectView, boolean, fld, int, raw, raws, text, word};

/// The decals of a ProjectileData (`NumDecals`).
pub const PROJECTILE_DECALS: usize = 6;
/// The longest life of a projectile in ticks (`Projectile::MaxLivingTicks`).
pub const MAX_LIVING_TICKS: u32 = 4095;
/// The emitters of an ExplosionData (`EC_NUM_EMITTERS`).
pub const EXPLOSION_EMITTERS: usize = 4;
/// The sub-explosions of an ExplosionData (`EC_MAX_SUB_EXPLOSIONS`).
pub const SUB_EXPLOSIONS: usize = 5;
/// The emitters of a DebrisData (`DDC_NUM_EMITTERS`).
pub const DEBRIS_EMITTERS: usize = 2;
/// The colour/size/time keys of a ParticleData.
pub const PARTICLE_KEYS: usize = 4;
/// The emitters of a SplashData (`NUM_EMITTERS`).
pub const SPLASH_EMITTERS: usize = 3;
/// The time keys of a SplashData (`NUM_TIME_KEYS`).
pub const SPLASH_KEYS: usize = 4;
/// The textures of a SplashData (`NUM_TEX`).
pub const SPLASH_TEXTURES: usize = 2;

/// A light radius's scale (`MaxLightRadius`): the radius travels as `writeFloat(radius / 20, 8)`.
const MAX_LIGHT_RADIUS: f32 = 20.0;

/// A particle size's scale (`MaxParticleSize`).
const MAX_PARTICLE_SIZE: f64 = 50.0;

/// The most `turbulence` a record keeps (units a second squared).
pub const MAX_TURBULENCE: f32 = 100.0;
/// The range of `turbulenceScale` (units, the size of the swirls).
pub const TURBULENCE_SCALE_RANGE: (f32, f32) = (0.01, 1_000.0);
/// The most `softDistance` a record keeps (units).
pub const MAX_SOFT_DISTANCE: f32 = 100.0;
/// The most `sizeByVelocity` a record keeps (seconds of a particle's speed).
pub const MAX_SIZE_BY_VELOCITY: f32 = 10.0;
/// The most `budget` an emitter record keeps (particles).
pub const MAX_EMITTER_BUDGET: u32 = 1 << 17;
/// The wire's `budget` width: it holds more than [`MAX_EMITTER_BUDGET`], and a reader cuts it back.
const BUDGET_BITS: u8 = 18;

/// `lit`: the plugin's rule for whether the particle is lit (the default), always lit, never lit.
pub const LIT_AUTO: u32 = 0;
pub const LIT_ON: u32 = 1;
pub const LIT_OFF: u32 = 2;

/// A float of a record held to `lo..=hi`, or `default` when it is not finite.
fn held(value: F32, default: f32, lo: f32, hi: f32) -> F32 {
    let v = value.get();
    F32::new(if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    })
}

/// `ProjectileData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectileData {
    pub projectile_shape_name: Vec<u8>,
    pub face_viewer: bool,
    /// `flag(scale != (1, 1, 1))`.
    pub scale: Option<[F32; 3]>,
    pub particle_emitter: Option<u32>,
    pub particle_water_emitter: Option<u32>,
    pub explosion: Option<u32>,
    pub water_explosion: Option<u32>,
    pub splash: Option<u32>,
    pub sound: Option<u32>,
    pub decals: [Option<u32>; PROJECTILE_DECALS],
    /// `flag(hasLight)`: `writeFloat(lightRadius / 20.0, 8)` and `writeFloat(channel, 7)` of red, green, blue.
    pub light: Option<(u32, [u32; 3])>,
    /// `flag(hasWaterLight)`: `writeFloat(channel, 7)` of red, green, blue.
    pub water_light: Option<[u32; 3]>,
    /// `ranged(lifetime, 0, 4095)`.
    pub lifetime: u32,
    pub arming_delay: u32,
    pub fade_delay: u32,
    /// `flag(isBallistic)`: gravityMod, bounceElasticity, bounceFriction.
    pub ballistic: Option<[F32; 3]>,
}

impl Layout for ProjectileData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.string("projectileShapeName", &mut self.projectile_shape_name)?;
        c.flag("faceViewer", &mut self.face_viewer)?;
        opt(c, "scale", &mut self.scale, |c, s| c.floats("scale", s))?;
        db_ref(c, "particleEmitter", &mut self.particle_emitter)?;
        db_ref(c, "particleWaterEmitter", &mut self.particle_water_emitter)?;
        db_ref(c, "explosion", &mut self.explosion)?;
        db_ref(c, "waterExplosion", &mut self.water_explosion)?;
        db_ref(c, "splash", &mut self.splash)?;
        db_ref(c, "sound", &mut self.sound)?;
        db_refs(c, "decals", &mut self.decals)?;
        opt(c, "hasLight", &mut self.light, |c, (radius, color)| {
            c.int("lightRadius", radius, 8)?;
            float_ints(c, "lightColor", color, 7)
        })?;
        opt(c, "hasWaterLight", &mut self.water_light, |c, color| {
            float_ints(c, "waterLightColor", color, 7)
        })?;
        c.ranged("lifetime", &mut self.lifetime, 0, MAX_LIVING_TICKS)?;
        c.ranged("armingDelay", &mut self.arming_delay, 0, MAX_LIVING_TICKS)?;
        c.ranged("fadeDelay", &mut self.fade_delay, 0, MAX_LIVING_TICKS)?;
        opt(c, "isBallistic", &mut self.ballistic, |c, b| {
            c.floats("ballistic", b)
        })
    }
}

pub fn pack_projectile_data(v: &dyn ObjectView, w: NetWidths) -> ProjectileData {
    let scale: [F32; 3] = raws(v, "scale", 0);
    let ticks = |name: &str| wrap_ranged(word(v, name, 0), 0, MAX_LIVING_TICKS);
    ProjectileData {
        projectile_shape_name: text(v, "projectileShapeName", 0),
        face_viewer: boolean(v, "faceViewer", 0),
        scale: scale.iter().any(|s| s.get() != 1.0).then_some(scale),
        particle_emitter: db(v, "particleEmitter", 0, w),
        particle_water_emitter: db(v, "particleWaterEmitter", 0, w),
        explosion: db(v, "explosion", 0, w),
        water_explosion: db(v, "waterExplosion", 0, w),
        splash: db(v, "splash", 0, w),
        sound: db(v, "sound", 0, w),
        decals: dbs(v, "decals", w),
        light: boolean(v, "hasLight", 0).then(|| {
            // `lightRadius / 20.0` is a double quotient, passed to writeFloat as an F32.
            let radius = (f64::from(fld(v, "lightRadius", 0)) / 20.0) as f32;
            (float_bits(radius, 8), float_ints_of(v, "lightColor", 0, 7))
        }),
        water_light: boolean(v, "hasWaterLight", 0)
            .then(|| float_ints_of(v, "waterLightColor", 0, 7)),
        lifetime: ticks("lifetime"),
        arming_delay: ticks("armingDelay"),
        fade_delay: ticks("fadeDelay"),
        ballistic: boolean(v, "isBallistic", 0).then(|| {
            [
                raw(v, "gravityMod", 0),
                raw(v, "bounceElasticity", 0),
                raw(v, "bounceFriction", 0),
            ]
        }),
    }
}

/// One size/time key of an explosion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExplosionKey {
    /// `writeFloat(time, 8)`.
    pub time: u32,
    /// `ranged((U32)(size * 100), 0, 16000)` of x, y, z.
    pub size: [u32; 3],
}

/// `ExplosionData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExplosionData {
    pub dts_file_name: Vec<u8>,
    pub sound_profile: Option<u32>,
    pub particle_emitter: Option<u32>,
    /// `int(14)`.
    pub particle_density: u32,
    pub particle_radius: F32,
    pub face_viewer: bool,
    /// `flag(explosionScale != (1, 1, 1))`: `int(16)` of `(S32)(axis * 100)`.
    pub explosion_scale: Option<[u32; 3]>,
    /// `int(14)` of `(S32)(playSpeed * 20)`.
    pub play_speed: u32,
    pub debris_theta_min: u32,
    pub debris_theta_max: u32,
    pub debris_phi_min: u32,
    pub debris_phi_max: u32,
    pub debris_num: u32,
    pub debris_num_variance: u32,
    /// `int(14)` of `(S32)(debrisVelocity * 10)`.
    pub debris_velocity: u32,
    /// `ranged((U32)(debrisVelocityVariance * 10), 0, 10000)`.
    pub debris_velocity_variance: u32,
    /// `int(16)` of `delayMS >> 5`, and so on.
    pub delay_ms: u32,
    pub delay_variance: u32,
    pub lifetime_ms: u32,
    pub lifetime_variance: u32,
    pub offset: F32,
    pub shake_camera: bool,
    pub cam_shake_freq: [F32; 3],
    pub cam_shake_amp: [F32; 3],
    pub cam_shake_duration: F32,
    pub cam_shake_radius: F32,
    pub cam_shake_falloff: F32,
    pub debris_list: Option<u32>,
    pub emitter_list: [Option<u32>; EXPLOSION_EMITTERS],
    pub explosion_list: [Option<u32>; SUB_EXPLOSIONS],
    pub keys: Vec<ExplosionKey>,
    /// `writeFloat(radius / 20, 8)` of the start and end radius.
    pub light_radius: [u32; 2],
    /// `writeFloat(channel, 7)` of the start colour's red, green, blue.
    pub light_start_color: [u32; 3],
    pub light_end_color: [u32; 3],
}

impl Layout for ExplosionData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.string("dtsFileName", &mut self.dts_file_name)?;
        db_ref(c, "soundProfile", &mut self.sound_profile)?;
        db_ref(c, "particleEmitter", &mut self.particle_emitter)?;
        c.int("particleDensity", &mut self.particle_density, 14)?;
        float(c, "particleRadius", &mut self.particle_radius)?;
        c.flag("faceViewer", &mut self.face_viewer)?;
        opt(c, "explosionScale", &mut self.explosion_scale, |c, s| {
            float_ints(c, "explosionScale", s, 16)
        })?;
        c.int("playSpeed", &mut self.play_speed, 14)?;
        c.ranged("debrisThetaMin", &mut self.debris_theta_min, 0, 180)?;
        c.ranged("debrisThetaMax", &mut self.debris_theta_max, 0, 180)?;
        c.ranged("debrisPhiMin", &mut self.debris_phi_min, 0, 360)?;
        c.ranged("debrisPhiMax", &mut self.debris_phi_max, 0, 360)?;
        c.ranged("debrisNum", &mut self.debris_num, 0, 1000)?;
        c.ranged("debrisNumVariance", &mut self.debris_num_variance, 0, 1000)?;
        c.int("debrisVelocity", &mut self.debris_velocity, 14)?;
        c.ranged(
            "debrisVelocityVariance",
            &mut self.debris_velocity_variance,
            0,
            10000,
        )?;
        c.int("delayMS", &mut self.delay_ms, 16)?;
        c.int("delayVariance", &mut self.delay_variance, 16)?;
        c.int("lifetimeMS", &mut self.lifetime_ms, 16)?;
        c.int("lifetimeVariance", &mut self.lifetime_variance, 16)?;
        float(c, "offset", &mut self.offset)?;
        c.flag("shakeCamera", &mut self.shake_camera)?;
        c.floats("camShakeFreq", &mut self.cam_shake_freq)?;
        c.floats("camShakeAmp", &mut self.cam_shake_amp)?;
        float(c, "camShakeDuration", &mut self.cam_shake_duration)?;
        float(c, "camShakeRadius", &mut self.cam_shake_radius)?;
        float(c, "camShakeFalloff", &mut self.cam_shake_falloff)?;
        db_ref(c, "debrisList", &mut self.debris_list)?;
        db_refs(c, "emitterList", &mut self.emitter_list)?;
        db_refs(c, "explosionList", &mut self.explosion_list)?;
        // ranged(count, 0, 4) is int(3); the times come first, then the sizes.
        c.seq(
            "keys",
            &mut self.keys,
            3,
            MAX_EXPLOSION_KEYS,
            8 + 3 * 14,
            |c, k| c.int("times", &mut k.time, 8),
        )?;
        for k in self.keys.iter_mut() {
            for s in k.size.iter_mut() {
                c.ranged("sizes", s, 0, 16000)?;
            }
        }
        float_ints(c, "lightRadius", &mut self.light_radius, 8)?;
        float_ints(c, "lightStartColor", &mut self.light_start_color, 7)?;
        float_ints(c, "lightEndColor", &mut self.light_end_color, 7)
    }
}

/// The keys a list of times sends: up to and including the first time of 1 or more, at most `max`.
fn key_count(times: impl Iterator<Item = f32>, max: usize) -> usize {
    let mut n = 0;
    for t in times.take(max) {
        n += 1;
        if t >= 1.0 {
            break;
        }
    }
    n.max(1)
}

pub fn pack_explosion_data(v: &dyn ObjectView, w: NetWidths) -> ExplosionData {
    let scale = [0, 1, 2].map(|k| fld(v, "explosionScale", k));
    let ranged = |name: &str, hi: u32| wrap_ranged(cast(fld(v, name, 0)), 0, hi);
    let ms = |name: &str| mask_bits((int(v, name, 0) as i32 >> 5) as u32, 16);
    let n = key_count(
        (0..MAX_EXPLOSION_KEYS).map(|k| fld(v, "times", k)),
        MAX_EXPLOSION_KEYS,
    );
    ExplosionData {
        dts_file_name: text(v, "dtsFileName", 0),
        sound_profile: db(v, "soundProfile", 0, w),
        particle_emitter: db(v, "particleEmitter", 0, w),
        particle_density: mask_bits(word(v, "particleDensity", 0), 14),
        particle_radius: raw(v, "particleRadius", 0),
        face_viewer: boolean(v, "faceViewer", 0),
        explosion_scale: scale
            .iter()
            .any(|&s| s != 1.0)
            .then(|| scale.map(|s| mask_bits(cast(s * 100.0), 16))),
        play_speed: mask_bits(cast(fld(v, "playSpeed", 0) * 20.0), 14),
        debris_theta_min: ranged("debrisThetaMin", 180),
        debris_theta_max: ranged("debrisThetaMax", 180),
        debris_phi_min: ranged("debrisPhiMin", 360),
        debris_phi_max: ranged("debrisPhiMax", 360),
        debris_num: wrap_ranged(word(v, "debrisNum", 0), 0, 1000),
        debris_num_variance: wrap_ranged(word(v, "debrisNumVariance", 0), 0, 1000),
        debris_velocity: mask_bits(cast(fld(v, "debrisVelocity", 0) * 10.0), 14),
        debris_velocity_variance: wrap_ranged(
            cast(fld(v, "debrisVelocityVariance", 0) * 10.0),
            0,
            10000,
        ),
        delay_ms: ms("delayMS"),
        delay_variance: ms("delayVariance"),
        lifetime_ms: ms("lifetimeMS"),
        lifetime_variance: ms("lifetimeVariance"),
        offset: raw(v, "offset", 0),
        shake_camera: boolean(v, "shakeCamera", 0),
        cam_shake_freq: raws(v, "camShakeFreq", 0),
        cam_shake_amp: raws(v, "camShakeAmp", 0),
        cam_shake_duration: raw(v, "camShakeDuration", 0),
        cam_shake_radius: raw(v, "camShakeRadius", 0),
        cam_shake_falloff: raw(v, "camShakeFalloff", 0),
        debris_list: db(v, "debrisList", 0, w),
        emitter_list: dbs(v, "emitterList", w),
        explosion_list: dbs(v, "explosionList", w),
        keys: (0..n)
            .map(|k| ExplosionKey {
                time: float_bits(fld(v, "times", k), 8),
                size: [0, 1, 2]
                    .map(|a| wrap_ranged(cast(fld(v, "sizes", 3 * k + a) * 100.0), 0, 16000)),
            })
            .collect(),
        light_radius: ["lightStartRadius", "lightEndRadius"]
            .map(|name| float_bits(fld(v, name, 0) / MAX_LIGHT_RADIUS, 8)),
        light_start_color: float_ints_of(v, "lightStartColor", 0, 7),
        light_end_color: float_ints_of(v, "lightEndColor", 0, 7),
    }
}

/// `DebrisData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DebrisData {
    pub elasticity: F32,
    pub friction: F32,
    pub num_bounces: u32,
    pub bounce_variance: u32,
    /// `minSpinSpeed`, `maxSpinSpeed`: sent twice; this is the first pair.
    pub spin_speed: [F32; 2],
    /// `write(bool)` bytes of render2D, explodeOnMaxBounce, staticOnMaxBounce, snapOnMaxBounce.
    pub bounce_flags: [u8; 4],
    pub lifetime: F32,
    pub lifetime_variance: F32,
    /// The second `minSpinSpeed`, `maxSpinSpeed`.
    pub spin_speed_again: [F32; 2],
    pub velocity: F32,
    pub velocity_variance: F32,
    /// `write(bool)` bytes of fade and useRadiusMass.
    pub fade_flags: [u8; 2],
    pub base_radius: F32,
    pub grav_modifier: F32,
    pub terminal_velocity: F32,
    pub ignore_water: u8,
    pub texture_name: Vec<u8>,
    pub shape_name: Vec<u8>,
    pub emitter_list: [Option<u32>; DEBRIS_EMITTERS],
    pub explosion: Option<u32>,
}

impl Layout for DebrisData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        float(c, "elasticity", &mut self.elasticity)?;
        float(c, "friction", &mut self.friction)?;
        c.raw32("numBounces", &mut self.num_bounces)?;
        c.raw32("bounceVariance", &mut self.bounce_variance)?;
        float(c, "minSpinSpeed", &mut self.spin_speed[0])?;
        float(c, "maxSpinSpeed", &mut self.spin_speed[1])?;
        for (name, b) in [
            "render2D",
            "explodeOnMaxBounce",
            "staticOnMaxBounce",
            "snapOnMaxBounce",
        ]
        .into_iter()
        .zip(self.bounce_flags.iter_mut())
        {
            c.raw8(name, b)?;
        }
        float(c, "lifetime", &mut self.lifetime)?;
        float(c, "lifetimeVariance", &mut self.lifetime_variance)?;
        float(c, "minSpinSpeed", &mut self.spin_speed_again[0])?;
        float(c, "maxSpinSpeed", &mut self.spin_speed_again[1])?;
        float(c, "velocity", &mut self.velocity)?;
        float(c, "velocityVariance", &mut self.velocity_variance)?;
        c.raw8("fade", &mut self.fade_flags[0])?;
        c.raw8("useRadiusMass", &mut self.fade_flags[1])?;
        float(c, "baseRadius", &mut self.base_radius)?;
        float(c, "gravModifier", &mut self.grav_modifier)?;
        float(c, "terminalVelocity", &mut self.terminal_velocity)?;
        c.raw8("ignoreWater", &mut self.ignore_water)?;
        c.string("textureName", &mut self.texture_name)?;
        c.string("shapeName", &mut self.shape_name)?;
        db_refs(c, "emitterList", &mut self.emitter_list)?;
        db_ref(c, "explosion", &mut self.explosion)
    }
}

pub fn pack_debris_data(v: &dyn ObjectView, w: NetWidths) -> DebrisData {
    let spin = [raw(v, "minSpinSpeed", 0), raw(v, "maxSpinSpeed", 0)];
    DebrisData {
        elasticity: raw(v, "elasticity", 0),
        friction: raw(v, "friction", 0),
        num_bounces: word(v, "numBounces", 0),
        bounce_variance: word(v, "bounceVariance", 0),
        spin_speed: spin,
        bounce_flags: [
            "render2D",
            "explodeOnMaxBounce",
            "staticOnMaxBounce",
            "snapOnMaxBounce",
        ]
        .map(|n| byte(v, n, 0)),
        lifetime: raw(v, "lifetime", 0),
        lifetime_variance: raw(v, "lifetimeVariance", 0),
        spin_speed_again: spin,
        velocity: raw(v, "velocity", 0),
        velocity_variance: raw(v, "velocityVariance", 0),
        fade_flags: [byte(v, "fade", 0), byte(v, "useRadiusMass", 0)],
        base_radius: raw(v, "baseRadius", 0),
        grav_modifier: raw(v, "gravModifier", 0),
        terminal_velocity: raw(v, "terminalVelocity", 0),
        ignore_water: byte(v, "ignoreWater", 0),
        texture_name: text(v, "textureName", 0),
        shape_name: text(v, "shapeName", 0),
        emitter_list: dbs(v, "emitterList", w),
        explosion: db(v, "explosion", 0, w),
    }
}

/// One colour/size/time key of a particle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParticleKey {
    /// `writeFloat(channel, 7)` of red, green, blue, alpha.
    pub color: [u32; 4],
    /// `writeFloat(size / 50, 14)`.
    pub size: u32,
    /// `writeFloat(time, 8)`.
    pub time: u32,
}

/// `ParticleData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParticleData {
    /// `writeFloat(dragCoefficient / 5, 10)`.
    pub drag_coefficient: u32,
    pub wind_coefficient: Option<F32>,
    /// `writeSignedFloat(gravityCoefficient / 10, 12)`.
    pub gravity_coefficient: u32,
    /// `writeFloat(inheritedVelFactor, 9)`.
    pub inherited_vel_factor: u32,
    pub constant_acceleration: Option<F32>,
    /// `int(10)` of `lifetimeMS >> 5`.
    pub lifetime_ms: u32,
    pub lifetime_variance_ms: u32,
    pub spin_speed: Option<F32>,
    /// `int(11)` of `(S32)(spinRandomMin + 1000)` and of the max.
    pub spin_random: Option<[u32; 2]>,
    pub use_inv_alpha: bool,
    /// One to four keys (the count travels as `int(2)` of keys - 1).
    pub keys: Vec<ParticleKey>,
    pub textures: Vec<Vec<u8>>,
    /// The engine protocol's block (none: every field at its default, or not on this connection).
    pub ext: Option<ParticleExt>,
}

impl ParticleData {
    /// The record as a connection of `widths` carries it: without its extension block unless the connection has them.
    #[must_use]
    pub fn for_widths(mut self, widths: NetWidths) -> ParticleData {
        if !widths.extensions {
            self.ext = None;
        }
        self
    }
}

/// The engine protocol's fields of a `ParticleData`, after TGE's. Their defaults leave TGE's look and motion as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticleExt {
    /// Curl noise acceleration (units a second squared; 0: none).
    pub turbulence: F32,
    /// The size of the noise's swirls (units).
    pub turbulence_scale: F32,
    /// Bounces off what the depth buffer shows.
    pub collide: bool,
    /// The share of the speed into a surface a bounce keeps, and the share of the speed along it a bounce loses.
    pub bounce_elasticity: F32,
    pub bounce_friction: F32,
    /// [`LIT_AUTO`], [`LIT_ON`] or [`LIT_OFF`].
    pub lit: u32,
    /// The depth over which the sprite fades into what is behind it (units).
    pub soft_distance: F32,
    /// Blends a flipbook's frame into the next.
    pub flipbook_blend: bool,
    /// The sprite stretched along its motion, its tail this many seconds of its speed behind it (0: TGE's square).
    pub size_by_velocity: F32,
}

/// The defaults of the fields of [`ParticleExt`] that are not 0 or false, as the text a script field's default is written in
/// (`ClassSpec::defaults`): each parses to its `ParticleExt::DEFAULT_*` (`the_scripts_defaults_are_the_records_defaults`).
pub const PARTICLE_EXT_DEFAULTS: &[(&str, &str)] = &[
    ("turbulenceScale", "1"),
    ("bounceElasticity", "0.3"),
    ("bounceFriction", "0.2"),
    ("softDistance", "0.4"),
];

impl ParticleExt {
    pub const DEFAULT_TURBULENCE_SCALE: f32 = 1.0;
    pub const DEFAULT_BOUNCE_ELASTICITY: f32 = 0.3;
    pub const DEFAULT_BOUNCE_FRICTION: f32 = 0.2;
    pub const DEFAULT_SOFT_DISTANCE: f32 = 0.4;

    /// Every value in range: a float that is not finite is its default, any other is held to its range; `lit` past
    /// [`LIT_OFF`] is [`LIT_AUTO`].
    #[must_use]
    pub fn sanitize(mut self) -> ParticleExt {
        self.turbulence = held(self.turbulence, 0.0, 0.0, MAX_TURBULENCE);
        self.turbulence_scale = held(
            self.turbulence_scale,
            Self::DEFAULT_TURBULENCE_SCALE,
            TURBULENCE_SCALE_RANGE.0,
            TURBULENCE_SCALE_RANGE.1,
        );
        self.bounce_elasticity = held(
            self.bounce_elasticity,
            Self::DEFAULT_BOUNCE_ELASTICITY,
            0.0,
            1.0,
        );
        self.bounce_friction = held(
            self.bounce_friction,
            Self::DEFAULT_BOUNCE_FRICTION,
            0.0,
            1.0,
        );
        self.soft_distance = held(
            self.soft_distance,
            Self::DEFAULT_SOFT_DISTANCE,
            0.0,
            MAX_SOFT_DISTANCE,
        );
        self.size_by_velocity = held(self.size_by_velocity, 0.0, 0.0, MAX_SIZE_BY_VELOCITY);
        if self.lit > LIT_OFF {
            self.lit = LIT_AUTO;
        }
        self
    }
}

impl Default for ParticleExt {
    fn default() -> ParticleExt {
        ParticleExt {
            turbulence: F32::new(0.0),
            turbulence_scale: F32::new(Self::DEFAULT_TURBULENCE_SCALE),
            collide: false,
            bounce_elasticity: F32::new(Self::DEFAULT_BOUNCE_ELASTICITY),
            bounce_friction: F32::new(Self::DEFAULT_BOUNCE_FRICTION),
            lit: LIT_AUTO,
            soft_distance: F32::new(Self::DEFAULT_SOFT_DISTANCE),
            flipbook_blend: false,
            size_by_velocity: F32::new(0.0),
        }
    }
}

impl Layout for ParticleExt {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        // What is written is in range, and so is what is read.
        *self = self.sanitize();
        float(c, "turbulence", &mut self.turbulence)?;
        float(c, "turbulenceScale", &mut self.turbulence_scale)?;
        c.flag("collide", &mut self.collide)?;
        float(c, "bounceElasticity", &mut self.bounce_elasticity)?;
        float(c, "bounceFriction", &mut self.bounce_friction)?;
        c.int("lit", &mut self.lit, 2)?;
        float(c, "softDistance", &mut self.soft_distance)?;
        c.flag("flipbookBlend", &mut self.flipbook_blend)?;
        float(c, "sizeByVelocity", &mut self.size_by_velocity)?;
        *self = self.sanitize();
        Ok(())
    }
}

impl Layout for ParticleData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.int("dragCoefficient", &mut self.drag_coefficient, 10)?;
        opt_float(c, "windCoefficient", &mut self.wind_coefficient)?;
        c.int("gravityCoefficient", &mut self.gravity_coefficient, 12)?;
        c.int("inheritedVelFactor", &mut self.inherited_vel_factor, 9)?;
        opt_float(c, "constantAcceleration", &mut self.constant_acceleration)?;
        c.int("lifetimeMS", &mut self.lifetime_ms, 10)?;
        c.int("lifetimeVarianceMS", &mut self.lifetime_variance_ms, 10)?;
        opt_float(c, "spinSpeed", &mut self.spin_speed)?;
        opt(c, "spinRandom", &mut self.spin_random, |c, s| {
            float_ints(c, "spinRandom", s, 11)
        })?;
        c.flag("useInvAlpha", &mut self.use_inv_alpha)?;
        let mut last = self.keys.len().clamp(1, PARTICLE_KEYS) as u32 - 1;
        c.int("keys", &mut last, 2)?;
        self.keys
            .resize_with(last as usize + 1, ParticleKey::default);
        for k in self.keys.iter_mut() {
            float_ints(c, "colors", &mut k.color, 7)?;
            c.int("sizes", &mut k.size, 14)?;
            c.int("times", &mut k.time, 8)?;
        }
        c.seq(
            "textures",
            &mut self.textures,
            6,
            MAX_PARTICLE_TEXTURES,
            1,
            |c, t| c.string("textureNameList", t),
        )?;
        if c.widths().extensions {
            opt(c, "ext", &mut self.ext, |c, e| e.layout(c))
        } else {
            self.ext = None;
            Ok(())
        }
    }
}

pub fn pack_particle_data(v: &dyn ObjectView) -> ParticleData {
    let spin = [fld(v, "spinRandomMin", 0), fld(v, "spinRandomMax", 0)];
    let shift = |name: &str| mask_bits((int(v, name, 0) as i32 >> 5) as u32, 10);
    let n = key_count(
        (0..PARTICLE_KEYS).map(|k| fld(v, "times", k)),
        PARTICLE_KEYS,
    );
    let textures = (0..MAX_PARTICLE_TEXTURES)
        .take_while(|&i| present(v, "textureNameList", i))
        .map(|i| text(v, "textureNameList", i))
        .collect();
    // A value the view lacks is the field's default.
    let or = |name: &str, default: f32| {
        if present(v, name, 0) {
            raw(v, name, 0)
        } else {
            F32::new(default)
        }
    };
    let ext = ParticleExt {
        turbulence: or("turbulence", 0.0),
        turbulence_scale: or("turbulenceScale", ParticleExt::DEFAULT_TURBULENCE_SCALE),
        collide: boolean(v, "collide", 0),
        bounce_elasticity: or("bounceElasticity", ParticleExt::DEFAULT_BOUNCE_ELASTICITY),
        bounce_friction: or("bounceFriction", ParticleExt::DEFAULT_BOUNCE_FRICTION),
        lit: word(v, "lit", 0),
        soft_distance: or("softDistance", ParticleExt::DEFAULT_SOFT_DISTANCE),
        flipbook_blend: boolean(v, "flipbookBlend", 0),
        size_by_velocity: or("sizeByVelocity", 0.0),
    }
    .sanitize();
    ParticleData {
        drag_coefficient: float_bits(fld(v, "dragCoefficient", 0) / 5.0, 10),
        wind_coefficient: unless(fld(v, "windCoefficient", 0), 1.0),
        gravity_coefficient: mask_bits(
            signed_float_bits(fld(v, "gravityCoefficient", 0) / 10.0, 12),
            12,
        ),
        inherited_vel_factor: float_bits(fld(v, "inheritedVelFactor", 0), 9),
        constant_acceleration: unless(fld(v, "constantAcceleration", 0), 0.0),
        lifetime_ms: shift("lifetimeMS"),
        lifetime_variance_ms: shift("lifetimeVarianceMS"),
        spin_speed: unless(fld(v, "spinSpeed", 0), 0.0),
        spin_random: (spin[0] != 0.0 || spin[1] != 0.0)
            .then(|| spin.map(|s| mask_bits(cast(s + 1000.0), 11))),
        use_inv_alpha: boolean(v, "useInvAlpha", 0),
        keys: (0..n)
            .map(|k| ParticleKey {
                color: float_ints_of(v, "colors", 4 * k, 7),
                size: float_bits(
                    (f64::from(fld(v, "sizes", k)) / MAX_PARTICLE_SIZE) as f32,
                    14,
                ),
                time: float_bits(fld(v, "times", k), 8),
            })
            .collect(),
        textures,
        ext: (ext != ParticleExt::default()).then_some(ext),
    }
}

/// `ParticleEmitterData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParticleEmitterData {
    /// `int(10)`.
    pub ejection_period_ms: u32,
    pub period_variance_ms: u32,
    /// `int(16)` of `(S32)(ejectionVelocity * 100)`.
    pub ejection_velocity: u32,
    /// `int(14)` of `(S32)(velocityVariance * 100)`.
    pub velocity_variance: u32,
    /// `flag(ejectionOffset != 0)`: `int(16)` of `(S32)(ejectionOffset * 100)`.
    pub ejection_offset: Option<u32>,
    pub theta_min: u32,
    pub theta_max: u32,
    /// `flag(phiReferenceVel != 0)`: `ranged(0, 360)`.
    pub phi_reference_vel: Option<u32>,
    /// `flag(phiVariance != 360)`: `ranged(0, 360)`.
    pub phi_variance: Option<u32>,
    pub override_advance: bool,
    pub orient_particles: bool,
    pub orient_on_velocity: bool,
    /// `int(10)` of `lifetimeMS >> 5`.
    pub lifetime_ms: u32,
    pub lifetime_variance_ms: u32,
    pub use_emitter_sizes: bool,
    pub use_emitter_colors: bool,
    /// The particle datablocks' ids, each a raw `U32` (not a ranged reference).
    pub data_block_ids: Vec<u32>,
    /// The engine protocol's block (none: every field at its default, or not on this connection).
    pub ext: Option<EmitterExt>,
}

impl ParticleEmitterData {
    /// The record as a connection of `widths` carries it: without its extension block unless the connection has them.
    #[must_use]
    pub fn for_widths(mut self, widths: NetWidths) -> ParticleEmitterData {
        if !widths.extensions {
            self.ext = None;
        }
        self
    }
}

/// The engine protocol's fields of a `ParticleEmitterData`, after TGE's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EmitterExt {
    /// Drawn as a ribbon bucket.
    pub trail: bool,
    /// The most particles the emitter may hold at once (0: no share of its own).
    pub budget: u32,
}

impl EmitterExt {
    /// Every value in range: `budget` is held to [`MAX_EMITTER_BUDGET`].
    #[must_use]
    pub fn sanitize(mut self) -> EmitterExt {
        self.budget = self.budget.min(MAX_EMITTER_BUDGET);
        self
    }
}

impl Layout for EmitterExt {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        *self = self.sanitize();
        c.flag("trail", &mut self.trail)?;
        c.int("budget", &mut self.budget, BUDGET_BITS)?;
        *self = self.sanitize();
        Ok(())
    }
}

impl Layout for ParticleEmitterData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.int("ejectionPeriodMS", &mut self.ejection_period_ms, 10)?;
        c.int("periodVarianceMS", &mut self.period_variance_ms, 10)?;
        c.int("ejectionVelocity", &mut self.ejection_velocity, 16)?;
        c.int("velocityVariance", &mut self.velocity_variance, 14)?;
        opt(c, "ejectionOffset", &mut self.ejection_offset, |c, o| {
            c.int("ejectionOffset", o, 16)
        })?;
        c.ranged("thetaMin", &mut self.theta_min, 0, 180)?;
        c.ranged("thetaMax", &mut self.theta_max, 0, 180)?;
        opt(c, "phiReferenceVel", &mut self.phi_reference_vel, |c, p| {
            c.ranged("phiReferenceVel", p, 0, 360)
        })?;
        opt(c, "phiVariance", &mut self.phi_variance, |c, p| {
            c.ranged("phiVariance", p, 0, 360)
        })?;
        c.flag("overrideAdvance", &mut self.override_advance)?;
        c.flag("orientParticles", &mut self.orient_particles)?;
        c.flag("orientOnVelocity", &mut self.orient_on_velocity)?;
        c.int("lifetimeMS", &mut self.lifetime_ms, 10)?;
        c.int("lifetimeVarianceMS", &mut self.lifetime_variance_ms, 10)?;
        c.flag("useEmitterSizes", &mut self.use_emitter_sizes)?;
        c.flag("useEmitterColors", &mut self.use_emitter_colors)?;
        c.seq(
            "dataBlockIds",
            &mut self.data_block_ids,
            32,
            MAX_EMITTER_PARTICLES,
            32,
            |c, id| c.raw32("dataBlockIds", id),
        )?;
        if c.widths().extensions {
            opt(c, "ext", &mut self.ext, |c, e| e.layout(c))
        } else {
            self.ext = None;
            Ok(())
        }
    }
}

pub fn pack_particle_emitter_data(v: &dyn ObjectView) -> ParticleEmitterData {
    let hundredths = |name: &str, bits: u8| mask_bits(cast(fld(v, name, 0) * 100.0), bits);
    let angle = |name: &str, hi: u32| wrap_ranged(cast(fld(v, name, 0)), 0, hi);
    let shift = |name: &str| mask_bits(word(v, name, 0) >> 5, 10);
    let ext = EmitterExt {
        trail: boolean(v, "trail", 0),
        budget: word(v, "budget", 0),
    }
    .sanitize();
    ParticleEmitterData {
        ejection_period_ms: mask_bits(word(v, "ejectionPeriodMS", 0), 10),
        period_variance_ms: mask_bits(word(v, "periodVarianceMS", 0), 10),
        ejection_velocity: hundredths("ejectionVelocity", 16),
        velocity_variance: hundredths("velocityVariance", 14),
        ejection_offset: unless(fld(v, "ejectionOffset", 0), 0.0)
            .map(|_| hundredths("ejectionOffset", 16)),
        theta_min: angle("thetaMin", 180),
        theta_max: angle("thetaMax", 180),
        phi_reference_vel: unless(fld(v, "phiReferenceVel", 0), 0.0)
            .map(|_| angle("phiReferenceVel", 360)),
        phi_variance: unless(fld(v, "phiVariance", 0), 360.0).map(|_| angle("phiVariance", 360)),
        override_advance: boolean(v, "overrideAdvance", 0),
        orient_particles: boolean(v, "orientParticles", 0),
        orient_on_velocity: boolean(v, "orientOnVelocity", 0),
        lifetime_ms: shift("lifetimeMS"),
        lifetime_variance_ms: shift("lifetimeVarianceMS"),
        use_emitter_sizes: boolean(v, "useEmitterSizes", 0),
        use_emitter_colors: boolean(v, "useEmitterColors", 0),
        data_block_ids: (0..MAX_EMITTER_PARTICLES)
            .take_while(|&i| present(v, "dataBlockIds", i))
            .map(|i| word(v, "dataBlockIds", i))
            .collect(),
        ext: (ext != EmitterExt::default()).then_some(ext),
    }
}

/// `ParticleEmitterNodeData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParticleEmitterNodeData {
    pub time_multiple: F32,
}

impl Layout for ParticleEmitterNodeData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        float(c, "timeMultiple", &mut self.time_multiple)
    }
}

pub fn pack_particle_emitter_node_data(v: &dyn ObjectView) -> ParticleEmitterNodeData {
    ParticleEmitterNodeData {
        time_multiple: raw(v, "timeMultiple", 0),
    }
}

/// `SplashData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SplashData {
    pub scale: [F32; 3],
    /// `S32`s' 32 bits: delayMS, delayVariance, lifetimeMS, lifetimeVariance.
    pub times_ms: [u32; 4],
    pub width: F32,
    pub num_segments: u32,
    pub velocity: F32,
    pub height: F32,
    pub acceleration: F32,
    pub tex_wrap: F32,
    pub tex_factor: F32,
    pub ejection_freq: F32,
    pub ejection_angle: F32,
    pub ring_lifetime: F32,
    pub start_radius: F32,
    pub explosion: Option<u32>,
    pub emitter_list: [Option<u32>; SPLASH_EMITTERS],
    pub colors: [[u8; 4]; SPLASH_KEYS],
    pub times: [F32; SPLASH_KEYS],
    pub texture_name: [Vec<u8>; SPLASH_TEXTURES],
}

impl Layout for SplashData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.floats("scale", &mut self.scale)?;
        for (name, x) in ["delayMS", "delayVariance", "lifetimeMS", "lifetimeVariance"]
            .into_iter()
            .zip(self.times_ms.iter_mut())
        {
            c.raw32(name, x)?;
        }
        float(c, "width", &mut self.width)?;
        c.raw32("numSegments", &mut self.num_segments)?;
        float(c, "velocity", &mut self.velocity)?;
        float(c, "height", &mut self.height)?;
        float(c, "acceleration", &mut self.acceleration)?;
        float(c, "texWrap", &mut self.tex_wrap)?;
        float(c, "texFactor", &mut self.tex_factor)?;
        float(c, "ejectionFreq", &mut self.ejection_freq)?;
        float(c, "ejectionAngle", &mut self.ejection_angle)?;
        float(c, "ringLifetime", &mut self.ring_lifetime)?;
        float(c, "startRadius", &mut self.start_radius)?;
        db_ref(c, "explosion", &mut self.explosion)?;
        db_refs(c, "emitterList", &mut self.emitter_list)?;
        for color in self.colors.iter_mut() {
            color_i(c, "colors", color)?;
        }
        c.floats("times", &mut self.times)?;
        super::strings(c, "textureName", &mut self.texture_name)
    }
}

pub fn pack_splash_data(v: &dyn ObjectView, w: NetWidths) -> SplashData {
    SplashData {
        scale: raws(v, "scale", 0),
        times_ms: ["delayMS", "delayVariance", "lifetimeMS", "lifetimeVariance"]
            .map(|n| word(v, n, 0)),
        width: raw(v, "width", 0),
        num_segments: word(v, "numSegments", 0),
        velocity: raw(v, "velocity", 0),
        height: raw(v, "height", 0),
        acceleration: raw(v, "acceleration", 0),
        tex_wrap: raw(v, "texWrap", 0),
        tex_factor: raw(v, "texFactor", 0),
        ejection_freq: raw(v, "ejectionFreq", 0),
        ejection_angle: raw(v, "ejectionAngle", 0),
        ring_lifetime: raw(v, "ringLifetime", 0),
        start_radius: raw(v, "startRadius", 0),
        explosion: db(v, "explosion", 0, w),
        emitter_list: dbs(v, "emitterList", w),
        colors: std::array::from_fn(|i| color_bytes(v, "colors", i)),
        times: raws(v, "times", 0),
        texture_name: texts(v, "textureName"),
    }
}

/// `DecalData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DecalData {
    pub size_x: F32,
    pub size_y: F32,
    pub texture_name: Vec<u8>,
}

impl Layout for DecalData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        float(c, "sizeX", &mut self.size_x)?;
        float(c, "sizeY", &mut self.size_y)?;
        c.string("textureName", &mut self.texture_name)
    }
}

pub fn pack_decal_data(v: &dyn ObjectView) -> DecalData {
    DecalData {
        size_x: raw(v, "sizeX", 0),
        size_y: raw(v, "sizeY", 0),
        texture_name: text(v, "textureName", 0),
    }
}
