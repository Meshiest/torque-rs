//! What the Item's and the Projectile's tick hosts share: the `Vm` as a step holds it ([`Slot`], the `ContainerWorld` borrow
//! contract: the container is built the first time a phase asks for the world and dropped before any script runs), the
//! container's ray with the hit's type mask, and the argument texts of their callbacks.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/shapeBase.cpp`
//! (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use collide::World;
use collide::math::{F3, sqrt_f32};
use ts_vm::sim::ObjId;
use ts_vm::text::format_fixed;
use ts_vm::vm::{Vm, VmError};

use super::player::tick::SimEnv;
use crate::scene::access::object_field;
use crate::scene::container_world::ContainerWorld;
use crate::scene::ext;

/// The `Vm` as a step holds it.
pub enum Slot<'a> {
    Vm(&'a mut Vm),
    World(ContainerWorld<'a>),
    Moving,
}

impl<'a> Slot<'a> {
    /// The plain `Vm` (a container built for a query is dropped first).
    pub fn vm(&mut self) -> &mut Vm {
        if matches!(self, Slot::World(_)) {
            let w = std::mem::replace(self, Slot::Moving);
            *self = Slot::Vm(w.into_vm());
        }
        match self {
            Slot::Vm(vm) => vm,
            _ => unreachable!("made plain just above"),
        }
    }

    /// A read of the `Vm` in either form.
    pub fn read<R>(&self, f: impl FnOnce(&Vm) -> R) -> R {
        match self {
            Slot::Vm(vm) => f(vm),
            Slot::World(w) => w.with_vm(f),
            Slot::Moving => unreachable!("the slot is refilled before it is read"),
        }
    }

    /// The container over the `Vm` (built on the first ask of a phase).
    pub fn world(&mut self, env: &'a SimEnv) -> &mut dyn World {
        if matches!(self, Slot::Vm(_)) {
            let Slot::Vm(vm) = std::mem::replace(self, Slot::Moving) else {
                unreachable!("tested just above");
            };
            *self = Slot::World(ContainerWorld::new(vm, &env.container, &env.shapes));
        }
        match self {
            Slot::World(w) => w,
            _ => unreachable!("made a world just above"),
        }
    }

    pub fn into_vm(self) -> &'a mut Vm {
        match self {
            Slot::Vm(vm) => vm,
            Slot::World(w) => w.into_vm(),
            Slot::Moving => unreachable!("the slot is refilled before it is read"),
        }
    }
}

/// `Container::castRay` over the server's container: the nearest hit with the type mask its object has now.
pub fn container_ray(
    vm: &mut Vm,
    env: &SimEnv,
    start: F3,
    end: F3,
    mask: u32,
) -> Result<Option<::sim::RayHit>, VmError> {
    let hit = crate::scene::container::container_ray(
        vm,
        &env.container,
        Some(&env.shapes),
        start,
        end,
        mask,
    )?;
    Ok(hit.map(|h| ::sim::RayHit {
        object: collide::ObjectId(h.object),
        point: h.point,
        normal: h.normal,
        type_mask: ext::get(vm.objects(), h.object).map_or(0, |e| e.type_mask),
    }))
}

/// `"%f %f %f"` of three floats widened.
pub fn text3_fixed(v: F3) -> Vec<u8> {
    v.iter()
        .map(|x| format_fixed(f64::from(*x), 6))
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes()
}

/// `ShapeBase::onCollision`'s two texts: the vector `"%f %f %f"` and its length `mSqrt(f32((x*x + y*y) + z*z))` as `"%f"`.
pub fn vector_args(v: F3) -> [Vec<u8>; 2] {
    let len = sqrt_f32(collide::math::dot(v, v));
    [text3_fixed(v), format_fixed(f64::from(len), 6).into_bytes()]
}

/// `%db.<name>(%obj, args...)` on the object's datablock (nothing for an object without one).
pub fn call_db(vm: &mut Vm, id: ObjId, name: &[u8], args: &[Vec<u8>]) -> Result<(), VmError> {
    let Some(db) = object_field(vm, id, b"dataBlock") else {
        return Ok(());
    };
    let mut argv = vec![id.to_string().into_bytes()];
    argv.extend_from_slice(args);
    vm.call_method(db, name, &argv)?;
    Ok(())
}

/// The collision-disable count of an object up (`disableCollision`) or down when it is not zero (`enableCollision`).
pub fn switch_collision(vm: &mut Vm, id: ObjId, disable: bool) {
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.collision_off = if disable {
            e.collision_off.saturating_add(1)
        } else {
            e.collision_off.saturating_sub(1)
        };
    }
}
