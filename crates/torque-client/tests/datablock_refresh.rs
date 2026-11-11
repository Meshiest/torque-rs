//! A datablock changed under a predictor is refreshed in place: the predicted Player's, the driven vehicle's and its wheels'
//! tuning equal a predictor freshly made from the new record, while the position, velocity and state the moves made stay the
//! same to the bit. A record sent again unchanged moves no generation and refreshes nothing. A flying vehicle's warps read the
//! record's mass at each update, so they take an edit at the next one.

use std::rc::Rc;

use collide::ObjectId;
use ghost_world::tge::{TgeClientSet, TgeDatablock};
use ghost_world::world::{Ghost, VehicleUpdate};
use ghosts::parts::GameBase;
use ghosts::player::Player as PlayerRecord;
use ghosts::vehicle::{
    FlyingVehicle, RigidState, VehiclePart, VehicleState, WheelSetup, WheeledVehicle,
};
use ghosts::{F32, TgeClass, TgeRecord, TgeShapeBase, Value};
use torque_client::conn::tge::TgeWorld;
use torque_client::predict::drive::{DrivenVehicle, ShapeCache};
use torque_client::predict::game::TorquePrediction;
use torque_client::predict::look::FlyingWarps;
use torque_client::predict::{FileReader, Predicted};

const PLAYER_DB: u32 = 10;
const VEHICLE_DB: u32 = 20;
const TIRE_DB: u32 = 21;
const SPRING_DB: u32 = 22;
const FLYING_DB: u32 = 30;
const PLAYER_GHOST: u32 = 1;
const VEHICLE_GHOST: u32 = 2;
const FLYING_GHOST: u32 = 3;
const SHAPE: &[u8] = b"kit/buggy.dts";
const EDITS: usize = 100;

/// xorshift64*, seeded.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// A float in `lo..hi`, never the same twice in a row to the bit (an edit always changes the field).
    fn float(&mut self, lo: f32, hi: f32) -> f32 {
        let f = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * f
    }
}

fn game_base(db: u32) -> TgeShapeBase {
    TgeShapeBase {
        game: GameBase {
            scale: None,
            datablock: Some(db),
        },
        body: None,
    }
}

fn ghost(class: TgeClass, record: TgeRecord) -> Ghost<TgeClientSet> {
    Ghost {
        class: Some(class),
        serial: 1,
        always: false,
        record: Some(record),
        part: (),
        position: None,
        updates: 0,
    }
}

/// A datablock of `class` with `fields` (name, element, value).
fn datablock(class: &str, fields: &[(&str, u32, Value)]) -> TgeDatablock {
    TgeDatablock {
        class: class.into(),
        fields: fields
            .iter()
            .map(|(n, i, v)| ((*n).to_string(), *i, v.clone()))
            .collect(),
    }
}

/// Sets element `index` of field `name` of `db` to `value`.
fn set(db: &mut TgeDatablock, name: &str, index: u32, value: f32) {
    match db
        .fields
        .iter_mut()
        .find(|(n, i, _)| n == name && *i == index)
    {
        Some(f) => f.2 = Value::F32(value),
        None => db.fields.push((name.to_string(), index, Value::F32(value))),
    }
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// The fields of a PlayerData an edit may change, with their ranges.
const PLAYER_FIELDS: &[(&str, u32, f32, f32)] = &[
    ("mass", 0, 50.0, 200.0),
    ("drag", 0, 0.1, 1.5),
    ("maxForwardSpeed", 0, 3.0, 20.0),
    ("maxBackwardSpeed", 0, 2.0, 10.0),
    ("maxSideSpeed", 0, 2.0, 10.0),
    ("maxUnderwaterForwardSpeed", 0, 1.0, 8.0),
    ("maxUnderwaterBackwardSpeed", 0, 1.0, 5.0),
    ("maxUnderwaterSideSpeed", 0, 1.0, 5.0),
    ("boundingBox", 0, 0.5, 3.0),
    ("boundingBox", 1, 0.5, 3.0),
    ("boundingBox", 2, 1.0, 4.0),
    ("rechargeRate", 0, 0.0, 1.0),
    ("maxEnergy", 0, 10.0, 200.0),
    ("runForce", 0, 500.0, 6000.0),
    ("jumpForce", 0, 100.0, 2000.0),
    ("maxStepHeight", 0, 0.5, 2.0),
];

fn player_datablock() -> TgeDatablock {
    datablock(
        "PlayerData",
        &[
            ("mass", 0, Value::F32(90.0)),
            ("drag", 0, Value::F32(0.3)),
            ("maxForwardSpeed", 0, Value::F32(10.0)),
            ("boundingBox", 0, Value::F32(1.2)),
            ("boundingBox", 1, Value::F32(1.2)),
            ("boundingBox", 2, Value::F32(2.3)),
            ("rechargeRate", 0, Value::F32(0.256)),
            ("maxEnergy", 0, Value::F32(60.0)),
        ],
    )
}

fn player_world() -> (TgeWorld, TgeDatablock) {
    let mut w = TgeWorld::default();
    let db = player_datablock();
    w.records.apply_datablock(PLAYER_DB, db.clone());
    w.records.ghosts.insert(
        PLAYER_GHOST,
        ghost(
            TgeClass::Player,
            TgeRecord::Player(PlayerRecord {
                shape: game_base(PLAYER_DB),
                ..PlayerRecord::default()
            }),
        ),
    );
    (w, db)
}

/// What the moves made of a predicted Player: everything a refresh must not touch.
fn player_motion(p: &Predicted) -> Vec<u32> {
    let s = &p.player.s;
    let mut v = bits(&s.obj_to_world);
    v.extend(bits(&s.velocity));
    v.extend(bits(&s.rot));
    v.extend(bits(&s.head));
    v.extend(bits(&[s.energy]));
    v.extend([
        s.action_state,
        s.jump_delay.cast_unsigned(),
        s.contact_timer,
    ]);
    v.push(s.pose as u32);
    v
}

/// The tuning a Player takes from its datablock.
fn player_tuning(p: &Predicted) -> Vec<u32> {
    let s = &p.player.s;
    let mut v = bits(&[s.mass, s.drag, s.recharge_rate]);
    v.extend(bits(&s.speeds));
    v.extend(bits(&s.obj_box.min));
    v.extend(bits(&s.obj_box.max));
    v.extend(bits(&s.scaled_box.min));
    v.extend(bits(&s.scaled_box.max));
    v.extend(bits(&s.pose_reach.min));
    v.extend(bits(&s.pose_reach.max));
    v
}

#[test]
fn a_players_tuning_is_refreshed_in_place_over_100_edits() {
    let game = TorquePrediction;
    let (mut world, mut db) = player_world();
    let pos = [3.0, 4.0, 5.0];
    let mut pr =
        Predicted::new(&world, PLAYER_GHOST, PLAYER_DB, pos, 0.7, &game).expect("a Player");
    {
        let s = &mut *pr.player.s;
        s.velocity = [1.5, -2.0, 0.3];
        s.energy = 12.5;
        s.head = [0.2, 0.0, -0.1];
        s.jump_delay = 3;
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut refreshed = 0;
    for edit in 0..EDITS {
        for _ in 0..3 {
            let (name, index, lo, hi) = PLAYER_FIELDS[rng.below(PLAYER_FIELDS.len())];
            set(&mut db, name, index, rng.float(lo, hi));
        }
        let before = world.datablock_generation(PLAYER_DB);
        let changed = world.records.apply_datablock(PLAYER_DB, db.clone());
        assert_eq!(changed, world.datablock_generation(PLAYER_DB) != before);
        let motion = player_motion(&pr);
        let did = pr.refresh_tuning(&world, &game);
        assert_eq!(
            did, changed,
            "edit {edit}: refreshed when the record changed"
        );
        refreshed += usize::from(did);
        assert_eq!(
            player_motion(&pr),
            motion,
            "edit {edit}: the moves' state stays"
        );
        let fresh =
            Predicted::new(&world, PLAYER_GHOST, PLAYER_DB, pos, 0.7, &game).expect("a Player");
        assert_eq!(pr.data, fresh.data, "edit {edit}: the datablock copy");
        assert_eq!(
            player_tuning(&pr),
            player_tuning(&fresh),
            "edit {edit}: the tuning"
        );
        assert_eq!(pr.generation, world.datablock_generation(PLAYER_DB));
    }
    assert!(
        refreshed >= EDITS - 1,
        "{refreshed} refreshes of {EDITS} edits"
    );
}

#[test]
fn a_record_sent_again_unchanged_moves_no_generation_and_refreshes_nothing() {
    let game = TorquePrediction;
    let (mut world, db) = player_world();
    let mut pr =
        Predicted::new(&world, PLAYER_GHOST, PLAYER_DB, [0.0; 3], 0.0, &game).expect("a Player");
    let (generation, epoch) = (world.datablock_generation(PLAYER_DB), world.datablock_epoch);
    assert_eq!(generation, 1);
    for _ in 0..100 {
        assert!(!world.records.apply_datablock(PLAYER_DB, db.clone()));
        assert!(!pr.refresh_tuning(&world, &game));
    }
    assert_eq!(world.datablock_generation(PLAYER_DB), generation);
    assert_eq!(world.datablock_epoch, epoch);
    // A record of another class under the id: the Player keeps its tuning and takes the generation, so it is not read again.
    world.records.apply_datablock(
        PLAYER_DB,
        datablock("ItemData", &[("mass", 0, Value::F32(1.0))]),
    );
    assert!(!pr.refresh_tuning(&world, &game));
    assert_eq!(pr.generation, world.datablock_generation(PLAYER_DB));
    assert_eq!(pr.player.s.mass, 90.0);
    assert!(!pr.refresh_tuning(&world, &game));
}

// ---- the wheeled vehicle

/// A buggy body with four hubs: hub0 and hub1 in front (steered), hub2 and hub3 behind.
fn reader() -> FileReader {
    let b = [-1.0, -2.0, -0.5, 1.0, 2.0, 0.5];
    let mut model =
        shapes::synth::hull_shape(b, &[("Col-1", b)], &["hub0", "hub1", "hub2", "hub3"], &[]);
    let hubs = [
        [-1.0, 1.5, -0.3],
        [1.0, 1.5, -0.3],
        [-1.0, -1.5, -0.3],
        [1.0, -1.5, -0.3],
    ];
    for (k, hub) in hubs.iter().enumerate() {
        model.shape.default_translations[k + 1] = *hub;
    }
    let bytes = shapes::write::write_dts(&model);
    Rc::new(move |path: &[u8]| (path == SHAPE).then(|| bytes.clone()))
}

const VEHICLE_FIELDS: &[(&str, u32, f32, f32)] = &[
    ("mass", 0, 500.0, 3000.0),
    ("drag", 0, 0.1, 1.5),
    ("bodyFriction", 0, 0.1, 1.0),
    ("bodyRestitution", 0, 0.0, 0.9),
    ("massCenter", 0, -0.3, 0.3),
    ("massCenter", 1, -0.5, 0.5),
    ("massCenter", 2, -0.3, 0.3),
    ("maxWheelSpeed", 0, 10.0, 60.0),
    ("engineTorque", 0, 1000.0, 9000.0),
    ("engineBrake", 0, 100.0, 1000.0),
    ("brakeTorque", 0, 1000.0, 9000.0),
    ("rechargeRate", 0, 0.0, 1.0),
    ("maxSteeringAngle", 0, 0.3, 0.9),
    ("minDrag", 0, 0.0, 1.0),
    ("maxDrag", 0, 1.0, 3.0),
];

const TIRE_FIELDS: &[(&str, u32, f32, f32)] = &[
    ("mass", 0, 10.0, 80.0),
    ("kineticFriction", 0, 0.2, 1.5),
    ("staticFriction", 0, 0.2, 1.5),
    ("restitution", 0, 0.0, 0.9),
    ("lateralForce", 0, 5000.0, 30000.0),
    ("lateralDamping", 0, 1000.0, 9000.0),
    ("lateralRelaxation", 0, 0.5, 3.0),
    ("longitudinalForce", 0, 5000.0, 30000.0),
    ("radius", 0, 0.3, 0.8),
];

const SPRING_FIELDS: &[(&str, u32, f32, f32)] = &[
    ("length", 0, 0.2, 0.8),
    ("force", 0, 1000.0, 9000.0),
    ("damping", 0, 500.0, 5000.0),
    ("antiSwayForce", 0, 0.0, 5000.0),
];

fn vehicle_world() -> (TgeWorld, [TgeDatablock; 3]) {
    let mut w = TgeWorld::default();
    let vehicle = datablock(
        "WheeledVehicleData",
        &[
            ("shapeFile", 0, Value::str(SHAPE)),
            ("mass", 0, Value::F32(1200.0)),
            ("drag", 0, Value::F32(0.6)),
            ("bodyFriction", 0, Value::F32(0.6)),
            ("bodyRestitution", 0, Value::F32(0.4)),
            ("maxWheelSpeed", 0, Value::F32(30.0)),
            ("engineTorque", 0, Value::F32(4000.0)),
            ("engineBrake", 0, Value::F32(600.0)),
            ("brakeTorque", 0, Value::F32(8000.0)),
            ("rechargeRate", 0, Value::F32(0.1)),
            ("maxSteeringAngle", 0, Value::F32(0.785)),
        ],
    );
    let tire = datablock(
        "WheeledVehicleTire",
        &[
            ("mass", 0, Value::F32(30.0)),
            ("kineticFriction", 0, Value::F32(0.7)),
            ("staticFriction", 0, Value::F32(0.8)),
            ("radius", 0, Value::F32(0.5)),
        ],
    );
    let spring = datablock(
        "WheeledVehicleSpring",
        &[
            ("length", 0, Value::F32(0.5)),
            ("force", 0, Value::F32(3000.0)),
            ("damping", 0, Value::F32(1500.0)),
        ],
    );
    w.records.apply_datablock(VEHICLE_DB, vehicle.clone());
    w.records.apply_datablock(TIRE_DB, tire.clone());
    w.records.apply_datablock(SPRING_DB, spring.clone());
    let setup = Some(WheelSetup {
        tire: TIRE_DB,
        spring: SPRING_DB,
        powered: true,
        steering: F32::new(0.0),
    });
    w.records.ghosts.insert(
        VEHICLE_GHOST,
        ghost(
            TgeClass::WheeledVehicle,
            TgeRecord::WheeledVehicle(WheeledVehicle {
                vehicle: VehiclePart {
                    shape: game_base(VEHICLE_DB),
                    ..VehiclePart::default()
                },
                setup: Some(vec![setup; 4]),
                ..WheeledVehicle::default()
            }),
        ),
    );
    (w, [vehicle, tire, spring])
}

fn new_vehicle(world: &TgeWorld, shapes: &mut ShapeCache, reader: &FileReader) -> DrivenVehicle {
    let mut work = 0;
    DrivenVehicle::new(
        world,
        VEHICLE_GHOST,
        shapes,
        Some(reader),
        &mut work,
        u64::MAX,
        &TorquePrediction,
    )
    .expect("a wheeled vehicle")
}

/// What the ticks made of a vehicle: its pose, momenta, velocities and its wheels' motion.
fn vehicle_motion(v: &DrivenVehicle) -> Vec<u32> {
    let s = &v.v.s;
    let r = &s.rigid;
    let mut out = bits(&s.obj_to_world);
    for f in [&r.lin_pos, &r.lin_vel, &r.lin_mom, &r.ang_vel, &r.ang_mom] {
        out.extend(bits(f));
    }
    out.extend(bits(&r.ang_pos));
    out.extend(bits(&s.steering));
    for w in &v.v.k.wheels {
        out.extend(bits(&[w.avel, w.apos, w.extension, w.dy, w.dx]));
    }
    out
}

/// The tuning a vehicle takes from its datablocks.
fn vehicle_tuning(v: &DrivenVehicle) -> Vec<u32> {
    let s = &v.v.s;
    let r = &s.rigid;
    let mut out = bits(&[
        s.mass,
        s.drag,
        s.recharge_rate,
        r.mass,
        r.one_over_mass,
        r.friction,
        r.restitution,
    ]);
    out.extend(bits(&r.com));
    out.extend(bits(&r.inertia));
    out.extend(bits(&r.inv_inertia));
    out.extend(bits(&s.obj_box.min));
    out.extend(bits(&s.obj_box.max));
    for w in &v.v.k.wheels {
        out.extend(bits(&[w.steering]));
        out.push(u32::from(w.tire.is_some()));
        out.push(u32::from(w.spring.is_some()));
    }
    out
}

#[test]
fn a_driven_vehicles_tuning_and_wheels_are_refreshed_in_place_over_100_edits() {
    let game = TorquePrediction;
    let (mut world, mut dbs) = vehicle_world();
    let reader = reader();
    let mut shapes = ShapeCache::default();
    let mut v = new_vehicle(&world, &mut shapes, &reader);
    assert_eq!(v.data.wheels.len(), 4, "the hubs make four wheel records");
    assert!(v.v.k.wheels[0].tire.is_some() && v.v.k.wheels[0].spring.is_some());
    {
        let s = &mut *v.v.s;
        s.rigid.lin_pos = [12.0, -7.0, 3.5];
        s.rigid.lin_mom = [1200.0 * 4.0, 0.0, 1200.0 * -0.5];
        s.rigid.lin_vel = [4.0, 0.0, -0.5];
        s.rigid.ang_mom = [10.0, 5.0, -3.0];
        s.rigid.ang_vel = [0.01, 0.005, -0.003];
        s.rigid.ang_pos = [0.0, 0.0, 0.3826834, 0.9238795];
        s.obj_to_world[3] = 12.0;
        s.obj_to_world[7] = -7.0;
        s.obj_to_world[11] = 3.5;
        s.steering = [0.2, 0.0];
        for (k, w) in s.rigid.lin_vel.iter().enumerate() {
            v.v.k.wheels[k].avel = w * 3.0;
            v.v.k.wheels[k].apos = 0.25 * k as f32;
            v.v.k.wheels[k].extension = 0.6;
        }
    }
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let (mut refreshed, mut wheel_edits) = (0, 0);
    for edit in 0..EDITS {
        let (which, fields): (usize, &[(&str, u32, f32, f32)]) = match rng.below(3) {
            0 => (0, VEHICLE_FIELDS),
            1 => (1, TIRE_FIELDS),
            _ => (2, SPRING_FIELDS),
        };
        let (name, index, lo, hi) = fields[rng.below(fields.len())];
        set(&mut dbs[which], name, index, rng.float(lo, hi));
        let id = [VEHICLE_DB, TIRE_DB, SPRING_DB][which];
        let changed = world.records.apply_datablock(id, dbs[which].clone());
        assert!(changed, "edit {edit}: a new value");
        wheel_edits += usize::from(which > 0);
        let motion = vehicle_motion(&v);
        let mut work = 0;
        let did = v.refresh_tuning(
            &world,
            &mut shapes,
            Some(&reader),
            &mut work,
            u64::MAX,
            &game,
        );
        assert_eq!(
            did,
            which == 0,
            "edit {edit}: the vehicle's own record refreshes it"
        );
        refreshed += usize::from(did);
        v.refresh_wheels(&world);
        assert_eq!(
            vehicle_motion(&v),
            motion,
            "edit {edit}: the ticks' state stays"
        );
        let fresh = new_vehicle(&world, &mut shapes, &reader);
        assert_eq!(*v.data, *fresh.data, "edit {edit}: the datablock copy");
        assert_eq!(
            vehicle_tuning(&v),
            vehicle_tuning(&fresh),
            "edit {edit}: the tuning"
        );
        for (k, (w, f)) in v.v.k.wheels.iter().zip(&fresh.v.k.wheels).enumerate() {
            assert_eq!(w.tire, f.tire, "edit {edit}: wheel {k} tire");
            assert_eq!(w.spring, f.spring, "edit {edit}: wheel {k} spring");
        }
        assert_eq!(v.generation, world.datablock_generation(VEHICLE_DB));
        assert_eq!(
            v.wheel_gens[0],
            [
                world.datablock_generation(TIRE_DB),
                world.datablock_generation(SPRING_DB)
            ]
        );
    }
    assert!(
        refreshed > 0 && wheel_edits > 0,
        "{refreshed} vehicle and {wheel_edits} wheel edits"
    );
    assert_eq!(v.v.k.wheels[0].tire.map(|t| t.0), Some(ObjectId(TIRE_DB)));
}

#[test]
fn a_vehicles_record_sent_again_unchanged_refreshes_nothing() {
    let game = TorquePrediction;
    let (mut world, dbs) = vehicle_world();
    let reader = reader();
    let mut shapes = ShapeCache::default();
    let mut v = new_vehicle(&world, &mut shapes, &reader);
    let epoch = world.datablock_epoch;
    for _ in 0..100 {
        for (id, db) in [VEHICLE_DB, TIRE_DB, SPRING_DB].iter().zip(&dbs) {
            assert!(!world.records.apply_datablock(*id, db.clone()));
        }
        let mut work = 0;
        assert!(!v.refresh_tuning(
            &world,
            &mut shapes,
            Some(&reader),
            &mut work,
            u64::MAX,
            &game
        ));
        let before = v.v.k.wheels.map(|w| (w.tire, w.spring));
        v.refresh_wheels(&world);
        assert_eq!(v.v.k.wheels.map(|w| (w.tire, w.spring)), before);
    }
    assert_eq!(world.datablock_epoch, epoch);
}

// ---- the flying vehicle

fn flying_update(seq: u64, mass: f32, speed: [f32; 3]) -> VehicleUpdate<TgeClientSet> {
    let momentum = speed.map(|v| F32::new(v * mass));
    VehicleUpdate {
        seq,
        index: FLYING_GHOST,
        record: TgeRecord::FlyingVehicle(FlyingVehicle {
            vehicle: VehiclePart {
                shape: game_base(FLYING_DB),
                state: Some(VehicleState {
                    rigid: Some(RigidState {
                        rotation: [F32::new(0.0), F32::new(0.0), F32::new(0.0), F32::new(1.0)],
                        linear_momentum: momentum,
                        ..RigidState::default()
                    }),
                    ..VehicleState::default()
                }),
                ..VehiclePart::default()
            },
            tail: None,
        }),
        position: Some([seq as f32, 0.0, 10.0]),
    }
}

#[test]
fn a_flying_vehicles_warps_read_the_edited_mass_at_the_next_update() {
    let mut world = TgeWorld::default();
    let mut db = datablock("FlyingVehicleData", &[("mass", 0, Value::F32(800.0))]);
    world.records.apply_datablock(FLYING_DB, db.clone());
    world.records.ghosts.insert(
        FLYING_GHOST,
        ghost(
            TgeClass::FlyingVehicle,
            TgeRecord::FlyingVehicle(FlyingVehicle {
                vehicle: VehiclePart {
                    shape: game_base(FLYING_DB),
                    ..VehiclePart::default()
                },
                tail: None,
            }),
        ),
    );
    let mut warps = FlyingWarps::default();
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let speed = [3.0, -1.0, 0.5];
    for edit in 1..=EDITS as u64 {
        let mass = rng.float(100.0, 5000.0);
        set(&mut db, "mass", 0, mass);
        assert!(world.records.apply_datablock(FLYING_DB, db.clone()));
        // The server sends the momentum of the same speed under the new mass.
        world
            .vehicle_updates
            .push_back(flying_update(edit, mass, speed));
        warps.update(&world, 32 * edit as u32);
        let w = warps.warps.get(&FLYING_GHOST).expect("a warp");
        for (got, want) in w.velocity.iter().zip(&speed) {
            assert!(
                (got - want).abs() < 1e-3,
                "edit {edit}: velocity {:?} under mass {mass}",
                w.velocity
            );
        }
    }
    assert_eq!(world.datablock_generation(FLYING_DB), EDITS as u64 + 1);
}
