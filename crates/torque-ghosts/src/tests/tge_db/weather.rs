//! Lightning, weather lightning, precipitation, the fireball atmosphere and fxLight.

use super::*;
use crate::datablocks::weather::{
    pack_fireball_atmosphere_data, pack_fx_light_data, pack_lightning_data,
    pack_precipitation_data, pack_weather_lightning_data,
};

#[test]
fn lightning_data_is_eight_thunders_eight_textures_and_a_strike() {
    let rec = pack_lightning_data(&MapView::default(), widths());
    check(
        &rec,
        "thunderSounds:1*8 strikeTextureNames:s*8 strikeSound:1",
        9 + 8 * EMPTY,
    );
    let v = with_strs(
        with_ids(MapView::default(), "thunderSounds", 2, 30),
        "strikeTextureNames",
        &["bolt"],
    )
    .with("strikeSound", 0, Value::Id(Some(40)));
    let rec = pack_lightning_data(&v, widths());
    assert_eq!(rec.strike_texture_names[0], b"bolt".to_vec());
    let golden = format!(
        "{} thunderSounds:1*6 strikeTextureNames:s*8 strikeSound:1 strikeSound:{{db}}",
        times("thunderSounds:1 thunderSounds:{db}", 2)
    );
    check_log(&rec, &spec(&golden));
    canonical(&rec, &ReadCtx::new(widths(), true));
}

#[test]
fn weather_lightning_data_is_its_textures_then_its_sounds() {
    let rec = pack_weather_lightning_data(&MapView::default(), widths());
    check(
        &rec,
        "strikeTextureNames:s*6 flashTextureNames:s*4 fuzzyTextureNames:s*2 strikeSound:1 thunderSounds:1*4",
        12 * EMPTY + 5,
    );
}

#[test]
fn precipitation_data_is_a_sound_two_names_and_its_drop_sizes() {
    let v = floats(&[("mDropSize", 0.5), ("mSplashSize", 0.5)])
        .with("mSplashMS", 0, Value::Int(250))
        .with("mUseTrueBillboards", 0, Value::Bool(true));
    let rec = pack_precipitation_data(&v, widths());
    assert_eq!((rec.splash_ms, rec.use_true_billboards), (250, true));
    check(
        &rec,
        "soundProfile:1 mDropName:s mSplashName:s mDropSize:32 mSplashSize:32 mSplashMS:32 mUseTrueBillboards:1",
        1 + 2 * EMPTY + 96 + 1,
    );
}

#[test]
fn fireball_atmosphere_data_is_one_datablock() {
    check(
        &pack_fireball_atmosphere_data(&MapView::default(), widths()),
        "fireball:1",
        1,
    );
    let rec = pack_fireball_atmosphere_data(
        &MapView::default().with("fireball", 0, Value::Id(Some(4))),
        widths(),
    );
    check(&rec, "fireball:1 fireball:{db}", 1 + db());
}

#[test]
fn fx_light_data_is_its_light_flare_and_animation_blocks() {
    let v = floats(&[("mRadius", 10.0), ("mFadeTime", 0.1)])
        .with("mLightOn", 0, Value::Bool(true))
        .floats("mColour", &[1.0, 0.5, 0.25, 1.0])
        .with("mUseColour", 0, Value::Bool(true))
        .with("mRedKeys", 0, Value::str("AZA"))
        .with("mLerpRotation", 0, Value::Bool(true));
    let rec = pack_fx_light_data(&v);
    assert_eq!((rec.light_on, rec.colour), (1, [255, 128, 64, 255]));
    assert_eq!((rec.use_flags[0], rec.lerp_flags[4]), (true, true));
    check_log(
        &rec,
        "mLightOn:8 mRadius:32 mBrightness:32 mColour:8*4 mFlareTextureName:s mFlareColour:8*4 mFlareOn:8 mFlareTP:8 \
         mConstantSizeOn:8 mConstantSize:32 mNearSize:32 mFarSize:32 mNearDistance:32 mFarDistance:32 mFadeTime:32 \
         mBlendMode:32 mUseColour:1 mUseBrightness:1 mUseRadius:1 mUseOffsets:1 mUseRotation:1 mLinkFlare:1 \
         mLinkFlareSize:1 mMinColour:8*4 mMaxColour:8*4 mMinBrightness:32 mMaxBrightness:32 mMinRadius:32 mMaxRadius:32 \
         mStartOffset:96 mEndOffset:96 mMinRotation:32 mMaxRotation:32 mSingleColourKeys:1 mRedKeys:s mGreenKeys:s \
         mBlueKeys:s mBrightnessKeys:s mRadiusKeys:s mOffsetKeys:s mRotationKeys:s mColourTime:32 mBrightnessTime:32 \
         mRadiusTime:32 mOffsetTime:32 mRotationTime:32 mLerpColour:1 mLerpBrightness:1 mLerpRadius:1 mLerpOffset:1 \
         mLerpRotation:1",
    );
    canonical(&rec, &ReadCtx::new(widths(), true));
    let bits = 8
        + 2 * 32
        + 32
        + EMPTY
        + 32
        + 3 * 8
        + 6 * 32
        + 32
        + 7
        + 2 * 32
        + 4 * 32
        + 2 * 96
        + 2 * 32
        + 1;
    assert_eq!(
        bit_total(&pack_fx_light_data(&MapView::default()), &table()),
        bits + 7 * EMPTY + 5 * 32 + 5
    );
}
