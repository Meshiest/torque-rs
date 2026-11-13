//! Hostile input: datablocks, states and moves with NaN, infinities, zeros, huge and negative values (all script-chosen),
//! both classes, many ticks. 0 panics; the caps hold (sub-steps per tick, rays per tick, the collision queue).
//! `VEHICLE_FUZZ` sets the run count (default 300).

use crate::common;

use common::Gen;
use torque_collide::ObjectId;
use torque_collide::math::{Box3, F3, IDENTITY};
use torque_vehicle::data::{
    FlyingVehicleData, SpringData, TireData, VehicleData, WheelRecord, WheeledVehicleData,
};
use torque_vehicle::flying::Flying;
use torque_vehicle::host::{ScanHit, VehicleHost};
use torque_vehicle::state::MAX_COLLISION_QUEUE;
use torque_vehicle::tick::{MAX_SUBSTEPS, Vehicle, process_tick};
use torque_vehicle::wheeled::{self, Wheeled};
use torque_vehicle::{Move, VehicleState};

const HOSTILE: [f32; 12] = [
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    0.0,
    -0.0,
    1e30,
    -1e30,
    3.4e38,
    -1.0,
    1e-30,
    f32::MIN_POSITIVE,
    65_536.0,
];

fn h(g: &mut Gen) -> f32 {
    if g.below(3) == 0 {
        HOSTILE[g.below(HOSTILE.len() as u64) as usize]
    } else {
        g.f()
    }
}

fn h3(g: &mut Gen) -> F3 {
    [h(g), h(g), h(g)]
}

fn base(g: &mut Gen) -> VehicleData {
    let mut b = VehicleData::default();
    b.shape.mass = h(g);
    b.shape.drag = h(g);
    b.shape.max_energy = h(g);
    b.shape.inherit_energy = g.below(4) == 0;
    b.body_restitution = h(g);
    b.body_friction = h(g);
    b.min_impact_speed = h(g);
    b.max_steering_angle = h(g);
    b.min_drag = h(g);
    b.integration = [-5, -1, 0, 1, 4, 1000, i32::MIN, i32::MAX][g.below(8) as usize];
    b.collision_tol = h(g);
    b.contact_tol = h(g);
    b.mass_center = h3(g);
    b.mass_box = h3(g);
    b.jet_force = h(g);
    b.jet_energy_drain = h(g);
    b.min_jet_energy = h(g);
    b
}

fn state(g: &mut Gen, db: &VehicleData) -> VehicleState {
    let mut m = IDENTITY;
    for x in m.iter_mut().take(12) {
        if g.below(4) == 0 {
            *x = h(g);
        }
    }
    let lo = h3(g);
    let hi = h3(g);
    let mut s = VehicleState::new(ObjectId(1), db, Box3::new(lo, hi), &m, h3(g));
    s.rigid.lin_vel = h3(g);
    s.rigid.ang_mom = h3(g);
    s.energy = h(g);
    s.water_coverage = h(g);
    s.steering = [h(g), h(g)];
    s
}

fn hostile_move(g: &mut Gen) -> Option<Move> {
    if g.below(5) == 0 {
        return None;
    }
    let mut m = Move {
        x: h(g),
        y: h(g),
        z: h(g),
        yaw: h(g),
        pitch: h(g),
        roll: h(g),
        free_look: g.below(2) == 0,
        triggers: [false; torque_vehicle::MAX_TRIGGERS],
    };
    for t in m.triggers.iter_mut() {
        *t = g.below(2) == 0;
    }
    Some(m)
}

/// A host whose rays hit at a hostile fraction, whose scan finds many items, and whose world has a box floor and a hostile body
/// hull (the body collision runs: GJK, features, the resolve loops), counting what the caps bound.
struct Hostile {
    ground: common::host::Floor,
    seed: u64,
    rays_this_tick: usize,
    max_rays: usize,
}

impl VehicleHost for Hostile {
    fn world(&mut self) -> &mut dyn torque_collide::World {
        &mut self.ground
    }
    fn sim_time(&self) -> u32 {
        self.ground.ground.time
    }
    fn cast_ray(
        &mut self,
        start: F3,
        _end: F3,
        _mask: u32,
    ) -> Option<torque_vehicle::host::RayHit> {
        self.rays_this_tick += 1;
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let k = (self.seed >> 33) as usize;
        if k.is_multiple_of(3) {
            return None;
        }
        Some(torque_vehicle::host::RayHit {
            t: HOSTILE[k % HOSTILE.len()],
            point: start,
            normal: [HOSTILE[(k / 7) % HOSTILE.len()], 0.0, 1.0],
            material: 0,
            object: None,
        })
    }
    fn scan(&mut self, _s: &mut VehicleState, _q: &Box3, out: &mut Vec<ScanHit>) {
        for i in 0..100 {
            out.push(ScanHit {
                object: ObjectId(1000 + i),
                velocity: [f32::NAN, 1e30, 0.0],
            });
        }
    }
}

fn runs() -> usize {
    std::env::var("VEHICLE_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

#[test]
fn hostile_vehicles_do_not_panic_and_hold_the_caps() {
    let mut g = Gen::new(77);
    let n = runs();
    for run in 0..n {
        let body = h3(&mut g);
        let mut host = Hostile {
            ground: common::host::Floor::new(
                ObjectId(1),
                body,
                [0.0, 0.0, -1.0],
                [10.0, 10.0, 1.0],
            ),
            seed: run as u64,
            rays_this_tick: 0,
            max_rays: 0,
        };
        if run % 2 == 0 {
            let mut db = WheeledVehicleData {
                base: base(&mut g),
                ..Default::default()
            };
            let wheels = g.below(9) as usize;
            for i in 0..wheels {
                db.wheels.push(WheelRecord {
                    opposite: [-1, (i ^ 1) as i32, 7][g.below(3) as usize],
                    pos: h3(&mut g),
                    hub_node: i as i32,
                    spring_seq: -1,
                    spring_len: h(&mut g),
                });
            }
            db.max_wheel_speed = h(&mut g);
            db.engine_torque = h(&mut g);
            db.brake_torque = h(&mut g);
            let s = state(&mut g, &db.base);
            let mut v = Vehicle::new(s, Wheeled::default());
            wheeled::on_new_data_block(&mut v.s, &mut v.k, &db);
            for i in 0..wheels {
                let tire = TireData {
                    radius: h(&mut g),
                    lateral_force: h(&mut g),
                    longitudinal_relaxation: h(&mut g),
                    static_friction: h(&mut g),
                    ..Default::default()
                };
                let spring = SpringData {
                    length: h(&mut g),
                    force: h(&mut g),
                    damping: h(&mut g),
                    anti_sway_force: h(&mut g),
                };
                wheeled::set_wheel_tire(&mut v, &db, i, (ObjectId(5), tire));
                wheeled::set_wheel_spring(&mut v, &db, i, (ObjectId(6), spring));
                wheeled::set_wheel_steering(&mut v, &db, i, h(&mut g));
            }
            for tick in 0..10u32 {
                host.ground.ground.time = tick * 32;
                host.rays_this_tick = 0;
                let mv = hostile_move(&mut g);
                let _ = process_tick(&mut v, &db, mv.as_ref(), &mut host);
                assert!(host.rays_this_tick <= MAX_SUBSTEPS as usize * wheels.max(1));
                host.max_rays = host.max_rays.max(host.rays_this_tick);
                assert!(v.s.collisions.len() <= MAX_COLLISION_QUEUE);
                let _ = wheeled::packet_words(&v, &db, &mut host).bits();
            }
        } else {
            let mut db = FlyingVehicleData {
                base: base(&mut g),
                ..Default::default()
            };
            db.maneuvering_force = h(&mut g);
            db.hover_height = h(&mut g);
            db.max_auto_speed = h(&mut g);
            db.auto_input_damping = h(&mut g);
            db.vert_thrust_multiple = h(&mut g);
            db.preload();
            let s = state(&mut g, &db.base);
            let mut v = Vehicle::new(s, Flying::default());
            for tick in 0..10u32 {
                host.ground.ground.time = tick * 32;
                host.rays_this_tick = 0;
                let mv = hostile_move(&mut g);
                let _ = process_tick(&mut v, &db, mv.as_ref(), &mut host);
                assert!(host.rays_this_tick <= MAX_SUBSTEPS as usize);
                assert!(v.s.collisions.len() <= MAX_COLLISION_QUEUE);
                let _ = torque_vehicle::flying::packet_words(&v, &db, &mut host).bits();
            }
        }
    }
}
