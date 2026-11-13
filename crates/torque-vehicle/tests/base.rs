//! The base Vehicle: the sub-step cap, the collision queue across sub-steps, `resolveCollision`'s 50 passes, the wake rules.

use crate::common;

use common::host::Ground;
use torque_collide::math::{Box3, F3, IDENTITY};
use torque_collide::{Collision, CollisionList, ObjectId};
use torque_vehicle::data::{FlyingVehicleData, VehicleData};
use torque_vehicle::flying::Flying;
use torque_vehicle::host::{ScanHit, VehicleHost};
use torque_vehicle::state::{self, RESOLVE_PASSES, VehicleState};
use torque_vehicle::tick::{MAX_SUBSTEPS, Vehicle, process_tick, substeps};

fn state(db: &VehicleData) -> VehicleState {
    let mut m = IDENTITY;
    m[11] = 20.0;
    VehicleState::new(
        ObjectId(1),
        db,
        Box3::new([-1.0; 3], [1.0; 3]),
        &m,
        [1.0; 3],
    )
}

/// `MAX_SUBSTEPS` caps only the count: the step size `f32(0.032 / f32(N))` is kept, and ordinary counts are unchanged.
#[test]
fn the_substep_cap_leaves_ordinary_counts_alone() {
    for n in [1, 2, 3, 4, 8] {
        let (count, dt) = substeps(n).unwrap();
        assert_eq!(count, n as u32);
        assert_eq!(dt, 0.032 / n as f32);
    }
    assert_eq!(substeps(0), None);
    // A negative N counts as unsigned (about four billion sub-steps): capped, dt negative.
    assert_eq!(substeps(-1), Some((MAX_SUBSTEPS, -0.032)));
    assert_eq!(substeps(i32::MIN).unwrap().0, MAX_SUBSTEPS);
    assert_eq!(substeps(1000).unwrap().0, MAX_SUBSTEPS);
}

/// A host whose trigger/item scan finds the same item at every sub-step, recording the `onCollision`s.
#[derive(Default)]
struct Scanner {
    ground: Ground,
    collisions: Vec<(ObjectId, F3)>,
}

impl VehicleHost for Scanner {
    fn world(&mut self) -> &mut dyn torque_collide::World {
        &mut self.ground
    }
    fn sim_time(&self) -> u32 {
        self.ground.time
    }
    fn scan(&mut self, _s: &mut VehicleState, _q: &Box3, out: &mut Vec<ScanHit>) {
        out.push(ScanHit {
            object: ObjectId(9),
            velocity: [0.0, 0.0, 1.0],
        });
    }
    fn on_collision(&mut self, _s: &mut VehicleState, other: ObjectId, vec: F3) -> bool {
        self.collisions.push((other, vec));
        true
    }
}

/// An item found by the scan of one sub-step is queued stamped `now + 250` and the next sub-step's `notifyCollision`
/// dispatches it, since sim time does not move inside a tick (such entries are dispatched: they are,
/// once, unless the scan ran in the last sub-step). The entry then blocks a re-queue until its stamp passes: 250 ms.
#[test]
fn scan_queued_items_are_dispatched_by_the_next_substep() {
    let mut db = FlyingVehicleData::default();
    db.base.integration = 4;
    let mut v = Vehicle::new(state(&db.base), Flying::default());
    let mut host = Scanner::default();
    host.ground.no_ground = true;
    process_tick(&mut v, &db, None, &mut host).unwrap();
    assert_eq!(host.collisions.len(), 1);
    assert_eq!(host.collisions[0].0, ObjectId(9));
    // The vector is the vehicle's velocity at that scan minus the item's.
    assert_eq!(v.s.collisions.len(), 1);
    // The next ticks within 250 ms: nothing new.
    for k in 1..8 {
        host.ground.time = 32 * k;
        process_tick(&mut v, &db, None, &mut host).unwrap();
    }
    assert_eq!(host.collisions.len(), 1);
    // Past the stamp (250 ms) the entry is refreshed and dispatched again by the following sub-step.
    host.ground.time = 32 * 8 + 32;
    process_tick(&mut v, &db, None, &mut host).unwrap();
    assert_eq!(host.collisions.len(), 2);
    // With one sub-step a tick, the scan's entry is never dispatched (next tick's stamp differs).
    let mut db1 = db;
    db1.base.integration = 1;
    let mut v1 = Vehicle::new(state(&db1.base), Flying::default());
    let mut host1 = Scanner::default();
    host1.ground.no_ground = true;
    for k in 0..12 {
        host1.ground.time = 32 * k;
        process_tick(&mut v1, &db1, None, &mut host1).unwrap();
    }
    assert!(host1.collisions.is_empty());
}

/// `resolveCollision` passes over the list while a pass hits; the 50th pass returns "no collision". Two contacts
/// with opposite normals at the centre of mass of a perfectly elastic body hit on every pass.
#[test]
fn resolve_collision_stops_after_fifty_passes() {
    let db = VehicleData {
        body_restitution: 1.0,
        ..Default::default()
    };
    let mut s = state(&db);
    s.rigid.lin_vel = [0.0, 0.0, -1.0];
    s.rigid.lin_mom = [0.0, 0.0, -1.0];
    let c = s.rigid.world_com;
    let mut list = CollisionList::new();
    for n in [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0]] {
        list.push(Collision {
            object: None,
            point: c,
            normal: n,
            distance: 0.0,
            ..Default::default()
        });
    }
    let mut host = Ground::default();
    assert!(!state::resolve_collision(&mut s, &list, &db, &mut host));
    assert_eq!(RESOLVE_PASSES, 50);
    // One contact: hit in the first pass, nothing in the second: true.
    let mut s = state(&db);
    s.rigid.lin_vel = [0.0, 0.0, -1.0];
    s.rigid.lin_mom = [0.0, 0.0, -1.0];
    list.count = 1;
    assert!(state::resolve_collision(&mut s, &list, &db, &mut host));
    assert_eq!(s.rigid.lin_vel[2], 1.0);
}

/// `setVelocity` leaves a sleeping body asleep; `applyImpulse` wakes it; `setTransform` wakes it and empties the
/// second list.
#[test]
fn the_wake_rules() {
    let db = VehicleData::default();
    let mut s = state(&db);
    s.rigid.set_at_rest();
    s.set_velocity([1.0, 0.0, 0.0]);
    assert!(s.rigid.at_rest);
    assert_eq!(s.rigid.lin_vel, [1.0, 0.0, 0.0]);
    s.apply_impulse(s.rigid.world_com, [0.0, 2.0, 0.0]);
    assert!(!s.rigid.at_rest);
    s.rigid.set_at_rest();
    s.contacts_count = 3;
    let mut host = Ground::default();
    s.set_transform(&IDENTITY, &mut host);
    assert!(!s.rigid.at_rest);
    assert_eq!(s.contacts_count, 0);
    assert_eq!(s.mask_bits & 0x40, 0x40);
    // setVelocity stores the velocity as given, and the momentum to match.
    s.set_velocity([300.0, 0.0, 0.0]);
    assert_eq!(s.rigid.lin_vel, [300.0, 0.0, 0.0]);
    assert_eq!(s.rigid.lin_mom, [300.0 * s.rigid.mass, 0.0, 0.0]);
}

/// `Vehicle::updateMove`: the steering accumulates the yaw and pitch, clamped to the max angle (a NaN sum takes the
/// negative limit), and keeps old - new; the null move zeroes it; the jet drains energy each tick while trigger 3 is held.
#[test]
fn update_move_steers_and_jets() {
    let mut db = VehicleData::default();
    db.shape.max_energy = 100.0;
    db.max_steering_angle = 0.5;
    let mut s = state(&db);
    s.energy = 10.0;
    let mut host = Ground::default();
    let mut mv = torque_vehicle::Move {
        yaw: 0.4,
        pitch: f32::NAN,
        ..Default::default()
    };
    mv.triggers[3] = true;
    state::update_move(&mut s, &db, Some(&mv), &mut host);
    assert_eq!(s.steering, [0.4, -0.5]);
    assert!(s.jetting);
    assert_eq!(s.energy, 10.0 - 0.8);
    state::update_move(&mut s, &db, Some(&mv), &mut host);
    assert_eq!(s.steering[0], 0.5);
    assert_eq!(s.steering_delta[0], 0.4 - 0.5);
    state::update_move(&mut s, &db, None, &mut host);
    assert_eq!(s.steering, [0.0; 2]);
    assert!(!s.jetting);
}

/// The 9-bit steering ints of the ghost packer: the extremes give 0 and 511, the middle 255 (truncated), x first.
#[test]
fn steering_ints_are_truncated_nine_bit_fractions() {
    use torque_vehicle::words::steering_ints;
    assert_eq!(steering_ints([-0.785, 0.785], 0.785), [0, 511]);
    assert_eq!(steering_ints([0.0, 0.0], 0.785), [255, 255]);
    // A NaN quotient converts to the integer 0, so its 9 bits are 0.
    assert_eq!(steering_ints([f32::NAN, 0.0], 0.785), [0, 255]);
}
