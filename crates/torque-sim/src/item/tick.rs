//! `Item::processTick`, `updateVelocity` and `updateWorkingCollisionSet`.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/item.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use torque_collide::Exhausted;
use torque_collide::math::{Box3, IDENTITY, mul_box};

use super::host::ItemHost;
use super::{CACHE_SENTINEL, Item, ItemData, net_mask};
use crate::scene::DT;

/// The ceiling of the at-rest counter: a resting item re-simulates one tick when the counter passes 0x40.
const REST_TICKS: i32 = 0x40;
/// How far past `maxVelocity` the speed may go before it is pulled back.
const MAX_VELOCITY_SLACK: f32 = 1.05;
/// The part of the excess speed taken off per tick.
const EXCESS_PULL: f32 = 0.1;
/// The working set's margin growth and pad (`Convex::updateWorkingList`'s).
const RADIUS_GROWTH: f32 = 1.1;
const RADIUS_PAD: f32 = 0.1;
/// Item gravity: 20 units/s^2 times `gravityMod`, not 9.81.
const GRAVITY: f32 = 20.0;

impl Item {
    /// `Item::processTick` for a server object. The move is ignored. A spent work budget stops the step where it was
    /// (deterministic; the state is as far as it got).
    ///
    /// The wake-up undo tests the flag `mAtRest` AFTER `updatePos` against the flag at the START of the tick; `setPosition` is
    /// `Item::setTransform`, so the undo also leaves mask bits `0x8000040` set (setTransform's `0xc000040` minus the `0x4000000`
    /// that `clearMaskBits` takes away), and a non-static item's `mAtRest` is cleared by it and set again by the undo's own store.
    pub fn process_tick(
        &mut self,
        db: &ItemData,
        host: &mut dyn ItemHost,
    ) -> Result<(), Exhausted> {
        (crate::library::get().item_process_tick)(self, db, host)
    }

    /// `Item::processTick` in plain arithmetic: [`PLAIN`](crate::library::PLAIN)'s entry.
    pub fn plain_process_tick(
        &mut self,
        db: &ItemData,
        host: &mut dyn ItemHost,
    ) -> Result<(), Exhausted> {
        // The collision-exempt timeout (a raw field clear, no clearNotify).
        {
            let s = &mut *self.s;
            if s.exempt.is_some() {
                s.exempt_ticks = s.exempt_ticks.wrapping_sub(1);
                if s.exempt_ticks == 0 {
                    s.exempt = None;
                }
            }
        }
        // The client warp (a positive warp counter) never runs on a server.
        let pos0 = self.s.position();
        let rest0 = self.s.at_rest;
        let mut cur = rest0;
        if rest0 && !self.s.is_static && !db.sticky {
            let s = &mut *self.s;
            s.rest_ticks = s.rest_ticks.wrapping_add(1);
            if s.rest_ticks > REST_TICKS {
                s.at_rest = false;
                s.rest_ticks = 0;
                cur = false;
            }
        }
        if self.s.is_static || cur || self.s.hidden {
            self.s.render_delta = [0.0; 3];
            return Ok(());
        }
        self.update_velocity(db, DT);
        self.update_working_collision_set(host)?;
        let collide = host.type_masks().item_collide;
        self.update_pos(db, collide, DT, host)?;
        if !rest0 || !self.s.at_rest {
            return Ok(());
        }
        // It woke from rest this tick and settled again: the whole move is undone.
        let mut m = self.s.scene.obj_to_world;
        m[3] = pos0[0];
        m[7] = pos0[1];
        m[11] = pos0[2];
        self.set_transform(&m, host);
        let s = &mut *self.s;
        s.render_delta = [0.0; 3];
        s.render_base = pos0;
        s.velocity = [0.0; 3];
        s.clear_mask(net_mask::POSITION);
        s.at_rest = true;
        Ok(())
    }

    /// `updateVelocity(dt)`: gravity; the speed pulled back toward `maxVelocity` (when it is positive) by a tenth of the excess
    /// once past 1.05 times it; the buoyancy; the drag.
    pub(crate) fn update_velocity(&mut self, db: &ItemData, dt: f32) {
        let s = &mut *self.s;
        s.velocity[2] -= GRAVITY * db.gravity_mod * dt;
        if db.max_velocity > 0.0 {
            let v = s.velocity;
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if len > db.max_velocity * MAX_VELOCITY_SLACK {
                let k = 1.0 - db.max_velocity / len;
                for c in s.velocity.iter_mut() {
                    *c -= *c * k * EXCESS_PULL;
                }
            }
        }
        s.velocity[2] -= s.buoyancy * (-GRAVITY * db.gravity_mod * s.zone_gravity) * dt;
        let drag = s.drag;
        for c in s.velocity.iter_mut() {
            *c -= *c * drag * dt;
        }
    }

    /// `updateWorkingCollisionSet(_, dt)`: the item's box (the object box times the full scale, moved by the transform)
    /// grown by `|v dt| * 1.1 + 0.1` on every side; the set is rebuilt over that box grown again by twice the margin when the
    /// cached box does not hold it. The thrower's collision is off while the set is built.
    pub(crate) fn update_working_collision_set(
        &mut self,
        host: &mut dyn ItemHost,
    ) -> Result<(), Exhausted> {
        let s = &mut *self.s;
        let v = s.velocity;
        // `OrthoBoxConvex::getBoundingBox(objToWorld, scale)`.
        let sc = s.scene.scale;
        let ob = s.scene.obj_box;
        let scaled = Box3::new(
            std::array::from_fn(|i| ob.min[i] * sc[i]),
            std::array::from_fn(|i| ob.max[i] * sc[i]),
        );
        let mut b = mul_box(&s.scene.obj_to_world, &scaled);
        let a = [v[0] * DT, v[1] * DT, v[2] * DT];
        let m = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() * RADIUS_GROWTH + RADIUS_PAD;
        for i in 0..3 {
            b.min[i] -= m;
            b.max[i] += m;
        }
        let c = s.working_box;
        let unset = c.min[0] == CACHE_SENTINEL;
        if !unset {
            let contained = c.min[0] <= b.min[0]
                && c.min[1] <= b.min[1]
                && c.min[2] <= b.min[2]
                && b.max[0] <= c.max[0]
                && b.max[1] <= c.max[1]
                && b.max[2] <= c.max[2];
            if contained {
                return Ok(());
            }
        }
        let mut cache = b;
        for i in 0..3 {
            cache.min[i] -= 2.0 * m;
            cache.max[i] += 2.0 * m;
        }
        s.working_box = cache;
        let id = s.id;
        let exempt = s.exempt;
        let host_collide = host.type_masks().item_collide;
        host.disable_collision(id);
        if let Some(e) = exempt {
            host.disable_collision(e);
        }
        let result = self.working.update(host.world(), id, &cache, host_collide);
        if let Some(e) = exempt {
            host.enable_collision(e);
        }
        host.enable_collision(id);
        result
    }
}

/// The identity matrix the sweep moves its box with (translation only).
pub(super) fn translation(t: [f32; 3]) -> torque_collide::math::Matrix {
    let mut m = IDENTITY;
    m[3] = t[0];
    m[7] = t[1];
    m[11] = t[2];
    m
}
