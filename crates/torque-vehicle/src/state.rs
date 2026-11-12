//! A vehicle's state (every field the server step reads or writes) and the base `Vehicle` code both classes share:
//! `ShapeBase::processTick` for a vehicle, `Vehicle::updateMove`, `onNewDataBlock`,
//! `setTransform`, `setPosition` with `resetWorldBox`, `setVelocity`,
//! setting the angular velocity (for a host), `applyImpulse`, `setEnergyLevel`, the working-set box, the body
//! collision (`findClosestState` and `getCollisionInfo`), `resolveCollision`, `resolveContacts`
//! the collision queue, `onCollision`'s push and `writePacketData`.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/vehicle.cpp` (Copyright (c)
//! 2012 GarageGames, LLC) and `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic,
//! Inc.); the code here is this crate's own.

use torque_collide::feature::StateList;
use torque_collide::math::{
    Box3, F3, IDENTITY, Matrix, affine_inverse, matrix_product, mul_box, quat_matrix,
};
use torque_collide::sat::{Penetration, Polytope};
use torque_collide::{
    CollisionList, Convex, ConvexKind, Exhausted, HullClass, ObjectId, WorkingSet, types,
};

pub use torque_collide::{MAX_COLLISION_QUEUE, QueuedCollision};

use crate::Move;
use crate::data::{ShapeBaseData, VehicleData};
use crate::host::VehicleHost;
use crate::math::{Quat, dot, len_sq, sub};
use crate::rigid::Rigid;

/// The limits in `resolveCollision`: at most 200 entries are looked at (the list holds 64, so the cap
/// never bites) and at most 50 passes are made; the 50th pass ends the call with "no collision".
pub const RESOLVE_ENTRIES: usize = 200;
pub const RESOLVE_PASSES: u32 = 50;
/// The id the vehicle's own body convex has in its state list (the working set numbers its entries from 1).
pub const BODY_CONVEX: u64 = u64::MAX;

/// Net mask bits a vehicle sets (`setMaskBits`).
pub mod net_mask {
    /// The pose (processTick's moved test).
    pub const POSE: u32 = 0x10;
    /// The energy (`setEnergyLevel`).
    pub const ENERGY: u32 = 0x20;
    /// `setTransform`.
    pub const WARP: u32 = 0x40;
    /// The wheel datablocks (`setWheelTire`/`Spring`/`Steering`/`Powered`).
    pub const WHEELS: u32 = 0x80;
    /// `useCreateHeight`.
    pub const INITIAL: u32 = 0x2;
}

/// The vehicle's state. Plain data, so a run can start from any state.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleState {
    pub id: ObjectId,
    // ---- SceneObject ----
    /// `mObjToWorld`, `mWorldToObj` and the render transform (`extendWheels` reads it).
    pub obj_to_world: Matrix,
    pub world_to_obj: Matrix,
    pub render_to_world: Matrix,
    /// Scale, object box, world box, sphere centre and radius.
    pub scale: F3,
    pub obj_box: Box3,
    pub world_box: Box3,
    pub world_center: F3,
    pub world_radius: f32,
    /// `mTypeMask`.
    pub type_mask: u32,
    /// Mask bits set this tick (the host ORs them into its ghost masks).
    pub mask_bits: u32,
    // ---- ShapeBase ----
    /// Energy and recharge rate.
    pub energy: f32,
    pub recharge_rate: f32,
    /// Mass, drag and buoyancy (`updateContainer`).
    pub mass: f32,
    pub drag: f32,
    pub buoyancy: f32,
    /// Liquid type and water coverage.
    pub liquid_type: u32,
    pub water_coverage: f32,
    /// Applied force and gravity multiplier.
    pub applied_force: F3,
    pub gravity_mod: f32,
    /// Damage, repair rate, repair reserve and damage state (0 = enabled).
    pub damage: f32,
    pub repair_rate: f32,
    pub repair_reserve: f32,
    pub damage_state: u32,
    /// The trigger states `onTrigger` compares against; the image slots with an image.
    pub trigger_state: [bool; crate::MAX_TRIGGERS],
    pub images: [bool; 4],
    /// Image slot 0's wet flag.
    pub image_wet: bool,
    /// The mount.
    pub mount: Option<ObjectId>,
    /// The collision queue (newest first).
    pub collisions: Vec<QueuedCollision>,
    // ---- Vehicle ----
    /// The last move.
    pub last_move: Move,
    /// Delta pos, the position change of the last tick (old - new), and the rotations before and after.
    pub delta_pos: F3,
    pub delta_pos_vec: F3,
    pub delta_rot: [Quat; 2],
    /// In liquid.
    pub in_liquid: bool,
    /// Steering (x from yaw, y from pitch) and the last change (old - new).
    pub steering: [f32; 2],
    pub steering_delta: [f32; 2],
    /// Throttle (the move's y), jetting, disableMove.
    pub throttle: f32,
    pub jetting: bool,
    pub disable_move: bool,
    /// The second contact list's count (only `setTransform` writes it: zero).
    pub contacts_count: u32,
    /// The rigid body.
    pub rigid: Rigid,
    /// The body convex's box (the object box times the scale, at `onAdd`).
    pub body_box: Box3,
    /// Consecutive low-energy sub-steps.
    pub rest_count: i32,
    // ---- a subclass ----
    /// The fields a subclass adds ([`crate::ext::VehicleFields`]); `None` for Torque's vehicle.
    pub ext: Option<Box<dyn crate::ext::VehicleFields>>,
}

impl VehicleState {
    /// A vehicle at `transform` (`onAdd` after `onNewDataBlock`): the object box, the rigid from the datablock
    /// and the transform, the body convex's box, the deltas.
    pub fn new(
        id: ObjectId,
        db: &VehicleData,
        obj_box: Box3,
        transform: &Matrix,
        scale: F3,
    ) -> VehicleState {
        let mut s = VehicleState {
            id,
            obj_to_world: *transform,
            world_to_obj: affine_inverse(transform),
            render_to_world: *transform,
            scale,
            obj_box,
            world_box: Box3::default(),
            world_center: [0.0; 3],
            world_radius: 0.0,
            type_mask: types::VEHICLE | types::SHAPE_BASE,
            mask_bits: 0,
            energy: 0.0,
            recharge_rate: 0.0,
            mass: db.shape.mass,
            drag: db.shape.drag,
            buoyancy: 0.0,
            liquid_type: 0,
            water_coverage: 0.0,
            applied_force: [0.0; 3],
            gravity_mod: 1.0,
            damage: 0.0,
            repair_rate: 0.0,
            repair_reserve: 0.0,
            damage_state: 0,
            trigger_state: [false; crate::MAX_TRIGGERS],
            images: [false; 4],
            image_wet: false,
            mount: None,
            collisions: Vec::new(),
            last_move: Move::default(),
            delta_pos: [0.0; 3],
            delta_pos_vec: [0.0; 3],
            delta_rot: [crate::math::QUAT_IDENTITY; 2],
            in_liquid: false,
            steering: [0.0; 2],
            steering_delta: [0.0; 2],
            throttle: 0.0,
            jetting: false,
            disable_move: false,
            contacts_count: 0,
            rigid: Rigid::default(),
            body_box: Box3::default(),
            rest_count: 0,
            ext: None,
        };
        s.reset_world_box();
        s.on_new_data_block(db);
        s.rigid.set_transform(transform);
        s.delta_rot = [s.rigid.ang_pos; 2];
        s.delta_pos = s.rigid.lin_pos;
        let k = |a: f32, b: f32| a * b;
        s.body_box = Box3::new(
            [
                k(obj_box.min[0], scale[0]),
                k(obj_box.min[1], scale[1]),
                k(obj_box.min[2], scale[2]),
            ],
            [
                k(obj_box.max[0], scale[0]),
                k(obj_box.max[1], scale[1]),
                k(obj_box.max[2], scale[2]),
            ],
        );
        s
    }

    /// The translation (`getPosition`).
    pub fn position(&self) -> F3 {
        [
            self.obj_to_world[3],
            self.obj_to_world[7],
            self.obj_to_world[11],
        ]
    }

    /// The view `collide` reads of this object (for the host's [`torque_collide::World`]).
    pub fn object_view(&self) -> torque_collide::ObjectView {
        torque_collide::ObjectView {
            obj_to_world: self.obj_to_world,
            world_to_obj: self.world_to_obj,
            scale: self.scale,
            obj_box: self.obj_box,
            world_box: self.world_box,
            world_radius: self.world_radius,
            type_mask: self.type_mask,
            collision_disabled: false,
        }
    }

    pub fn set_mask(&mut self, bits: u32) {
        self.mask_bits |= bits;
    }

    /// The vehicle's own body convex (a `ShapeBaseConvex`, hull 0, its box fixed at `onAdd`).
    pub fn body_convex(&self) -> Convex {
        Convex {
            id: BODY_CONVEX,
            object: self.id,
            kind: ConvexKind::Hull {
                class: HullClass::SHAPE_BASE,
                slot: 0,
                bounds: self.body_box,
            },
        }
    }

    /// `Vehicle::onNewDataBlock`'s rigid part: mass, `1/mass`, friction, restitution, the centre of mass, the diagonal
    /// inertia `mass * 0.4` inverted by `fullInverse`, the world tensor. (`ShapeBase::onNewDataBlock`'s
    /// mass and drag too.)
    pub fn on_new_data_block(&mut self, db: &VehicleData) {
        self.mass = db.shape.mass;
        self.drag = db.shape.drag;
        let r = &mut self.rigid;
        r.mass = db.shape.mass;
        r.one_over_mass = 1.0 / db.shape.mass;
        r.friction = db.body_friction;
        r.restitution = db.body_restitution;
        r.set_center_of_mass(db.mass_center);
        let k = r.mass * 0.4;
        let mut m = IDENTITY;
        m[0] = k;
        m[5] = k;
        m[10] = k;
        r.inertia = m;
        r.inv_inertia = m;
        crate::math::full_inverse(&mut r.inv_inertia);
        r.update_inertial_tensor();
    }

    /// `resetWorldBox`: the object box times the scale, moved by the transform; a box whose max is below its min on any
    /// axis (a NaN too) becomes -1..1; the sphere's centre and radius (`torque_collide::math::world_sphere`).
    pub fn reset_world_box(&mut self) {
        let s = self.scale;
        let b = self.obj_box;
        let scaled = Box3::new(
            std::array::from_fn(|i| b.min[i] * s[i]),
            std::array::from_fn(|i| b.max[i] * s[i]),
        );
        let mut w = mul_box(&self.obj_to_world, &scaled);
        if !(w.max[0] >= w.min[0] && w.max[1] >= w.min[1] && w.max[2] >= w.min[2]) {
            w = Box3::new([-1.0; 3], [1.0; 3]);
        }
        self.world_box = w;
        (self.world_center, self.world_radius) = torque_collide::math::world_sphere(&w);
    }

    /// `setPosition(pos, rot)`: the matrix from the quaternion (`setMatrix`) with the translation, its affine
    /// inverse, the world box, the host's rebin and render transform. The rigid is not touched.
    pub fn set_position(&mut self, pos: F3, rot: &Quat, host: &mut dyn VehicleHost) {
        let mut m = quat_matrix(*rot);
        m[3] = pos[0];
        m[7] = pos[1];
        m[11] = pos[2];
        self.world_to_obj = affine_inverse(&m);
        self.obj_to_world = m;
        self.reset_world_box();
        self.render_to_world = m;
        host.moved(self);
    }

    /// `Vehicle::setTransform(mat)`: mask 0x40, the rigid from the matrix, the object at it, awake, the second list
    /// emptied.
    pub fn set_transform(&mut self, mat: &Matrix, host: &mut dyn VehicleHost) {
        self.set_mask(net_mask::WARP);
        self.rigid.set_transform(mat);
        self.world_to_obj = affine_inverse(mat);
        self.obj_to_world = *mat;
        self.reset_world_box();
        self.render_to_world = *mat;
        host.moved(self);
        self.rigid.at_rest = false;
        self.contacts_count = 0;
    }

    /// The `setVelocity` console method (through the [`library`](crate::library) table).
    pub fn set_velocity(&mut self, v: F3) {
        (crate::library::get().set_velocity)(self, v)
    }

    /// Sets the angular velocity directly (a host's method, through the [`library`](crate::library) table).
    pub fn set_angular_velocity(&mut self, v: F3) {
        (crate::library::get().set_angular_velocity)(self, v)
    }

    /// `Vehicle::applyImpulse(pos, J)`: `r = pos - wcom` and `Rigid::applyImpulse` (which wakes the body).
    pub fn apply_impulse(&mut self, pos: F3, j: F3) {
        let r = sub(pos, self.rigid.world_com);
        self.rigid.apply_impulse(r, j);
    }

    /// `getEnergyLevel`: this vehicle's energy, or the mount chain's when the datablock inherits.
    pub fn energy_level(&self, db: &ShapeBaseData, host: &mut dyn VehicleHost) -> f32 {
        if db.inherit_energy {
            host.mount_energy(self)
        } else {
            self.energy
        }
    }

    /// `Vehicle::setEnergyLevel(e)`: clamped to `[0, maxEnergy]` (`e > max` takes the max; `0 > e` takes 0; a NaN
    /// stays) unless the datablock inherits (then the mount's, when there is one) or the vehicle is disabled; mask 0x20 always.
    pub fn set_energy_level(&mut self, e: f32, db: &ShapeBaseData, host: &mut dyn VehicleHost) {
        if !db.inherit_energy {
            if self.damage_state == 0 {
                let max = db.max_energy;
                self.energy = if e > max {
                    max
                } else if 0.0 > e {
                    0.0
                } else {
                    e
                };
            }
        } else if self.mount.is_some() {
            host.set_mount_energy(self, e);
        }
        self.set_mask(net_mask::ENERGY);
    }

    /// The no-zone result of `updateContainer`: the datablock's drag, no buoyancy, gravity 1, no applied force, dry.
    pub fn clear_container(&mut self, db: &ShapeBaseData) {
        self.drag = db.drag;
        self.buoyancy = 0.0;
        self.gravity_mod = 1.0;
        self.applied_force = [0.0; 3];
        self.water_coverage = 0.0;
    }
}

/// `ShapeBase::processTick` for a server vehicle: energy, damage, image slot 0's wet byte, the fade (the host's), the
/// image slots, `onTrigger` for each trigger that changed (only with a move). As `torque_sim::player`'s but for the energy (see
/// below; the flash and white-out decay are client-visible only and left out).
pub fn shape_base_tick(
    s: &mut VehicleState,
    db: &ShapeBaseData,
    mv: Option<&Move>,
    host: &mut dyn VehicleHost,
) {
    // Energy: `e > max` takes the max, else `0 > e` takes 0 (a NaN stays); a change (a NaN counts) goes
    // through `Vehicle::setEnergyLevel` (mask 0x20).
    if s.damage_state == 0 && !db.inherit_energy {
        let old = s.energy;
        let e = s.recharge_rate + old;
        let max = db.max_energy;
        let new = if e > max {
            max
        } else if 0.0 > e {
            0.0
        } else {
            e
        };
        s.energy = new;
        if new != old {
            s.set_energy_level(new, db, host);
        }
    }
    if !db.is_invincible {
        let old = s.damage;
        let mut r = old - s.repair_rate;
        if db.max_damage < r {
            r = db.max_damage;
        }
        if r <= 0.0 {
            r = 0.0;
        }
        s.damage = r;
        let mut reserve = s.repair_reserve;
        if r < reserve {
            s.repair_reserve = r;
            reserve = r;
        }
        if 0.0 < reserve {
            let step = if reserve < db.repair_rate {
                reserve
            } else {
                db.repair_rate
            };
            r -= step;
            s.damage = r;
            s.repair_reserve = reserve - step;
        }
        if old != r {
            host.damage_changed(s);
        }
    }
    host.advance_threads(s);
    let wet = f64::from(s.water_coverage) > 0.4;
    if s.images[0] && s.image_wet != wet {
        s.set_mask(0x0010_0000);
        s.image_wet = wet;
    }
    host.fade_tick(s);
    for slot in 0..4 {
        if s.images[slot] {
            host.image_tick(s, slot);
        }
    }
    if let Some(mv) = mv {
        for slot in 0..crate::MAX_TRIGGERS {
            if mv.triggers[slot] != s.trigger_state[slot] {
                s.trigger_state[slot] = mv.triggers[slot];
                host.on_trigger(s, slot, mv.triggers[slot]);
            }
        }
    }
}

/// `Vehicle::updateMove(mv)`; `None` is the null move (the code compares the pointer).
pub fn update_move(
    s: &mut VehicleState,
    db: &VehicleData,
    mv: Option<&Move>,
    host: &mut dyn VehicleHost,
) {
    let m = mv.copied().unwrap_or_default();
    s.last_move = m;
    if s.damage_state == 0 {
        host.set_image_trigger(s, 0, m.triggers[0]);
        host.set_image_trigger(s, 1, m.triggers[1]);
    }
    if !s.disable_move {
        s.throttle = m.y;
    }
    if mv.is_some() {
        let lim = db.max_steering_angle;
        let clamp = |v: f32| {
            let a = if v > lim { lim } else { v };
            if a > -lim { a } else { -lim }
        };
        let old = s.steering;
        let sx = m.yaw + old[0];
        let nx = clamp(sx);
        s.steering[0] = nx;
        let sy = m.pitch + old[1];
        let ny = clamp(sy);
        s.steering[1] = ny;
        s.steering_delta = [(old[0] - nx), (old[1] - ny)];
    } else {
        s.steering = [0.0; 2];
    }
    if !m.triggers[3] {
        s.jetting = false;
        return;
    }
    if !s.jetting {
        let e = s.energy_level(&db.shape, host);
        if !(e >= db.min_jet_energy) {
            return;
        }
        s.jetting = true;
    }
    let mut e2 = (s.energy_level(&db.shape, host)) - db.jet_energy_drain;
    if 0.0 > e2 {
        e2 = 0.0;
        s.jetting = false;
    }
    s.set_energy_level(e2, &db.shape, host);
}

/// `queueCollision(obj, vec)` on the vehicle's queue (the rules are [`torque_collide::queue_collision`]'s).
pub fn queue_collision(s: &mut VehicleState, now: u32, obj: ObjectId, vec: F3) {
    torque_collide::queue_collision(&mut s.collisions, now, obj, vec);
}

/// `ShapeBase::notifyCollision`: each entry stamped `now + 250` that still has its object runs the two
/// `onCollision`s and loses its object.
pub fn notify_collision(s: &mut VehicleState, now: u32, host: &mut dyn VehicleHost) {
    let stamp = now.wrapping_add(250);
    let mut i = 0;
    while i < s.collisions.len() {
        let e = s.collisions[i];
        if e.expiry == stamp
            && let Some(obj) = e.object
        {
            s.collisions[i].object = None;
            if !host.on_collision(s, obj, e.vec) {
                return;
            }
        }
        i += 1;
    }
}

/// The push of `Vehicle::onCollision` onto the other vehicle `target` hit by the vehicle at `pusher_pos` with the
/// vector `vec`: when `|target.v|^2 <= |vec|^2` (a NaN pushes) the target wakes and takes `applyImpulse(r = pusher_pos, vec)`
/// (the pusher's world position, not relative to anything).
pub fn collision_push(pusher_pos: F3, target: &mut Rigid, vec: F3) {
    if len_sq(target.lin_vel) > len_sq(vec) {
        return;
    }
    target.at_rest = false;
    target.apply_impulse(pusher_pos, vec);
}

/// The working-set query of `updateWorkingCollisionSet` (through the [`library`](crate::library) table).
pub fn working_box(s: &VehicleState, world: &dyn torque_collide::World) -> Box3 {
    (crate::library::get().working_box)(s, world)
}

/// TGE's working-set query: the body convex's box at the object transform grown on every side by
/// `l = ((|v| + 50) * dt) * 1.1 + 0.1`.
pub fn plain_working_box(s: &VehicleState, world: &dyn torque_collide::World) -> Box3 {
    let mut b = s
        .body_convex()
        .bounding_box_at(world, &s.obj_to_world, s.scale)
        .unwrap_or_default();
    let len = (len_sq(s.rigid.lin_vel).sqrt() + 50.0) * crate::tick::DT;
    let l = len * 1.1 + 0.1;
    for i in 0..3 {
        b.min[i] -= l;
        b.max[i] += l;
    }
    b
}

/// The body and its collision state between ticks: the working set, the GJK states and the collision list. Heap-held.
#[derive(Debug, Default)]
pub struct Body {
    pub working: WorkingSet,
    pub states: StateList,
    pub list: CollisionList,
    working_vec: Vec<Convex>,
    /// Scratch for [`Body::hold_out`]: the body, the piece it is tested against, and their overlap.
    me: Polytope,
    other: Polytope,
    overlap: Penetration,
}

impl Clone for Body {
    fn clone(&self) -> Self {
        Body {
            working: self.working.clone(),
            states: self.states.clone(),
            list: self.list.clone(),
            ..Body::default()
        }
    }
}

impl Body {
    /// `updateWorkingCollisionSet(mask)`: the grown box, collision off, `Convex::updateWorkingList`, collision on.
    pub fn update_working_set(
        &mut self,
        s: &VehicleState,
        host: &mut dyn VehicleHost,
    ) -> Result<(), Exhausted> {
        let query = working_box(s, host.world());
        let mask = host.type_masks().vehicle_working;
        host.disable_collision(s);
        let r = self.working.update(host.world(), s.id, &query, mask);
        host.enable_collision(s);
        r
    }

    /// The body collision of `updatePos` (and the Wheeled contact prep): the heap matrix from the rigid (`setMatrix`, raw
    /// translation) times the hull's node transform, the list emptied, `findClosestState` with the collision tolerance, and when
    /// that state is within it (`dist <= tol`), `getCollisionInfo`.
    pub fn collide(&mut self, s: &VehicleState, db: &VehicleData, host: &mut dyn VehicleHost) {
        let world = host.world();
        let c = hull_matrix(s, world);
        self.list.count = 0;
        let me = s.body_convex();
        let tol = db.collision_tol;
        self.working_vec.clear();
        self.working_vec.extend(self.working.iter().copied());
        let found = self
            .states
            .find_closest_state(world, &me, &c, s.scale, &self.working_vec, tol);
        if let Some(i) = found
            && tol >= self.states.states[i].dist
        {
            self.states
                .collision_info(world, &me, &c, s.scale, &mut self.list, tol);
        }
    }
}

/// The body hull's matrix at the rigid's pose (`setMatrix`, raw translation) times the hull's node transform.
fn hull_matrix(s: &VehicleState, world: &dyn torque_collide::World) -> Matrix {
    let mut m = quat_matrix(s.rigid.ang_pos);
    m[3] = s.rigid.lin_pos[0];
    m[7] = s.rigid.lin_pos[1];
    m[11] = s.rigid.lin_pos[2];
    match world.hull(s.id, 0).and_then(|h| h.node) {
        Some(node) => matrix_product(&m, &node),
        None => m,
    }
}

/// How deep the body may sit in a map piece before [`Body::hold_out`] moves it back out (m).
pub const HOLD_SLOP: f32 = 0.01;
/// The share of the depth past [`HOLD_SLOP`] that one sub-step moves the body back out.
pub const HOLD_PUSH: f32 = 0.5;
/// The passes of [`Body::hold_out`]'s impulses over one overlap's corners.
pub const HOLD_PASSES: u32 = 32;

impl Body {
    /// Holds the body out of the map pieces (interiors, terrain) it overlaps, after `resolveContacts`.
    ///
    /// TGE's responses leave a slow push into a surface (closing slower than `contactTol`) to `resolveContacts`' penalty
    /// spring, which a steady drive force or gravity overpowers; and its contacts come from `getCollisionInfo`'s features
    /// round GJK's closest points, which stop meaning anything once the two overlap (a box's feature is one corner or
    /// edge unless the direction is nearly square to it, and two crossing edges report the line between them as the normal).
    /// So a vehicle held against a tree creeps into the trunk until its contacts fall outside `collisionTol`, then drives
    /// through it.
    ///
    /// Here, for each static map piece (an interior hull, a terrain triangle) the body's GJK state finds within the collision
    /// tolerance, the separating axis test ([`torque_collide::sat`]) gives how deep the body hull is in it and the way out, exact
    /// however deep (for a terrain triangle, straight up its slope from the deepest corner under it). The corners of
    /// their common volume may not close along that way any further, nor slide along it past the body's `friction` times that
    /// push (sequential impulses over [`HOLD_PASSES`] passes, the sum at each corner never pulling), and the body moves out by
    /// [`HOLD_PUSH`] of the depth past [`HOLD_SLOP`] (its position only, so no speed is added). Moving objects are left to TGE's responses. The work is charged to the world's
    /// budget; a spent budget stops it where it was.
    pub fn hold_out(&mut self, s: &mut VehicleState, db: &VehicleData, host: &mut dyn VehicleHost) {
        let world = host.world();
        let Some(hull) = world.hull(s.id, 0) else {
            return;
        };
        let Some(accel) = hull.accel.as_ref() else {
            return;
        };
        let c = hull_matrix(s, world);
        let mut built = false;
        for st in &self.states.states {
            let terrain = match st.b.kind {
                ConvexKind::InteriorHull { .. } => false,
                ConvexKind::TerrainTri { .. } => true,
                _ => continue,
            };
            if !(st.dist <= db.collision_tol) {
                continue;
            }
            let Some(shape) = world.map_shape(st.b.object) else {
                continue;
            };
            if !built {
                if !world.charge(torque_collide::work::FEATURE * accel.verts.len() as u64) {
                    return;
                }
                self.me.set_hull(accel, &c, s.scale);
                built = true;
            }
            self.other.set_map_piece(&shape, &st.b.kind);
            let axes = (self.me.normals().len() + self.other.normals().len()) as u64
                + (self.me.edges().len() * self.other.edges().len()) as u64;
            let points = (self.me.points().len() + self.other.points().len()) as u64;
            if !world.charge(torque_collide::work::FEATURE * (axes + 1) * (points + 1)) {
                return;
            }
            if !torque_collide::sat::penetration(&self.me, &self.other, &mut self.overlap) {
                continue;
            }
            if terrain && !below_terrain(&self.other, &mut self.overlap) {
                continue;
            }
            let n = self.overlap.normal;
            let rb = &mut s.rigid;
            let mut sum = [0.0f32; torque_collide::sat::MAX_POINTS];
            let mut slide = [[0.0f32; 3]; torque_collide::sat::MAX_POINTS];
            for _ in 0..HOLD_PASSES {
                let mut changed = false;
                for (k, &p) in self.overlap.points.iter().enumerate() {
                    let r = sub(p, rb.world_com);
                    let vn = dot(rb.velocity_at(r), n);
                    let j = (sum[k] - vn * rb.zero_impulse(r, n)).max(0.0);
                    let dj = j - sum[k];
                    if dj.abs() > 1e-6 {
                        sum[k] = j;
                        rb.apply_impulse(r, n.map(|x| x * dj));
                        changed = true;
                    }
                    // Friction: the sliding along the surface stopped, the sum at the corner held to `friction` times
                    // its push.
                    let v = rb.velocity_at(r);
                    let vn = dot(v, n);
                    let vt: F3 = std::array::from_fn(|i| v[i] - n[i] * vn);
                    let ul = len_sq(vt).sqrt();
                    if !(ul > 1e-6) {
                        continue;
                    }
                    let t = vt.map(|x| -x / ul);
                    let jt = ul * rb.zero_impulse(r, t);
                    let mut want: F3 = std::array::from_fn(|i| slide[k][i] + t[i] * jt);
                    // `bodyFriction` is script data: below 0 (or NaN) would push the corner along, so it counts as 0.
                    let cap = rb.friction.max(0.0) * sum[k];
                    let wl = len_sq(want).sqrt();
                    if wl > cap {
                        want = want.map(|x| x * (cap / wl));
                    }
                    let djt: F3 = std::array::from_fn(|i| want[i] - slide[k][i]);
                    if len_sq(djt) > 1e-12 {
                        slide[k] = want;
                        rb.apply_impulse(r, djt);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            let push = (self.overlap.depth - HOLD_SLOP) * HOLD_PUSH;
            if push > 0.0 {
                let by = n.map(|x| x * push);
                rb.lin_pos = std::array::from_fn(|i| rb.lin_pos[i] + by[i]);
                rb.update_center_of_mass();
                self.me.translate(by);
            }
        }
    }
}

/// For a terrain triangle: the way out is straight up its slope, not the least way out of its prism (which may be a side,
/// across into the next triangle), and the depth is how far the deepest corner of the common volume is under its top. False
/// when nothing is under it.
fn below_terrain(piece: &Polytope, overlap: &mut Penetration) -> bool {
    let (Some(&up), Some(&corner)) = (piece.normals().first(), piece.points().first()) else {
        return false;
    };
    let up = if up[2] < 0.0 { up.map(|x| -x) } else { up };
    let top = dot(up, corner);
    let depth = overlap
        .points
        .iter()
        .map(|&q| top - dot(up, q))
        .fold(0.0f32, f32::max);
    overlap.normal = up;
    overlap.depth = depth;
    depth > 0.0
}

/// `Vehicle::resolveCollision(rigid, list)` (through the [`library`](crate::library) table).
pub fn resolve_collision(
    s: &mut VehicleState,
    list: &CollisionList,
    db: &VehicleData,
    host: &mut dyn VehicleHost,
) -> bool {
    (crate::library::get().resolve_collision)(s, list, db, host)
}

/// `Vehicle::resolveCollision(rigid, list)`: passes over the entries (at most [`RESOLVE_ENTRIES`]) within the collision
/// tolerance whose normal speed is below `-contactTol`: each gets `Rigid::resolveCollision`, and when its object is a shape
/// (`ShapeBaseObjectType`) a queued collision with the relative velocity. Repeats while a pass hit something; the
/// [`RESOLVE_PASSES`]th pass returns false. True when anything was hit.
pub fn plain_resolve_collision(
    s: &mut VehicleState,
    list: &CollisionList,
    db: &VehicleData,
    host: &mut dyn VehicleHost,
) -> bool {
    let count = list.count.min(RESOLVE_ENTRIES).min(list.entries.len());
    let now = host.sim_time();
    let mut any = false;
    let mut pass = 0u32;
    loop {
        let mut hit = false;
        for e in &list.entries[..count] {
            if !(e.distance < db.collision_tol) {
                continue;
            }
            let r = sub(e.point, s.rigid.world_com);
            let v = s.rigid.velocity_at(r);
            if !(dot(v, e.normal) < -db.contact_tol) {
                continue;
            }
            s.rigid.resolve_collision(e.point, e.normal);
            hit = true;
            any = true;
            if let Some(obj) = e.object
                && host
                    .world()
                    .object(obj)
                    .is_some_and(|o| o.type_mask & types::SHAPE_BASE != 0)
            {
                let ov = host.object_velocity(obj);
                queue_collision(s, now, obj, sub(v, ov));
            }
        }
        pass += 1;
        if pass >= RESOLVE_PASSES {
            return false;
        }
        if !hit {
            return any;
        }
    }
}

/// `Vehicle::resolveContacts(rigid, list, dt)` (through the [`library`](crate::library) table).
pub fn resolve_contacts(s: &mut VehicleState, list: &CollisionList, db: &VehicleData, dt: f32) {
    (crate::library::get().resolve_contacts)(s, list, db, dt)
}

/// `Vehicle::resolveContacts(rigid, list, dt)`: for each entry within the collision tolerance whose normal speed is within
/// the contact tolerance in size, a penalty force `((tol - dist) - vn / contactTol) * 2|Z|` along the normal (`Z` the zero
/// impulse) and, when that is positive and the tangent speed is not zero, friction up to `friction` times it; the forces and
/// torques summed, then `LM += F*dt`, `AM += T*dt` and `updateVelocity`.
pub fn plain_resolve_contacts(
    s: &mut VehicleState,
    list: &CollisionList,
    db: &VehicleData,
    dt: f32,
) {
    let rb = &mut s.rigid;
    let mut p = [0.0f32; 3];
    let mut l = [0.0f32; 3];
    let count = list.count.min(list.entries.len());
    for e in &list.entries[..count] {
        if !(e.distance < db.collision_tol) {
            continue;
        }
        let r = sub(e.point, rb.world_com);
        let v = rb.velocity_at(r);
        let n = e.normal;
        let vn = dot(v, n);
        if !(vn.abs() < db.contact_tol) {
            continue;
        }
        let zi = 2.0 * rb.zero_impulse(r, n).abs();
        let mut sv = (db.collision_tol - e.distance) * zi - (vn / db.contact_tol) * zi;
        let mut f: F3 = n.map(|c| c * sv);
        let mut uv: F3 = std::array::from_fn(|i| v[i] - n[i] * vn);
        let ul = len_sq(uv).sqrt();
        if sv > 0.0 && ul != 0.0 {
            uv = uv.map(|c| c / -ul);
            let mut u = ul * rb.zero_impulse(r, uv);
            sv *= rb.friction;
            if u > sv {
                u = sv;
            }
            f = std::array::from_fn(|i| f[i] + uv[i] * u);
        }
        p = std::array::from_fn(|i| p[i] + f[i]);
        let t = [
            r[1] * f[2] - r[2] * f[1],
            r[2] * f[0] - r[0] * f[2],
            r[0] * f[1] - r[1] * f[0],
        ];
        l = std::array::from_fn(|i| l[i] + t[i]);
    }
    for i in 0..3 {
        rb.lin_mom[i] += p[i] * dt;
        rb.ang_mom[i] += l[i] * dt;
    }
    rb.update_velocity();
}

/// The rest test after the contacts (through the [`library`](crate::library) table).
pub fn rest_test(s: &mut VehicleState, list_empty: bool, dt: f32) {
    (crate::library::get().rest_test)(s, list_empty, dt)
}

/// The rest test after the contacts: no contacts resets the counter; else, when the kinetic energy is below half the energy
/// of a tick's fall (`0.5 * mass * G^2`, `G = -20 * dt`), the counter grows and past 10 the body is put at rest.
pub fn plain_rest_test(s: &mut VehicleState, list_empty: bool, dt: f32) {
    if list_empty {
        s.rest_count = 0;
        return;
    }
    let k = s.rigid.kinetic_energy();
    let g = -20.0 * dt;
    let kg = 0.5 * s.rigid.mass * g * g;
    if k < 0.5 * kg {
        s.rest_count = s.rest_count.wrapping_add(1);
        if s.rest_count > 10 {
            s.rigid.set_at_rest();
        }
    }
}

/// The scan and the callbacks at the end of a sub-step: the trigger/corpse/item scan over the
/// body's box at the object transform (corpses and items queued with `v - their v`), then, when `collided`, the impact test
/// on the velocity change since the sub-step began (`speed > minImpactSpeed` calls `onImpact`), then the liquid callbacks.
pub fn end_of_step(
    s: &mut VehicleState,
    db: &VehicleData,
    collided: bool,
    v0: F3,
    host: &mut dyn VehicleHost,
    scratch: &mut Vec<crate::host::ScanHit>,
) {
    let query = s
        .body_convex()
        .bounding_box_at(host.world(), &s.obj_to_world, s.scale)
        .unwrap_or_default();
    scratch.clear();
    host.scan(s, &query, scratch);
    let now = host.sim_time();
    for hit in scratch.iter() {
        let v = s.rigid.lin_vel;
        let vec = sub(v, hit.velocity);
        queue_collision(s, now, hit.object, vec);
    }
    if collided {
        let dv = sub(s.rigid.lin_vel, v0);
        let speed = len_sq(dv).sqrt();
        if speed > db.min_impact_speed {
            host.on_impact(s, dv);
        }
    }
    if !s.in_liquid {
        if s.water_coverage != 0.0 {
            let (c, t) = (s.water_coverage, s.liquid_type);
            host.on_enter_liquid(s, c, t);
            s.in_liquid = true;
        }
    } else if s.water_coverage == 0.0 {
        let t = s.liquid_type;
        host.on_leave_liquid(s, t);
        s.in_liquid = false;
    }
}

/// The `setVelocity` console method: the velocity and the momentum `v * mass`. [`PLAIN`](crate::library::PLAIN)'s
/// `set_velocity`.
pub fn plain_set_velocity(s: &mut VehicleState, v: F3) {
    let r = &mut s.rigid;
    r.lin_vel = v;
    r.lin_mom = v.map(|c| c * r.mass);
}

/// Sets the angular velocity directly (a host's method): the angular velocity and the momentum `v * mass`.
/// [`PLAIN`](crate::library::PLAIN)'s `set_angular_velocity`.
pub fn plain_set_angular_velocity(s: &mut VehicleState, v: F3) {
    let r = &mut s.rigid;
    r.ang_vel = v;
    r.ang_mom = v.map(|c| c * r.mass);
}
