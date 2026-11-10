//! `FlyingVehicle`: `updateMove`, `updateForces` and the base `Vehicle::updatePos`
//! it is stepped with (Flying has no `processTick` of its own: it runs `Vehicle::processTick`, which is
//! [`crate::tick::process_tick`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/flyingVehicle.cpp` and
//! `T3D/vehicles/vehicle.cpp` (each Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_collide::Exhausted;
use torque_collide::math::{F3, quat_matrix};

use crate::Move;
use crate::math::{GRAVITY, add, dot, len_sq, scale, sub};

/// The thrust states of [`Flying::thrust`].
const THRUST_FORWARD: u32 = 0;
const THRUST_BACKWARD: u32 = 1;
use crate::data::FlyingVehicleData;
use crate::host::VehicleHost;
use crate::state;
use crate::tick::{Kind, Vehicle};

/// What a FlyingVehicle adds to the state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Flying {
    /// `useCreateHeight` (the console method sets it and mask 2).
    pub use_create_height: bool,
    /// The move's x and y (raw copies).
    pub move_x: f32,
    pub move_y: f32,
    /// Thrust: 0 forward, 1 backward (a NaN y too), 2 none.
    pub thrust: u32,
}

impl Kind for Flying {
    type Data = FlyingVehicleData;

    fn base(db: &FlyingVehicleData) -> &crate::data::VehicleData {
        &db.base
    }

    /// `FlyingVehicle::updateMove(mv)`: the base, the null move zeroing the steering, the steering damped by
    /// `autoInputDamping` below `maxAutoSpeed` (strict; a NaN speed does not damp), the move's x and y kept, the thrust state.
    fn update_move(
        v: &mut Vehicle<Flying>,
        db: &FlyingVehicleData,
        mv: Option<&Move>,
        host: &mut dyn VehicleHost,
    ) {
        state::update_move(&mut v.s, &db.base, mv, host);
        let s = &mut v.s;
        if mv.is_none() {
            s.steering = [0.0; 2];
        }
        let lv = s.rigid.lin_vel;
        let speed = len_sq(lv).sqrt();
        if speed < db.max_auto_speed {
            s.steering[0] *= db.auto_input_damping;
            s.steering[1] *= db.auto_input_damping;
        }
        let m = mv.copied().unwrap_or_default();
        v.k.move_x = m.x;
        v.k.move_y = m.y;
        v.k.thrust = if m.y == 0.0 {
            2
        } else if m.y > 0.0 {
            0
        } else {
            1
        };
    }

    fn step(
        v: &mut Vehicle<Flying>,
        db: &FlyingVehicleData,
        dt: f32,
        host: &mut dyn VehicleHost,
    ) -> Result<(), Exhausted> {
        update_pos(v, db, dt, host)
    }
}

/// `Vehicle::updatePos(dt)`, one sub-step: the forces, the solver's part ([`crate::solver::BodySolver::substep`]: the body
/// collision and the contacts with the rest test unless at rest, the integration unless (still) at rest), `notifyCollision`,
/// then the scan, the impact and the liquid callbacks.
pub fn update_pos(
    v: &mut Vehicle<Flying>,
    db: &FlyingVehicleData,
    dt: f32,
    host: &mut dyn VehicleHost,
) -> Result<(), Exhausted> {
    let base = &db.base;
    let v0 = v.s.rigid.lin_vel;
    v.s.rigid.force = [0.0; 3];
    v.s.rigid.torque = [0.0; 3];
    update_forces(v, db, host);
    let Vehicle {
        s,
        body,
        scan,
        solver,
        ..
    } = v;
    let collided = crate::solver::substep(solver, body, s, base, dt, host, None)?;
    let now = host.sim_time();
    state::notify_collision(s, now, host);
    state::end_of_step(s, base, collided, v0, host, scan);
    Ok(())
}

/// `FlyingVehicle::updateForces` (through the [`library`](crate::library) table).
pub fn update_forces(v: &mut Vehicle<Flying>, db: &FlyingVehicleData, host: &mut dyn VehicleHost) {
    (crate::library::get().flying_update_forces)(v, db, host)
}

/// `FlyingVehicle::updateForces`: gravity, the drags, below `maxAutoSpeed` the auto-stop terms, the hover lift from
/// `getHeight`, the surface forces, the boost when jetting, the manoeuvring thrust, the steering and roll torques, the
/// applied force, buoyancy and the container drag. Writes the force and the torque; wakes the body first.
pub fn plain_update_forces(
    v: &mut Vehicle<Flying>,
    db: &FlyingVehicleData,
    host: &mut dyn VehicleHost,
) {
    let height = hover_height(v, db, host);
    let s = &mut v.s;
    s.rigid.at_rest = false;
    let m = quat_matrix(s.rigid.ang_pos);
    let xv = [m[0], m[4], m[8]];
    let yv = [m[1], m[5], m[9]];
    let zv = [m[2], m[6], m[10]];
    let lv = s.rigid.lin_vel;
    let speed = len_sq(lv).sqrt();
    let mass = s.rigid.mass;
    let g = s.gravity_mod;
    let mut force = [0.0, 0.0, GRAVITY * mass * g];
    let mut torque = [0.0; 3];
    force = sub(force, scale(lv, db.base.min_drag));
    torque = sub(torque, scale(s.rigid.ang_mom, db.rotational_drag));
    if speed < db.max_auto_speed {
        let auto_scale = 1.0 - speed / db.max_auto_speed;
        let gf = db.auto_angular_force * auto_scale;
        torque = sub(torque, scale(xv, gf * yv[2]));
        let sf = db.auto_linear_force * auto_scale;
        force = sub(force, scale(yv, sf * dot(yv, lv)));
        force = sub(force, scale(xv, sf * dot(xv, lv)));
    }
    let mut vf = -GRAVITY * mass * g;
    if height <= 1.0 {
        if height > 0.0 {
            vf -= vf * height * 0.1;
        } else {
            vf += db.base.jet_force * -height;
        }
    }
    force = add(force, scale(zv, vf));
    force = sub(
        force,
        scale(xv, speed * dot(xv, lv) * db.horizontal_surface_force),
    );
    force = sub(
        force,
        scale(zv, speed * dot(zv, lv) * db.vertical_surface_force),
    );
    if s.jetting {
        let j = db.base.jet_force;
        force = match v.k.thrust {
            THRUST_FORWARD => add(force, scale(yv, j)),
            THRUST_BACKWARD => sub(force, scale(yv, j)),
            _ => add(force, scale(zv, j * db.vert_thrust_multiple)),
        };
    }
    force = add(force, scale(yv, v.k.move_y * db.maneuvering_force));
    force = add(force, scale(xv, v.k.move_x * db.maneuvering_force));
    let sa = db.base.max_steering_angle;
    let mut steer = [s.steering[0] / sa, s.steering[1] / sa];
    steer[0] *= steer[0].abs();
    steer[1] *= steer[1].abs();
    torque = sub(torque, scale(xv, steer[1] * db.steering_force));
    torque = sub(torque, scale(zv, steer[0] * db.steering_force));
    torque = add(torque, scale(yv, steer[0] * db.steering_roll_force));
    let ar = db.auto_angular_force * xv[2] - db.roll_force * dot(xv, lv);
    torque = add(torque, scale(yv, ar));
    force = add(force, s.applied_force);
    force[2] -= s.buoyancy * GRAVITY * mass * g;
    force = sub(force, scale(lv, s.drag));
    s.rigid.force = force;
    s.rigid.torque = torque;
}

/// `FlyingVehicle::getHeight`: a ray from the position down `hoverHeight + 10` (the create height when that is on), with the
/// body's collision off; `(r * t - height) / 10` with `t` the hit's fraction (1 when it meets nothing).
fn hover_height(
    v: &mut Vehicle<Flying>,
    db: &FlyingVehicleData,
    host: &mut dyn VehicleHost,
) -> f32 {
    let height = if v.k.use_create_height {
        db.create_hover_height
    } else {
        db.hover_height
    };
    let r = 10.0 + height;
    let p = v.s.position();
    host.disable_collision(&v.s);
    let mask = host.type_masks().vehicle_ray;
    let t = host
        .cast_ray(p, [p[0], p[1], p[2] - r], mask)
        .map_or(1.0, |h| h.t);
    host.enable_collision(&v.s);
    (r * t - height) / 10.0
}

/// The `useCreateHeight(bool)` console method: the flag and mask 2.
pub fn use_create_height(v: &mut Vehicle<Flying>, on: bool) {
    v.k.use_create_height = on;
    v.s.set_mask(state::net_mask::INITIAL);
}

/// The hover ray's end for a vehicle at `p` (for hosts that pre-check rays): `(p.x, p.y, p.z - (H + 10))`.
pub fn hover_ray_end(p: F3, h: f32) -> F3 {
    let hp = h + 10.0;
    [p[0], p[1], (p[2] - hp)]
}

/// The Flying packet words (the packet hook forwards to `Vehicle::writePacketData`: nothing of its own).
pub fn packet_words(
    v: &Vehicle<Flying>,
    db: &FlyingVehicleData,
    host: &mut dyn VehicleHost,
) -> crate::words::VehicleWords {
    crate::words::base_words(&v.s, &db.base.shape, host, crate::words::KindWords::Flying)
}
