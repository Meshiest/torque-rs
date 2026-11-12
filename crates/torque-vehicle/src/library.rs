//! The vehicle library table: the routines a host may install other implementations of, called through one set of function
//! pointers as `torque_collide::library` does for the vector and matrix routines.
//!
//! Every entry of [`VehicleLibrary`] is called through [`get`]. [`PLAIN`] holds this crate's own code, plain `f32` arithmetic
//! in the order Torque's source writes it; a host installs another table once, before its first step, with [`install`].

use torque_collide::CollisionList;
use torque_collide::library::Seam;
use torque_collide::math::{Box3, F3, Matrix};

use crate::data::{FlyingVehicleData, VehicleData, WheeledVehicleData};
use crate::flying::Flying;
use crate::host::VehicleHost;
use crate::math::Quat;
use crate::rigid::Rigid;
use crate::state::VehicleState;
use crate::tick::Vehicle;
use crate::wheeled::Wheeled;

/// The routines called through the table. Each entry names the Torque routine it stands for.
#[derive(Debug, Clone, Copy)]
pub struct VehicleLibrary {
    /// `QuatF::mul(a, b)`.
    pub quat_mul: fn(&Quat, &Quat) -> Quat,
    /// `QuatF::inverse`.
    pub quat_inverse: fn(&Quat) -> Quat,
    /// `QuatF::normalize`.
    pub quat_normalize: fn(&Quat) -> Quat,
    /// `QuatF::set(const MatrixF&)`.
    pub quat_from_matrix: fn(&Matrix) -> Quat,
    /// `processTick`'s test whether the orientation moved: both quaternions normalised, `a * inverse(b)` away from identity.
    pub quat_rotated: fn(&Quat, &Quat) -> bool,
    /// `Point3F::len` with its zero test.
    pub len_eps: fn(F3) -> f32,
    /// `Point3F::lenSquared`.
    pub len_sq: fn(F3) -> f32,
    /// `MatrixF::fullInverse`.
    pub full_inverse: fn(&mut Matrix) -> bool,
    /// `Rigid::integrate(dt)`.
    pub rigid_integrate: fn(&mut Rigid, f32),
    /// `Rigid::updateVelocity`.
    pub rigid_update_velocity: fn(&mut Rigid),
    /// `Rigid::applyImpulse(r, J)`.
    pub rigid_apply_impulse: fn(&mut Rigid, F3, F3),
    /// `Rigid::getZeroImpulse(r, n)`.
    pub rigid_zero_impulse: fn(&Rigid, F3, F3) -> f32,
    /// `Rigid::getVelocity(r)`.
    pub rigid_velocity_at: fn(&Rigid, F3) -> F3,
    /// `Rigid::resolveCollision(pos, n)`.
    pub rigid_resolve_collision: fn(&mut Rigid, F3, F3) -> bool,
    /// `Rigid::getKineticEnergy`.
    pub rigid_kinetic_energy: fn(&Rigid) -> f32,
    /// `Rigid::setObjectInertia` for a box (`setBoxInertia`).
    pub rigid_set_box_inertia: fn(&mut Rigid, F3),
    /// `updateWorkingCollisionSet`'s box.
    pub working_box: fn(&VehicleState, &dyn torque_collide::World) -> Box3,
    /// `Vehicle::resolveCollision`.
    pub resolve_collision:
        fn(&mut VehicleState, &CollisionList, &VehicleData, &mut dyn VehicleHost) -> bool,
    /// `Vehicle::resolveContacts`.
    pub resolve_contacts: fn(&mut VehicleState, &CollisionList, &VehicleData, f32),
    /// The rest test of `updatePos`.
    pub rest_test: fn(&mut VehicleState, bool, f32),
    /// `WheeledVehicle::extendWheels`.
    pub extend_wheels:
        fn(&mut VehicleState, &mut Wheeled, &WheeledVehicleData, &mut dyn VehicleHost),
    /// `WheeledVehicle::updateForces(dt)`.
    pub wheeled_update_forces:
        fn(&mut Vehicle<Wheeled>, &WheeledVehicleData, f32, &mut dyn VehicleHost),
    /// `FlyingVehicle::updateForces`.
    pub flying_update_forces: fn(&mut Vehicle<Flying>, &FlyingVehicleData, &mut dyn VehicleHost),
    /// `writePacketData`'s steering as two 9-bit integers.
    pub steering_ints: fn([f32; 2], f32) -> [u32; 2],
    /// The `setVelocity` console method's velocity and momentum.
    pub set_velocity: fn(&mut VehicleState, F3),
    /// Setting the angular velocity directly (a host's method): the angular velocity and momentum.
    pub set_angular_velocity: fn(&mut VehicleState, F3),
}

/// This crate's own routines: plain `f32` arithmetic in the order Torque's source writes it.
pub static PLAIN: VehicleLibrary = VehicleLibrary {
    quat_mul: crate::math::plain_quat_mul,
    quat_inverse: crate::math::plain_quat_inverse,
    quat_normalize: crate::math::plain_quat_normalize,
    quat_from_matrix: crate::math::plain_quat_from_matrix,
    quat_rotated: crate::math::plain_quat_rotated,
    len_eps: crate::math::plain_len_eps,
    len_sq: crate::math::plain_len_sq,
    full_inverse: crate::math::plain_full_inverse,
    rigid_integrate: Rigid::plain_integrate,
    rigid_update_velocity: Rigid::plain_update_velocity,
    rigid_apply_impulse: Rigid::plain_apply_impulse,
    rigid_zero_impulse: Rigid::plain_zero_impulse,
    rigid_velocity_at: Rigid::plain_velocity_at,
    rigid_resolve_collision: Rigid::plain_resolve_collision,
    rigid_kinetic_energy: Rigid::plain_kinetic_energy,
    rigid_set_box_inertia: Rigid::plain_set_box_inertia,
    working_box: crate::state::plain_working_box,
    resolve_collision: crate::state::plain_resolve_collision,
    resolve_contacts: crate::state::plain_resolve_contacts,
    rest_test: crate::state::plain_rest_test,
    extend_wheels: crate::wheeled::plain_extend_wheels,
    wheeled_update_forces: crate::wheeled::plain_update_forces,
    flying_update_forces: crate::flying::plain_update_forces,
    steering_ints: crate::words::plain_steering_ints,
    set_velocity: crate::state::plain_set_velocity,
    set_angular_velocity: crate::state::plain_set_angular_velocity,
};

static SEAM: Seam<VehicleLibrary> = Seam::new(&PLAIN);

/// The table in use: the installed one, else [`PLAIN`].
#[inline]
pub fn get() -> &'static VehicleLibrary {
    SEAM.get()
}

/// Installs `library` for the rest of the process. Only the first install takes; installing the same table again is a
/// no-op. Returns false when another table was installed first.
pub fn install(library: &'static VehicleLibrary) -> bool {
    SEAM.install(library)
}
