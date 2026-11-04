//! The vehicle datablocks and the wheel parts.

use super::shapes::{SB_DEFAULT, SB_DEFAULT_BITS, shape_defaults};
use super::*;
use crate::datablocks::vehicles::{
    pack_flying_vehicle_data, pack_hover_vehicle_data, pack_spring, pack_tire, pack_vehicle_data,
    pack_wheeled_vehicle_data,
};

/// VehicleData's own list with no datablock set.
const VEHICLE_BODY: &str = "body.restitution:32 body.friction:32 body.sound:1*2 minImpactSpeed:32 softImpactSpeed:32 \
     hardImpactSpeed:32 minRollSpeed:32 maxSteeringAngle:32 maxDrag:32 minDrag:32 integration:32 collisionTol:32 \
     contactTol:32 massCenter:96 massBox:96 jetForce:32 jetEnergyDrain:32 minJetEnergy:32 cameraRoll:1 cameraLag:32 \
     cameraDecay:32 cameraOffset:32 triggerDustHeight:32 dustHeight:32 numDmgEmitterAreas:32 exitSplashSoundVel:32 \
     softSplashSoundVel:32 medSplashSoundVel:32 hardSplashSoundVel:32 waterSound:1*5 dustEmitter:1 damageEmitterList:1*3 \
     splashEmitterList:1*2 damageEmitterOffset:96*2 damageLevelTolerance:64 splashFreqMod:32 splashVelEpsilon:32 \
     collDamageThresholdVel:32 collDamageMultiplier:32";

/// Seven forces, the integration word, two tolerances, two points, three jet numbers, the camera flag, ten more floats,
/// the twelve datablock flags, the damage offsets and levels and four floats.
const VEHICLE_BODY_BITS: usize = 2 * 32
    + 2
    + 7 * 32
    + 32
    + 2 * 32
    + 2 * 96
    + 3 * 32
    + 1
    + 10 * 32
    + 5
    + 1
    + 3
    + 2
    + 2 * 96
    + 64
    + 4 * 32;

fn vehicle_golden() -> String {
    format!("{SB_DEFAULT} {VEHICLE_BODY}")
}

#[test]
fn vehicle_data_is_the_shapebase_part_and_its_body() {
    let rec = pack_vehicle_data(&shape_defaults(), widths());
    assert_eq!(VEHICLE_BODY_BITS, 1390);
    check(&rec, &vehicle_golden(), SB_DEFAULT_BITS + VEHICLE_BODY_BITS);
}

#[test]
fn vehicle_data_reads_its_points_and_its_datablocks() {
    let mut v = shape_defaults()
        .floats("massCenter", &[0.0, 0.0, 1.5])
        .floats("damageEmitterOffset", &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
        .with("integration", 0, Value::Int(4));
    v = with_ids(v, "body.sound", 2, 10);
    v = with_ids(v, "waterSound", 5, 20);
    v = with_ids(v, "damageEmitterList", 3, 30);
    v = with_ids(v, "splashEmitterList", 2, 40);
    v = v.with("dustEmitter", 0, Value::Id(Some(50)));
    let rec = pack_vehicle_data(&v, widths());
    assert_eq!(rec.mass_center[2], F32::new(1.5));
    assert_eq!(
        rec.damage_emitter_offset[1],
        [F32::new(4.0), F32::new(5.0), F32::new(6.0)]
    );
    assert_eq!(
        (rec.integration, rec.water_sound[4], rec.dust_emitter),
        (4, Some(24), Some(50))
    );
    let golden = vehicle_golden()
        .replace("body.sound:1*2", &times("body.sound:1 body.sound:{db}", 2))
        .replace("waterSound:1*5", &times("waterSound:1 waterSound:{db}", 5))
        .replace("dustEmitter:1", "dustEmitter:1 dustEmitter:{db}")
        .replace(
            "damageEmitterList:1*3",
            &times("damageEmitterList:1 damageEmitterList:{db}", 3),
        )
        .replace(
            "splashEmitterList:1*2",
            &times("splashEmitterList:1 splashEmitterList:{db}", 2),
        );
    check(
        &rec,
        &golden,
        SB_DEFAULT_BITS + VEHICLE_BODY_BITS + 13 * db(),
    );
}

#[test]
fn wheeled_vehicle_data_adds_its_tire_emitter_sounds_and_engine() {
    let v = with_ids(shape_defaults(), "sound", 4, 70).with("engineTorque", 0, Value::F32(4000.0));
    let rec = pack_wheeled_vehicle_data(&v, widths());
    assert_eq!(rec.engine_torque, F32::new(4000.0));
    let golden = format!(
        "{} tireEmitter:1 {} maxWheelSpeed:32 engineTorque:32 engineBrake:32 brakeTorque:32",
        vehicle_golden(),
        times("sound:1 sound:{db}", 4)
    );
    check(
        &rec,
        &golden,
        SB_DEFAULT_BITS + VEHICLE_BODY_BITS + 1 + 4 * (1 + db()) + 4 * 32,
    );
}

#[test]
fn flying_vehicle_data_adds_sounds_jets_and_fifteen_forces() {
    let rec = pack_flying_vehicle_data(&shape_defaults(), widths());
    let golden = format!(
        "{} sound:1*2 jetEmitter:1*4 maneuveringForce:32 horizontalSurfaceForce:32 verticalSurfaceForce:32 \
         autoInputDamping:32 steeringForce:32 steeringRollForce:32 rollForce:32 autoAngularForce:32 rotationalDrag:32 \
         autoLinearForce:32 maxAutoSpeed:32 hoverHeight:32 createHoverHeight:32 minTrailSpeed:32 vertThrustMultiple:32",
        vehicle_golden()
    );
    check(
        &rec,
        &golden,
        SB_DEFAULT_BITS + VEHICLE_BODY_BITS + 6 + 15 * 32,
    );
}

#[test]
fn hover_vehicle_data_adds_its_forces_then_its_trail_and_datablocks() {
    let v = shape_defaults()
        .with("pitchForce", 0, Value::F32(2.0))
        .with("dustTrailEmitter", 0, Value::Id(Some(90)));
    let rec = pack_hover_vehicle_data(&v, widths());
    assert_eq!(rec.forces[16], F32::new(2.0));
    let golden = format!(
        "{} dragForce:32 vertFactor:32 floatingThrustFactor:32 mainThrustForce:32 reverseThrustForce:32 \
         strafeThrustForce:32 turboFactor:32 stabLenMin:32 stabLenMax:32 stabSpringConstant:32 stabDampingConstant:32 \
         gyroDrag:32 normalForce:32 restorativeForce:32 steeringForce:32 rollForce:32 pitchForce:32 dustTrailOffset:96 \
         triggerTrailHeight:32 dustTrailFreqMod:32 sound:1*3 jetEmitter:1*3 dustTrailEmitter:1 dustTrailEmitter:{{db}} \
         floatingGravMag:32 brakingForce:32 brakingActivationSpeed:32",
        vehicle_golden()
    );
    check(
        &rec,
        &golden,
        SB_DEFAULT_BITS + VEHICLE_BODY_BITS + 17 * 32 + 96 + 64 + 7 + db() + 96,
    );
}

#[test]
fn the_wheel_parts_are_a_shape_name_and_floats() {
    let tire = pack_tire(&MapView::default().with("radius", 0, Value::F32(0.6)));
    assert_eq!(tire.values[4], F32::new(0.6));
    check(
        &tire,
        "shapeName:s mass:32 staticFriction:32 kineticFriction:32 restitution:32 radius:32 lateralForce:32 \
         lateralDamping:32 lateralRelaxation:32 longitudinalForce:32 longitudinalDamping:32 longitudinalRelaxation:32",
        EMPTY + 11 * 32,
    );
    let spring = pack_spring(&MapView::default().with("antiSway", 0, Value::F32(3.0)));
    assert_eq!(spring.anti_sway, F32::new(3.0));
    check(&spring, "length:32 force:32 damping:32 antiSway:32", 128);
}
