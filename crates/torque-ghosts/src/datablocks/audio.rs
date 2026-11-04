//! TGE's audio datablocks: [`AudioProfile`], [`AudioDescription`], [`AudioEnvironment`] and [`AudioSampleEnvironment`].
//!
//! ```text
//! AudioProfile:           db mDescriptionObject; db mSampleEnvironment; string mFilename
//! AudioDescription:       float(6) mVolume; flag(mIsLooping): S32 mLoopCount, mMinLoopGap, mMaxLoopGap; flag mIsStreaming;
//!                         flag(mIs3D): F32 mReferenceDistance, mMaxDistance, int(9) mConeInsideAngle, int(9)
//!                         mConeOutsideAngle, int(6) mConeOutsideVolume, normal vector(8) mConeVector, F32 mEnvironmentLevel;
//!                         int(3) mType
//! AudioEnvironment:       flag(mUseRoom): ranged(mRoom, 0, 26) [else: ranged S32 mRoomHF (-10000..0), mReflections
//!                         (-10000..10000), mReverb (-10000..2000); ranged float mRoomRolloffFactor (0.1..10, 8),
//!                         mDecayTime (0.1..20, 8), mDecayHFRatio (0.1..20, 8), mReflectionsDelay (0..0.3, 9), mReverbDelay
//!                         (0..0.1, 7); ranged S32 mRoomVolume (-10000..0); ranged float mEffectVolume (0..1, 8), mDamping
//!                         (0..2, 9), mEnvironmentSize (1..100, 10), mEnvironmentDiffusion (0..1, 8), mAirAbsorption
//!                         (-100..0, 10); int(6) mFlags]
//! AudioSampleEnvironment: ranged S32 mDirect (-10000..1000), mDirectHF (-10000..0), mRoom (-10000..1000), mRoomHF
//!                         (-10000..0); ranged float mObstruction (0..1, 9), mObstructionLFRatio (0..1, 8), mOcclusion (0..1,
//!                         9), mOcclusionLFRatio (0..1, 8), mOcclusionRoomRatio (0..10, 9), mRoomRolloff (0..10, 9),
//!                         mAirAbsorption (0..10, 9); ranged S32 mOutsideVolumeHF (-10000..0); int(3) mFlags
//! ```
//! A ranged S32 travels as `ranged(value - min, 0, max - min)` (the record holds `value - min`); a ranged float as
//! `int(bits)` of the value clamped into its range and scaled to `0..2^bits - 1`. The room is one of EAX's 26 preset
//! environments (`EAX_ENVIRONMENT_GENERIC` .. `EAX_ENVIRONMENT_COUNT`).
//!
//! **View fields** are the members without their `mDescription.` prefix (`mVolume`, `mIs3D`, `mConeVector` 0..3, ...).
//!
//! **The cone's outside volume** is a float from 0 to 1. TGE's `packData` writes it with `writeInt` (the float cut to an
//! integer, so only 0 or 1) while its `unpackData` reads `readFloat(6)`; this packer writes `writeFloat(volume, 6)`, the form the
//! reader reads. The width is the same.
//!
//! Credit: these follow TGE 1.4's `audio/audioDataBlock.cc` (Copyright (C) GarageGames.com, Inc.); Torque3D's MIT-licensed
//! sound datablocks that replaced them are `sfx/sfxProfile.cpp`, `sfx/sfxDescription.cpp` (each Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.) and `sfx/sfxEnvironment.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! with other fields. The lists are TGE's; the code is this crate's own.

use super::{db, db_ref, float};
use crate::codec::{Codec, F32, Layout, NetWidths, R, mask_bits, opt, wrap_ranged};
use crate::motion::{Normal, float_bits};
use crate::{ObjectView, boolean, fld, int, raw, raws, text, word};

/// The EAX preset rooms (`EAX_ENVIRONMENT_COUNT`); a room travels as `ranged(room, 0, 26)`.
pub const EAX_ROOMS: u32 = 26;

/// `AudioProfile::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioProfile {
    pub description: Option<u32>,
    pub sample_environment: Option<u32>,
    pub filename: Vec<u8>,
}

impl Layout for AudioProfile {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        db_ref(c, "mDescriptionObject", &mut self.description)?;
        db_ref(c, "mSampleEnvironment", &mut self.sample_environment)?;
        c.string("mFilename", &mut self.filename)
    }
}

pub fn pack_audio_profile(v: &dyn ObjectView, w: NetWidths) -> AudioProfile {
    AudioProfile {
        description: db(v, "mDescriptionObject", 0, w),
        sample_environment: db(v, "mSampleEnvironment", 0, w),
        filename: text(v, "mFilename", 0),
    }
}

/// A looping description's counts (`S32`s' 32 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Looping {
    pub loop_count: u32,
    pub min_loop_gap: u32,
    pub max_loop_gap: u32,
}

/// A 3D description's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spatial {
    pub reference_distance: F32,
    pub max_distance: F32,
    /// `int(9)`.
    pub cone_inside_angle: u32,
    /// `int(9)`.
    pub cone_outside_angle: u32,
    /// `int(6)`.
    pub cone_outside_volume: u32,
    pub cone_vector: Normal,
    pub environment_level: F32,
}

/// `AudioDescription::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioDescription {
    /// `writeFloat(mVolume, 6)`.
    pub volume: u32,
    /// `flag(mIsLooping)`.
    pub looping: Option<Looping>,
    pub is_streaming: bool,
    /// `flag(mIs3D)`.
    pub spatial: Option<Spatial>,
    /// `int(3)`.
    pub audio_type: u32,
}

impl Layout for AudioDescription {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.int("mVolume", &mut self.volume, 6)?;
        opt(c, "mIsLooping", &mut self.looping, |c, l| {
            c.raw32("mLoopCount", &mut l.loop_count)?;
            c.raw32("mMinLoopGap", &mut l.min_loop_gap)?;
            c.raw32("mMaxLoopGap", &mut l.max_loop_gap)
        })?;
        c.flag("mIsStreaming", &mut self.is_streaming)?;
        opt(c, "mIs3D", &mut self.spatial, |c, s| {
            float(c, "mReferenceDistance", &mut s.reference_distance)?;
            float(c, "mMaxDistance", &mut s.max_distance)?;
            c.int("mConeInsideAngle", &mut s.cone_inside_angle, 9)?;
            c.int("mConeOutsideAngle", &mut s.cone_outside_angle, 9)?;
            c.int("mConeOutsideVolume", &mut s.cone_outside_volume, 6)?;
            s.cone_vector.layout(c, 8)?;
            float(c, "mEnvironmentLevel", &mut s.environment_level)
        })?;
        c.int("mType", &mut self.audio_type, 3)
    }
}

pub fn pack_audio_description(v: &dyn ObjectView) -> AudioDescription {
    AudioDescription {
        volume: float_bits(fld(v, "mVolume", 0), 6),
        looping: boolean(v, "mIsLooping", 0).then(|| Looping {
            loop_count: word(v, "mLoopCount", 0),
            min_loop_gap: word(v, "mMinLoopGap", 0),
            max_loop_gap: word(v, "mMaxLoopGap", 0),
        }),
        is_streaming: boolean(v, "mIsStreaming", 0),
        spatial: boolean(v, "mIs3D", 0).then(|| Spatial {
            reference_distance: raw(v, "mReferenceDistance", 0),
            max_distance: raw(v, "mMaxDistance", 0),
            cone_inside_angle: mask_bits(word(v, "mConeInsideAngle", 0), 9),
            cone_outside_angle: mask_bits(word(v, "mConeOutsideAngle", 0), 9),
            cone_outside_volume: float_bits(fld(v, "mConeOutsideVolume", 0), 6),
            cone_vector: Normal::pack(raws::<3>(v, "mConeVector", 0).map(F32::get), 8),
            environment_level: raw(v, "mEnvironmentLevel", 0),
        }),
        audio_type: mask_bits(word(v, "mType", 0), 3),
    }
}

/// A ranged `S32`: `ranged(value - min, 0, max - min)`.
fn ranged_s32<C: Codec>(c: &mut C, name: &'static str, v: &mut u32, min: i32, max: i32) -> R {
    c.ranged(name, v, 0, max.abs_diff(min))
}

/// The wire value of a ranged `S32`.
fn pack_s32(view: &dyn ObjectView, name: &str, min: i32, max: i32) -> u32 {
    let v = int(view, name, 0) as i32;
    wrap_ranged(v.wrapping_sub(min) as u32, 0, max.abs_diff(min))
}

/// The wire value of a ranged float (`writeRangedF32`).
fn pack_f32(view: &dyn ObjectView, name: &str, min: f32, max: f32, bits: u8) -> u32 {
    let v = (torque_net::library::get().write_ranged_f32)(fld(view, name, 0), min, max, bits);
    mask_bits(v, bits)
}

/// An environment's parameters (sent when it does not use a preset room); the ranged `S32`s hold `value - min`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EnvironmentParams {
    pub room_hf: u32,
    pub reflections: u32,
    pub reverb: u32,
    pub room_rolloff_factor: u32,
    pub decay_time: u32,
    pub decay_hf_ratio: u32,
    pub reflections_delay: u32,
    pub reverb_delay: u32,
    pub room_volume: u32,
    pub effect_volume: u32,
    pub damping: u32,
    pub environment_size: u32,
    pub environment_diffusion: u32,
    pub air_absorption: u32,
    /// `int(6)`.
    pub flags: u32,
}

/// `AudioEnvironment::packData`: a preset room or the parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEnvironment {
    /// `flag(mUseRoom)` set: `ranged(mRoom, 0, 26)`.
    Room(u32),
    Params(EnvironmentParams),
}

impl Default for AudioEnvironment {
    fn default() -> AudioEnvironment {
        AudioEnvironment::Params(EnvironmentParams::default())
    }
}

impl Layout for AudioEnvironment {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        let mut use_room = matches!(self, AudioEnvironment::Room(_));
        c.flag("mUseRoom", &mut use_room)?;
        if use_room {
            let mut room = match self {
                AudioEnvironment::Room(r) => *r,
                AudioEnvironment::Params(_) => 0,
            };
            c.ranged("mRoom", &mut room, 0, EAX_ROOMS)?;
            *self = AudioEnvironment::Room(room);
            return Ok(());
        }
        let mut p = match self {
            AudioEnvironment::Params(p) => *p,
            AudioEnvironment::Room(_) => EnvironmentParams::default(),
        };
        ranged_s32(c, "mRoomHF", &mut p.room_hf, -10000, 0)?;
        ranged_s32(c, "mReflections", &mut p.reflections, -10000, 10000)?;
        ranged_s32(c, "mReverb", &mut p.reverb, -10000, 2000)?;
        c.int("mRoomRolloffFactor", &mut p.room_rolloff_factor, 8)?;
        c.int("mDecayTime", &mut p.decay_time, 8)?;
        c.int("mDecayHFRatio", &mut p.decay_hf_ratio, 8)?;
        c.int("mReflectionsDelay", &mut p.reflections_delay, 9)?;
        c.int("mReverbDelay", &mut p.reverb_delay, 7)?;
        ranged_s32(c, "mRoomVolume", &mut p.room_volume, -10000, 0)?;
        c.int("mEffectVolume", &mut p.effect_volume, 8)?;
        c.int("mDamping", &mut p.damping, 9)?;
        c.int("mEnvironmentSize", &mut p.environment_size, 10)?;
        c.int("mEnvironmentDiffusion", &mut p.environment_diffusion, 8)?;
        c.int("mAirAbsorption", &mut p.air_absorption, 10)?;
        c.int("mFlags", &mut p.flags, 6)?;
        *self = AudioEnvironment::Params(p);
        Ok(())
    }
}

pub fn pack_audio_environment(v: &dyn ObjectView) -> AudioEnvironment {
    if boolean(v, "mUseRoom", 0) {
        return AudioEnvironment::Room(wrap_ranged(word(v, "mRoom", 0), 0, EAX_ROOMS));
    }
    AudioEnvironment::Params(EnvironmentParams {
        room_hf: pack_s32(v, "mRoomHF", -10000, 0),
        reflections: pack_s32(v, "mReflections", -10000, 10000),
        reverb: pack_s32(v, "mReverb", -10000, 2000),
        room_rolloff_factor: pack_f32(v, "mRoomRolloffFactor", 0.1, 10.0, 8),
        decay_time: pack_f32(v, "mDecayTime", 0.1, 20.0, 8),
        decay_hf_ratio: pack_f32(v, "mDecayHFRatio", 0.1, 20.0, 8),
        reflections_delay: pack_f32(v, "mReflectionsDelay", 0.0, 0.3, 9),
        reverb_delay: pack_f32(v, "mReverbDelay", 0.0, 0.1, 7),
        room_volume: pack_s32(v, "mRoomVolume", -10000, 0),
        effect_volume: pack_f32(v, "mEffectVolume", 0.0, 1.0, 8),
        damping: pack_f32(v, "mDamping", 0.0, 2.0, 9),
        environment_size: pack_f32(v, "mEnvironmentSize", 1.0, 100.0, 10),
        environment_diffusion: pack_f32(v, "mEnvironmentDiffusion", 0.0, 1.0, 8),
        air_absorption: pack_f32(v, "mAirAbsorption", -100.0, 0.0, 10),
        flags: mask_bits(word(v, "mFlags", 0), 6),
    })
}

/// `AudioSampleEnvironment::packData`; the ranged `S32`s hold `value - min`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioSampleEnvironment {
    pub direct: u32,
    pub direct_hf: u32,
    pub room: u32,
    pub room_hf: u32,
    pub obstruction: u32,
    pub obstruction_lf_ratio: u32,
    pub occlusion: u32,
    pub occlusion_lf_ratio: u32,
    pub occlusion_room_ratio: u32,
    pub room_rolloff: u32,
    pub air_absorption: u32,
    pub outside_volume_hf: u32,
    /// `int(3)`.
    pub flags: u32,
}

impl Layout for AudioSampleEnvironment {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        ranged_s32(c, "mDirect", &mut self.direct, -10000, 1000)?;
        ranged_s32(c, "mDirectHF", &mut self.direct_hf, -10000, 0)?;
        ranged_s32(c, "mRoom", &mut self.room, -10000, 1000)?;
        ranged_s32(c, "mRoomHF", &mut self.room_hf, -10000, 0)?;
        c.int("mObstruction", &mut self.obstruction, 9)?;
        c.int("mObstructionLFRatio", &mut self.obstruction_lf_ratio, 8)?;
        c.int("mOcclusion", &mut self.occlusion, 9)?;
        c.int("mOcclusionLFRatio", &mut self.occlusion_lf_ratio, 8)?;
        c.int("mOcclusionRoomRatio", &mut self.occlusion_room_ratio, 9)?;
        c.int("mRoomRolloff", &mut self.room_rolloff, 9)?;
        c.int("mAirAbsorption", &mut self.air_absorption, 9)?;
        ranged_s32(
            c,
            "mOutsideVolumeHF",
            &mut self.outside_volume_hf,
            -10000,
            0,
        )?;
        c.int("mFlags", &mut self.flags, 3)
    }
}

pub fn pack_audio_sample_environment(v: &dyn ObjectView) -> AudioSampleEnvironment {
    AudioSampleEnvironment {
        direct: pack_s32(v, "mDirect", -10000, 1000),
        direct_hf: pack_s32(v, "mDirectHF", -10000, 0),
        room: pack_s32(v, "mRoom", -10000, 1000),
        room_hf: pack_s32(v, "mRoomHF", -10000, 0),
        obstruction: pack_f32(v, "mObstruction", 0.0, 1.0, 9),
        obstruction_lf_ratio: pack_f32(v, "mObstructionLFRatio", 0.0, 1.0, 8),
        occlusion: pack_f32(v, "mOcclusion", 0.0, 1.0, 9),
        occlusion_lf_ratio: pack_f32(v, "mOcclusionLFRatio", 0.0, 1.0, 8),
        occlusion_room_ratio: pack_f32(v, "mOcclusionRoomRatio", 0.0, 10.0, 9),
        room_rolloff: pack_f32(v, "mRoomRolloff", 0.0, 10.0, 9),
        air_absorption: pack_f32(v, "mAirAbsorption", 0.0, 10.0, 9),
        outside_volume_hf: pack_s32(v, "mOutsideVolumeHF", -10000, 0),
        flags: mask_bits(word(v, "mFlags", 0), 3),
    }
}
