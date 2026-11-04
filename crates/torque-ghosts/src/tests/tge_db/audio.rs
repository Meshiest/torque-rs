//! The audio datablocks.

use super::*;
use crate::datablocks::audio::{
    AudioEnvironment, pack_audio_description, pack_audio_environment, pack_audio_profile,
    pack_audio_sample_environment,
};
use crate::motion::Normal;

#[test]
fn audio_profile_is_two_datablocks_and_a_file_name() {
    check(
        &pack_audio_profile(&MapView::default(), widths()),
        "mDescriptionObject:1 mSampleEnvironment:1 mFilename:s",
        2 + EMPTY,
    );
    let v = MapView::default()
        .with("mDescriptionObject", 0, Value::Id(Some(12)))
        .with("mFilename", 0, Value::str("sound/boom.wav"));
    let rec = pack_audio_profile(&v, widths());
    assert_eq!(rec.description, Some(12));
    check_log(
        &rec,
        &spec("mDescriptionObject:1 mDescriptionObject:{db} mSampleEnvironment:1 mFilename:s"),
    );
    canonical(&rec, &ReadCtx::new(widths(), true));
}

#[test]
fn audio_description_with_tges_defaults_is_twelve_bits() {
    let rec = pack_audio_description(&floats(&[("mVolume", 1.0)]));
    assert_eq!(rec.volume, 63);
    check(
        &rec,
        "mVolume:6 mIsLooping:1 mIsStreaming:1 mIs3D:1 mType:3",
        12,
    );
}

#[test]
fn a_looping_3d_description_sends_its_loop_and_cone() {
    let v = floats(&[
        ("mVolume", 0.5),
        ("mReferenceDistance", 2.0),
        ("mMaxDistance", 50.0),
        ("mConeOutsideVolume", 0.5),
    ])
    .with("mIsLooping", 0, Value::Bool(true))
    .with("mLoopCount", 0, Value::Int(-1))
    .with("mIs3D", 0, Value::Bool(true))
    .with("mConeInsideAngle", 0, Value::Int(360))
    .with("mConeOutsideAngle", 0, Value::Int(360))
    .floats("mConeVector", &[0.0, 0.0, 1.0])
    .with("mType", 0, Value::Int(2));
    let rec = pack_audio_description(&v);
    let spatial = rec.spatial.unwrap();
    assert_eq!(rec.looping.unwrap().loop_count, u32::MAX);
    // The outside volume goes as the 6-bit fraction the reader reads.
    assert_eq!(spatial.cone_outside_volume, 31);
    assert_eq!(spatial.cone_vector, Normal::pack([0.0, 0.0, 1.0], 8));
    check(
        &rec,
        "mVolume:6 mIsLooping:1 mLoopCount:32 mMinLoopGap:32 mMaxLoopGap:32 mIsStreaming:1 mIs3D:1 \
         mReferenceDistance:32 mMaxDistance:32 mConeInsideAngle:9 mConeOutsideAngle:9 mConeOutsideVolume:6 normal.phi:9 \
         normal.theta:8 mEnvironmentLevel:32 mType:3",
        12 + 96 + 64 + 18 + 6 + 17 + 32,
    );
}

#[test]
fn audio_environment_is_a_preset_room_or_its_parameters() {
    let room = pack_audio_environment(&MapView::default().with("mUseRoom", 0, Value::Bool(true)));
    assert_eq!(room, AudioEnvironment::Room(0));
    check(&room, "mUseRoom:1 mRoom:5", 6);
    let v = floats(&[
        ("mDecayTime", 1.49),
        ("mEnvironmentSize", 7.5),
        ("mAirAbsorption", -5.0),
        ("mEffectVolume", 2.0),
    ])
    .with("mRoomHF", 0, Value::Int(-100))
    .with("mReflections", 0, Value::Int(-10000))
    .with("mFlags", 0, Value::Int(0x3f));
    let rec = pack_audio_environment(&v);
    let AudioEnvironment::Params(p) = rec else {
        panic!("{rec:?}")
    };
    // Ranged S32s travel as the offset from their minimum.
    assert_eq!(
        (p.room_hf, p.reflections, p.reverb, p.room_volume),
        (9900, 0, 10000, 10000)
    );
    assert_eq!(
        p.decay_time,
        ((1.49f32 - 0.1) / (20.0 - 0.1) * 255.0).floor() as u32
    );
    assert_eq!(
        p.air_absorption,
        ((-5.0f32 + 100.0) / 100.0 * 1023.0).floor() as u32
    );
    // The effect volume clamps into 0..1.
    assert_eq!(p.effect_volume, 255);
    check(
        &rec,
        "mUseRoom:1 mRoomHF:14 mReflections:15 mReverb:14 mRoomRolloffFactor:8 mDecayTime:8 mDecayHFRatio:8 \
         mReflectionsDelay:9 mReverbDelay:7 mRoomVolume:14 mEffectVolume:8 mDamping:9 mEnvironmentSize:10 \
         mEnvironmentDiffusion:8 mAirAbsorption:10 mFlags:6",
        149,
    );
}

#[test]
fn audio_sample_environment_is_four_levels_seven_ratios_a_level_and_flags() {
    let rec = pack_audio_sample_environment(&MapView::default());
    assert_eq!((rec.direct, rec.direct_hf), (10000, 10000));
    check(
        &rec,
        "mDirect:14 mDirectHF:14 mRoom:14 mRoomHF:14 mObstruction:9 mObstructionLFRatio:8 mOcclusion:9 \
         mOcclusionLFRatio:8 mOcclusionRoomRatio:9 mRoomRolloff:9 mAirAbsorption:9 mOutsideVolumeHF:14 mFlags:3",
        4 * 14 + 9 + 8 + 9 + 8 + 9 + 9 + 9 + 14 + 3,
    );
}
