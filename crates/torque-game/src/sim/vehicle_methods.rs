//! The vehicles' console methods and the mount answers (their driven tail and ghost view are the ghost layer's).
//!
//! - `setWheelSteering`, `setWheelPowered`, `setWheelTire`, `setWheelSpring`, `getWheelCount` (Torque: the
//!   `WheeledVehicle` methods): [`register`] over `vehicle::wheeled`; a host's own wheel methods read the index with
//!   [`wheel_index`]
//! - `useCreateHeight` (Torque: `Vehicle`'s): [`register`]
//! - the mount as a `Vehicle` (`updateMove`'s head copy) (Torque: `updateMove`'s cast): [`wheeled_mount`], else
//!   [`vehicle_mount`]
//!
//! The mount methods (`mountObject`, `getMountedObject`, ...), damage and energy are every ShapeBase's ([`super::mount`],
//! [`super::vitals`]); `setTransform`, `setVelocity`, `applyImpulse` and `getVelocity` dispatch to [`super::vehicle`].
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `T3D/vehicles/vehicle.cpp`, `T3D/vehicles/wheeledVehicle.cpp` and `T3D/vehicles/flyingVehicle.cpp` (Copyright (c) 2012
//! GarageGames, LLC). The code here is this crate's own.

use collide::ObjectId;
use collide::math::F3;
use ts_vm::fields::atob;
use ts_vm::sim::ObjId;
use ts_vm::text::{atof, atoi, to_f32};
use ts_vm::vm::{Native, NativeCall, Vm};
use vehicle::wheeled;

use super::vehicle::{Body, Data, data, ext_of, state, tire_data, with_body};
use crate::methods::register_method;
use crate::scene::camera::is_class;

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&b""[..], Vec::as_slice)
}

/// The datablock's hub count (0 without a WheeledVehicleData).
fn hubs(vm: &Vm, id: ObjId) -> usize {
    match data(vm, id) {
        Some(Data::Wheeled(d, _)) => wheeled::wheel_count(&d),
        _ => 0,
    }
}

/// The wheel argument: `atoi`, refused below 0 and at or past the hub count (with a warning that names `method`).
pub fn wheel_index(vm: &mut Vm, id: ObjId, c: &NativeCall, method: &str) -> Option<usize> {
    let i = atoi(arg(c, 2));
    let n = hubs(vm, id);
    match usize::try_from(i).ok().filter(|i| *i < n) {
        Some(i) => Some(i),
        None => {
            vm.warn(format!(
                "{method}: wheel index {i} out of bounds, vehicle has {n} hubs"
            ));
            None
        }
    }
}

/// Runs `f` on a WheeledVehicle's body (false when it is not one or its tick has it).
fn on_wheeled(
    vm: &mut Vm,
    id: ObjId,
    f: impl FnOnce(&mut vehicle::Vehicle<wheeled::Wheeled>, &vehicle::WheeledVehicleData) -> bool,
) -> bool {
    with_body(vm, id, |b, d| match (b, d) {
        (Body::Wheeled(v), Data::Wheeled(d, _)) => f(v, d),
        _ => false,
    })
    .unwrap_or(false)
}

/// The WheeledVehicle methods and `useCreateHeight`.
pub fn register(vm: &mut Vm) {
    register_method(vm, "WheeledVehicle", "setWheelSteering", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let Some(i) = wheel_index(vm, id, c, "setWheelSteering") else {
            return Native::Bool(false);
        };
        let value = to_f32(atof(arg(c, 3)));
        Native::Bool(on_wheeled(vm, id, |v, d| {
            wheeled::set_wheel_steering(v, d, i, value)
        }))
    });
    register_method(vm, "WheeledVehicle", "setWheelPowered", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let Some(i) = wheel_index(vm, id, c, "setWheelPowered") else {
            return Native::Bool(false);
        };
        let on = atob(arg(c, 3));
        Native::Bool(on_wheeled(vm, id, |v, d| {
            wheeled::set_wheel_powered(v, d, i, on)
        }))
    });
    // The tire looked up first (a `WheeledVehicleTire` by name or id), then the index; mask 0x80.
    register_method(vm, "WheeledVehicle", "setWheelTire", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let Some(tire) = vm
            .objects()
            .find(arg(c, 3))
            .filter(|t| is_class(vm, *t, "WheeledVehicleTire"))
        else {
            vm.warn(format!(
                "setWheelTire: {} datablock does not exist (or is not a tire)",
                String::from_utf8_lossy(ts_vm::text::until_nul(arg(c, 3)))
            ));
            return Native::Bool(false);
        };
        let Some(i) = wheel_index(vm, id, c, "setWheelTire") else {
            return Native::Bool(false);
        };
        let t = tire_data(vm, tire);
        Native::Bool(on_wheeled(vm, id, |v, d| {
            wheeled::set_wheel_tire(v, d, i, (ObjectId(tire), t))
        }))
    });
    register_method(vm, "WheeledVehicle", "setWheelSpring", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let Some(spring) = vm
            .objects()
            .find(arg(c, 3))
            .filter(|t| is_class(vm, *t, "WheeledVehicleSpring"))
        else {
            vm.warn(format!(
                "setWheelSpring: {} datablock does not exist (or is not a spring)",
                String::from_utf8_lossy(ts_vm::text::until_nul(arg(c, 3)))
            ));
            return Native::Bool(false);
        };
        let Some(i) = wheel_index(vm, id, c, "setWheelSpring") else {
            return Native::Bool(false);
        };
        let s = super::vehicle::spring_data(vm, spring);
        Native::Bool(on_wheeled(vm, id, |v, d| {
            wheeled::set_wheel_spring(v, d, i, (ObjectId(spring), s))
        }))
    });
    register_method(vm, "WheeledVehicle", "getWheelCount", |vm, c| {
        let n = c.this_object.map_or(0, |id| hubs(vm, id));
        Native::Int(i32::try_from(n).unwrap_or(0))
    });
    register_method(vm, "FlyingVehicle", "useCreateHeight", |vm, c| {
        if let Some(id) = c.this_object {
            let on = atob(arg(c, 2));
            with_body(vm, id, |b, _| {
                if let Body::Flying(v) = b {
                    vehicle::flying::use_create_height(v, on);
                }
            });
        }
        Native::Void
    });
}

// ------------------------------------------------------------------------------------------------------------- the driven tail

/// The mount as a WheeledVehicle (`sim::player`'s driver path; `updateMove`'s head code asks it first): its steering.
pub fn wheeled_mount(vm: &Vm, id: ObjId) -> Option<[f32; 2]> {
    let e = ext_of(vm, id)?;
    if !matches!(e.body.as_ref(), Some(Body::Wheeled(_))) && !is_class(vm, id, "WheeledVehicle") {
        return None;
    }
    let s = e.state()?;
    let Some(Data::Wheeled(..)) = data(vm, id) else {
        return None;
    };
    Some(s.steering)
}

/// The mount as a FlyingVehicle for the head copy of `updateMove` (the cast is to the base `Vehicle`): its steering and no
/// strafe steering (the driver's cast to WheeledVehicle fails for it).
pub fn vehicle_mount(vm: &Vm, id: ObjId) -> Option<[f32; 2]> {
    let s = ext_of(vm, id)?.state()?;
    Some(s.steering)
}

/// Whether the connection whose control object is `ctl` controls vehicle `id`: the vehicle is in its control chain, or the
/// vehicle's controller (a driving Player) is (`setControllingClient` hands the driver's client to the vehicle).
pub fn driven_by(vm: &Vm, ctl: ObjId, id: ObjId) -> bool {
    let chain = crate::scene::camera::chain(vm, ctl);
    chain.contains(&id)
        || super::mount::controlling_object(vm, id).is_some_and(|holder| chain.contains(&holder))
}

/// The state's world translation, for the callers that read a vehicle's position (`getPosition` reads the VM's copy).
pub fn position(vm: &Vm, id: ObjId) -> Option<F3> {
    state(vm, id).map(vehicle::VehicleState::position)
}
