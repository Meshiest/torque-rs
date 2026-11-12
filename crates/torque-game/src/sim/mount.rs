//! Mounting, for every `ShapeBase`: `ShapeBase::mountObject`,
//! `unmountObject`, the `onMount`/`onUnmount` hooks (`ShapeBase`'s and `Player`'s) and the
//! console methods over them.
//!
//! **State.** [`MountState`] in the object's `ObjectExt` is the one truth: what the object is mounted on, its node
//! and the objects mounted on it (newest first, linked through). A Player's `PlayerState::mount`/`mount_node` are a copy
//! the tick refreshes ([`super::player::sync_mount`]).
//!
//! **Deletes.** A deleted mount unmounts every object on it (their `onDeleteNotify`), a deleted rider leaves its mount's list
//! (`ShapeBase::onRemove`'s `unmount`); both run the `onUnmount` script.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/shapeBase.cpp`
//! and `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this
//! crate's own.

use ts_vm::sim::ObjId;
use ts_vm::vm::{Native, Vm, VmError};

use crate::methods::register_method;
use crate::scene::camera::is_class;
use crate::scene::ext;
use crate::scene::ghost as ghosts;

/// The mount chain the cycle check of `mountObject` walks, at most (the check keeps the chain acyclic;
/// this bounds it whatever happens).
pub const SIM_MOUNT_CHAIN: usize = 1024;
/// The mount nodes (`ShapeBaseData::NumMountPoints`: the node argument is taken below 32).
pub const MOUNT_NODES: u32 = 32;

/// An object's mount links.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountState {
    /// The object this one is mounted on.
    pub on: Option<ObjId>,
    /// Its node (kept after an unmount: `onUnmount` is told it).
    pub node: u32,
    /// The objects mounted on this one, the most recent first.
    pub riders: Vec<ObjId>,
}

/// What `id` is mounted on and its node (no copy of the rider list).
pub fn link(vm: &Vm, id: ObjId) -> (Option<ObjId>, u32) {
    ext::get(vm.objects(), id).map_or((None, 0), |e| (e.mount.on, e.mount.node))
}

/// The objects mounted on `id`, the most recent first.
pub fn riders(vm: &Vm, id: ObjId) -> &[ObjId] {
    ext::get(vm.objects(), id).map_or(&[], |e| e.mount.riders.as_slice())
}

/// One unit of work per rider a method walks; `false` when the budget refuses.
fn pay_riders(vm: &Vm, id: ObjId) -> bool {
    vm.charge_work(riders(vm, id).len() as u64)
}

/// What `id` is mounted on.
pub fn mount_of(vm: &Vm, id: ObjId) -> Option<ObjId> {
    ext::get(vm.objects(), id)
        .and_then(|e| e.mount.on)
        .filter(|m| vm.objects().contains(*m))
}

fn mount_mut(vm: &mut Vm, id: ObjId) -> Option<&mut MountState> {
    ext::get_mut(vm.objects_mut(), id).map(|e| &mut e.mount)
}

/// The object that controls `id`: a Camera's or Player's control link, or the generic one.
pub fn controlling_object(vm: &Vm, id: ObjId) -> Option<ObjId> {
    let e = ext::get(vm.objects(), id)?;
    e.camera
        .as_ref()
        .and_then(|c| c.controlling_object)
        .or(e.controlled_by)
        .filter(|o| vm.objects().contains(*o))
}

/// `ShapeBase::mountObject(obj, node)` on `mount`: refused when `obj` is `mount` or carries it (the chain up from `mount`
/// meets `obj`), or (engine-only) when the scene hierarchy refuses it ([`crate::scene::links::mount_refused`]: `obj` has a
/// parent, or carries `mount` through parents); `obj` leaves its old mount first; it joins the head of the list with its node (0 for 32 and above), follows
/// `mount` in the process list unless `mount`'s controlling object is `obj`, is marked 0x80 and its `onMount` runs.
pub fn mount_object(vm: &mut Vm, mount: ObjId, obj: ObjId, node: u32) -> Result<(), VmError> {
    if crate::scene::links::mount_refused(vm, mount, obj) {
        return Ok(());
    }
    let mut at = Some(mount);
    let mut steps = 0;
    while let Some(m) = at {
        if m == obj || steps >= SIM_MOUNT_CHAIN {
            return Ok(());
        }
        if !vm.charge_work(1) {
            return Err(VmError::StepLimit);
        }
        steps += 1;
        at = ext::get(vm.objects(), m).and_then(|e| e.mount.on);
    }
    if let Some(old) = ext::get(vm.objects(), obj).and_then(|e| e.mount.on) {
        unmount_object(vm, old, obj)?;
    }
    if !pay_riders(vm, mount) {
        return Err(VmError::StepLimit);
    }
    let node = if node < MOUNT_NODES { node } else { 0 };
    if let Some(m) = mount_mut(vm, obj) {
        m.on = Some(mount);
        m.node = node;
    }
    if let Some(m) = mount_mut(vm, mount) {
        m.riders.retain(|r| *r != obj);
        m.riders.insert(0, obj);
    }
    if controlling_object(vm, mount) != Some(obj) {
        crate::sim::process_after(vm, obj, Some(mount));
    }
    super::player::sync_mount(vm, obj);
    ghosts::mark(vm, obj, 0x80);
    on_mount(vm, obj, mount, node, true)
}

/// `ShapeBase::unmountObject(obj)` on `mount`: nothing unless `obj` is mounted on it; it leaves the list, lets go of its
/// process-list edge (unless `mount`'s controlling object is `obj`), is marked 0x80 and its `onUnmount` runs with the node it
/// had.
pub fn unmount_object(vm: &mut Vm, mount: ObjId, obj: ObjId) -> Result<(), VmError> {
    if ext::get(vm.objects(), obj).and_then(|e| e.mount.on) != Some(mount) {
        return Ok(());
    }
    if !pay_riders(vm, mount) {
        return Err(VmError::StepLimit);
    }
    if let Some(m) = mount_mut(vm, mount) {
        m.riders.retain(|r| *r != obj);
    }
    if controlling_object(vm, mount) != Some(obj) {
        crate::sim::clear_after(vm, obj);
    }
    let node = match mount_mut(vm, obj) {
        Some(m) => {
            m.on = None;
            m.node
        }
        None => 0,
    };
    super::player::sync_mount(vm, obj);
    ghosts::mark(vm, obj, 0x80);
    on_mount(vm, obj, mount, node, false)
}

/// `onMount` (`ShapeBase`'s: the water coverage cleared, then `%db.onMount(%obj, %mount, %node)`) and `onUnmount`
/// (a Player's first plays action 0 forward, then `%db.onUnmount(%obj, %mount, %node)`).
fn on_mount(
    vm: &mut Vm,
    obj: ObjId,
    mount: ObjId,
    node: u32,
    mounting: bool,
) -> Result<(), VmError> {
    if mounting {
        if let Some(s) = super::player::state_mut(vm, obj) {
            s.water_coverage = 0.0;
        }
    } else if crate::scene::shapebase::is_player(vm, obj) {
        super::player::play_action(vm, obj, 0, true, false, false, false);
    }
    let Some(db) = crate::scene::access::object_field(vm, obj, b"dataBlock") else {
        return Ok(());
    };
    let name: &[u8] = if mounting { b"onMount" } else { b"onUnmount" };
    vm.call_method(
        db,
        name,
        &[
            obj.to_string().into_bytes(),
            mount.to_string().into_bytes(),
            node.to_string().into_bytes(),
        ],
    )?;
    Ok(())
}

/// `ShapeBase::onRemove`'s part: the object leaves its mount, and every object mounted on it is unmounted (their delete
/// notices).
pub fn removed(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    if let Some(m) = ext::get(vm.objects(), id).and_then(|e| e.mount.on) {
        unmount_object(vm, m, id)?;
    }
    let riders = ext::get(vm.objects(), id).map_or_else(Vec::new, |e| e.mount.riders.clone());
    for r in riders {
        if vm.objects().contains(r) {
            unmount_object(vm, id, r)?;
        }
    }
    Ok(())
}

/// `sscanf(s, "%d")`: the number when the text starts (after blanks and a sign) with a digit, else nothing.
pub fn scan_int(s: &[u8]) -> Option<i32> {
    let mut v = [0];
    (ts_vm::fields::scan_ints(s, &mut v) == 1).then_some(v[0])
}

pub fn id_value(id: Option<ObjId>) -> i32 {
    id.map_or(0, |id| i32::try_from(id).unwrap_or(0))
}

fn shape_base(vm: &Vm, name: &[u8]) -> Option<ObjId> {
    vm.objects()
        .find(name)
        .filter(|t| is_class(vm, *t, "ShapeBase"))
}

fn fail(r: Result<(), VmError>) -> Option<Native> {
    r.err().map(Native::Fail)
}

/// The console methods.
pub fn register(vm: &mut Vm) {
    // `mountObject(obj, node)`: the object by name or id (a ShapeBase or a SpotLight, else false); the node by `%d` (-1 when
    // unparsed); a node 0..31 mounts. True whenever the object was found, except (engine-only) false when the scene hierarchy
    // refuses the mount.
    register_method(vm, "ShapeBase", "mountObject", |vm, c| {
        let Some(this) = c.this_object else {
            return Native::Bool(false);
        };
        // A SpotLight (the engine's own, after Torque3D's `SceneObject::mountObject`) rides the node; nothing of the shape changes.
        if let Some(light) = vm.objects().find(c.arg(2))
            && crate::scene::lights::is_spot(vm, light)
        {
            if crate::scene::links::mount_refused(vm, this, light) {
                return Native::Bool(false);
            }
            let node = scan_int(c.arg(3)).unwrap_or(-1);
            if let Ok(node) = u32::try_from(node)
                && node < MOUNT_NODES
            {
                crate::scene::lights::mount_spot(vm, light, Some((this, node)));
            }
            return Native::Bool(true);
        }
        let Some(obj) = shape_base(vm, c.arg(2)) else {
            return Native::Bool(false);
        };
        if crate::scene::links::mount_refused(vm, this, obj) {
            return Native::Bool(false);
        }
        let node = scan_int(c.arg(3)).unwrap_or(-1);
        if let Ok(node) = u32::try_from(node)
            && node < MOUNT_NODES
            && let Some(f) = fail(mount_object(vm, this, obj, node))
        {
            return f;
        }
        Native::Bool(true)
    });
    // `unmountObject(obj)`.
    register_method(vm, "ShapeBase", "unmountObject", |vm, c| {
        let Some(this) = c.this_object else {
            return Native::Bool(false);
        };
        if let Some(light) = vm.objects().find(c.arg(2))
            && crate::scene::lights::is_spot(vm, light)
        {
            if crate::scene::lights::spot_mount(vm, light).is_some_and(|(on, _)| on == this) {
                crate::scene::lights::mount_spot(vm, light, None);
            }
            return Native::Bool(true);
        }
        let Some(obj) = shape_base(vm, c.arg(2)) else {
            return Native::Bool(false);
        };
        if let Some(f) = fail(unmount_object(vm, this, obj)) {
            return f;
        }
        Native::Bool(true)
    });
    // `unmount()`.
    register_method(vm, "ShapeBase", "unmount", |vm, c| {
        if let Some(this) = c.this_object
            && let Some(m) = ext::get(vm.objects(), this).and_then(|e| e.mount.on)
            && let Some(f) = fail(unmount_object(vm, m, this))
        {
            return f;
        }
        Native::Void
    });
    register_method(vm, "ShapeBase", "isMounted", |vm, c| {
        Native::Bool(c.this_object.and_then(|id| mount_of(vm, id)).is_some())
    });
    register_method(vm, "ShapeBase", "getObjectMount", |vm, c| {
        Native::Int(id_value(c.this_object.and_then(|id| mount_of(vm, id))))
    });
    register_method(vm, "ShapeBase", "getMountedObjectCount", |vm, c| {
        let n = c.this_object.map_or(0, |id| riders(vm, id).len());
        Native::Int(i32::try_from(n).unwrap_or(i32::MAX))
    });
    // `getMountedObject(i)`: the i-th of the list (a negative index 0), 0 past its end.
    register_method(vm, "ShapeBase", "getMountedObject", |vm, c| {
        let i = c.int(2);
        let found = c.this_object.and_then(|id| {
            let i = usize::try_from(i).ok()?;
            riders(vm, id).get(i).copied()
        });
        Native::Int(id_value(found))
    });
    // `getMountedObjectNode(i)`: its node, -1 past the end or for a negative index.
    register_method(vm, "ShapeBase", "getMountedObjectNode", |vm, c| {
        let i = c.int(2);
        let found = c.this_object.and_then(|id| {
            let i = usize::try_from(i).ok()?;
            let r = riders(vm, id).get(i).copied()?;
            Some(link(vm, r).1)
        });
        Native::Int(found.map_or(-1, |n| i32::try_from(n).unwrap_or(-1)))
    });
    // `getMountNodeObject(node)`: the first object of the list on that node.
    register_method(vm, "ShapeBase", "getMountNodeObject", |vm, c| {
        let node = c.int(2);
        let Some(id) = c.this_object else {
            return Native::Int(0);
        };
        if !pay_riders(vm, id) {
            return Native::Fail(VmError::StepLimit);
        }
        let found = riders(vm, id)
            .iter()
            .copied()
            .find(|r| i64::from(link(vm, *r).1) == i64::from(node));
        Native::Int(id_value(found))
    });
}
