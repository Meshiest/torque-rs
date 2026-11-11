//! `Rigid`: the integrator, the inertia tensors, the impulses and the static-surface collision response, as TGE 1.4's
//! `game/rigid.cc` writes them. The quaternion and matrix helpers are in [`crate::math`]; the routines whose arithmetic a host
//! may replace go through the [`library`] table, and the `plain_*` methods are this crate's own.
//! `getKineticEnergy` inverts the orientation before `mulP`, which inverts it again.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/rigid.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use torque_collide::math::{
    F3, IDENTITY, Matrix, matrix_product, mul_v, quat_matrix, transcendental, transpose,
};

use crate::library;
use crate::math::{
    Quat, add, cross, dot, full_inverse, len_sq, quat_inverse, quat_mul, quat_mul_p,
    quat_normalize, scale, sub,
};

/// The number of words in [`Rigid::to_words`].
pub const WORDS: usize = 84;

/// The rigid body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rigid {
    /// Force and torque: the accumulators `updateForces` fills and `integrate` reads (it does not clear them).
    pub force: F3,
    pub torque: F3,
    /// Linear velocity, position and linear momentum.
    pub lin_vel: F3,
    pub lin_pos: F3,
    pub lin_mom: F3,
    /// Angular velocity, orientation and angular momentum.
    pub ang_vel: F3,
    pub ang_pos: Quat,
    pub ang_mom: F3,
    /// Object inertia, its inverse, and the inverse world inertia `R * invI * R^T`.
    pub inertia: Matrix,
    pub inv_inertia: Matrix,
    pub inv_world_inertia: Matrix,
    /// Centre of mass (object space) and in the world.
    pub com: F3,
    pub world_com: F3,
    /// Mass and its reciprocal (each set by `onNewDataBlock`; they need not agree).
    pub mass: f32,
    pub one_over_mass: f32,
    /// Restitution and friction.
    pub restitution: f32,
    pub friction: f32,
    /// At rest.
    pub at_rest: bool,
}

impl Default for Rigid {
    /// The values the Vehicle constructor leaves: zeros, the identity orientation and inverse tensors, mass 1,
    /// restitution 0.3, friction 0.5. `inertia` is not set by the constructor (zero here).
    fn default() -> Self {
        Rigid {
            force: [0.0; 3],
            torque: [0.0; 3],
            lin_vel: [0.0; 3],
            lin_pos: [0.0; 3],
            lin_mom: [0.0; 3],
            ang_vel: [0.0; 3],
            ang_pos: crate::math::QUAT_IDENTITY,
            ang_mom: [0.0; 3],
            inertia: [0.0; 16],
            inv_inertia: IDENTITY,
            inv_world_inertia: IDENTITY,
            com: [0.0; 3],
            world_com: [0.0; 3],
            mass: 1.0,
            one_over_mass: 1.0,
            restitution: 0.3,
            friction: 0.5,
            at_rest: false,
        }
    }
}

impl Rigid {
    /// The fields as [`WORDS`] words in declaration order (`at_rest` as 0 or 1 in the last), for vectors and tests.
    pub fn to_words(&self) -> [u32; WORDS] {
        let mut w = [0u32; WORDS];
        let mut n = 0;
        let mut put = |vals: &[f32]| {
            for v in vals {
                w[n] = v.to_bits();
                n += 1;
            }
        };
        put(&self.force);
        put(&self.torque);
        put(&self.lin_vel);
        put(&self.lin_pos);
        put(&self.lin_mom);
        put(&self.ang_vel);
        put(&self.ang_pos);
        put(&self.ang_mom);
        put(&self.inertia);
        put(&self.inv_inertia);
        put(&self.inv_world_inertia);
        put(&self.com);
        put(&self.world_com);
        put(&[
            self.mass,
            self.one_over_mass,
            self.restitution,
            self.friction,
        ]);
        w[n] = u32::from(self.at_rest);
        w
    }

    /// The inverse of [`Rigid::to_words`] (`at_rest` from the low byte).
    pub fn from_words(w: &[u32; WORDS]) -> Rigid {
        fn take<const N: usize>(w: &[u32; WORDS], n: &mut usize) -> [f32; N] {
            let out = std::array::from_fn(|i| f32::from_bits(w[*n + i]));
            *n += N;
            out
        }
        let n = &mut 0;
        let force = take(w, n);
        let torque = take(w, n);
        let lin_vel = take(w, n);
        let lin_pos = take(w, n);
        let lin_mom = take(w, n);
        let ang_vel = take(w, n);
        let ang_pos = take(w, n);
        let ang_mom = take(w, n);
        let inertia = take(w, n);
        let inv_inertia = take(w, n);
        let inv_world_inertia = take(w, n);
        let com = take(w, n);
        let world_com = take(w, n);
        let [mass, one_over_mass, restitution, friction] = take(w, n);
        Rigid {
            force,
            torque,
            lin_vel,
            lin_pos,
            lin_mom,
            ang_vel,
            ang_pos,
            ang_mom,
            inertia,
            inv_inertia,
            inv_world_inertia,
            com,
            world_com,
            mass,
            one_over_mass,
            restitution,
            friction,
            at_rest: w[*n] & 0xff != 0,
        }
    }

    /// `Rigid::integrate(dt)` (through the table).
    pub fn integrate(&mut self, dt: f32) {
        (library::get().rigid_integrate)(self, dt)
    }

    /// `Rigid::updateVelocity()` (through the table).
    pub fn update_velocity(&mut self) {
        (library::get().rigid_update_velocity)(self)
    }

    /// `Rigid::applyImpulse(r, J)` (through the table).
    pub fn apply_impulse(&mut self, r: F3, j: F3) {
        (library::get().rigid_apply_impulse)(self, r, j)
    }

    /// `Rigid::getZeroImpulse(r, n)` (through the table).
    pub fn zero_impulse(&self, r: F3, n: F3) -> f32 {
        (library::get().rigid_zero_impulse)(self, r, n)
    }

    /// `Rigid::getVelocity(r)` (through the table).
    pub fn velocity_at(&self, r: F3) -> F3 {
        (library::get().rigid_velocity_at)(self, r)
    }

    /// `Rigid::resolveCollision(pos, n)` against a static surface (through the table).
    pub fn resolve_collision(&mut self, pos: F3, n: F3) -> bool {
        (library::get().rigid_resolve_collision)(self, pos, n)
    }

    /// `Rigid::getKineticEnergy()` (through the table).
    pub fn kinetic_energy(&self) -> f32 {
        (library::get().rigid_kinetic_energy)(self)
    }

    /// `Rigid::setObjectInertia(extents)` for a solid box (through the table).
    pub fn set_box_inertia(&mut self, ext: F3) {
        (library::get().rigid_set_box_inertia)(self, ext)
    }

    /// `Rigid::integrate(dt)`: the orientation turned by the angular velocity over `dt` (the position turned with it about the
    /// centre of mass), the angular momentum, the position and the linear momentum stepped, the velocity, then the world
    /// tensor, the velocities and the world centre of mass.
    pub fn plain_integrate(&mut self, dt: f32) {
        let angle = len_sq(self.ang_vel).sqrt();
        if angle != 0.0 {
            let (sin_half, cos_half) = transcendental::sin_cos_f32(angle * dt * -0.5);
            let k = sin_half * (1.0 / angle);
            let av = self.ang_vel;
            let dq: Quat = [av[0] * k, av[1] * k, av[2] * k, cos_half];
            self.ang_pos = quat_normalize(&quat_mul(&self.ang_pos, &dq));
            let lp = sub(self.lin_pos, self.world_com);
            self.lin_pos = add(quat_mul_p(&dq, lp), self.world_com);
        }
        self.ang_mom = add(self.ang_mom, scale(self.torque, dt));
        self.lin_pos = add(self.lin_pos, scale(self.lin_vel, dt));
        self.lin_mom = add(self.lin_mom, scale(self.force, dt));
        self.lin_vel = scale(self.lin_mom, self.one_over_mass);
        self.update_inertial_tensor();
        self.update_velocity();
        self.update_center_of_mass();
    }

    /// `Rigid::updateVelocity()`: the velocities from the momenta (`IW * AM` for the angular one).
    pub fn plain_update_velocity(&mut self) {
        self.lin_vel = scale(self.lin_mom, self.one_over_mass);
        self.ang_vel = mul_v(&self.inv_world_inertia, self.ang_mom);
    }

    /// `Rigid::updateInertialTensor()`: `IW = (R * invI) * R^T` with `R` from the orientation.
    pub fn update_inertial_tensor(&mut self) {
        let r = quat_matrix(self.ang_pos);
        let tmp = matrix_product(&r, &self.inv_inertia);
        self.inv_world_inertia = matrix_product(&tmp, &transpose(&r));
    }

    /// `Rigid::updateCenterOfMass()`: `wcom = Q.mulP(com) + P`.
    pub fn update_center_of_mass(&mut self) {
        self.world_com = add(quat_mul_p(&self.ang_pos, self.com), self.lin_pos);
    }

    /// `Rigid::applyImpulse(r, J)`: wakes the body; the momentum and velocity, then `AM += r x J` and `AV = IW * AM`.
    pub fn plain_apply_impulse(&mut self, r: F3, j: F3) {
        self.at_rest = false;
        self.lin_mom = add(self.lin_mom, j);
        self.lin_vel = scale(self.lin_mom, self.one_over_mass);
        self.ang_mom = add(self.ang_mom, cross(r, j));
        self.ang_vel = mul_v(&self.inv_world_inertia, self.ang_mom);
    }

    /// `Rigid::getZeroImpulse(r, n)`: `1 / (m1 + n . ((IW * (r x n)) x r))`.
    pub fn plain_zero_impulse(&self, r: F3, n: F3) -> f32 {
        let a = cross(r, n);
        let b = mul_v(&self.inv_world_inertia, a);
        let c = cross(b, r);
        1.0 / (self.one_over_mass + dot(c, n))
    }

    /// `Rigid::getVelocity(r)`: `AV x r` plus the linear velocity.
    pub fn plain_velocity_at(&self, r: F3) -> F3 {
        add(cross(self.ang_vel, r), self.lin_vel)
    }

    /// `Rigid::resolveCollision(pos, n)` against a static surface: wakes the body; when the closing speed `-(v . n)` is at
    /// least 0, the normal impulse with restitution and the friction impulse, clamped to `friction * j`, along the tangent
    /// when its speed is not 0. Always true.
    pub fn plain_resolve_collision(&mut self, pos: F3, n: F3) -> bool {
        self.at_rest = false;
        let r = sub(pos, self.world_com);
        let v = self.velocity_at(r);
        let nv = -dot(v, n);
        if nv >= 0.0 {
            let zi = self.zero_impulse(r, n);
            let mut j = nv * (1.0 + self.restitution) * zi;
            let mut impulse = scale(n, j);
            let mut uv = add(v, scale(n, nv));
            let ul = len_sq(uv).sqrt();
            if ul != 0.0 {
                uv = scale(uv, 1.0 / -ul);
                let mut u = ul * self.zero_impulse(r, uv);
                j *= self.friction;
                if u > j {
                    u = j;
                }
                impulse = add(impulse, scale(uv, u));
            }
            self.apply_impulse(r, impulse);
        }
        true
    }

    /// `Rigid::getKineticEnergy()`: `0.5 * (mass * |v|^2 + w.x^2 / I0 + w.y^2 / I5 + w.z^2 / I10)` with `w` the angular velocity
    /// through the inverted orientation's `mulP` and `I` the inverse tensor's diagonal.
    pub fn plain_kinetic_energy(&self) -> f32 {
        let q = quat_inverse(&self.ang_pos);
        let w = quat_mul_p(&q, self.ang_vel);
        let f = &self.inv_inertia;
        0.5 * ((self.mass * dot(self.lin_vel, self.lin_vel))
            + w[0] * w[0] / f[0]
            + w[1] * w[1] / f[5]
            + w[2] * w[2] / f[10])
    }

    /// `Rigid::setCenterOfMass(c)`.
    pub fn set_center_of_mass(&mut self, c: F3) {
        self.com = c;
        self.world_com = add(quat_mul_p(&self.ang_pos, c), self.lin_pos);
    }

    /// `Rigid::setTransform(mat)`: the orientation from the matrix (`QuatF::set`), the position its translation, then the
    /// world centre of mass.
    pub fn set_transform(&mut self, mat: &Matrix) {
        self.ang_pos = crate::math::quat_from_matrix(mat);
        self.lin_pos = [mat[3], mat[7], mat[11]];
        self.world_com = add(quat_mul_p(&self.ang_pos, self.com), self.lin_pos);
    }

    /// `Rigid::setObjectInertia(extents)`: a solid box, `mass / 12` times the sums of the other two extents' squares on the
    /// diagonal, copied and inverted (left as is when the determinant is below 1e-5), then the world tensor.
    pub fn plain_set_box_inertia(&mut self, ext: F3) {
        let ot = self.mass / 12.0;
        let (a, b, c) = (ext[0] * ext[0], ext[1] * ext[1], ext[2] * ext[2]);
        let mut m = IDENTITY;
        m[0] = ot * (b + c);
        m[5] = ot * (c + a);
        m[10] = ot * (a + b);
        self.inertia = m;
        self.inv_inertia = m;
        full_inverse(&mut self.inv_inertia);
        self.update_inertial_tensor();
    }

    /// `Rigid::setAtRest()`: every momentum and velocity zero, at rest.
    pub fn set_at_rest(&mut self) {
        self.lin_mom = [0.0; 3];
        self.ang_mom = [0.0; 3];
        self.lin_vel = [0.0; 3];
        self.ang_vel = [0.0; 3];
        self.at_rest = true;
    }
}
