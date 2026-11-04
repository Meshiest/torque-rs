//! `ShapeBase::processTick` as a server Player runs it, `getEnergyLevel`, and the collision queue
//! (`queueCollision`, `notifyCollision`).
//!
//! Not here (no server state the words or the step read): the sequence sounds and their mask bits. The script threads'
//! step (`advanceThreads`) and the fade are the host's ([`PlayerHost::advance_threads`],
//! [`PlayerHost::fade_tick`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeBase.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::ObjectId;
use torque_collide::math::F3;

use super::data::PlayerData;
use super::host::PlayerHost;
use super::{MAX_COLLISION_QUEUE, MAX_MOUNT_DEPTH, PlayerState, QueuedCollision, net_mask};
use crate::Move;

/// The coverage above which image slot 0 is wet (`mWaterCoverage > 0.4`, compared as `f64`).
const WET_COVERAGE: f64 = 0.4;

/// The tick for a server object (`netFlags` bit 1 clear, bit 8 set).
pub(super) fn process_tick(
    s: &mut PlayerState,
    db: &PlayerData,
    mv: Option<&Move>,
    host: &mut dyn PlayerHost,
) {
    // Energy: alive and not inheriting the mount's. `e > max` takes the max, else `0 > e` takes 0; a NaN fails
    // both compares and stays (and counts as a change).
    if s.damage_state == 0 && !db.inherit_energy {
        let old = s.energy;
        let e = old + s.recharge_rate;
        s.energy = e;
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
            host.energy_changed(s);
        }
    }
    // Damage and repair.
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
    // The script threads (`playThread`'s sequences play on the server, the arm a held tool raises among them; the
    // muzzle and the mount points read the pose they leave).
    host.advance_threads(s);
    // Image slot 0's wet flag.
    let wet = f64::from(s.water_coverage) > WET_COVERAGE;
    if s.images[0] && s.image_wet != wet {
        s.set_mask(net_mask::WET);
        s.image_wet = wet;
    }
    host.fade_tick(s);
    // The image slots: each slot's datablock is tested as its turn comes (an earlier slot's script may have mounted
    // or unmounted it), so the host's `image_tick` tests it, not the mirror `s.images`.
    for slot in 0..4 {
        host.image_tick(s, slot);
    }
    // `onTrigger` for each trigger that changed.
    if let Some(mv) = mv {
        for slot in 0..crate::MAX_TRIGGERS {
            if mv.triggers[slot] != s.trigger_state[slot] {
                s.trigger_state[slot] = mv.triggers[slot];
                host.on_trigger(s, slot, mv.triggers[slot]);
            }
        }
    }
    // The flash and the white-out decay.
    let g = host.globals();
    if 0.0 < s.damage_flash {
        s.damage_flash -= g.damage_flash_dec;
        if s.damage_flash <= 0.0 {
            s.damage_flash = 0.0;
        }
    }
    if 0.0 < s.white_out {
        s.white_out -= g.white_out_dec;
        if s.white_out <= 0.0 {
            s.white_out = 0.0;
        }
    }
}

/// `getEnergyLevel`: the energy, or the first mount's up the chain whose datablock does not inherit; 0 when the chain
/// ends (or passes [`MAX_MOUNT_DEPTH`]).
pub(super) fn energy_level(s: &PlayerState, inherit: bool, host: &mut dyn PlayerHost) -> f32 {
    if !inherit {
        return s.energy;
    }
    let mut at = s.mount;
    for _ in 0..MAX_MOUNT_DEPTH {
        let Some(id) = at else {
            return 0.0;
        };
        let Some(link) = host.energy_link(id) else {
            return 0.0;
        };
        if !link.inherit {
            return link.energy;
        }
        at = link.mount;
    }
    0.0
}

/// `ShapeBase::queueCollision(obj, vec)`: the walk drops expired entries before the match; a match not expired keeps
/// its old vector; an expired match is refreshed; no match adds an entry at the head, 250 ms from now.
pub fn queue_collision(s: &mut PlayerState, now: u32, obj: ObjectId, vec: F3) {
    let mut i = 0;
    while i < s.collisions.len() {
        let e = s.collisions[i];
        if e.id == obj {
            if now <= e.expiry {
                return;
            }
            let e = &mut s.collisions[i];
            e.expiry = now.wrapping_add(250);
            e.object = Some(obj);
            e.vec = vec;
            return;
        }
        if e.expiry < now {
            s.collisions.remove(i);
        } else {
            i += 1;
        }
    }
    if s.collisions.len() >= MAX_COLLISION_QUEUE {
        return;
    }
    s.collisions.insert(
        0,
        QueuedCollision {
            object: Some(obj),
            id: obj,
            expiry: now.wrapping_add(250),
            vec,
        },
    );
}

/// `ShapeBase::notifyCollision`: each entry queued this tick (expiry `now + 250`) with an object runs the two
/// `onCollision`s and loses its object.
pub fn notify_collision(s: &mut PlayerState, now: u32, host: &mut dyn PlayerHost) {
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
