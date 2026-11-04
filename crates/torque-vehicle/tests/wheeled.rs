//! WheeledVehicle: a hand-worked `updateForces` tick on flat ground, the datablock's `preload`, `onNewDataBlock`, the wheel
//! methods, determinism.

mod common;

use common::host::Ground;
use torque_collide::ObjectId;
use torque_collide::math::{Box3, F3, IDENTITY};
use torque_vehicle::data::{PreloadShape, SpringData, TireData, WheelRecord, WheeledVehicleData};
use torque_vehicle::tick::{Vehicle, process_tick};
use torque_vehicle::wheeled::{self, Wheeled};
use torque_vehicle::{Move, VehicleState};

/// A vehicle of `db` at height `z`, its wheels given `tire` and `spring`.
fn build(db: &WheeledVehicleData, z: f32, tire: TireData, spring: SpringData) -> Vehicle<Wheeled> {
    let mut m = IDENTITY;
    m[11] = z;
    let s = VehicleState::new(
        ObjectId(7),
        &db.base,
        Box3::new([-1.0, -2.0, -0.5], [1.0, 2.0, 1.0]),
        &m,
        [1.0; 3],
    );
    let mut v = Vehicle::new(s, Wheeled::default());
    wheeled::on_new_data_block(&mut v.s, &mut v.k, db);
    for i in 0..db.wheels.len() {
        assert!(wheeled::set_wheel_tire(
            &mut v,
            db,
            i,
            (ObjectId(100), tire)
        ));
        assert!(wheeled::set_wheel_spring(
            &mut v,
            db,
            i,
            (ObjectId(101), spring)
        ));
    }
    v
}

fn record(pos: F3, opposite: i32) -> WheelRecord {
    WheelRecord {
        opposite,
        pos,
        hub_node: 0,
        spring_seq: -1,
        spring_len: 0.0,
    }
}

/// One wheel straight under the centre of mass, the body at rest at height 1, flat ground at z = 0. Worked by hand:
/// the hub ray from z 0.5 to `0.5 - f32(L * f32(1 + R/L))` = -1 hits at t = 0.5 / 1.5; the extension is
/// `f32((t - kf) / (1 - kf))` with `kf = f32(k / (k + 1))`, `k = f32(R / L)`; at rest the damper is `-0.0` (kept: only an ordered
/// negative is zeroed), so the wheel pushes `f32(force * (1 - ext))` straight up through the centre of mass: no torque; the tire
/// is still, so no friction; the brake keeps the wheel still. Gravity takes `f32(mass * 20)`, and the momentum is `f32(Fz * dt)`.
#[test]
fn update_forces_on_flat_ground_by_hand() {
    let db = WheeledVehicleData {
        wheels: vec![record([0.0, 0.0, -0.5], -1)],
        base: torque_vehicle::VehicleData {
            shape: torque_vehicle::data::ShapeBaseData {
                mass: 100.0,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let tire = TireData {
        radius: 0.5,
        ..Default::default()
    };
    let spring = SpringData {
        length: 1.0,
        force: 1000.0,
        damping: 0.0,
        anti_sway_force: 0.0,
    };
    let mut v = build(&db, 1.0, tire, spring);
    v.s.drag = 0.0;
    let mut host = Ground::default();
    let dt = 0.008f32;
    wheeled::update_forces(&mut v, &db, dt, &mut host);
    assert_eq!(host.rays, 1);
    // The ray: from z 0.5 down the spring's length times `1 + radius / length`; t from the plane.
    let ts = tire.radius / spring.length;
    let end_z = 0.5 + -spring.length * (1.0 + ts);
    let t = 0.5 / (0.5 - end_z);
    let ts = ts / (1.0 + ts);
    let ext = (t - ts) / (1.0 - ts);
    let w = &v.k.wheels[0];
    assert!(w.contact);
    assert_eq!(w.extension, ext);
    assert_eq!(w.avel, 0.0);
    assert!(!w.slipping);
    // The spring alone holds the body against the gravity of 100 kg.
    let spring_f = spring.force * (1.0 - ext);
    let fz = spring_f + -20.0 * 100.0;
    let lm_z = fz * dt;
    assert_eq!(v.s.rigid.lin_mom, [0.0, 0.0, lm_z]);
    assert_eq!(v.s.rigid.ang_mom, [0.0; 3]);
    assert_eq!(v.s.rigid.lin_vel[2], lm_z * (1.0 / 100.0));
    // The accumulators are zeroed at the end of updateForces.
    assert_eq!(v.s.rigid.force, [0.0; 3]);
}

/// The datablock's wheel records from a shape (`preload`): hubs 0, 1 and 3 present (2 skipped, no record used), springs
/// on hubs 0 and 3 with a travel `f32(z(0) - z(1))`; hub 1 (no spring) reads the pose the last animation left; the first earlier
/// record within 0.5 in y pairs up and takes its mirrored position.
#[test]
fn preload_builds_the_wheel_records() {
    struct Shape {
        seq_pos: Option<(i32, f32)>,
    }
    impl PreloadShape for Shape {
        fn find_node(&self, name: &str) -> Option<i32> {
            match name {
                "hub0" => Some(10),
                "hub1" => Some(11),
                "hub3" => Some(13),
                _ => None,
            }
        }
        fn find_sequence(&self, name: &str) -> Option<i32> {
            match name {
                "spring0" => Some(20),
                "spring3" => Some(23),
                "steering" => Some(5),
                _ => None,
            }
        }
        fn animate(&mut self, seq: i32, pos: f32) {
            self.seq_pos = Some((seq, pos));
        }
        fn node_translation(&self, node: i32) -> F3 {
            // Hubs sit at x = +-1 by node parity, y = 1.5 front / -1.5 rear; the springs lower z by 0.25 at position 1.
            let drop = match self.seq_pos {
                Some((_, 1.0)) => 0.25,
                _ => 0.0,
            };
            let x = if node % 2 == 0 { 1.0 } else { -1.0 };
            let y = if node < 12 { 1.5 } else { -1.3 };
            [x, y, -0.5 - drop]
        }
    }
    let mut db = WheeledVehicleData::default();
    let mut shape = Shape { seq_pos: None };
    db.preload(&mut shape);
    assert_eq!(db.wheels.len(), 3);
    assert_eq!(db.steering_seq, 5);
    assert_eq!(db.brakelight_seq, -1);
    let w0 = db.wheels[0];
    assert_eq!((w0.hub_node, w0.spring_seq, w0.spring_len), (10, 20, 0.25));
    assert_eq!(w0.pos, [1.0, 1.5, -0.5]);
    // Hub 1: no spring, the pose spring0 left at position 1 (z -0.75); paired with record 0 (same y): x mirrored, y and z taken.
    let w1 = db.wheels[1];
    assert_eq!((w1.spring_seq, w1.opposite), (-1, 0));
    assert_eq!(w1.pos, [-1.0, 1.5, -0.5]);
    assert_eq!(db.wheels[0].opposite, 1);
    // Hub 3: y -1.3 pairs with nothing earlier (1.5 is 2.8 away).
    let w2 = db.wheels[2];
    assert_eq!((w2.hub_node, w2.spring_seq, w2.opposite), (13, 23, -1));
}

/// `onNewDataBlock`: the box inertia from `massBox` when all three are positive, else the object box; the front
/// wheels (pos.y > 0) steer; tires and springs cleared.
#[test]
fn on_new_data_block_sets_the_box_inertia_and_the_steering() {
    let mut db = WheeledVehicleData {
        wheels: vec![record([1.0, 1.5, -0.5], 1), record([-1.0, -1.5, -0.5], 0)],
        ..Default::default()
    };
    db.base.shape.mass = 120.0;
    let v = build(&db, 2.0, TireData::default(), SpringData::default());
    let mut r = torque_vehicle::Rigid {
        mass: 120.0,
        ..Default::default()
    };
    r.set_box_inertia([2.0, 4.0, 1.5]);
    assert_eq!(v.s.rigid.inertia, r.inertia);
    assert_eq!(v.s.rigid.inv_inertia, r.inv_inertia);
    let mut db2 = db.clone();
    db2.base.mass_box = [1.0, 2.0, 3.0];
    let v2 = build(&db2, 2.0, TireData::default(), SpringData::default());
    r.set_box_inertia([1.0, 2.0, 3.0]);
    assert_eq!(v2.s.rigid.inertia, r.inertia);
    assert_eq!(v.k.wheels[0].steering, 1.0);
    assert_eq!(v.k.wheels[1].steering, 0.0);
    assert!(v.k.wheels[0].powered);
}

/// `setWheelSteering` clamps to [-1, 1] and refuses a wheel past the count.
#[test]
fn wheel_methods_clamp_and_check_the_index() {
    let db = WheeledVehicleData {
        wheels: vec![record([1.0, 1.0, 0.0], -1)],
        ..Default::default()
    };
    let mut v = build(&db, 2.0, TireData::default(), SpringData::default());
    v.s.mask_bits = 0;
    assert!(wheeled::set_wheel_steering(&mut v, &db, 0, 3.0));
    assert_eq!(v.k.wheels[0].steering, 1.0);
    assert!(wheeled::set_wheel_steering(&mut v, &db, 0, -7.0));
    assert_eq!(v.k.wheels[0].steering, -1.0);
    assert_eq!(v.s.mask_bits, 0x80);
    assert!(!wheeled::set_wheel_steering(&mut v, &db, 1, 0.5));
    assert!(!wheeled::set_wheel_powered(&mut v, &db, 8, false));
    assert_eq!(wheeled::wheel_count(&db), 1);
}

/// A four-wheel car driven forward then turning, twice from the same start: the words are equal bit for bit, tick for tick.
#[test]
fn a_drive_is_deterministic() {
    let mut db = WheeledVehicleData {
        wheels: vec![
            record([1.0, 1.5, -0.4], 1),
            record([-1.0, 1.5, -0.4], 0),
            record([1.0, -1.5, -0.4], 3),
            record([-1.0, -1.5, -0.4], 2),
        ],
        ..Default::default()
    };
    db.base.shape.mass = 200.0;
    db.base.integration = 4;
    db.engine_torque = 4000.0;
    db.max_wheel_speed = 30.0;
    let tire = TireData {
        radius: 0.4,
        longitudinal_force: 6000.0,
        lateral_force: 18000.0,
        ..Default::default()
    };
    let spring = SpringData {
        length: 0.5,
        force: 3000.0,
        damping: 600.0,
        anti_sway_force: 1000.0,
    };
    let start = build(&db, 1.2, tire, spring);
    let moves: Vec<Option<Move>> = (0..60)
        .map(|i| {
            if i % 10 == 9 {
                None
            } else {
                Some(Move {
                    y: 1.0,
                    yaw: if i > 30 { 0.05 } else { 0.0 },
                    ..Default::default()
                })
            }
        })
        .collect();
    let run = || {
        let mut host = Ground::default();
        torque_vehicle::run::run_wheeled(&start, &db, &moves, &mut host)
    };
    let a = run();
    let b = run();
    assert_eq!(a.len(), 60);
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.words.bits(), y.words.bits());
        assert!(!x.exhausted);
    }
    // It moved and the wheels spun.
    let last = &a[59].words;
    assert!(last.position[1].abs() > 0.01 || last.lin_mom[1].abs() > 0.01);
    let mut v = start.clone();
    let mut host = Ground::default();
    for m in &moves {
        process_tick(&mut v, &db, m.as_ref(), &mut host).unwrap();
    }
    assert!(v.k.wheels.iter().take(4).any(|w| w.avel != 0.0));
    // Four sub-steps a tick, a ray per wheel each.
    assert_eq!(host.rays, 60 * 4 * 4);
}

/// A subclass that pushes up with 2000 and turns the steering to 0.25 on every move.
#[derive(Debug)]
struct Lifter;

impl wheeled::WheeledClass for Lifter {
    fn after_move(&self, s: &mut torque_vehicle::VehicleState, _mv: &Move) {
        s.steering[0] = 0.25;
    }
    #[allow(clippy::too_many_arguments)]
    fn extra_forces(
        &self,
        _s: &torque_vehicle::VehicleState,
        _k: &Wheeled,
        _db: &WheeledVehicleData,
        _axes: &wheeled::Axes,
        _vel: [f32; 3],
        _throttle: f32,
        force: &mut [f32; 3],
        _torque: &mut [f32; 3],
    ) {
        force[2] += 2000.0;
    }
}

/// The step asks the host's subclass at its two points: its force joins the sum before the momentum step (here it cancels the
/// gravity of a 100 kg body in the air), and its steering follows the move. Without one, Torque's vehicle falls.
#[test]
fn a_subclass_adds_its_forces_and_steering() {
    let db = WheeledVehicleData {
        base: torque_vehicle::VehicleData {
            shape: torque_vehicle::data::ShapeBaseData {
                mass: 100.0,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let start = build(&db, 10.0, TireData::default(), SpringData::default());
    let mut v = start.clone();
    v.s.drag = 0.0;
    let mut host = Ground {
        no_ground: true,
        class: Some(std::rc::Rc::new(Lifter)),
        ..Ground::default()
    };
    wheeled::update_forces(&mut v, &db, 0.008, &mut host);
    assert_eq!(v.s.rigid.lin_mom, [0.0; 3]);
    wheeled::update_move(&mut v, &db, Some(&Move::default()), &mut host);
    assert_eq!(v.s.steering[0], 0.25);

    let mut plain = start.clone();
    plain.s.drag = 0.0;
    let mut host = Ground {
        no_ground: true,
        ..Ground::default()
    };
    wheeled::update_forces(&mut plain, &db, 0.008, &mut host);
    assert!(plain.s.rigid.lin_mom[2] < 0.0);
}
