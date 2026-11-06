//! `WheeledVehicle`: the wheels (eight structs), `extendWheels`, `updateForces`, the
//! sub-step with its contact prep, `updateMove`, `onNewDataBlock`, the datablock's
//! `preload`, the wheel console methods, and the subclass seam ([`WheeledClass`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/wheeledVehicle.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_collide::math::{F3, Matrix, mul_p, mul_v, normalize, quat_matrix};
use torque_collide::{Exhausted, ObjectId};

use crate::Move;
use crate::data::{SpringData, TireData, WheelRecord, WheeledVehicleData};
use crate::host::VehicleHost;
use crate::math::{GRAVITY, add, cross, dot, len_sq, scale, sub};
use crate::state::{self, VehicleState, net_mask};
use crate::tick::{Kind, Vehicle};

/// The wheel structs an object owns (eight) and the most `preload` records.
pub const WHEELS: usize = 8;

/// One wheel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    /// The tire datablock (`setWheelTire`) and the spring (`setWheelSpring`): the id the wire carries and the fields.
    /// Null until script sets them (`onNewDataBlock` clears them); a wheel without both is inert.
    pub tire: Option<(ObjectId, TireData)>,
    pub spring: Option<(ObjectId, SpringData)>,
    /// Spring extension (1 = fully extended), angular velocity and angular position (client only).
    pub extension: f32,
    pub avel: f32,
    pub apos: f32,
    /// Longitudinal and lateral tire deformation.
    pub dy: f32,
    pub dx: f32,
    /// The ray hit this sub-step, with the normal, material, point and object of the hit.
    pub contact: bool,
    pub normal: F3,
    pub material: u32,
    pub point: F3,
    pub object: Option<ObjectId>,
    /// Steering factor, powered (constructor 1) and slipping.
    pub steering: f32,
    pub powered: bool,
    pub slipping: bool,
    /// The power factor and the slip amount (`updateForces`).
    pub power: f32,
    pub slip: f32,
}

impl Default for Wheel {
    /// The constructor's wheel: powered, everything else zero.
    fn default() -> Self {
        Wheel {
            tire: None,
            spring: None,
            extension: 0.0,
            avel: 0.0,
            apos: 0.0,
            dy: 0.0,
            dx: 0.0,
            contact: false,
            normal: [0.0; 3],
            material: 0,
            point: [0.0; 3],
            object: None,
            steering: 0.0,
            powered: true,
            slipping: false,
            power: 0.0,
            slip: 0.0,
        }
    }
}

/// What a WheeledVehicle adds to the state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Wheeled {
    pub wheels: [Wheel; WHEELS],
    /// The brake (the move's trigger 2).
    pub brake: bool,
}

impl Kind for Wheeled {
    type Data = WheeledVehicleData;

    fn base(db: &WheeledVehicleData) -> &crate::data::VehicleData {
        &db.base
    }

    fn update_move(
        v: &mut Vehicle<Wheeled>,
        db: &WheeledVehicleData,
        mv: Option<&Move>,
        host: &mut dyn VehicleHost,
    ) {
        update_move(v, db, mv, host);
    }

    fn step(
        v: &mut Vehicle<Wheeled>,
        db: &WheeledVehicleData,
        dt: f32,
        host: &mut dyn VehicleHost,
    ) -> Result<(), Exhausted> {
        substep(v, db, dt, host)
    }
}

/// The count of wheels the datablock has, at most [`WHEELS`].
fn count(db: &WheeledVehicleData) -> usize {
    db.wheels.len().min(WHEELS)
}

/// A subclass of the WheeledVehicle: its overrides of the step, one method per extension point. The class's own datablock
/// fields live in the implementing value; its per-object fields in [`VehicleState::ext`]. The host hands it over with [`VehicleHost::wheeled_class`].
pub trait WheeledClass: std::fmt::Debug {
    /// `updateMove`, after the base and the brake, with the move the vehicle runs (the null move when none): a steering change
    /// of the class's own. None by default.
    fn after_move(&self, _s: &mut VehicleState, _mv: &Move) {}

    /// `updateForces`, after the wheels and the jet and before the container's buoyancy and drag: forces (`force`) and torques
    /// (`torque`) the class adds, given the linear velocity `vel`, the body's axes and the throttle. None by default.
    #[allow(clippy::too_many_arguments)]
    fn extra_forces(
        &self,
        _s: &VehicleState,
        _k: &Wheeled,
        _db: &WheeledVehicleData,
        _axes: &Axes,
        _vel: [f32; 3],
        _throttle: f32,
        _force: &mut [f32; 3],
        _torque: &mut [f32; 3],
    ) {
    }
}

/// `WheeledVehicle::updateMove(mv)`: the base and the brake from trigger 2, then the subclass's steering
/// ([`WheeledClass::after_move`]).
pub fn update_move(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    mv: Option<&Move>,
    host: &mut dyn VehicleHost,
) {
    state::update_move(&mut v.s, &db.base, mv, host);
    let m = mv.copied().unwrap_or_default();
    v.k.brake = m.triggers[2];
    if let Some(class) = host.wheeled_class() {
        class.after_move(&mut v.s, &m);
    }
}

/// `extendWheels` (through the [`library`](crate::library) table).
pub fn extend_wheels(
    s: &mut VehicleState,
    k: &mut Wheeled,
    db: &WheeledVehicleData,
    host: &mut dyn VehicleHost,
) {
    (crate::library::get().extend_wheels)(s, k, db, host)
}

/// `extendWheels`: with the body's collision off, one ray per wheel with a tire and a spring from the hub (the wheel record's
/// position moved by the transform) down the transform's z axis by the spring's length plus the tire radius; a hit sets the
/// extension from the hit fraction, the contact's normal, point, material and object; a miss leaves the extension at 1 and
/// marks the wheel slipping.
pub fn plain_extend_wheels(
    s: &mut VehicleState,
    k: &mut Wheeled,
    db: &WheeledVehicleData,
    host: &mut dyn VehicleHost,
) {
    host.disable_collision(s);
    let m: Matrix = s.render_to_world;
    for i in 0..count(db) {
        let w = &mut k.wheels[i];
        let (Some((_, tire)), Some((_, spring))) = (w.tire, w.spring) else {
            continue;
        };
        w.extension = 1.0;
        let start = mul_p(&m, db.wheels[i].pos);
        let vec = mul_v(&m, [0.0, 0.0, -spring.length]);
        let ts = tire.radius / spring.length;
        let end: F3 = std::array::from_fn(|c| start[c] + vec[c] * (1.0 + ts));
        let ts = ts / (1.0 + ts);
        let ray = host.type_masks().vehicle_ray;
        match host.cast_ray(start, end, ray) {
            Some(hit) => {
                w.contact = true;
                w.extension = if hit.t < ts {
                    0.0
                } else {
                    (hit.t - ts) / (1.0 - ts)
                };
                w.normal = hit.normal;
                w.point = hit.point;
                w.material = hit.material;
                w.object = hit.object;
            }
            None => {
                w.contact = false;
                w.slipping = true;
            }
        }
    }
    host.enable_collision(s);
}

/// The axes of the rigid's rotation matrix (`setMatrix`): columns right (0, 4, 8), forward (1, 5, 9), up (2, 6, 10).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axes {
    pub right: [f32; 3],
    pub fwd: [f32; 3],
    pub up: [f32; 3],
}

impl Axes {
    pub fn of(q: &crate::math::Quat) -> Axes {
        let m = quat_matrix(*q);
        Axes {
            right: [m[0], m[4], m[8]],
            fwd: [m[1], m[5], m[9]],
            up: [m[2], m[6], m[10]],
        }
    }
}

/// `WheeledVehicle::updateForces(dt)` (through the [`library`](crate::library) table).
pub fn update_forces(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    dt: f32,
    host: &mut dyn VehicleHost,
) {
    (crate::library::get().wheeled_update_forces)(v, db, dt, host)
}

/// `WheeledVehicle::updateForces(dt)`: the wheels extended, then per wheel with a tire and a spring the suspension (spring,
/// damping, anti-sway) and the tire forces (longitudinal and lateral, limited by the friction circle) on a contact, the
/// relaxation off one, the drive and the brake; then the jet, the class's extra forces, buoyancy, drag, gravity and the
/// momentum step.
pub fn plain_update_forces(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    dt: f32,
    host: &mut dyn VehicleHost,
) {
    extend_wheels(&mut v.s, &mut v.k, db, host);
    let s = &mut v.s;
    let k = &mut v.k;
    let n = count(db);
    let a_momentum = s.mass / n as f32;
    let ax = Axes::of(&s.rigid.ang_pos);
    let (bx, by, bz) = (ax.right, ax.fwd, ax.up);
    let quadratic_steering = -(s.steering[0] * s.steering[0].abs());
    let (sin_steering, cos_steering) = quadratic_steering.sin_cos();
    let (engine_torque, brake_vel) = if k.brake {
        (0.0, (db.brake_torque / a_momentum) * dt)
    } else if s.throttle != 0.0 {
        let mut t = db.engine_torque * s.throttle;
        if s.throttle > 0.0 && s.jetting {
            t *= 2.0;
        }
        (t, 0.0)
    } else {
        (0.0, (db.engine_brake / a_momentum) * dt)
    };
    let mut force = [0.0f32; 3];
    let mut torque = [0.0f32; 3];
    let pos0 = s.rigid.lin_pos;
    for i in 0..n {
        let mut w = k.wheels[i];
        let (Some((_, tire)), Some((_, spring))) = (w.tire, w.spring) else {
            continue;
        };
        let rec: WheelRecord = db.wheels[i];
        let mut fy = 0.0f32;
        if w.contact {
            // The hub in world space and its velocity.
            let p = rec.pos;
            let pos: F3 =
                std::array::from_fn(|c| bx[c] * p[0] + by[c] * p[1] + bz[c] * p[2] + pos0[c]);
            let r = sub(pos, s.rigid.world_com);
            let local_vel = s.rigid.velocity_at(r);
            let spring_f = spring.force * (1.0 - w.extension);
            let mut damping = spring.damping * -(dot(bz, local_vel) / spring.length);
            if damping < 0.0 {
                damping = 0.0;
            }
            let mut anti_sway = 0.0f32;
            if let Some(o) = usize::try_from(rec.opposite).ok().filter(|&o| o < WHEELS) {
                if k.wheels[o].contact {
                    anti_sway = (k.wheels[o].extension - w.extension) * spring.anti_sway_force;
                }
                if anti_sway < 0.0 {
                    anti_sway = 0.0;
                }
            }
            let fv = scale(bz, spring_f + damping + anti_sway);
            torque = add(torque, cross(r, fv));
            force = add(force, fv);
            // The tire's axes on the contact surface.
            let wheel_x = add(
                scale(bx, cos_steering),
                scale(by, sin_steering * w.steering),
            );
            let tire_y = normalize(cross(w.normal, wheel_x));
            let tire_x = normalize(cross(tire_y, w.normal));
            let contact = sub(w.point, s.rigid.world_com);
            let wheel_vel = s.rigid.velocity_at(contact);
            let x_vel = dot(tire_x, wheel_vel);
            let y_vel = dot(tire_y, wheel_vel);
            // Longitudinal and lateral tire deflection and force.
            let ddy =
                (w.avel * tire.radius - y_vel) - tire.longitudinal_relaxation * w.avel.abs() * w.dy;
            w.dy += ddy * dt;
            fy = tire.longitudinal_force * w.dy + tire.longitudinal_damping * ddy;
            let ddx = x_vel - tire.lateral_relaxation * w.avel.abs() * w.dx;
            w.dx += ddx * dt;
            let mut fx = -(tire.lateral_force * w.dx + tire.lateral_damping * ddx);
            // The friction circle.
            let mut vertical_load = spring_f + damping + anti_sway;
            if vertical_load < 0.0 {
                vertical_load = 0.0;
            }
            let mu = if w.slipping {
                tire.kinetic_friction
            } else {
                tire.static_friction
            };
            let mut f_n = vertical_load * mu;
            f_n *= f_n;
            let f_w = fx * fx + fy * fy;
            if f_w > f_n {
                let kk = (f_n / f_w).sqrt();
                fy *= kk;
                fx *= kk;
                w.dy *= kk;
                w.dx *= kk;
                w.slip = 1.0 - kk;
                w.slipping = true;
            } else {
                w.slipping = false;
                w.slip = 0.0;
            }
            let fv = add(scale(tire_x, fx), scale(tire_y, fy));
            let at = sub(pos, scale(bz, spring.length * w.extension));
            let r = sub(at, s.rigid.world_com);
            torque = add(torque, cross(r, fv));
            force = add(force, fv);
        } else {
            w.power = 0.0;
            w.slip = 0.0;
            w.dy += (-tire.longitudinal_relaxation * w.avel.abs() * w.dy) * dt;
            w.dx += (-tire.lateral_relaxation * w.avel.abs() * w.dx) * dt;
        }
        // The drive and the brake.
        w.power = if w.powered {
            let max_avel = db.max_wheel_speed / tire.radius;
            if w.avel.abs() > max_avel {
                0.0
            } else {
                1.0 - w.avel.abs() / max_avel
            }
        } else {
            0.0
        };
        w.avel += ((w.power * engine_torque - fy * tire.radius) / a_momentum) * dt;
        if brake_vel > w.avel.abs() {
            w.avel = 0.0;
        } else if w.avel > 0.0 {
            w.avel -= brake_vel;
        } else {
            w.avel += brake_vel;
        }
        k.wheels[i] = w;
    }
    if s.jetting {
        force = add(force, scale(by, db.base.jet_force));
    }
    if let Some(class) = host.wheeled_class() {
        class.extra_forces(
            s,
            k,
            db,
            &ax,
            s.rigid.lin_vel,
            s.throttle,
            &mut force,
            &mut torque,
        );
    }
    let r_ = &mut s.rigid;
    force[2] += -s.buoyancy * GRAVITY * r_.mass;
    force = sub(force, scale(r_.lin_vel, s.drag));
    torque = sub(torque, scale(r_.ang_mom, s.drag));
    if r_.at_rest && (len_sq(force) != 0.0 || len_sq(torque) != 0.0) {
        r_.at_rest = false;
    }
    force[2] += GRAVITY * r_.mass;
    r_.lin_mom = add(r_.lin_mom, scale(force, dt));
    r_.ang_mom = add(r_.ang_mom, scale(torque, dt));
    r_.update_velocity();
    r_.force = [0.0; 3];
    r_.torque = [0.0; 3];
    if r_.at_rest {
        r_.set_at_rest();
    }
}

/// The sub-step: the old velocity, the accumulators zeroed, `updateForces`, the contact prep and the rest test unless at
/// rest, the integration unless (still) at rest, `notifyCollision`, the scan, the impact and the liquid callbacks.
pub fn substep(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    dt: f32,
    host: &mut dyn VehicleHost,
) -> Result<(), Exhausted> {
    let base = &db.base;
    let v0 = v.s.rigid.lin_vel;
    v.s.rigid.force = [0.0; 3];
    v.s.rigid.torque = [0.0; 3];
    update_forces(v, db, dt, host);
    let mut touched = false;
    if !v.s.rigid.at_rest {
        touched = contact_prep(v, db, dt, host);
        let empty = v.body.list.count == 0;
        state::rest_test(&mut v.s, empty, dt);
    }
    if !v.s.rigid.at_rest {
        v.s.rigid.integrate(dt);
    }
    let now = host.sim_time();
    state::notify_collision(&mut v.s, now, host);
    state::end_of_step(&mut v.s, base, touched, v0, host, &mut v.scan);
    Ok(())
}

/// The contact prep: the body collision, `resolveCollision`, the host's hook when the body touched
/// ([`VehicleHost::wheeled_body_contact`]), `resolveContacts`, then [`Body::hold_out`](state::Body::hold_out). True when the body touched.
pub fn contact_prep(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    dt: f32,
    host: &mut dyn VehicleHost,
) -> bool {
    let base = &db.base;
    v.body.collide(&v.s, base, host);
    let touched = state::resolve_collision(&mut v.s, &v.body.list, base, host);
    if touched {
        host.wheeled_body_contact(&mut v.s, &v.k);
    }
    state::resolve_contacts(&mut v.s, &v.body.list, base, dt);
    v.body.hold_out(&mut v.s, base, host);
    touched
}

/// `WheeledVehicle::onNewDataBlock`'s server part after the base's: the box inertia (`massBox` when all three are
/// above zero, else the object box's size), mass, `1/mass`, friction, restitution, the centre of mass and its world point,
/// and each wheel of the datablock reset: tire and spring cleared, extension 1, steering 1 for a wheel in front (`pos.y > 0`).
/// The host runs the script's `onNewDataBlock` after it, and sets every mask bit.
pub fn on_new_data_block(s: &mut VehicleState, k: &mut Wheeled, db: &WheeledVehicleData) {
    s.on_new_data_block(&db.base);
    let mb = db.base.mass_box;
    // massBox only when all three are ordered and above zero.
    let ext = if !(mb[0] > 0.0 && mb[1] > 0.0 && mb[2] > 0.0) {
        let b = s.obj_box;
        [
            (b.max[0] - b.min[0]),
            (b.max[1] - b.min[1]),
            (b.max[2] - b.min[2]),
        ]
    } else {
        mb
    };
    let r = &mut s.rigid;
    r.set_box_inertia(ext);
    r.mass = db.base.shape.mass;
    r.one_over_mass = 1.0 / db.base.shape.mass;
    r.friction = db.base.body_friction;
    r.restitution = db.base.body_restitution;
    r.com = db.base.mass_center;
    let wc = crate::math::quat_mul_p(&r.ang_pos, r.com);
    r.world_com = [
        (wc[0] + r.lin_pos[0]),
        (r.lin_pos[1] + wc[1]),
        (r.lin_pos[2] + wc[2]),
    ];
    for i in 0..count(db) {
        let w = &mut k.wheels[i];
        w.tire = None;
        w.spring = None;
        w.extension = 1.0;
        w.avel = 0.0;
        w.apos = 0.0;
        w.contact = false;
        w.object = None;
        w.slip = 0.0;
        if db.wheels[i].pos[1] > 0.0 {
            w.steering = 1.0;
        }
    }
    s.set_mask(u32::MAX);
}

/// `setWheelSteering(wheel, value)`: the value clamped to [-1, 1] (`f <= 1 ? (f <= -1 ? -1 : f) : 1`; a NaN stays),
/// mask 0x80. False for a wheel index past the datablock's count.
pub fn set_wheel_steering(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    i: usize,
    value: f32,
) -> bool {
    if i >= count(db) {
        return false;
    }
    v.k.wheels[i].steering = if value <= 1.0 {
        if value <= -1.0 { -1.0 } else { value }
    } else if value.is_nan() {
        value
    } else {
        1.0
    };
    v.s.set_mask(net_mask::WHEELS);
    true
}

/// `setWheelPowered(wheel, bool)`, mask 0x80.
pub fn set_wheel_powered(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    i: usize,
    on: bool,
) -> bool {
    if i >= count(db) {
        return false;
    }
    v.k.wheels[i].powered = on;
    v.s.set_mask(net_mask::WHEELS);
    true
}

/// `setWheelTire(wheel, tire)`, mask 0x80 (the host resolves the datablock; a wrong class is the host's warning).
pub fn set_wheel_tire(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    i: usize,
    tire: (ObjectId, TireData),
) -> bool {
    if i >= count(db) {
        return false;
    }
    v.k.wheels[i].tire = Some(tire);
    v.s.set_mask(net_mask::WHEELS);
    true
}

/// `setWheelSpring(wheel, spring)`, mask 0x80.
pub fn set_wheel_spring(
    v: &mut Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    i: usize,
    spring: (ObjectId, SpringData),
) -> bool {
    if i >= count(db) {
        return false;
    }
    v.k.wheels[i].spring = Some(spring);
    v.s.set_mask(net_mask::WHEELS);
    true
}

/// `getWheelCount()`.
pub fn wheel_count(db: &WheeledVehicleData) -> usize {
    count(db)
}

/// The Wheeled packet words: the base, the brake, each wheel's angular velocity, deformations and slipping.
pub fn packet_words(
    v: &Vehicle<Wheeled>,
    db: &WheeledVehicleData,
    host: &mut dyn VehicleHost,
) -> crate::words::VehicleWords {
    let wheels = v.k.wheels[..count(db)]
        .iter()
        .map(|w| crate::words::WheelWords {
            avel: w.avel,
            dy: w.dy,
            dx: w.dx,
            slipping: w.slipping,
        })
        .collect();
    crate::words::base_words(
        &v.s,
        &db.base.shape,
        host,
        crate::words::KindWords::Wheeled {
            brake: v.k.brake,
            wheels,
        },
    )
}
