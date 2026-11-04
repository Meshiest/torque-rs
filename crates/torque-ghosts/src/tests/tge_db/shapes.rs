//! ShapeBaseData and the classes over it, and ShapeBaseImageData.

use super::*;
use crate::datablocks::image::{IMAGE_STATES, Transitions, pack_image_data};
use crate::datablocks::shapes::{
    ShapeBaseData, pack_item_data, pack_player_data, pack_shapebase_data, pack_static_shape_data,
};
use crate::motion::{Signed, float_bits, signed_float_bits};

/// ShapeBaseData with TGE's constructor defaults: none of the nine compared floats is sent.
pub(super) const SB_DEFAULT: &str = "computeCRC:1 shapeName:s cloakTexName:s mass:1 drag:1 density:1 maxEnergy:1 \
     cameraMaxDist:1 cameraMinDist:1 cameraDefaultFov:1 cameraMinFov:1 cameraMaxFov:1 debrisShapeName:s \
     observeThroughObject:1 debris:1 emap:1 isInvincible:1 renderWhenDestroyed:1 explosion:1 underwaterExplosion:1 \
     inheritEnergyFromMount:1 firstPersonOnly:1 useEyePoint:1";

/// Twenty flags and three empty strings.
pub(super) const SB_DEFAULT_BITS: usize = 20 + 3 * EMPTY;

/// The view of a ShapeBaseData with TGE's constructor defaults (mass 1, density 1, camera 0.2 / 90 / 5 / 120).
pub(super) fn shape_defaults() -> MapView {
    floats(&[
        ("mass", 1.0),
        ("density", 1.0),
        ("cameraMinDist", 0.2),
        ("cameraDefaultFov", 90.0),
        ("cameraMinFov", 5.0),
        ("cameraMaxFov", 120.0),
    ])
}

#[test]
fn shapebase_data_with_tges_defaults_is_its_flags_and_three_strings() {
    let rec = pack_shapebase_data(&shape_defaults(), widths());
    assert_eq!(rec, ShapeBaseData::default());
    check(&rec, SB_DEFAULT, SB_DEFAULT_BITS);
}

#[test]
fn shapebase_data_sends_every_float_that_differs() {
    let v = floats(&[
        ("mass", 2.0),
        ("drag", 0.5),
        ("density", 3.0),
        ("maxEnergy", 100.0),
        ("cameraMaxDist", 4.0),
        ("cameraMinDist", 1.0),
        ("cameraDefaultFov", 70.0),
        ("cameraMinFov", 10.0),
        ("cameraMaxFov", 110.0),
    ])
    .with("computeCRC", 0, Value::Bool(true))
    .with("mCRC", 0, Value::Int(0x1234_5678))
    .with("shapeName", 0, Value::str("shapes/thing.dts"))
    .with("cloakTexName", 0, Value::str("cloak"))
    .with("debrisShapeName", 0, Value::str("shapes/bits.dts"))
    .with("debris", 0, Value::Id(Some(40)))
    .with("explosion", 0, Value::Id(Some(41)))
    .with("underwaterExplosion", 0, Value::Id(Some(42)))
    .with("useEyePoint", 0, Value::Bool(true));
    let rec = pack_shapebase_data(&v, widths());
    assert_eq!(rec.crc, Some(0x1234_5678));
    assert_eq!(rec.debris, Some(40));
    check_log(
        &rec,
        &spec(
            "computeCRC:1 mCRC:32 shapeName:s cloakTexName:s mass:1 mass:32 drag:1 drag:32 density:1 density:32 \
             maxEnergy:1 maxEnergy:32 cameraMaxDist:1 cameraMaxDist:32 cameraMinDist:1 cameraMinDist:32 \
             cameraDefaultFov:1 cameraDefaultFov:32 cameraMinFov:1 cameraMinFov:32 cameraMaxFov:1 cameraMaxFov:32 \
             debrisShapeName:s observeThroughObject:1 debris:1 debris:{db} emap:1 isInvincible:1 renderWhenDestroyed:1 \
             explosion:1 explosion:{db} underwaterExplosion:1 underwaterExplosion:{db} inheritEnergyFromMount:1 \
             firstPersonOnly:1 useEyePoint:1",
        ),
    );
    canonical(&rec, &ReadCtx::new(widths(), true));
}

#[test]
fn the_default_fov_is_clamped_into_the_fov_range_before_it_is_compared() {
    let pack = |fov: f32| {
        let v = shape_defaults().with("cameraDefaultFov", 0, Value::F32(fov));
        pack_shapebase_data(&v, widths()).camera_default_fov
    };
    assert_eq!(pack(200.0), Some(F32::new(120.0)));
    assert_eq!(pack(1.0), Some(F32::new(5.0)));
    // A NaN fails both compares and becomes the low end.
    assert_eq!(pack(f32::NAN), Some(F32::new(5.0)));
    assert_eq!(pack(90.0), None);
}

#[test]
fn static_shape_data_adds_a_flag_and_a_word() {
    let rec = pack_static_shape_data(&shape_defaults(), widths());
    let golden = format!("{SB_DEFAULT} noIndividualDamage:1 dynamicTypeField:32");
    check(&rec, &golden, SB_DEFAULT_BITS + 1 + 32);
}

#[test]
fn item_data_with_tges_defaults_sends_its_density_and_drag() {
    // ItemData's constructor sets density 2 and drag 0.5, which differ from ShapeBaseData's.
    let v = shape_defaults()
        .with("density", 0, Value::F32(2.0))
        .with("drag", 0, Value::F32(0.5))
        .with("gravityMod", 0, Value::F32(1.0))
        .with("maxVelocity", 0, Value::F32(-1.0))
        .with("lightTime", 0, Value::Int(1000))
        .with("lightRadius", 0, Value::F32(10.0));
    let rec = pack_item_data(&v, widths());
    let golden = SB_DEFAULT
        .replace("drag:1", "drag:1 drag:32")
        .replace("density:1", "density:1 density:32")
        + " friction:10 elasticity:10 sticky:1 gravityMod:1 maxVelocity:1 lightType:1";
    check(
        &rec,
        &golden,
        SB_DEFAULT_BITS + 64 + 10 + 10 + 1 + 1 + 1 + 1,
    );
}

#[test]
fn item_data_quantises_its_floats_and_sends_its_light() {
    let v = shape_defaults()
        .with("friction", 0, Value::F32(0.5))
        .with("elasticity", 0, Value::F32(0.25))
        .with("gravityMod", 0, Value::F32(-0.5))
        .with("maxVelocity", 0, Value::F32(30.0))
        .with("lightType", 0, Value::Int(2))
        .floats("lightColor", &[1.0, 0.5, 0.0, 1.0])
        .with("lightTime", 0, Value::Int(500))
        .with("lightRadius", 0, Value::F32(4.0))
        .with("lightOnlyStatic", 0, Value::Bool(true));
    let rec = pack_item_data(&v, widths());
    assert_eq!(rec.friction, (0.5f32 * 1023.0) as u32);
    assert_eq!(rec.friction, float_bits(0.5, 10));
    assert_eq!(rec.gravity_mod, Some(signed_float_bits(-0.5, 11)));
    let light = rec.light.clone().unwrap();
    assert_eq!((light.light_type, light.color), (2, [127, 63, 0, 127]));
    let golden = format!(
        "{SB_DEFAULT} friction:10 elasticity:10 sticky:1 gravityMod:1 gravityMod:11 maxVelocity:1 maxVelocity:32 \
         lightType:1 lightType:2 lightColor:7*4 lightTime:32 lightRadius:32 lightOnlyStatic:1"
    );
    check(
        &rec,
        &golden,
        SB_DEFAULT_BITS + 21 + 12 + 33 + 1 + 2 + 28 + 64 + 1,
    );
}

/// PlayerData's own list with no datablock set.
const PLAYER_BODY: &str = "renderFirstPerson:1 minLookAngle:32 maxLookAngle:32 maxFreelookAngle:32 maxTimeScale:32 \
     maxStepHeight:32 runForce:32 runEnergyDrain:32 minRunEnergy:32 maxForwardSpeed:32 maxBackwardSpeed:32 maxSideSpeed:32 \
     maxUnderwaterForwardSpeed:32 maxUnderwaterBackwardSpeed:32 maxUnderwaterSideSpeed:32 runSurfaceAngle:32 \
     recoverDelay:32 recoverRunForceScale:32 jumpForce:32 jumpEnergyDrain:32 minJumpEnergy:32 minJumpSpeed:32 \
     maxJumpSpeed:32 jumpSurfaceAngle:32 jumpDelay:7 horizMaxSpeed:32 horizResistSpeed:32 horizResistFactor:32 \
     upMaxSpeed:32 upResistSpeed:32 upResistFactor:32 splashVelocity:32 splashAngle:32 splashFreqMod:32 \
     splashVelEpsilon:32 bubbleEmitTime:32 medSplashSoundVel:32 hardSplashSoundVel:32 exitSplashSoundVel:32 \
     footSplashHeight:32 minImpactSpeed:32 sound:1*18 boxSize:96 footPuffEmitter:1 footPuffNumParts:32 \
     footPuffRadius:32 decalData:1 decalOffset:32 dustEmitter:1 splash:1 splashEmitterList:1*3 groundImpactMinSpeed:32 \
     groundImpactShakeFreq:96 groundImpactShakeAmp:96 groundImpactShakeDuration:32 groundImpactShakeFalloff:32";

#[test]
fn player_data_is_the_shapebase_part_and_its_movement_table() {
    let rec = pack_player_data(&shape_defaults(), widths());
    let bits = SB_DEFAULT_BITS
        + 1
        + 15 * 32
        + 32
        + 7 * 32
        + 7
        + 16 * 32
        + 18
        + 96
        + (1 + 32 + 32)
        + (1 + 32)
        + 2
        + 3
        + 32
        + 2 * 96
        + 2 * 32;
    check(&rec, &format!("{SB_DEFAULT} {PLAYER_BODY}"), bits);
}

#[test]
fn player_data_sends_each_datablock_it_points_at() {
    let mut v = with_ids(shape_defaults(), "sound", 18, 100);
    v = with_ids(v, "splashEmitterList", 3, 200);
    for (i, name) in ["footPuffEmitter", "decalData", "dustEmitter", "splash"]
        .iter()
        .enumerate()
    {
        v = v.with(name, 0, Value::Id(Some(300 + i as u32)));
    }
    let v = v
        .with("jumpDelay", 0, Value::Int(200))
        .with("runForce", 0, Value::F32(4320.0));
    let rec = pack_player_data(&v, widths());
    assert_eq!(rec.jump_delay, 200 & 0x7f);
    assert_eq!(rec.sound[17], Some(117));
    assert_eq!(rec.run_force, F32::new(4320.0));
    let body = PLAYER_BODY
        .replace("sound:1*18", &times("sound:1 sound:{db}", 18))
        .replace(
            "splashEmitterList:1*3",
            &times("splashEmitterList:1 splashEmitterList:{db}", 3),
        )
        .replace(
            "footPuffEmitter:1",
            "footPuffEmitter:1 footPuffEmitter:{db}",
        )
        .replace("decalData:1", "decalData:1 decalData:{db}")
        .replace("dustEmitter:1", "dustEmitter:1 dustEmitter:{db}")
        .replace("splash:1 ", "splash:1 splash:{db} ");
    let plain = bit_total(&pack_player_data(&shape_defaults(), widths()), &table());
    check(&rec, &format!("{SB_DEFAULT} {body}"), plain + 25 * db());
}

/// ShapeBaseImageData with nothing set: no light, no state.
const IMAGE_DEFAULT: &str = "computeCRC:1 emap:1 shapeName:s mountPoint:32 mountOffset:1 eyeOffset:1 \
     correctMuzzleVector:1 firstPerson:1 mass:32 usesEnergy:1 minEnergy:32 hasFlash:1 projectile:1 cloakable:1 \
     lightType:2 shellExitDir:96 shellExitVariance:32 shellVelocity:32 casing:1 state:1*31";

#[test]
fn image_data_with_no_states_is_its_header_and_31_flags() {
    let rec = pack_image_data(&MapView::default(), widths());
    assert!(rec.states.iter().all(Option::is_none));
    let bits = 1
        + 1
        + EMPTY
        + 32
        + 1
        + 1
        + 1
        + 1
        + 32
        + 1
        + 32
        + 1
        + 1
        + 1
        + 2
        + 96
        + 32
        + 32
        + 1
        + IMAGE_STATES;
    check(&rec, IMAGE_DEFAULT, bits);
}

/// One state with every optional part.
const STATE_FULL: &str = "state:1 state.name:s transition.loaded:5*2 transition.ammo:5*2 transition.target:5*2 \
     transition.wet:5*2 transition.trigger:5*2 transition.timeout:5 timeoutValue:1 timeoutValue:32 waitForTimeout:1 fire:1 \
     ejectShell:1 scaleAnimation:1 direction:1 energyDrain:1 energyDrain:32 loaded:3 spin:3 recoil:3 sequence:1 \
     sequence:1 sequence:15 sequenceVis:1 sequenceVis:1 sequenceVis:15 flashSequence:1 ignoreLoadedForReady:1 emitter:1 \
     emitter:{db} emitterTime:32 emitterNode:32 sound:1 sound:{db}";

/// One named state with nothing optional: a sequence of -1 and zero timeout and drain stay home.
const STATE_PLAIN: &str = "state:1 state.name:s transition.loaded:5*2 transition.ammo:5*2 transition.target:5*2 \
     transition.wet:5*2 transition.trigger:5*2 transition.timeout:5 timeoutValue:1 waitForTimeout:1 fire:1 ejectShell:1 \
     scaleAnimation:1 direction:1 energyDrain:1 loaded:3 spin:3 recoil:3 sequence:1 sequenceVis:1 flashSequence:1 \
     ignoreLoadedForReady:1 emitter:1 sound:1";

#[test]
fn image_states_send_their_transitions_in_tges_order() {
    let mut v = MapView::default()
        .with("computeCRC", 0, Value::Bool(true))
        .with("mCRC", 0, Value::Int(7))
        .with("shapeName", 0, Value::str("gun.dts"))
        .with(
            "mountOffset",
            0,
            Value::Xform(Some([1.0f32.to_bits(), 0, 0, 0, 0, 0, 0])),
        )
        .with("projectile", 0, Value::Id(Some(60)))
        .with("lightType", 0, Value::Int(3))
        .floats("lightColor", &[1.0, 1.0, 1.0, 1.0])
        .with("casing", 0, Value::Id(Some(61)));
    // State 0 "Activate": every transition distinct, so the order shows. State 2 "Ready": nothing optional.
    v = v.with("state.name", 0, Value::str("Activate"));
    for (field, pair) in [
        ("state.transition.loaded", [1, 2]),
        ("state.transition.ammo", [3, 4]),
        ("state.transition.target", [5, 6]),
        ("state.transition.wet", [7, 8]),
        ("state.transition.trigger", [9, 10]),
    ] {
        v = v
            .with(field, 0, Value::Int(pair[0]))
            .with(field, 1, Value::Int(pair[1]));
    }
    v = v
        .with("state.transition.timeout", 0, Value::Int(-1))
        .with("state.timeoutValue", 0, Value::F32(0.5))
        .with("state.energyDrain", 0, Value::F32(2.0))
        .with("state.loaded", 0, Value::Int(1))
        .with("state.spin", 0, Value::Int(4))
        .with("state.recoil", 0, Value::Int(3))
        .with("state.sequence", 0, Value::Int(-5))
        .with("state.sequenceVis", 0, Value::Int(9))
        .with("state.emitter", 0, Value::Id(Some(62)))
        .with("state.emitterTime", 0, Value::F32(0.2))
        .with("state.emitterNode", 0, Value::Int(1))
        .with("state.sound", 0, Value::Id(Some(63)));
    v = v.with("state.name", 2, Value::str("Ready"));
    for field in [
        "state.transition.loaded",
        "state.transition.ammo",
        "state.transition.target",
        "state.transition.wet",
        "state.transition.trigger",
    ] {
        v = v
            .with(field, 4, Value::Int(-1))
            .with(field, 5, Value::Int(-1));
    }
    v = v
        .with("state.transition.timeout", 2, Value::Int(-1))
        .with("state.sequence", 2, Value::Int(-1))
        .with("state.sequenceVis", 2, Value::Int(-1));
    let rec = pack_image_data(&v, widths());
    let first = rec.states[0].clone().unwrap();
    // Each transition travels as the target + 1: [0] is NotLoaded / NoAmmo / NoTarget / NotWet / TriggerUp.
    assert_eq!(
        first.transition,
        Transitions {
            loaded: [2, 3],
            ammo: [4, 5],
            target: [6, 7],
            wet: [8, 9],
            trigger: [10, 11],
            timeout: 0,
        }
    );
    assert_eq!(
        first.sequence,
        Some(Signed {
            negative: true,
            magnitude: 5
        })
    );
    assert_eq!(
        rec.states[2].as_ref().unwrap().transition,
        Transitions::default()
    );
    let golden = IMAGE_DEFAULT
        .replace("computeCRC:1", "computeCRC:1 mCRC:32")
        .replace("mountOffset:1", "mountOffset:1 mountOffset:193")
        .replace("projectile:1", "projectile:1 projectile:{db}")
        .replace(
            "lightType:2",
            "lightType:2 lightRadius:32 lightTime:32 lightColor:7*4",
        )
        .replace("casing:1", "casing:1 casing:{db}")
        .replace(
            "state:1*31",
            &format!(
                "{STATE_FULL} state:1 {STATE_PLAIN} {}",
                times("state:1", 28)
            ),
        );
    check_log(&rec, &spec(&golden));
    canonical(&rec, &ReadCtx::new(widths(), true));
}

#[test]
fn an_image_light_type_of_none_drops_the_light_block() {
    let v = MapView::default().with("lightType", 0, Value::Int(4)).with(
        "lightRadius",
        0,
        Value::F32(3.0),
    );
    // 4 wraps to 0 in the 2-bit field: the block follows the value on the wire, so it is not sent.
    let rec = pack_image_data(&v, widths());
    assert_eq!(rec.light_type, 0);
    check_log(&rec, IMAGE_DEFAULT);
}
