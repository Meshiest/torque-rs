//! FlyingVehicle: a hand-worked thrust, the hover lift, the steering damping, determinism.

mod common;

use common::host::Ground;
use torque_collide::ObjectId;
use torque_collide::math::{Box3, IDENTITY};
use torque_vehicle::data::FlyingVehicleData;
use torque_vehicle::flying::{self, Flying};
use torque_vehicle::tick::{Kind, Vehicle};
use torque_vehicle::{Move, VehicleState};

/// A flying vehicle with typical numbers.
fn craft() -> FlyingVehicleData {
    let mut db = FlyingVehicleData::default();
    db.base.shape.mass = 200.0;
    db.base.shape.drag = 1.7;
    db.base.min_drag = 50.0;
    db.base.max_drag = 40.0;
    db.base.integration = 4;
    db.base.max_steering_angle = 0.785;
    db.maneuvering_force = 6400.0;
    db.horizontal_surface_force = 20.0;
    db.vertical_surface_force = 20.0;
    db.auto_input_damping = 0.55;
    db.steering_force = 1500.0;
    db.steering_roll_force = 200.0;
    db.roll_force = 10.0;
    db.auto_angular_force = 1400.0;
    db.rotational_drag = 20.0;
    db.auto_linear_force = 100.0;
    db.max_auto_speed = 10.0;
    db.preload();
    db
}

fn at(z: f32, db: &FlyingVehicleData) -> Vehicle<Flying> {
    let mut m = IDENTITY;
    m[11] = z;
    let s = VehicleState::new(
        ObjectId(3),
        &db.base,
        Box3::new([-1.0, -1.5, -0.2], [1.0, 1.5, 0.2]),
        &m,
        [1.0; 3],
    );
    Vehicle::new(s, Flying::default())
}

/// At rest, level, high above anything (the hover ray misses), full forward thrust: worked by hand. The forward
/// axis is y, so the thrust is `f32(M5 * f32(move.y * maneuveringForce))` = 6400 along y and nothing else acts in y (no velocity,
/// no applied force). In z: the miss gives a height `h = (1 * 12 - 2) / 10` = 1, so the lift is `L0 - L0 * h * 0.1` with
/// `L0 = 200 * 20`, on top of the gravity `200 * -20`; with no velocity the surface terms are zero.
#[test]
fn thrust_and_hover_by_hand() {
    let db = craft();
    assert_eq!(db.max_speed, 128.0);
    let mut v = at(50.0, &db);
    v.k.move_y = 1.0;
    let mut host = Ground {
        no_ground: true,
        ..Default::default()
    };
    flying::update_forces(&mut v, &db, &mut host);
    assert_eq!(host.rays, 1);
    let l0 = 200.0f32 * 20.0;
    let fz = -200.0 * 20.0 + (l0 - l0 * 1.0 * 0.1);
    assert_eq!(v.s.rigid.force, [0.0, 6400.0, fz]);
    assert_eq!(v.s.rigid.torque, [0.0, 0.0, 0.0]);
    assert!(!v.s.rigid.at_rest);
}

/// Close to the ground the ray hits: a height `h = (t * 12 - 2) / 10` below zero gives the lift `L0 + jetForce * -h`.
#[test]
fn hover_lift_near_the_ground() {
    let db = craft();
    let mut v = at(1.0, &db);
    let mut host = Ground::default();
    flying::update_forces(&mut v, &db, &mut host);
    // The ray runs from z 1 to 1 - 12 = -11: t = 1/12.
    let t = 1.0f32 / 12.0;
    let h = (12.0 * t - 2.0) / 10.0;
    assert!(h < 0.0);
    let l0 = 200.0f32 * 20.0;
    let fz = -200.0 * 20.0 + (l0 + db.base.jet_force * -h);
    assert_eq!(v.s.rigid.force[2], fz);
}

/// `updateMove`: the steering damped by autoInputDamping below maxAutoSpeed, the null move zeroing it, the thrust
/// state from y.
#[test]
fn update_move_damps_the_steering_and_sets_the_thrust() {
    let db = craft();
    let mut v = at(5.0, &db);
    let mut host = Ground::default();
    let mv = Move {
        yaw: 0.5,
        pitch: -0.2,
        y: -1.0,
        ..Default::default()
    };
    Flying::update_move(&mut v, &db, Some(&mv), &mut host);
    assert_eq!(v.s.steering[0], 0.5 * 0.55);
    assert_eq!(v.s.steering[1], -0.2 * 0.55);
    assert_eq!(v.k.thrust, 1);
    Flying::update_move(&mut v, &db, None, &mut host);
    assert_eq!(v.s.steering, [0.0; 2]);
    assert_eq!(v.k.thrust, 2);
}

/// Pitch, yaw and thrust from rest, twice: equal words tick for tick.
#[test]
fn a_flight_is_deterministic() {
    let db = craft();
    let start = at(3.0, &db);
    let moves: Vec<Option<Move>> = (0..80)
        .map(|i| {
            Some(Move {
                y: if i < 40 { 1.0 } else { 0.3 },
                yaw: if i % 7 == 0 { 0.1 } else { 0.0 },
                pitch: if i % 11 == 0 { -0.05 } else { 0.0 },
                ..Default::default()
            })
        })
        .collect();
    let run = || {
        let mut host = Ground::default();
        torque_vehicle::run::run_flying(&start, &db, &moves, &mut host)
    };
    let a = run();
    let b = run();
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.words.bits(), y.words.bits());
    }
    assert!(a[79].words.position[1] > 1.0);
}

/// A flying vehicle dropped onto a box floor through the body collision (`findClosestState`, `getCollisionInfo`, the resolve
/// loops): it lands, stops falling through, and the run is deterministic.
#[test]
fn a_craft_lands_on_a_box_floor() {
    let mut db = craft();
    db.hover_height = 0.5;
    db.create_hover_height = 0.5;
    let start = at(1.5, &db);
    let run = || {
        let mut host = common::host::Floor::new(
            ObjectId(3),
            [1.0, 1.5, 0.2],
            [0.0, 0.0, -1.0],
            [10.0, 10.0, 1.0],
        );
        host.ground.no_ground = true;
        let moves = vec![None; 120];
        let out = torque_vehicle::run::run_flying(&start, &db, &moves, &mut host);
        (out, host.ground.callbacks.len())
    };
    let (a, _) = run();
    let (b, _) = run();
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.words.bits(), y.words.bits());
    }
    // The floor's top is z = 0 and the body reaches 0.2 below its centre: it rests near z 0.2, not below the floor.
    let z = a[119].words.position[2];
    assert!(z > 0.0 && z < 1.0, "{z}");
    let lowest = a
        .iter()
        .map(|t| t.words.position[2])
        .fold(f32::INFINITY, f32::min);
    assert!(lowest > 0.1, "fell through: {lowest}");
    // It came down from 1.5 (the lift is only 0.9 of the weight with no ground under the ray) and the floor stopped it.
    assert!(z < 1.0 && a[60].words.lin_mom[2].abs() < 1000.0);
}
