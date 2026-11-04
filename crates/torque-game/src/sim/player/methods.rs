//! The Player's console methods, and the ShapeBase ones that read a Player's own words (an AIPlayer's are its AI's).
//!
//! - `setVelocity`, `getVelocity`, `applyImpulse`: `ShapeBase::setVelocity` -> `Player::setVelocity` (what
//!   [`crate::library::GameLibrary::set_velocity`] keeps, mask); `"%g %g %g"`; `v + f32(J / mass)`
//! - `getState`: `Dead`, `Mounted`, `Recover`, `Move`
//! - `setControlObject`, `getControlObject`, `clearControlObject`: over `Player::setControlObject`
//! - `getEyeTransform`, `getEyePoint`, `getEyeVector`: over the eye transform (a Player's)
//!
//! An Item's are its own (`crate::sim::item`). Any other ShapeBase has no velocity here (`getVelocity`
//! is `0 0 0`, the setters do nothing); the vehicles' are in [`crate::sim::vehicle`].
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/player.cpp` and
//! `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this
//! crate's own.

use ::sim::player::PlayerState;
use collide::ObjectId;
use collide::math::{F3, Matrix};
use ts_vm::sim::ObjId;
use ts_vm::text::{atof, format_fixed, format_g, scan_floats, to_f32};
use ts_vm::vm::{Native, NativeCall, Vm};

use super::{state, state_mut};
use crate::methods::register_method;
use crate::scene::camera::{self, is_class};
use crate::scene::ext;
use crate::scene::ghost as ghosts;
use crate::scene::shapebase::is_player;

pub fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&b""[..], Vec::as_slice)
}

/// `sscanf(s, "%f %f %f")` into zeros.
pub fn vec3(c: &NativeCall, i: usize) -> F3 {
    let mut v = [0.0f32; 3];
    scan_floats(arg(c, i), &mut v);
    v
}

pub fn f(c: &NativeCall, i: usize) -> f32 {
    c.argv.get(i).map_or(0.0, |a| to_f32(atof(a)))
}

pub fn text3(v: F3, fixed: bool) -> Vec<u8> {
    v.iter()
        .map(|x| {
            if fixed {
                format_fixed(f64::from(*x), 6)
            } else {
                format_g(f64::from(*x))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes()
}

/// `Player::setVelocity`.
pub fn set_velocity(vm: &mut Vm, id: ObjId, v: F3) {
    if let Some(s) = state_mut(vm, id) {
        s.velocity = (crate::library::get().set_velocity)(v);
        ghosts::mark(vm, id, 0x200_0000);
    }
}

/// `Player::applyImpulse` before its `setVelocity`: per axis `J / mass` is a float first, and that float is added to the
/// velocity.
fn impulse_velocity(v: F3, j: F3, mass: f32) -> F3 {
    [0, 1, 2].map(|i| j[i] / mass + v[i])
}

/// `Player::applyImpulse`: `v + f32(J / mass)` per axis, then `setVelocity`.
pub fn apply_impulse(vm: &mut Vm, id: ObjId, j: F3) {
    let Some(s) = state(vm, id) else {
        return;
    };
    let n = impulse_velocity(s.velocity, j, s.mass);
    set_velocity(vm, id, n);
}

// ------------------------------------------------------------------------------------------------------------------- control

/// `setControllingObject`: the object that controls `obj`, a Camera's in its camera state. For any other
/// ShapeBase, `ShapeBase::setControllingObject` also orders the process list: a holder clears the object's process flag (the
/// list passes the object by) and makes the object process after it (`processAfter`, the list dirty); letting go sets the flag
/// again, drops the object's own edge (no dirty list) and, when the old holder is mounted on the object, makes the holder
/// process after it again. In the pass where a driver dismounts, the vehicle (after its driver) is therefore ticked by the list
/// too.
pub fn set_controlling(vm: &mut Vm, obj: ObjId, holder: Option<ObjId>) {
    let mut shape_base = false;
    let mut old = None;
    if let Some(e) = ext::get_mut(vm.objects_mut(), obj) {
        match e.camera.as_mut() {
            Some(c) => c.controlling_object = holder,
            None => {
                shape_base = true;
                old = e.controlled_by;
                e.controlled_by = holder;
            }
        }
    }
    if shape_base {
        match holder {
            Some(h) => crate::sim::process_after(vm, obj, Some(h)),
            None => {
                crate::sim::clear_after(vm, obj);
                if let Some(o) = old.filter(|o| crate::sim::mount::mount_of(vm, *o) == Some(obj)) {
                    crate::sim::process_after(vm, o, Some(obj));
                }
            }
        }
    }
    ghosts::mark(vm, obj, 0x100);
}

/// `setControlObject(0)` on whatever controls an object (a Camera or a Player).
pub fn release_holder(vm: &mut Vm, holder: ObjId) {
    if camera::is_camera(vm, holder) {
        camera::set_control_object(vm, holder, None);
    } else if is_player(vm, holder) {
        set_control_object(vm, holder, None);
    }
}

/// `Player::setControlObject(obj)`: the old one is let go; a new one (not this Player) is first let go by what controls it
/// and by its connection, then held, and told its controller.
pub fn set_control_object(vm: &mut Vm, id: ObjId, obj: Option<ObjId>) {
    let old = state(vm, id).and_then(|s| s.control_object).map(|o| o.0);
    if let Some(old) = old.filter(|o| vm.objects().contains(*o)) {
        set_controlling(vm, old, None);
    }
    let new = obj.filter(|o| *o != id);
    if let Some(new) = new {
        if let Some(holder) = crate::sim::mount::controlling_object(vm, new).filter(|h| *h != id) {
            release_holder(vm, holder);
        }
        ghosts::release_controlling_connection(vm, new);
        set_controlling(vm, new, Some(id));
    }
    if let Some(s) = state_mut(vm, id) {
        s.control_object = new.map(ObjectId);
    }
}

/// `ShapeBase::onRemove`'s control links: the Player that held the object lets it go, and an object this Player held loses its
/// controller (their delete notices).
pub fn control_removed(vm: &mut Vm, id: ObjId) {
    if let Some(holder) = crate::sim::mount::controlling_object(vm, id)
        && let Some(s) = state_mut(vm, holder)
        && s.control_object == Some(ObjectId(id))
    {
        s.control_object = None;
    }
    if let Some(c) = state(vm, id).and_then(|s| s.control_object).map(|o| o.0)
        && vm.objects().contains(c)
    {
        set_controlling(vm, c, None);
    }
}

// ------------------------------------------------------------------------------------------------------------------- eye

/// The eye transform of a ShapeBase: a Player's `getEyeTransform`, else `ShapeBase`'s (the eye node after the transform, or
/// the transform alone).
pub fn eye_transform(vm: &mut Vm, id: ObjId) -> Matrix {
    if state(vm, id).is_some()
        && let Some(m) = super::tick::eye_of(vm, id)
    {
        return m;
    }
    let eye = crate::scene::shapebase::state(vm, id)
        .and_then(|s| s.body.as_ref())
        .and_then(|b| b.kept.facts.as_ref())
        .map_or(-1, |f| f.eye_node);
    // The eye node's matrix is a mount-style product; with no eye node the object's transform.
    let object = crate::scene::access::matrix_of(vm, id).unwrap_or(collide::math::IDENTITY);
    let node = crate::scene::shapebase::state(vm, id)
        .and_then(|s| s.body.as_ref())
        .and_then(|b| {
            b.instance
                .node_transform(usize::try_from(eye).ok()?)
                .copied()
        });
    match node {
        Some(mut n) => {
            let s = crate::scene::access::scale_of(vm, id);
            n[3] *= s[0];
            n[7] *= s[1];
            n[11] *= s[2];
            collide::math::matrix_product(&object, &n)
        }
        None => object,
    }
}

// ---------------------------------------------------------------------------------------------------------------- registration

/// `getState`.
fn player_state_name(s: &PlayerState) -> &'static str {
    if s.damage_state != 0 {
        "Dead"
    } else if s.mount.is_some() {
        "Mounted"
    } else if s.action_state == 2 {
        "Recover"
    } else {
        "Move"
    }
}

/// The water coverage of a ShapeBase (the Player's, the Item's and the vehicles' container update writes it; 0 from the
/// constructor, for every other class), for a host's own methods.
pub fn water_coverage(vm: &Vm, id: ObjId) -> f32 {
    if let Some(s) = state(vm, id) {
        s.water_coverage
    } else if let Some(s) = crate::sim::item::state(vm, id) {
        s.water_coverage
    } else if let Some(s) = crate::sim::vehicle::state(vm, id) {
        s.water_coverage
    } else {
        0.0
    }
}

pub fn register(vm: &mut Vm) {
    register_method(vm, "ShapeBase", "setVelocity", |vm, c| {
        if let Some(id) = c.this_object {
            let v = (crate::library::get().set_velocity)(vec3(c, 2));
            if crate::sim::item::is_item(vm, id) {
                crate::sim::item::set_velocity(vm, id, v);
            } else if crate::sim::vehicle::is_vehicle(vm, id) {
                crate::sim::vehicle::set_velocity(vm, id, v);
            } else {
                set_velocity(vm, id, v);
            }
        }
        Native::Bool(true)
    });
    register_method(vm, "ShapeBase", "getVelocity", |vm, c| {
        let v = c
            .this_object
            .map_or([0.0; 3], |id| crate::sim::item::object_velocity(vm, id));
        Native::Str(text3(v, false))
    });
    register_method(vm, "ShapeBase", "applyImpulse", |vm, c| {
        if let Some(id) = c.this_object {
            let pos = vec3(c, 2);
            if crate::sim::item::is_item(vm, id) {
                crate::sim::item::apply_impulse(vm, id, vec3(c, 3));
            } else if crate::sim::vehicle::is_vehicle(vm, id) {
                crate::sim::vehicle::apply_impulse(vm, id, pos, vec3(c, 3));
            } else {
                apply_impulse(vm, id, vec3(c, 3));
            }
        }
        Native::Bool(true)
    });
    register_method(vm, "ShapeBase", "getEyeTransform", |vm, c| {
        let m = c.this_object.map(|id| eye_transform(vm, id));
        Native::Str(m.map_or_else(Vec::new, |m| {
            crate::scene::object::transform_text(crate::geometry::of(vm), &m)
        }))
    });
    register_method(vm, "ShapeBase", "getEyePoint", |vm, c| {
        let m = c.this_object.map(|id| eye_transform(vm, id));
        Native::Str(m.map_or_else(Vec::new, |m| text3([m[3], m[7], m[11]], false)))
    });
    register_method(vm, "ShapeBase", "getEyeVector", |vm, c| {
        let m = c.this_object.map(|id| eye_transform(vm, id));
        Native::Str(m.map_or_else(Vec::new, |m| text3([m[1], m[5], m[9]], false)))
    });
    register_method(vm, "Player", "getState", |vm, c| {
        let name = c
            .this_object
            .and_then(|id| state(vm, id))
            .map_or("", player_state_name);
        Native::Str(name.as_bytes().to_vec())
    });
    // `setControlObject(obj)`: a ShapeBase by name or id is held (true); anything else lets go (false).
    register_method(vm, "Player", "setControlObject", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let target = vm
            .objects()
            .find(arg(c, 2))
            .filter(|t| is_class(vm, *t, "ShapeBase"));
        set_control_object(vm, id, target);
        Native::Bool(target.is_some())
    });
    // `getControlObject()`: the id as a float, 0 for none.
    register_method(vm, "Player", "getControlObject", |vm, c| {
        let o = c
            .this_object
            .and_then(|id| state(vm, id))
            .and_then(|s| s.control_object)
            .map(|o| o.0)
            .filter(|o| vm.objects().contains(*o));
        Native::Float(f64::from(o.unwrap_or(0)))
    });
    register_method(vm, "Player", "clearControlObject", |vm, c| {
        if let Some(id) = c.this_object {
            set_control_object(vm, id, None);
        }
        Native::Void
    });
    register_extras(vm);
}

/// A plane through `point` with the normal `n` made unit, `d` the library's
/// [`plane_offset`](crate::library::GameLibrary::plane_offset).
fn plane_at(point: F3, normal: F3) -> collide::math::Plane {
    let n = collide::math::normalize(normal);
    let offset = (crate::library::get().plane_offset)(point, n);
    collide::math::Plane::new(n[0], n[1], n[2], offset)
}

/// `Player::checkDismountPosition(oldMat, mat)` for the console method (each point `sscanf`'d into an identity
/// matrix's column 3): the Player and its mount out of collision (a missing mount is skipped), a container ray from the old
/// point to the new with `sCollisionMoveMask` (the host's [`player_move_collide`](collide::TypeMasks::player_move_collide);
/// a hit: false), then the box `objBox * scale + pos` (the class's
/// [`dismount_box_scale`](crate::sim_classes::SimClasses::dismount_box_scale), TGE's whole box by default) with its six
/// planes (-x at min, +y at max, +x at max, -y at min, -z at min, +z at max) fed by the objects of the mask in it, early out
/// (any polygon: false); both back in collision. Work charged through the container; a refused charge answers false.
fn check_dismount_point(vm: &mut Vm, id: ObjId, old: F3, pos: F3) -> bool {
    use collide::{PolyList as _, World as _};
    let Some(env) = crate::sim::vehicle::env_of(vm, id) else {
        return false;
    };
    let Some(scene) = ext::get(vm.objects(), id).and_then(|e| e.scene) else {
        return false;
    };
    let mask = crate::masks::type_masks(vm).player_move_collide;
    let box_scale = crate::sim_classes::sim_classes(vm).dismount_box_scale();
    let mount = crate::sim::mount::mount_of(vm, id);
    let out: Vec<ObjId> = std::iter::once(id).chain(mount).collect();
    for &o in &out {
        crate::sim::hosted::switch_collision(vm, o, true);
    }
    let clear = (|| {
        let hit = crate::scene::container::container_ray(
            vm,
            &env.container,
            Some(&env.shapes),
            old,
            pos,
            mask,
        );
        match hit {
            Ok(None) => {}
            _ => return false,
        }
        let lo = scene.object_min;
        let hi = scene.object_max;
        let min = [0, 1, 2].map(|i| lo[i] * box_scale + pos[i]);
        let max = [0, 1, 2].map(|i| hi[i] * box_scale + pos[i]);
        let mut early = collide::poly::EarlyOutPolyList::new();
        early.planes.extend_from_slice(&[
            plane_at(min, [-1.0, 0.0, 0.0]),
            plane_at(max, [0.0, 1.0, 0.0]),
            plane_at(max, [1.0, 0.0, 0.0]),
            plane_at(min, [0.0, -1.0, 0.0]),
            plane_at(min, [0.0, 0.0, -1.0]),
            plane_at(max, [0.0, 0.0, 1.0]),
        ]);
        early.normal = [0.0; 3];
        let query = collide::math::Box3::new(min, max);
        let mut world =
            crate::scene::container_world::ContainerWorld::new(vm, &env.container, &env.shapes);
        let mut ids = Vec::new();
        if world
            .find_objects(
                &query,
                mask,
                None,
                collide::convex::MAX_WORKING_CANDIDATES,
                &mut ids,
            )
            .is_err()
        {
            return false;
        }
        let mut working = collide::WorkingSet::default();
        for obj in ids {
            working.clear();
            if working.build_convex(&mut world, obj, &query).is_err() {
                return false;
            }
            for c in working.iter() {
                if c.poly_list(&mut world, &mut early).is_err() {
                    return false;
                }
                if early.early_out {
                    return false;
                }
            }
        }
        early.is_empty()
    })();
    for &o in &out {
        crate::sim::hosted::switch_collision(vm, o, false);
    }
    clear
}

/// The methods a host's Player scripts call from the callbacks: `checkDismountPoint`, `setDamageFlash`/`getDamageFlash` and
/// `playAudio`.
fn register_extras(vm: &mut Vm) {
    // `checkDismountPoint(oldPos, pos)`: a script's dismount tries its points with it.
    register_method(vm, "Player", "checkDismountPoint", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let (old, pos) = (vec3(c, 2), vec3(c, 3));
        Native::Bool(check_dismount_point(vm, id, old, pos))
    });
    // `setDamageFlash(v)`: below 0 is 0, above 1 is 1 (a NaN is kept).
    register_method(vm, "ShapeBase", "setDamageFlash", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Void;
        };
        let v = f(c, 2);
        // `f32::clamp` keeps a NaN, as the two compares do.
        let v = v.clamp(0.0, 1.0);
        if let Some(s) = state_mut(vm, id) {
            s.damage_flash = v;
        } else if let Some(cam) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.camera.as_mut())
        {
            cam.flash = v;
        }
        Native::Void
    });
    register_method(vm, "ShapeBase", "getDamageFlash", |vm, c| {
        let v = c.this_object.map_or(0.0, |id| match state(vm, id) {
            Some(s) => s.damage_flash,
            None => ext::get(vm.objects(), id)
                .and_then(|e| e.camera.as_ref())
                .map_or(0.0, |cam| cam.flash),
        });
        Native::Float(f64::from(v))
    });
    // `playAudio(slot, profile)`: a slot past 3 is false; an `AudioProfile` by name or id is true (none is false); one with a
    // description that the slot is not already playing starts in the slot (mask 0x1000 << slot).
    register_method(vm, "ShapeBase", "playAudio", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let slot = c.argv.get(2).map_or(0, |a| ts_vm::text::atoi(a) as u32);
        if slot > 3 {
            return Native::Bool(false);
        }
        let Some(profile) = vm
            .objects()
            .find(arg(c, 3))
            .filter(|p| is_class(vm, *p, "AudioProfile"))
        else {
            return Native::Bool(false);
        };
        let described = crate::scene::access::object_field(vm, profile, b"description").is_some();
        if described
            && let Some(st) = ext::get_mut(vm.objects_mut(), id)
                .map(|e| &mut **e.shape.get_or_insert_with(Box::default))
            && st.sounds[slot as usize] != Some(profile)
        {
            st.sounds[slot as usize] = Some(profile);
            ghosts::mark(vm, id, 0x1000 << slot);
        }
        Native::Bool(true)
    });
}

#[cfg(test)]
#[path = "methods_tests.rs"]
mod tests;
