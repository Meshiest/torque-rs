//! Projectiles, explosions, debris, particles and their emitters, splashes and decals.

use super::*;
use crate::datablocks::effects::{
    ExplosionData, ParticleData, ParticleEmitterData, pack_debris_data, pack_decal_data,
    pack_explosion_data, pack_particle_data, pack_particle_emitter_data,
    pack_particle_emitter_node_data, pack_projectile_data, pack_splash_data,
};
use crate::datablocks::{MAX_EMITTER_PARTICLES, MAX_EXPLOSION_KEYS, MAX_PARTICLE_TEXTURES};

/// The bits of `rec` with the field logged as `name` (its first occurrence) replaced by `value`.
pub(super) fn with_field<L: Layout>(rec: &L, name: &str, value: u32) -> (Vec<u8>, usize) {
    let t = table();
    let log = layout_log(rec, &t, [0.0; 3]);
    let at: usize = log
        .iter()
        .take_while(|(n, _)| *n != name)
        .map(|(_, w)| w)
        .sum();
    let width = log.iter().find(|(n, _)| *n == name).unwrap().1;
    let mut w = writer();
    encode(rec, &mut w, &t, widths()).unwrap();
    let n = w.bits();
    let bytes = w.into_bytes();
    let mut r = BitReader::with_bit_len(&bytes, n);
    let mut bits: Vec<bool> = (0..n).map(|_| r.read_flag().unwrap()).collect();
    for k in 0..width {
        bits[at + k] = (value >> k) & 1 == 1;
    }
    let mut out = BitWriter::new();
    for b in bits {
        out.write_flag(b);
    }
    (out.into_bytes(), n)
}

/// Decodes `L` from `bytes`; the error's text.
pub(super) fn refused<L: Layout + Debug>(bytes: &[u8], bits: usize) -> String {
    let got: Result<L, _> = decode(
        &mut reader(bytes, bits),
        &table(),
        &mut free_charge(),
        &ReadCtx::new(widths(), true),
    );
    format!("{:?}", got.expect_err("a count over its cap decoded"))
}

const PROJECTILE_DEFAULT: &str = "projectileShapeName:s faceViewer:1 scale:1 particleEmitter:1 particleWaterEmitter:1 \
     explosion:1 waterExplosion:1 splash:1 sound:1 decals:1*6 hasLight:1 hasWaterLight:1 lifetime:12 armingDelay:12 \
     fadeDelay:12 isBallistic:1";

#[test]
fn projectile_data_with_tges_defaults_is_its_flags_and_three_tick_counts() {
    let v = floats(&[("lifetime", 625.0), ("fadeDelay", 625.0)]).floats("scale", &[1.0, 1.0, 1.0]);
    let rec = pack_projectile_data(&v, widths());
    assert_eq!((rec.lifetime, rec.fade_delay), (625, 625));
    check(&rec, PROJECTILE_DEFAULT, EMPTY + 1 + 1 + 6 + 6 + 2 + 36 + 1);
}

#[test]
fn projectile_data_sends_its_scale_lights_and_ballistics() {
    let mut v = MapView::default()
        .with("projectileShapeName", 0, Value::str("bullet.dts"))
        .floats("scale", &[2.0, 1.0, 1.0])
        .with("hasLight", 0, Value::Bool(true))
        .with("lightRadius", 0, Value::F32(10.0))
        .floats("lightColor", &[1.0, 0.5, 0.0, 1.0])
        .with("hasWaterLight", 0, Value::Bool(true))
        .with("isBallistic", 0, Value::Bool(true))
        .with("gravityMod", 0, Value::F32(0.8))
        .with("lifetime", 0, Value::Int(5000));
    v = with_ids(v, "decals", 6, 10);
    for (i, name) in [
        "particleEmitter",
        "particleWaterEmitter",
        "explosion",
        "waterExplosion",
        "splash",
        "sound",
    ]
    .iter()
    .enumerate()
    {
        v = v.with(name, 0, Value::Id(Some(20 + i as u32)));
    }
    let rec = pack_projectile_data(&v, widths());
    // 10 / 20.0 = 0.5 of 255; the colour's channels of 127.
    assert_eq!(rec.light, Some((127, [127, 63, 0])));
    // 5000 ticks wrap into the 12-bit range.
    assert_eq!(rec.lifetime, 5000 & 4095);
    assert_eq!(rec.ballistic.unwrap()[0], F32::new(0.8));
    let golden = PROJECTILE_DEFAULT
        .replace("scale:1", "scale:1 scale:96")
        .replace("decals:1*6", &times("decals:1 decals:{db}", 6))
        .replace("hasLight:1", "hasLight:1 lightRadius:8 lightColor:7*3")
        .replace("hasWaterLight:1", "hasWaterLight:1 waterLightColor:7*3")
        .replace("isBallistic:1", "isBallistic:1 ballistic:96");
    let golden = [
        "particleEmitter",
        "particleWaterEmitter",
        "explosion",
        "waterExplosion",
        "splash",
        "sound",
    ]
    .iter()
    .fold(golden, |g, n| {
        g.replacen(&format!(" {n}:1"), &format!(" {n}:1 {n}:{{db}}"), 1)
    });
    check_log(&rec, &spec(&golden));
    canonical(&rec, &ReadCtx::new(widths(), true));
}

const EXPLOSION_DEFAULT: &str = "dtsFileName:s soundProfile:1 particleEmitter:1 particleDensity:14 particleRadius:32 \
     faceViewer:1 explosionScale:1 playSpeed:14 debrisThetaMin:8 debrisThetaMax:8 debrisPhiMin:9 debrisPhiMax:9 \
     debrisNum:10 debrisNumVariance:10 debrisVelocity:14 debrisVelocityVariance:14 delayMS:16 delayVariance:16 \
     lifetimeMS:16 lifetimeVariance:16 offset:32 shakeCamera:1 camShakeFreq:96 camShakeAmp:96 camShakeDuration:32 \
     camShakeRadius:32 camShakeFalloff:32 debrisList:1 emitterList:1*4 explosionList:1*5 keys:3 times:8*2 sizes:14*6 \
     lightRadius:8*2 lightStartColor:7*3 lightEndColor:7*3";

/// The view of an ExplosionData with TGE's constructor defaults: scale 1, play speed 1, times 0, 1, 1, 1.
fn explosion_defaults() -> MapView {
    floats(&[("playSpeed", 1.0)])
        .floats("explosionScale", &[1.0, 1.0, 1.0])
        .floats("times", &[0.0, 1.0, 1.0, 1.0])
}

#[test]
fn explosion_data_with_tges_defaults_sends_two_keys() {
    let rec = pack_explosion_data(&explosion_defaults(), widths());
    assert_eq!((rec.play_speed, rec.keys.len()), (20, 2));
    let bits = EMPTY
        + 2
        + 14
        + 32
        + 2
        + 14
        + (8 + 8 + 9 + 9 + 10 + 10)
        + 14
        + 14
        + 4 * 16
        + 32
        + 1
        + 2 * 96
        + 3 * 32
        + 10
        + 3
        + 2 * (8 + 3 * 14)
        + 16
        + 42;
    check(&rec, EXPLOSION_DEFAULT, bits);
}

#[test]
fn explosion_data_quantises_its_scale_speeds_times_and_light() {
    let v = explosion_defaults()
        .floats("explosionScale", &[2.0, 1.0, 0.5])
        .floats("times", &[0.0, 0.25, 0.5, 0.75])
        .floats(
            "sizes",
            &[
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
            ],
        )
        .with("debrisThetaMax", 0, Value::F32(90.0))
        .with("debrisVelocity", 0, Value::F32(3.5))
        .with("debrisVelocityVariance", 0, Value::F32(1.5))
        .with("delayMS", 0, Value::Int(1000))
        .with("lightStartRadius", 0, Value::F32(10.0));
    let rec = pack_explosion_data(&v, widths());
    assert_eq!(rec.explosion_scale, Some([200, 100, 50]));
    assert_eq!(
        (
            rec.debris_theta_max,
            rec.debris_velocity,
            rec.debris_velocity_variance
        ),
        (90, 35, 15)
    );
    assert_eq!(rec.delay_ms, 1000 >> 5);
    // No time reaches 1: all four keys go.
    assert_eq!(rec.keys.len(), MAX_EXPLOSION_KEYS);
    assert_eq!(rec.keys[3].size, [1000, 1100, 1200]);
    assert_eq!(rec.keys[1].time, (0.25f32 * 255.0) as u32);
    assert_eq!(rec.light_radius, [127, 0]);
    let golden = EXPLOSION_DEFAULT
        .replace("explosionScale:1", "explosionScale:1 explosionScale:16*3")
        .replace("times:8*2 sizes:14*6", "times:8*4 sizes:14*12");
    check_log(&rec, &golden);
    canonical(&rec, &ReadCtx::new(widths(), true));
    // A key count of 5..7 is refused.
    for n in 5..8 {
        let (bytes, bits) = with_field(&rec, "keys", n);
        assert!(refused::<ExplosionData>(&bytes, bits).contains("keys"));
    }
}

#[test]
fn debris_data_sends_its_spin_speeds_twice_and_its_bools_as_bytes() {
    let v = floats(&[("minSpinSpeed", -10.0), ("maxSpinSpeed", 20.0)])
        .with("render2D", 0, Value::Bool(true))
        .with("explosion", 0, Value::Id(Some(5)));
    let rec = pack_debris_data(&v, widths());
    assert_eq!(rec.spin_speed, rec.spin_speed_again);
    assert_eq!(rec.bounce_flags, [1, 0, 0, 0]);
    check(
        &rec,
        "elasticity:32 friction:32 numBounces:32 bounceVariance:32 minSpinSpeed:32 maxSpinSpeed:32 render2D:8 \
         explodeOnMaxBounce:8 staticOnMaxBounce:8 snapOnMaxBounce:8 lifetime:32 lifetimeVariance:32 minSpinSpeed:32 \
         maxSpinSpeed:32 velocity:32 velocityVariance:32 fade:8 useRadiusMass:8 baseRadius:32 gravModifier:32 \
         terminalVelocity:32 ignoreWater:8 textureName:s shapeName:s emitterList:1*2 explosion:1 explosion:{db}",
        4 * 32 + 2 * 32 + 4 * 8 + 6 * 32 + 2 * 8 + 3 * 32 + 8 + 2 * EMPTY + 3 + db(),
    );
}

const PARTICLE_DEFAULT: &str = "dragCoefficient:10 windCoefficient:1 gravityCoefficient:12 inheritedVelFactor:9 \
     constantAcceleration:1 lifetimeMS:10 lifetimeVarianceMS:10 spinSpeed:1 spinRandom:1 useInvAlpha:1 keys:2 \
     colors:7*4 sizes:14 times:8 colors:7*4 sizes:14 times:8 textures:6";

/// The view of a ParticleData with TGE's constructor defaults: wind 1, lifetime 1000, white, size 1, times 0, 1, 2, 2.
fn particle_defaults() -> MapView {
    floats(&[("windCoefficient", 1.0), ("lifetimeMS", 1000.0)])
        .floats("colors", &[1.0; 16])
        .floats("sizes", &[1.0; 4])
        .floats("times", &[0.0, 1.0, 2.0, 2.0])
}

#[test]
fn particle_data_with_tges_defaults_sends_two_keys_and_no_texture() {
    let rec = pack_particle_data(&particle_defaults());
    assert_eq!(rec.lifetime_ms, 1000 >> 5);
    // writeSignedFloat(0, 12): half of 4095.
    assert_eq!(rec.gravity_coefficient, 2047);
    assert_eq!(rec.keys[0].size, (1.0 / 50.0 * 16383.0f64) as u32);
    assert_eq!(rec.keys[0].color, [127; 4]);
    check(
        &rec,
        PARTICLE_DEFAULT,
        10 + 1 + 12 + 9 + 1 + 20 + 1 + 1 + 1 + 2 + 2 * (28 + 14 + 8) + 6,
    );
}

#[test]
fn particle_data_sends_its_spin_range_and_textures_up_to_the_first_missing_one() {
    let v = with_strs(
        particle_defaults(),
        "textureNameList",
        &["a.png", "b.png", "c.png"],
    )
    .with("textureNameList", 4, Value::str("after a gap"))
    .with("spinRandomMin", 0, Value::F32(-90.0))
    .with("spinRandomMax", 0, Value::F32(90.0))
    .with("dragCoefficient", 0, Value::F32(2.5))
    .floats("times", &[0.0, 0.3, 0.6, 0.9]);
    let rec = pack_particle_data(&v);
    assert_eq!(rec.spin_random, Some([910, 1090]));
    assert_eq!(rec.drag_coefficient, (0.5f32 * 1023.0) as u32);
    assert_eq!(rec.textures.len(), 3);
    assert_eq!(rec.keys.len(), 4);
    let golden = PARTICLE_DEFAULT
        .replace("spinRandom:1", "spinRandom:1 spinRandom:11*2")
        .replace(
            "times:8 textures:6",
            "times:8 colors:7*4 sizes:14 times:8 colors:7*4 sizes:14 times:8 textures:6",
        )
        + " textureNameList:s*3";
    check_log(&rec, &golden);
    canonical(&rec, &ReadCtx::new(widths(), true));
}

#[test]
fn particle_textures_over_the_cap_are_refused() {
    let names: Vec<String> = (0..MAX_PARTICLE_TEXTURES)
        .map(|i| format!("t{i}"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let rec = pack_particle_data(&with_strs(particle_defaults(), "textureNameList", &refs));
    assert_eq!(rec.textures.len(), MAX_PARTICLE_TEXTURES);
    canonical(&rec, &ReadCtx::new(widths(), true));
    // A count of 51..63 claims more names than the cap (the bits after it are names, so the cap is what refuses).
    let empty = ParticleData {
        textures: vec![Vec::new(); 13],
        ..pack_particle_data(&particle_defaults())
    };
    let (bytes, bits) = with_field(&empty, "textures", 51);
    assert!(refused::<ParticleData>(&bytes, bits).contains("textures"));
}

const EMITTER_DEFAULT: &str = "ejectionPeriodMS:10 periodVarianceMS:10 ejectionVelocity:16 velocityVariance:14 \
     ejectionOffset:1 thetaMin:8 thetaMax:8 phiReferenceVel:1 phiVariance:1 overrideAdvance:1 orientParticles:1 \
     orientOnVelocity:1 lifetimeMS:10 lifetimeVarianceMS:10 useEmitterSizes:1 useEmitterColors:1 dataBlockIds:32";

#[test]
fn particle_emitter_data_with_tges_defaults_and_its_particle_ids() {
    let defaults = floats(&[
        ("ejectionPeriodMS", 100.0),
        ("ejectionVelocity", 2.0),
        ("velocityVariance", 1.0),
        ("thetaMax", 90.0),
        ("phiVariance", 360.0),
        ("orientOnVelocity", 1.0),
    ]);
    let rec = pack_particle_emitter_data(&defaults);
    assert_eq!(
        (
            rec.ejection_period_ms,
            rec.ejection_velocity,
            rec.velocity_variance,
            rec.theta_max
        ),
        (100, 200, 100, 90)
    );
    check(
        &rec,
        EMITTER_DEFAULT,
        10 + 10 + 16 + 14 + 1 + 16 + 2 + 3 + 20 + 2 + 32,
    );
    let v = (0..3).fold(defaults, |v, i| {
        v.with("dataBlockIds", i, Value::Int(100 + i as i64))
    });
    let v = v
        .with("phiVariance", 0, Value::F32(180.0))
        .with("ejectionOffset", 0, Value::F32(0.5));
    let rec = pack_particle_emitter_data(&v);
    assert_eq!(rec.data_block_ids, vec![100, 101, 102]);
    assert_eq!(
        (rec.phi_variance, rec.ejection_offset),
        (Some(180), Some(50))
    );
    let golden = EMITTER_DEFAULT
        .replace("ejectionOffset:1", "ejectionOffset:1 ejectionOffset:16")
        .replace("phiVariance:1", "phiVariance:1 phiVariance:9")
        + " dataBlockIds:32*3";
    check(&rec, &golden, 126 + 16 + 9 + 3 * 32);
}

#[test]
fn emitter_particle_ids_over_the_cap_are_refused() {
    let rec = ParticleEmitterData {
        data_block_ids: vec![7; MAX_EMITTER_PARTICLES],
        ..ParticleEmitterData::default()
    };
    canonical(&rec, &ReadCtx::new(widths(), true));
    let (bytes, bits) = with_field(&rec, "dataBlockIds", MAX_EMITTER_PARTICLES as u32 + 1);
    assert!(refused::<ParticleEmitterData>(&bytes, bits).contains("dataBlockIds"));
    // The encoder cuts a longer list to the cap.
    let long = ParticleEmitterData {
        data_block_ids: vec![7; MAX_EMITTER_PARTICLES + 5],
        ..ParticleEmitterData::default()
    };
    assert_eq!(bit_total(&long, &table()), bit_total(&rec, &table()));
}

#[test]
fn the_small_effect_datablocks() {
    let node = pack_particle_emitter_node_data(&floats(&[("timeMultiple", 1.0)]));
    assert_eq!(node.time_multiple, F32::new(1.0));
    check(&node, "timeMultiple:32", 32);
    let decal =
        pack_decal_data(&floats(&[("sizeX", 0.5)]).with("textureName", 0, Value::str("decal.png")));
    check_log(&decal, "sizeX:32 sizeY:32 textureName:s");
    canonical(&decal, &ReadCtx::new(widths(), true));
    check(
        &pack_decal_data(&MapView::default()),
        "sizeX:32 sizeY:32 textureName:s",
        64 + EMPTY,
    );
}

#[test]
fn splash_data_sends_its_colours_as_bytes() {
    let v = MapView::default()
        .floats("colors", &[1.0, 0.0, 0.5, 1.0])
        .with("numSegments", 0, Value::Int(10))
        .with("explosion", 0, Value::Id(Some(9)));
    let rec = pack_splash_data(&v, widths());
    // ColorF to ColorI rounds: 0.5 * 255 + 0.5.
    assert_eq!(rec.colors[0], [255, 0, 128, 255]);
    check(
        &rec,
        "scale:96 delayMS:32 delayVariance:32 lifetimeMS:32 lifetimeVariance:32 width:32 numSegments:32 velocity:32 \
         height:32 acceleration:32 texWrap:32 texFactor:32 ejectionFreq:32 ejectionAngle:32 ringLifetime:32 \
         startRadius:32 explosion:1 explosion:{db} emitterList:1*3 colors:8*16 times:128 textureName:s*2",
        96 + 4 * 32 + 32 + 32 + 9 * 32 + 1 + db() + 3 + 128 + 128 + 2 * EMPTY,
    );
}

// The engine protocol's extension blocks.

use crate::datablocks::effects::{
    EmitterExt, LIT_AUTO, MAX_EMITTER_BUDGET, MAX_SIZE_BY_VELOCITY, MAX_SOFT_DISTANCE,
    MAX_TURBULENCE, ParticleExt, TURBULENCE_SCALE_RANGE,
};

fn with_extensions() -> NetWidths {
    widths().with_extensions(true)
}

/// A view of a ParticleData whose engine fields are not their defaults.
fn particle_ext_view() -> MapView {
    floats(&[
        ("turbulence", 4.0),
        ("turbulenceScale", 2.5),
        ("bounceElasticity", 0.6),
        ("bounceFriction", 0.1),
        ("softDistance", 1.5),
        ("sizeByVelocity", 0.05),
    ])
    .with("collide", 0, Value::Bool(true))
    .with("flipbookBlend", 0, Value::Bool(true))
    .with("lit", 0, Value::Int(2))
}

/// `rec` written in `widths`: its bytes and bit count.
fn written<L: Layout>(rec: &L, widths: NetWidths) -> (Vec<u8>, usize) {
    let mut w = writer();
    encode(rec, &mut w, &table(), widths).unwrap();
    let bits = w.bits();
    (w.into_bytes(), bits)
}

fn read<L: Layout>(
    bytes: &[u8],
    bits: usize,
    widths: NetWidths,
) -> Result<L, torque_net::NetError> {
    decode(
        &mut reader(bytes, bits),
        &table(),
        &mut free_charge(),
        &ReadCtx::new(widths, true),
    )
}

/// `rec` written in the extended widths with the first field logged as `name` replaced by `value` (its bits, low first), and
/// what a reader makes of it.
fn hostile<L: Layout + Debug>(rec: &L, name: &str, value: u32) -> L {
    let t = table();
    let mut w = writer();
    let mut c = Enc::logging(&mut w, &t, with_extensions());
    rec.clone().layout(&mut c).unwrap();
    let log = c.into_log();
    let at: usize = log
        .iter()
        .take_while(|(n, _)| *n != name)
        .map(|(_, w)| w)
        .sum();
    let width = log
        .iter()
        .find(|(n, _)| *n == name)
        .expect("a logged field")
        .1;
    let (bytes, bits) = written(rec, with_extensions());
    let mut r = reader(&bytes, bits);
    let mut flags: Vec<bool> = (0..bits).map(|_| r.read_flag().unwrap()).collect();
    for k in 0..width {
        flags[at + k] = (value >> k) & 1 == 1;
    }
    let mut out = writer();
    flags.into_iter().for_each(|b| out.write_flag(b));
    read(&out.into_bytes(), bits, with_extensions()).unwrap_or_else(|e| panic!("{name}: {e:?}"))
}

#[test]
fn torques_records_end_where_tges_do_and_the_engines_carry_the_extension_after() {
    let rec = pack_particle_data(&particle_ext_view());
    assert!(rec.ext.is_some());
    // Torque's widths: the record is TGE's bit for bit, the block not written.
    let plain = ParticleData {
        ext: None,
        ..rec.clone()
    };
    assert_eq!(written(&rec, widths()), written(&plain, widths()));
    let (bytes, bits) = written(&rec, widths());
    let back: ParticleData = read(&bytes, bits, widths()).unwrap();
    assert_eq!(back, plain);
    // The engine's: TGE's bits, then a presence flag and 196 bits of block (six floats of 32, two flags and a 2-bit enum).
    let (bytes, bits) = written(&rec, with_extensions());
    assert_eq!(bits, bit_total(&plain, &table()) + 1 + 196);
    let back: ParticleData = read(&bytes, bits, with_extensions()).unwrap();
    assert_eq!(back, rec);
    // Every field at its default is the flag alone, and a datablock with no value for them is that.
    let defaults = pack_particle_data(&particle_defaults());
    assert_eq!(defaults.ext, None);
    assert_eq!(
        written(&defaults, with_extensions()).1,
        bit_total(&defaults, &table()) + 1
    );
    let emitter = pack_particle_emitter_data(
        &floats(&[])
            .with("trail", 0, Value::Bool(true))
            .with("budget", 0, Value::Int(500)),
    );
    assert_eq!(
        emitter.ext,
        Some(EmitterExt {
            trail: true,
            budget: 500
        })
    );
    let (bytes, bits) = written(&emitter, with_extensions());
    let plain = ParticleEmitterData {
        ext: None,
        ..emitter.clone()
    };
    assert_eq!(bits, bit_total(&plain, &table()) + 1 + 1 + 18);
    assert_eq!(
        read::<ParticleEmitterData>(&bytes, bits, with_extensions()).unwrap(),
        emitter
    );
    let (bytes, bits) = written(&emitter, widths());
    assert_eq!((bytes, bits), written(&plain, widths()));
}

#[test]
fn a_hostile_value_of_each_particle_field_is_held_to_its_range() {
    let rec = pack_particle_data(&particle_ext_view());
    let ext = |name: &str, bits: u32| hostile(&rec, name, bits).ext.expect("the block");
    let (nan, inf) = (f32::NAN.to_bits(), f32::INFINITY.to_bits());
    let neg = (-3.0f32).to_bits();
    let huge = 1.0e30f32.to_bits();
    let d = ParticleExt::default();
    // turbulence
    assert_eq!(ext("turbulence", nan).turbulence, d.turbulence);
    assert_eq!(ext("turbulence", inf).turbulence, d.turbulence);
    assert_eq!(ext("turbulence", neg).turbulence.get(), 0.0);
    assert_eq!(ext("turbulence", huge).turbulence.get(), MAX_TURBULENCE);
    // turbulenceScale
    assert_eq!(
        ext("turbulenceScale", nan).turbulence_scale,
        d.turbulence_scale
    );
    assert_eq!(
        ext("turbulenceScale", 0).turbulence_scale.get(),
        TURBULENCE_SCALE_RANGE.0
    );
    assert_eq!(
        ext("turbulenceScale", neg).turbulence_scale.get(),
        TURBULENCE_SCALE_RANGE.0
    );
    assert_eq!(
        ext("turbulenceScale", huge).turbulence_scale.get(),
        TURBULENCE_SCALE_RANGE.1
    );
    // bounceElasticity and bounceFriction
    assert_eq!(
        ext("bounceElasticity", nan).bounce_elasticity,
        d.bounce_elasticity
    );
    assert_eq!(ext("bounceElasticity", neg).bounce_elasticity.get(), 0.0);
    assert_eq!(ext("bounceElasticity", huge).bounce_elasticity.get(), 1.0);
    assert_eq!(
        ext("bounceFriction", inf).bounce_friction,
        d.bounce_friction
    );
    assert_eq!(ext("bounceFriction", neg).bounce_friction.get(), 0.0);
    assert_eq!(ext("bounceFriction", huge).bounce_friction.get(), 1.0);
    // softDistance
    assert_eq!(ext("softDistance", nan).soft_distance, d.soft_distance);
    assert_eq!(ext("softDistance", neg).soft_distance.get(), 0.0);
    assert_eq!(
        ext("softDistance", huge).soft_distance.get(),
        MAX_SOFT_DISTANCE
    );
    // sizeByVelocity
    assert_eq!(
        ext("sizeByVelocity", nan).size_by_velocity,
        d.size_by_velocity
    );
    assert_eq!(ext("sizeByVelocity", neg).size_by_velocity.get(), 0.0);
    assert_eq!(
        ext("sizeByVelocity", huge).size_by_velocity.get(),
        MAX_SIZE_BY_VELOCITY
    );
    // lit: the 2-bit value past the enum is the default.
    assert_eq!(ext("lit", 3).lit, LIT_AUTO);
    assert_eq!(ext("lit", 1).lit, 1);
    // The others of the record are as sent.
    let other = ext("turbulence", huge);
    assert!(other.collide && other.flipbook_blend);
    assert_eq!(other.soft_distance.get(), 1.5);
}

#[test]
fn a_hostile_emitter_budget_is_held_to_its_cap_and_the_writer_holds_to_it_too() {
    let rec = pack_particle_emitter_data(&floats(&[]).with("trail", 0, Value::Bool(true)).with(
        "budget",
        0,
        Value::Int(10),
    ));
    let all_ones = (1u32 << 18) - 1;
    assert!(all_ones > MAX_EMITTER_BUDGET);
    let back = hostile(&rec, "budget", all_ones).ext.expect("the block");
    assert_eq!(
        back,
        EmitterExt {
            trail: true,
            budget: MAX_EMITTER_BUDGET
        }
    );
    assert_eq!(
        hostile(&rec, "budget", MAX_EMITTER_BUDGET + 1)
            .ext
            .unwrap()
            .budget,
        MAX_EMITTER_BUDGET
    );
    // A packer given a value past the cap, or one that is not finite, sends one that is not.
    let packed = pack_particle_emitter_data(&floats(&[]).with("trail", 0, Value::Bool(true)).with(
        "budget",
        0,
        Value::Int(i64::MAX),
    ));
    assert_eq!(packed.ext.unwrap().budget, MAX_EMITTER_BUDGET);
    let packed = pack_particle_data(
        &floats(&[("turbulence", f32::NAN), ("sizeByVelocity", f32::INFINITY)]).with(
            "collide",
            0,
            Value::Bool(true),
        ),
    );
    let ext = packed.ext.expect("collide is not a default");
    assert_eq!(
        (ext.turbulence.get(), ext.size_by_velocity.get()),
        (0.0, 0.0)
    );
    // A record built by hand with values out of range is written in range.
    let wild = ParticleData {
        ext: Some(ParticleExt {
            turbulence: F32::new(f32::NAN),
            lit: 9,
            ..ParticleExt::default()
        }),
        ..ParticleData::default()
    };
    let (bytes, bits) = written(&wild, with_extensions());
    let back: ParticleData = read(&bytes, bits, with_extensions()).unwrap();
    assert_eq!(back.ext, Some(ParticleExt::default()));
}

/// The text a script field's default is written in is the record's default.
#[test]
fn the_scripts_defaults_are_the_records_defaults() {
    let d = ParticleExt::default();
    let text = |name: &str| -> f32 {
        crate::datablocks::effects::PARTICLE_EXT_DEFAULTS
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no default for {name}"))
            .1
            .parse()
            .unwrap()
    };
    assert_eq!(text("turbulenceScale"), d.turbulence_scale.get());
    assert_eq!(text("bounceElasticity"), d.bounce_elasticity.get());
    assert_eq!(text("bounceFriction"), d.bounce_friction.get());
    assert_eq!(text("softDistance"), d.soft_distance.get());
    // And they are all the non-zero ones: every other field of the block defaults to 0 or false.
    assert_eq!(crate::datablocks::effects::PARTICLE_EXT_DEFAULTS.len(), 4);
    assert_eq!(
        (d.turbulence.get(), d.size_by_velocity.get(), d.lit),
        (0.0, 0.0, LIT_AUTO)
    );
    assert!(!d.collide && !d.flipbook_blend);
}

// The distortion block (`DecalData`, `ShapeBaseData` and the classes built on it).

use crate::datablocks::distortion::{
    DistortionExt, MAP_SCALE_RANGE, MAX_EDGE, MAX_SCROLL, MAX_STRENGTH,
};
use crate::datablocks::effects::DecalData;
use crate::datablocks::shapes::{ShapeBaseData, pack_shapebase_data};

/// A view whose distortion fields bend, tint and split.
fn distortion_view() -> MapView {
    floats(&[
        ("distortion", 0.5),
        ("distortionEdge", 3.0),
        ("distortionMapScale", 2.0),
        ("distortionChroma", 0.25),
    ])
    .floats("distortionScroll", &[0.1, -0.7])
    .floats("distortionTint", &[0.8, 0.9, 1.0, 0.3])
    .with("distortionMap", 0, Value::str("haze.png"))
    .with("distortionOnly", 0, Value::Bool(true))
}

#[test]
fn a_distortion_block_rides_the_engines_records_only() {
    let decal = pack_decal_data(&distortion_view().with("textureName", 0, Value::str("d.png")));
    let ext = decal.distortion.clone().expect("the block");
    assert_eq!(ext.map, b"haze.png".to_vec());
    assert_eq!(ext.tint.map(F32::get), [0.8, 0.9, 1.0, 0.3]);
    assert!(ext.only);
    let plain = DecalData {
        distortion: None,
        ..decal.clone()
    };
    // Torque's widths: TGE's bits.
    assert_eq!(written(&decal, widths()), written(&plain, widths()));
    // The engine's: TGE's bits and the block's flag, then ten floats, the map's name and a flag.
    let (bytes, bits) = written(&decal, with_extensions());
    assert_eq!(
        bits,
        written(&plain, with_extensions()).1 + 10 * 32 + string_bits(b"haze.png") + 1
    );
    assert_eq!(
        read::<DecalData>(&bytes, bits, with_extensions()).unwrap(),
        decal
    );
    // A decal that does not bend sends the flag alone, whatever its other fields (a tint alone, a class's unset `ColorF`: an
    // opaque black, is no bend).
    let none = pack_decal_data(
        &floats(&[("distortionMapScale", 3.0), ("distortionEdge", 2.0)])
            .floats("distortionTint", &[0.0, 0.0, 0.0, 1.0]),
    );
    assert_eq!(none.distortion, None);
    assert_eq!(
        written(&none, with_extensions()).1,
        bit_total(&none, &table()) + 1
    );
    // A shape's: the packer leaves the block out on Torque's widths, and it round-trips on the engine's.
    let shape = pack_shapebase_data(&distortion_view(), with_extensions());
    assert_eq!(shape.distortion, Some(ext));
    assert_eq!(
        pack_shapebase_data(&distortion_view(), widths()).distortion,
        None
    );
    let (bytes, bits) = written(&shape, with_extensions());
    assert_eq!(
        read::<ShapeBaseData>(&bytes, bits, with_extensions()).unwrap(),
        shape
    );
    let bare = ShapeBaseData {
        distortion: None,
        ..shape.clone()
    };
    assert_eq!(written(&shape, widths()), written(&bare, widths()));
    // A scale never set (0) reads as 1.
    let unset = pack_decal_data(&floats(&[("distortion", 0.2)]));
    assert_eq!(unset.distortion.expect("a bend").map_scale.get(), 1.0);
}

/// The bits a string takes on the wire (the codec's own count).
fn string_bits(s: &[u8]) -> usize {
    let mut w = writer();
    let t = table();
    let mut c = Enc::logging(&mut w, &t, widths());
    c.string("s", &mut s.to_vec()).unwrap();
    c.into_log().iter().map(|(_, b)| b).sum()
}

#[test]
fn a_hostile_value_of_each_distortion_field_is_held_to_its_range() {
    let rec = pack_decal_data(&distortion_view());
    let ext = |name: &str, bits: u32| hostile(&rec, name, bits).distortion.expect("the block");
    let (nan, inf) = (f32::NAN.to_bits(), f32::INFINITY.to_bits());
    let huge = 1.0e30f32.to_bits();
    let neg_huge = (-1.0e30f32).to_bits();
    let d = DistortionExt::default();
    assert_eq!(ext("distortion", nan).strength, d.strength);
    assert_eq!(ext("distortion", huge).strength.get(), MAX_STRENGTH);
    assert_eq!(ext("distortion", neg_huge).strength.get(), -MAX_STRENGTH);
    assert_eq!(ext("distortionEdge", inf).edge, d.edge);
    assert_eq!(ext("distortionEdge", huge).edge.get(), MAX_EDGE);
    assert_eq!(ext("distortionEdge", neg_huge).edge.get(), 0.0);
    assert_eq!(ext("distortionMapScale", nan).map_scale, d.map_scale);
    assert_eq!(
        ext("distortionMapScale", 0).map_scale.get(),
        MAP_SCALE_RANGE.0
    );
    assert_eq!(
        ext("distortionMapScale", huge).map_scale.get(),
        MAP_SCALE_RANGE.1
    );
    // The vectors (more than one word: the reader's sanitize, which the layout runs on what it read).
    let vectors = DistortionExt {
        scroll: [F32::new(1.0e30), F32::new(f32::NAN)],
        tint: [
            F32::new(1.0e30),
            F32::new(-1.0e30),
            F32::new(f32::NAN),
            F32::new(0.5),
        ],
        ..DistortionExt::default()
    }
    .sanitize();
    assert_eq!(vectors.scroll.map(F32::get), [MAX_SCROLL, 0.0]);
    assert_eq!(vectors.tint.map(F32::get), [1.0, 0.0, 1.0, 0.5]);
    assert_eq!(ext("distortionChroma", huge).chroma.get(), 1.0);
    assert_eq!(ext("distortionChroma", nan).chroma, d.chroma);
    // The rest of the record is as sent.
    let other = ext("distortion", huge);
    assert_eq!(other.edge.get(), 3.0);
    assert!(other.only);
    // A record built by hand out of range is written in range.
    let wild = DecalData {
        distortion: Some(DistortionExt {
            strength: F32::new(f32::NAN),
            chroma: F32::new(9.0),
            ..DistortionExt::default()
        }),
        ..DecalData::default()
    };
    let (bytes, bits) = written(&wild, with_extensions());
    let back: DecalData = read(&bytes, bits, with_extensions()).unwrap();
    assert_eq!(
        back.distortion.map(|e| (e.strength.get(), e.chroma.get())),
        Some((0.0, 1.0))
    );
}
