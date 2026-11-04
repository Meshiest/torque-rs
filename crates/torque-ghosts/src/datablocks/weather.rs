//! TGE's weather and light datablocks: [`LightningData`], [`WeatherLightningData`], [`PrecipitationData`],
//! [`FireballAtmosphereData`] and [`FxLightData`].
//!
//! ```text
//! LightningData:          db x8 thunderSounds; string x8 strikeTextureNames; db strikeSound
//! WeatherLightningData:   string x6 strikeTextureNames, x4 flashTextureNames, x2 fuzzyTextureNames; db strikeSound;
//!                         db x4 thunderSounds
//! PrecipitationData:      db soundProfile; string mDropName, mSplashName; F32 mDropSize, mSplashSize; S32 mSplashMS;
//!                         flag mUseTrueBillboards
//! FireballAtmosphereData: db fireball
//! fxLightData:            bool mLightOn; F32 mRadius, mBrightness; ColorI mColour; string mFlareTextureName;
//!                         ColorI mFlareColour; bool mFlareOn, mFlareTP, mConstantSizeOn; F32 mConstantSize, mNearSize,
//!                         mFarSize, mNearDistance, mFarDistance, mFadeTime; U32 mBlendMode; flag mUseColour, mUseBrightness,
//!                         mUseRadius, mUseOffsets, mUseRotation, mLinkFlare, mLinkFlareSize; ColorI mMinColour, mMaxColour;
//!                         F32 mMinBrightness, mMaxBrightness, mMinRadius, mMaxRadius; F32 x3 mStartOffset, x3 mEndOffset;
//!                         F32 mMinRotation, mMaxRotation; flag mSingleColourKeys; string x7 keys (red, green, blue,
//!                         brightness, radius, offset, rotation); F32 x5 times (colour .. rotation); flag x5 lerp (colour ..
//!                         rotation)
//! ```
//! `bool` is `write(bool)`, one byte; `ColorI` is `write(ColorF)`, four bytes.
//!
//! Credit: these follow TGE 1.4's `game/fx/lightning.cc`, `game/fx/weatherLightning.cpp`, `game/fx/precipitation.cc` and
//! `game/fireballAtmosphere.cc` (Copyright (C) GarageGames.com, Inc.) and `game/fx/fxLight.cc` ("Written by Melvyn May,
//! Started on 4th May 2002", part of the Torque Game Engine). Lightning and Precipitation were carried into the MIT-licensed
//! Torque3D as `T3D/fx/lightning.cpp` and `T3D/fx/precipitation.cpp` (each Copyright (c) 2012 GarageGames, LLC); Torque3D
//! has no WeatherLightning, FireballAtmosphere or fxLight. The lists are TGE's; the code is this crate's own.

use super::{byte, color_bytes, color_i, db, db_ref, db_refs, dbs, float, strings, texts};
use crate::codec::{Codec, F32, Layout, NetWidths, R};
use crate::{ObjectView, boolean, raw, raws, text, word};

/// The thunder sounds of a LightningData (`MaxThunders`).
pub const LIGHTNING_THUNDERS: usize = 8;
/// The strike textures of a LightningData (`MaxTextures`).
pub const LIGHTNING_TEXTURES: usize = 8;

/// `LightningData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightningData {
    pub thunder_sounds: [Option<u32>; LIGHTNING_THUNDERS],
    pub strike_texture_names: [Vec<u8>; LIGHTNING_TEXTURES],
    pub strike_sound: Option<u32>,
}

impl Layout for LightningData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        db_refs(c, "thunderSounds", &mut self.thunder_sounds)?;
        strings(c, "strikeTextureNames", &mut self.strike_texture_names)?;
        db_ref(c, "strikeSound", &mut self.strike_sound)
    }
}

pub fn pack_lightning_data(v: &dyn ObjectView, w: NetWidths) -> LightningData {
    LightningData {
        thunder_sounds: dbs(v, "thunderSounds", w),
        strike_texture_names: texts(v, "strikeTextureNames"),
        strike_sound: db(v, "strikeSound", 0, w),
    }
}

/// `WeatherLightningData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WeatherLightningData {
    pub strike_texture_names: [Vec<u8>; 6],
    pub flash_texture_names: [Vec<u8>; 4],
    pub fuzzy_texture_names: [Vec<u8>; 2],
    pub strike_sound: Option<u32>,
    pub thunder_sounds: [Option<u32>; 4],
}

impl Layout for WeatherLightningData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        strings(c, "strikeTextureNames", &mut self.strike_texture_names)?;
        strings(c, "flashTextureNames", &mut self.flash_texture_names)?;
        strings(c, "fuzzyTextureNames", &mut self.fuzzy_texture_names)?;
        db_ref(c, "strikeSound", &mut self.strike_sound)?;
        db_refs(c, "thunderSounds", &mut self.thunder_sounds)
    }
}

pub fn pack_weather_lightning_data(v: &dyn ObjectView, w: NetWidths) -> WeatherLightningData {
    WeatherLightningData {
        strike_texture_names: texts(v, "strikeTextureNames"),
        flash_texture_names: texts(v, "flashTextureNames"),
        fuzzy_texture_names: texts(v, "fuzzyTextureNames"),
        strike_sound: db(v, "strikeSound", 0, w),
        thunder_sounds: dbs(v, "thunderSounds", w),
    }
}

/// `PrecipitationData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrecipitationData {
    pub sound_profile: Option<u32>,
    pub drop_name: Vec<u8>,
    pub splash_name: Vec<u8>,
    pub drop_size: F32,
    pub splash_size: F32,
    /// An `S32`'s 32 bits.
    pub splash_ms: u32,
    pub use_true_billboards: bool,
}

impl Layout for PrecipitationData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        db_ref(c, "soundProfile", &mut self.sound_profile)?;
        c.string("mDropName", &mut self.drop_name)?;
        c.string("mSplashName", &mut self.splash_name)?;
        float(c, "mDropSize", &mut self.drop_size)?;
        float(c, "mSplashSize", &mut self.splash_size)?;
        c.raw32("mSplashMS", &mut self.splash_ms)?;
        c.flag("mUseTrueBillboards", &mut self.use_true_billboards)
    }
}

pub fn pack_precipitation_data(v: &dyn ObjectView, w: NetWidths) -> PrecipitationData {
    PrecipitationData {
        sound_profile: db(v, "soundProfile", 0, w),
        drop_name: text(v, "mDropName", 0),
        splash_name: text(v, "mSplashName", 0),
        drop_size: raw(v, "mDropSize", 0),
        splash_size: raw(v, "mSplashSize", 0),
        splash_ms: word(v, "mSplashMS", 0),
        use_true_billboards: boolean(v, "mUseTrueBillboards", 0),
    }
}

/// `FireballAtmosphereData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FireballAtmosphereData {
    pub fireball: Option<u32>,
}

impl Layout for FireballAtmosphereData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        db_ref(c, "fireball", &mut self.fireball)
    }
}

pub fn pack_fireball_atmosphere_data(v: &dyn ObjectView, w: NetWidths) -> FireballAtmosphereData {
    FireballAtmosphereData {
        fireball: db(v, "fireball", 0, w),
    }
}

/// fxLight's flare sizes and distances, in wire order.
pub const FX_FLARE_FLOATS: [&str; 6] = [
    "mConstantSize",
    "mNearSize",
    "mFarSize",
    "mNearDistance",
    "mFarDistance",
    "mFadeTime",
];

/// fxLight's animation switches, in wire order.
pub const FX_USE_FLAGS: [&str; 7] = [
    "mUseColour",
    "mUseBrightness",
    "mUseRadius",
    "mUseOffsets",
    "mUseRotation",
    "mLinkFlare",
    "mLinkFlareSize",
];

/// fxLight's animation ranges, in wire order.
pub const FX_RANGE_FLOATS: [&str; 4] = [
    "mMinBrightness",
    "mMaxBrightness",
    "mMinRadius",
    "mMaxRadius",
];

/// fxLight's key strings, in wire order.
pub const FX_KEYS: [&str; 7] = [
    "mRedKeys",
    "mGreenKeys",
    "mBlueKeys",
    "mBrightnessKeys",
    "mRadiusKeys",
    "mOffsetKeys",
    "mRotationKeys",
];

/// fxLight's animation times, in wire order.
pub const FX_TIMES: [&str; 5] = [
    "mColourTime",
    "mBrightnessTime",
    "mRadiusTime",
    "mOffsetTime",
    "mRotationTime",
];

/// fxLight's interpolation switches, in wire order.
pub const FX_LERP_FLAGS: [&str; 5] = [
    "mLerpColour",
    "mLerpBrightness",
    "mLerpRadius",
    "mLerpOffset",
    "mLerpRotation",
];

/// `fxLightData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FxLightData {
    pub light_on: u8,
    pub radius: F32,
    pub brightness: F32,
    pub colour: [u8; 4],
    pub flare_texture_name: Vec<u8>,
    pub flare_colour: [u8; 4],
    /// `write(bool)` bytes of mFlareOn, mFlareTP, mConstantSizeOn.
    pub flare_flags: [u8; 3],
    /// The fields of [`FX_FLARE_FLOATS`].
    pub flare: [F32; 6],
    pub blend_mode: u32,
    /// The fields of [`FX_USE_FLAGS`].
    pub use_flags: [bool; 7],
    pub min_colour: [u8; 4],
    pub max_colour: [u8; 4],
    /// The fields of [`FX_RANGE_FLOATS`].
    pub ranges: [F32; 4],
    pub start_offset: [F32; 3],
    pub end_offset: [F32; 3],
    pub min_rotation: F32,
    pub max_rotation: F32,
    pub single_colour_keys: bool,
    /// The fields of [`FX_KEYS`].
    pub keys: [Vec<u8>; 7],
    /// The fields of [`FX_TIMES`].
    pub times: [F32; 5],
    /// The fields of [`FX_LERP_FLAGS`].
    pub lerp_flags: [bool; 5],
}

impl Layout for FxLightData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.raw8("mLightOn", &mut self.light_on)?;
        float(c, "mRadius", &mut self.radius)?;
        float(c, "mBrightness", &mut self.brightness)?;
        color_i(c, "mColour", &mut self.colour)?;
        c.string("mFlareTextureName", &mut self.flare_texture_name)?;
        color_i(c, "mFlareColour", &mut self.flare_colour)?;
        for (name, b) in ["mFlareOn", "mFlareTP", "mConstantSizeOn"]
            .into_iter()
            .zip(self.flare_flags.iter_mut())
        {
            c.raw8(name, b)?;
        }
        for (name, x) in FX_FLARE_FLOATS.into_iter().zip(self.flare.iter_mut()) {
            float(c, name, x)?;
        }
        c.raw32("mBlendMode", &mut self.blend_mode)?;
        for (name, b) in FX_USE_FLAGS.into_iter().zip(self.use_flags.iter_mut()) {
            c.flag(name, b)?;
        }
        color_i(c, "mMinColour", &mut self.min_colour)?;
        color_i(c, "mMaxColour", &mut self.max_colour)?;
        for (name, x) in FX_RANGE_FLOATS.into_iter().zip(self.ranges.iter_mut()) {
            float(c, name, x)?;
        }
        c.floats("mStartOffset", &mut self.start_offset)?;
        c.floats("mEndOffset", &mut self.end_offset)?;
        float(c, "mMinRotation", &mut self.min_rotation)?;
        float(c, "mMaxRotation", &mut self.max_rotation)?;
        c.flag("mSingleColourKeys", &mut self.single_colour_keys)?;
        for (name, s) in FX_KEYS.into_iter().zip(self.keys.iter_mut()) {
            c.string(name, s)?;
        }
        for (name, x) in FX_TIMES.into_iter().zip(self.times.iter_mut()) {
            float(c, name, x)?;
        }
        for (name, b) in FX_LERP_FLAGS.into_iter().zip(self.lerp_flags.iter_mut()) {
            c.flag(name, b)?;
        }
        Ok(())
    }
}

pub fn pack_fx_light_data(v: &dyn ObjectView) -> FxLightData {
    FxLightData {
        light_on: byte(v, "mLightOn", 0),
        radius: raw(v, "mRadius", 0),
        brightness: raw(v, "mBrightness", 0),
        colour: color_bytes(v, "mColour", 0),
        flare_texture_name: text(v, "mFlareTextureName", 0),
        flare_colour: color_bytes(v, "mFlareColour", 0),
        flare_flags: ["mFlareOn", "mFlareTP", "mConstantSizeOn"].map(|n| byte(v, n, 0)),
        flare: FX_FLARE_FLOATS.map(|n| raw(v, n, 0)),
        blend_mode: word(v, "mBlendMode", 0),
        use_flags: FX_USE_FLAGS.map(|n| boolean(v, n, 0)),
        min_colour: color_bytes(v, "mMinColour", 0),
        max_colour: color_bytes(v, "mMaxColour", 0),
        ranges: FX_RANGE_FLOATS.map(|n| raw(v, n, 0)),
        start_offset: raws(v, "mStartOffset", 0),
        end_offset: raws(v, "mEndOffset", 0),
        min_rotation: raw(v, "mMinRotation", 0),
        max_rotation: raw(v, "mMaxRotation", 0),
        single_colour_keys: boolean(v, "mSingleColourKeys", 0),
        keys: FX_KEYS.map(|n| text(v, n, 0)),
        times: FX_TIMES.map(|n| raw(v, n, 0)),
        lerp_flags: FX_LERP_FLAGS.map(|n| boolean(v, n, 0)),
    }
}
