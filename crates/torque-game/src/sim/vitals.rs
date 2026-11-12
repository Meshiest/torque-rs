//! Damage and energy, for every `ShapeBase`: `setDamageLevel`, `setDamageState`, the Player's
//! `updateDamageLevel`/`updateDamageState`, `setEnergyLevel`,
//! `getEnergyLevel` and `getEnergyValue`, and the console methods over them.
//!
//! **Where the words live.** A Player's are its `PlayerState` (energy, recharge, damage, repair rate, reserve, state); a
//! Camera's energy is its `CameraExt`'s (the camera's tick steps it); every other ShapeBase keeps them
//! in [`Vitals`] (Item, StaticShape and the vehicles).
//!
//! **Scripts.** A changed damage level calls `%db.onDamage(%obj, "%f" delta)` (a server object, as every object here is); a
//! state change calls `onEnabled`, `onDisabled` (only from Enabled) or `onDestroyed` with the old state's name; the Player's
//! level update then sets the state from `maxDamage <= damage` again, so `setDamageLevel` over the maximum disables it with its
//! `onDisabled`.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/shapeBase.cpp`
//! and `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this
//! crate's own.

use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::text::format_fixed;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::methods::register_method;
use crate::scene::access::{object_field, value_named};
use crate::scene::camera::is_class;
use crate::scene::ext;
use crate::scene::ghost as ghosts;
use crate::scene::shapebase::is_player;

/// The state names.
pub const STATE_NAMES: [&str; 3] = ["Enabled", "Disabled", "Destroyed"];
/// `DamageMask` (0x20).
const DAMAGE_MASK: u32 = 0x20;
/// The Player and corpse type bits `updateDamageState` swaps.
const PLAYER_TYPE: u32 = 0x4000;
const CORPSE_TYPE: u32 = 0x10_0000;
/// The energy-inheritance chain `getEnergyLevel` walks, at most this deep (a mount chain is finite but may be long).
pub const SIM_ENERGY_CHAIN: usize = 1024;

/// A ShapeBase's damage and energy words.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vitals {
    pub energy: f32,
    pub recharge_rate: f32,
    pub damage: f32,
    pub repair_rate: f32,
    pub repair_reserve: f32,
    pub damage_state: u32,
}

/// The words of `id` now.
pub fn get(vm: &Vm, id: ObjId) -> Vitals {
    if let Some(s) = super::player::state(vm, id) {
        return Vitals {
            energy: s.energy,
            recharge_rate: s.recharge_rate,
            damage: s.damage,
            repair_rate: s.repair_rate,
            repair_reserve: s.repair_reserve,
            damage_state: s.damage_state,
        };
    }
    let Some(e) = ext::get(vm.objects(), id) else {
        return Vitals::default();
    };
    let mut v = e.vitals.unwrap_or_default();
    if let Some(c) = e.camera.as_ref() {
        v.energy = c.energy;
    }
    v
}

/// Stores the words of `id`.
pub fn put(vm: &mut Vm, id: ObjId, v: &Vitals) {
    if let Some(s) = super::player::state_mut(vm, id) {
        s.energy = v.energy;
        s.recharge_rate = v.recharge_rate;
        s.damage = v.damage;
        s.repair_rate = v.repair_rate;
        s.repair_reserve = v.repair_reserve;
        s.damage_state = v.damage_state;
        return;
    }
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        if let Some(c) = e.camera.as_mut() {
            c.energy = v.energy;
        }
        e.vitals = Some(*v);
    }
}

fn db_float(vm: &Vm, db: Option<ObjId>, name: &[u8], default: f32) -> f32 {
    match db.and_then(|db| value_named(vm, db, name)) {
        Some(Value::Float(x)) => x,
        Some(Value::Int(n)) => n as f32,
        _ => default,
    }
}

fn db_bool(vm: &Vm, db: Option<ObjId>, name: &[u8]) -> bool {
    matches!(
        db.and_then(|db| value_named(vm, db, name)),
        Some(Value::Bool(true))
    )
}

fn datablock(vm: &Vm, id: ObjId) -> Option<ObjId> {
    object_field(vm, id, b"dataBlock").filter(|d| is_class(vm, *d, "ShapeBaseData"))
}

/// The datablock's `maxEnergy`, `maxDamage`, `isInvincible` and `inheritEnergyFromMount`, read live.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_energy: f32,
    pub max_damage: f32,
    pub invincible: bool,
    pub inherit: bool,
}

pub fn limits(vm: &Vm, id: ObjId) -> Limits {
    let db = datablock(vm, id);
    Limits {
        max_energy: db_float(vm, db, b"maxEnergy", 0.0),
        max_damage: db_float(vm, db, b"maxDamage", 1.0),
        invincible: db_bool(vm, db, b"isInvincible"),
        inherit: db_bool(vm, db, b"inheritEnergyFromMount"),
    }
}

/// The first datablock of a ShapeBase: energy, damage, state and reserve 0; the recharge rate is the
/// datablock's (every datablock). A later datablock scales the damage and the energy by the new maximum over the old.
pub fn new_datablock(vm: &mut Vm, id: ObjId, old: Option<ObjId>, db: ObjId) {
    let recharge = db_float(vm, Some(db), b"rechargeRate", 0.0);
    let mut v = get(vm, id);
    match old.filter(|o| vm.objects().contains(*o)) {
        None => {
            v.energy = 0.0;
            v.damage = 0.0;
            v.damage_state = 0;
            v.repair_reserve = 0.0;
        }
        Some(old) => scale_words(&mut v, maxima(vm, old), maxima(vm, db)),
    }
    v.recharge_rate = recharge;
    put(vm, id, &v);
}

/// A datablock's `maxDamage` (1 unset) and `maxEnergy` (0 unset), the pair the words are scaled by.
pub fn maxima(vm: &Vm, db: ObjId) -> (f32, f32) {
    (
        db_float(vm, Some(db), b"maxDamage", 1.0),
        db_float(vm, Some(db), b"maxEnergy", 0.0),
    )
}

/// The damage and energy as the same share of new maxima (`onNewDataBlock`'s scaling; a share of a maximum of zero is zero).
fn scale_words(v: &mut Vitals, (old_damage, old_energy): (f32, f32), (damage, energy): (f32, f32)) {
    let ratio = |x: f32, max: f32| if max <= 0.0 { 0.0 } else { x / max };
    v.damage = damage * ratio(v.damage, old_damage);
    v.energy = energy * ratio(v.energy, old_energy);
}

/// The words scaled from the maxima `old` the object was running under to the ones `db` has now: the refresh of an object
/// whose own datablock was edited, where the old values are gone from the datablock and the caller kept them.
pub fn rescale(vm: &mut Vm, id: ObjId, old: (f32, f32), db: ObjId) {
    let new = maxima(vm, db);
    if old == new {
        return;
    }
    let mut v = get(vm, id);
    scale_words(&mut v, old, new);
    put(vm, id, &v);
}

/// `ShapeBase::setDamageLevel`: nothing for an invincible datablock; the level clamped (`max < v` takes the max, then
/// `v <= 0` takes 0; a NaN passes both); a change runs the level update, marks 0x20 and calls `onDamage` with the
/// difference.
pub fn set_damage_level(vm: &mut Vm, id: ObjId, value: f32) -> Result<(), VmError> {
    let l = limits(vm, id);
    if l.invincible {
        return Ok(());
    }
    let mut v = get(vm, id);
    let old = v.damage;
    let mut new = value;
    if l.max_damage < new {
        new = l.max_damage;
    }
    if new <= 0.0 {
        new = 0.0;
    }
    v.damage = new;
    put(vm, id, &v);
    // Only an ordered equal skips (a NaN on either side is a change).
    if old == new {
        return Ok(());
    }
    update_damage_level(vm, id)?;
    ghosts::mark(vm, id, DAMAGE_MASK);
    let Some(db) = datablock(vm, id) else {
        return Ok(());
    };
    let delta = (crate::library::get().difference)(new, old);
    vm.call_method(
        db,
        b"onDamage",
        &[
            id.to_string().into_bytes(),
            format_fixed(delta, 6).into_bytes(),
        ],
    )?;
    Ok(())
}

/// The level update: a Player's sets the state from `maxDamage <= damage`; the other classes' only move the
/// damage thread (not modelled).
pub fn update_damage_level(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    if is_player(vm, id) {
        let max = limits(vm, id).max_damage;
        let state = u32::from(max <= get(vm, id).damage);
        set_damage_state(vm, id, state)?;
    }
    Ok(())
}

/// The state update: a Player is a Player while enabled and a corpse otherwise (type bits 0x4000/0x100000).
pub fn update_damage_state(vm: &mut Vm, id: ObjId) {
    if !is_player(vm, id) {
        return;
    }
    let dead = get(vm, id).damage_state != 0;
    let fix = |m: u32| {
        if dead {
            (m & !PLAYER_TYPE) | CORPSE_TYPE
        } else {
            (m & !CORPSE_TYPE) | PLAYER_TYPE
        }
    };
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.type_mask = fix(e.type_mask);
    }
    if let Some(s) = super::player::state_mut(vm, id) {
        s.type_mask = fix(s.type_mask);
    }
}

/// `ShapeBase::setDamageState`: nothing for the same state; marks 0x20; Enabled calls `onEnabled`, Disabled `onDisabled`
/// (only from Enabled), Destroyed first disables (from Enabled) and calls `onDestroyed`; any state but Enabled clears the
/// reserve and the energy; the script is told the old state's name; then the state and level updates.
pub fn set_damage_state(vm: &mut Vm, id: ObjId, state: u32) -> Result<(), VmError> {
    let mut v = get(vm, id);
    let old = v.damage_state;
    if old == state {
        return Ok(());
    }
    ghosts::mark(vm, id, DAMAGE_MASK);
    let last = STATE_NAMES
        .get(old as usize)
        .copied()
        .unwrap_or("")
        .as_bytes()
        .to_vec();
    let script: Option<&[u8]> = match state {
        0 => Some(b"onEnabled"),
        1 => (old == 0).then_some(&b"onDisabled"[..]),
        2 => {
            if old == 0 {
                set_damage_state(vm, id, 1)?;
            }
            Some(b"onDestroyed")
        }
        _ => None,
    };
    v = get(vm, id);
    v.damage_state = state;
    if state != 0 {
        v.repair_reserve = 0.0;
        v.energy = 0.0;
    }
    put(vm, id, &v);
    if let Some(name) = script
        && let Some(db) = datablock(vm, id)
    {
        vm.call_method(db, name, &[id.to_string().into_bytes(), last])?;
    }
    update_damage_state(vm, id);
    update_damage_level(vm, id)
}

/// `ShapeBase::setEnergyLevel`: not inheriting: while enabled, `max < e` takes the max, `0 <= e` keeps it,
/// anything else (below 0, a NaN) is 0; a disabled object keeps its energy. Inheriting: the mount's `setEnergyLevel`.
pub fn set_energy_level(vm: &mut Vm, id: ObjId, e: f32) {
    let mut at = id;
    for _ in 0..SIM_ENERGY_CHAIN {
        // One unit a link (a refused budget stops the walk: the run ends with the step limit).
        if !vm.charge_work(1) {
            return;
        }
        let l = limits(vm, at);
        if !l.inherit {
            let mut v = get(vm, at);
            if v.damage_state == 0 {
                v.energy = if l.max_energy < e {
                    l.max_energy
                } else if 0.0 <= e {
                    e
                } else {
                    0.0
                };
                put(vm, at, &v);
            }
            return;
        }
        match super::mount::mount_of(vm, at) {
            Some(m) => at = m,
            None => return,
        }
    }
}

/// `getEnergyLevel`: the first object up the mount chain whose datablock does not inherit; 0 when the chain ends.
pub fn energy_level(vm: &Vm, id: ObjId) -> f32 {
    let mut at = id;
    for _ in 0..SIM_ENERGY_CHAIN {
        if !vm.charge_work(1) {
            return 0.0;
        }
        if !limits(vm, at).inherit {
            return get(vm, at).energy;
        }
        match super::mount::mount_of(vm, at) {
            Some(m) => at = m,
            None => return 0.0,
        }
    }
    0.0
}

/// `getEnergyValue`: the energy over `maxEnergy` (0 for a maximum not above 0); inheriting, the mount's level over the
/// mount's maximum.
pub fn energy_value(vm: &Vm, id: ObjId) -> f32 {
    let l = limits(vm, id);
    if !l.inherit {
        if 0.0 < l.max_energy {
            return get(vm, id).energy / l.max_energy;
        }
        return 0.0;
    }
    if let Some(m) = super::mount::mount_of(vm, id) {
        let max = limits(vm, m).max_energy;
        if 0.0 < max {
            return energy_level(vm, m) / max;
        }
    }
    0.0
}

fn arg_f(c: &NativeCall, i: usize) -> Option<f32> {
    (c.argc() > i).then(|| c.float(i))
}

fn fail(r: Result<(), VmError>) -> Native {
    match r {
        Ok(()) => Native::Void,
        Err(e) => Native::Fail(e),
    }
}

/// The console methods.
pub fn register(vm: &mut Vm) {
    register_method(vm, "ShapeBase", "setDamageLevel", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Void;
        };
        fail(set_damage_level(vm, id, arg_f(c, 2).unwrap_or(0.0)))
    });
    // `getDamageLevel` and `getDamagePercent`: -1 for a ghost (never here).
    register_method(vm, "ShapeBase", "getDamageLevel", |vm, c| {
        Native::Float(
            c.this_object
                .map_or(0.0, |id| f64::from(get(vm, id).damage)),
        )
    });
    register_method(vm, "ShapeBase", "getDamagePercent", |vm, c| {
        Native::Float(c.this_object.map_or(0.0, |id| {
            let max = limits(vm, id).max_damage;
            f64::from(get(vm, id).damage / max)
        }))
    });
    // `applyDamage(v)`: a positive amount is added to the level.
    register_method(vm, "ShapeBase", "applyDamage", |vm, c| {
        let (Some(id), Some(v)) = (c.this_object, arg_f(c, 2)) else {
            return Native::Void;
        };
        if 0.0 < v {
            let d = v + get(vm, id).damage;
            return fail(set_damage_level(vm, id, d));
        }
        Native::Void
    });
    // `applyRepair(v)`: a positive amount joins the reserve, which never exceeds the damage.
    register_method(vm, "ShapeBase", "applyRepair", |vm, c| {
        let (Some(id), Some(a)) = (c.this_object, arg_f(c, 2)) else {
            return Native::Void;
        };
        if 0.0 < a {
            let mut v = get(vm, id);
            v.repair_reserve += a;
            if v.damage < v.repair_reserve {
                v.repair_reserve = v.damage;
            }
            put(vm, id, &v);
        }
        Native::Void
    });
    // `setRepairRate(v)`: `0 <= v` is kept, anything else (a NaN, no argument) is 0.
    register_method(vm, "ShapeBase", "setRepairRate", |vm, c| {
        if let Some(id) = c.this_object {
            let a = arg_f(c, 2).filter(|a| 0.0 <= *a).unwrap_or(0.0);
            let mut v = get(vm, id);
            v.repair_rate = a;
            put(vm, id, &v);
        }
        Native::Void
    });
    register_method(vm, "ShapeBase", "getRepairRate", |vm, c| {
        Native::Float(
            c.this_object
                .map_or(0.0, |id| f64::from(get(vm, id).repair_rate)),
        )
    });
    register_method(vm, "ShapeBase", "getRechargeRate", |vm, c| {
        Native::Float(
            c.this_object
                .map_or(0.0, |id| f64::from(get(vm, id).recharge_rate)),
        )
    });
    // `setDamageState(name)`: the state named (ignoring case), true; false for any other name.
    register_method(vm, "ShapeBase", "setDamageState", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let name = c.cstr(2);
        let Some(state) = STATE_NAMES
            .iter()
            .position(|n| n.as_bytes().eq_ignore_ascii_case(name))
        else {
            return Native::Bool(false);
        };
        match set_damage_state(vm, id, state as u32) {
            Ok(()) => Native::Bool(true),
            Err(e) => Native::Fail(e),
        }
    });
    register_method(vm, "ShapeBase", "getDamageState", |vm, c| {
        let state = c.this_object.map_or(0, |id| get(vm, id).damage_state);
        Native::Str(
            STATE_NAMES
                .get(state as usize)
                .copied()
                .unwrap_or("")
                .as_bytes()
                .to_vec(),
        )
    });
    register_method(vm, "ShapeBase", "isDisabled", |vm, c| {
        Native::Bool(
            c.this_object
                .is_some_and(|id| get(vm, id).damage_state != 0),
        )
    });
    register_method(vm, "ShapeBase", "isEnabled", |vm, c| {
        Native::Bool(
            c.this_object
                .is_some_and(|id| get(vm, id).damage_state == 0),
        )
    });
    register_method(vm, "ShapeBase", "isDestroyed", |vm, c| {
        Native::Bool(
            c.this_object
                .is_some_and(|id| get(vm, id).damage_state == 2),
        )
    });
    register_method(vm, "ShapeBase", "setEnergyLevel", |vm, c| {
        if let Some(id) = c.this_object {
            set_energy_level(vm, id, arg_f(c, 2).unwrap_or(0.0));
        }
        Native::Void
    });
    register_method(vm, "ShapeBase", "getEnergyLevel", |vm, c| {
        Native::Float(
            c.this_object
                .map_or(0.0, |id| f64::from(energy_level(vm, id))),
        )
    });
    register_method(vm, "ShapeBase", "getEnergyPercent", |vm, c| {
        Native::Float(
            c.this_object
                .map_or(0.0, |id| f64::from(energy_value(vm, id))),
        )
    });
}
