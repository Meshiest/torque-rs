//! `Item::updatePos(mask, dt)`.
//!
//! The move has three parts: the first ray (the top centre of the old box to the bottom centre of the box at the end position),
//! whose hit already bounces the velocity; the sweep loop of at most [`MAX_ITEM_SUBSTEPS`] sub-steps, each a presence test of
//! the working set against a swept box, then the extruded sweep and the same response; and the tail that stores the position,
//! decides rest, and runs the script callbacks.
//!
//! The swept box takes the smaller of each max and the max moved by the step (`sw.max = min(max, max + v dt)`).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/item.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use torque_collide::math::vec3::dot;
use torque_collide::math::{Box3, F3, Matrix, Plane, mul_box, mul_p};
use torque_collide::{Exhausted, MAX_COLLISIONS, types, work};

use super::host::ItemHost;
use super::tick::translation;
use super::{Item, ItemData, MAX_ITEM_SUBSTEPS, net_mask};
use crate::scene::{RayHit, queue_collision};

/// The margin added to the impact speed before the bounce.
const BOUNCE_MARGIN: f32 = 0.002;
/// Below this speed a hit on solid world stops the item.
const STOP_SPEED: f32 = 0.15;

/// The velocity response of a hit with normal `n` at impact speed `impact` (`-(n . v)`): the tangential speed clipped to
/// `friction * impact`, then `v = (v + n (impact (elasticity + 1) + 0.002)) - s`. The same in the first ray and the loop.
fn respond(db: &ItemData, v: &mut F3, n: F3, impact: f32) {
    let mut sv: F3 = std::array::from_fn(|i| v[i] + n[i] * impact);
    let len = dot(sv, sv).sqrt();
    // Equal to zero skips, a NaN goes in.
    if len != 0.0 {
        let lim = db.friction * impact;
        if len > lim {
            let r = lim / len;
            for c in sv.iter_mut() {
                *c *= r;
            }
        }
    }
    let bounce = impact * (db.elasticity + 1.0) + BOUNCE_MARGIN;
    for i in 0..3 {
        v[i] = v[i] + n[i] * bounce - sv[i];
    }
}

/// The plane through `c` with normal `n` (`PlaneF::set`).
fn box_plane(n: F3, c: F3) -> Plane {
    Plane::new(n[0], n[1], n[2], -dot(n, c))
}

impl Item {
    pub(super) fn update_pos(
        &mut self,
        db: &ItemData,
        cast_mask: u32,
        dt: f32,
        host: &mut dyn ItemHost,
    ) -> Result<(), Exhausted> {
        let id = self.s.id;
        let now = host.sim_time();
        let masks = *host.type_masks();
        let p0 = self.s.position();
        self.s.render_delta = p0;
        let mut hit = false;
        let mut solid = true;
        let exempt = self.s.exempt;

        // The first ray.
        {
            let v = self.s.velocity;
            let e1: F3 = std::array::from_fn(|i| p0[i] + v[i] * dt);
            let ob = self.s.scene.obj_box;
            let cx = (ob.min[0] + ob.max[0]) * 0.5;
            let cy = (ob.min[1] + ob.max[1]) * 0.5;
            let a = mul_p(&translation(p0), [cx, cy, ob.max[2]]);
            let b = mul_p(&translation(e1), [cx, cy, ob.min[2]]);
            if let Some(e) = exempt {
                host.disable_collision(e);
            }
            let ray = host.cast_ray(a, b, cast_mask);
            if let Some(h) = ray {
                let n = h.normal;
                let impact = -dot(n, v);
                // A negative or NaN impact skips (the item is moving away from the surface).
                if impact >= 0.0 {
                    hit = true;
                    respond(db, &mut self.s.velocity, n, impact);
                    solid = h.type_mask & masks.item_solid != 0;
                    self.queue_shape_hit(&h, now, host);
                }
            }
            if let Some(e) = exempt {
                host.enable_collision(e);
            }
        }

        // The sweep loop.
        let mut p = p0;
        let mut dt_rem = dt;
        let mut sub = 0u32;
        let ob = self.s.scene.obj_box;
        loop {
            let v = self.s.velocity;
            let vt: F3 = std::array::from_fn(|i| v[i] * dt_rem);
            let e: F3 = std::array::from_fn(|i| p[i] + vt[i]);
            // The box at the end position, and the swept box built from it.
            let bx = mul_box(&translation(e), &ob);
            let sw = Box3::new(
                std::array::from_fn(|i| bx.min[i].min(bx.min[i] + vt[i])),
                std::array::from_fn(|i| bx.max[i].min(bx.max[i] + vt[i])),
            );
            let (mn, mx) = (bx.min, bx.max);
            let planes: [Plane; 6] = [
                box_plane([-1.0, 0.0, 0.0], mn),
                box_plane([0.0, 1.0, 0.0], mx),
                box_plane([1.0, 0.0, 0.0], mx),
                box_plane([0.0, -1.0, 0.0], mn),
                box_plane([0.0, 0.0, -1.0], mn),
                box_plane([0.0, 0.0, 1.0], mx),
            ];
            // The presence test: the first working-set convex whose polygons reach the swept box's planes.
            let early = &mut self.scratch.early;
            early.clear();
            early.normal = [0.0; 3];
            early.planes.clear();
            early.planes.extend_from_slice(&planes);
            let mut present = false;
            for c in self.working.iter() {
                let world = host.world();
                if !world.charge(work::CONVEX) {
                    return Err(Exhausted);
                }
                let Some(view) = world.object(c.object) else {
                    continue;
                };
                if view.type_mask & masks.item_collide == 0 {
                    continue;
                }
                let Some(bb) = c.bounding_box(world) else {
                    continue;
                };
                if !sw.overlaps(&bb) {
                    continue;
                }
                c.poly_list(world, &mut *early)?;
                if early.early_out {
                    present = true;
                    break;
                }
            }
            if !present {
                p = e;
                break;
            }
            // The box at the sub-step start extruded to the end, swept through the working set.
            let sc = &mut *self.scratch;
            sc.hedron.build_box(&translation(p), &ob);
            let delta: F3 = std::array::from_fn(|i| e[i] - p[i]);
            sc.ext.extrude(&sc.hedron, delta);
            sc.ext.set_velocity(self.s.velocity);
            sc.ext.collisions.reset();
            for c in self.working.iter() {
                let world = host.world();
                if !world.charge(work::CONVEX) {
                    return Err(Exhausted);
                }
                let Some(view) = world.object(c.object) else {
                    continue;
                };
                if view.type_mask & masks.item_collide == 0 {
                    continue;
                }
                let Some(bb) = c.bounding_box(world) else {
                    continue;
                };
                if !sw.overlaps(&bb) {
                    continue;
                }
                c.poly_list(world, &mut sc.ext)?;
            }
            let t = sc.ext.collisions.t;
            // T at or above one, or a NaN, is a free move.
            if !(t < 1.0) {
                p = e;
                break;
            }
            let adv = t * dt_rem;
            let v = self.s.velocity;
            p = std::array::from_fn(|i| p[i] + v[i] * adv);
            dt_rem -= adv;
            // The best contact: the most head-on (`val > best`, strict, best starting at zero).
            let count = sc.ext.collisions.count.min(MAX_COLLISIONS);
            let mut best = 0.0f32;
            let mut best_at: Option<usize> = None;
            for (i, c) in sc.ext.collisions.entries[..count].iter().enumerate() {
                let val = -dot(c.normal, v);
                if val > best {
                    best = val;
                    best_at = Some(i);
                }
            }
            if let Some(i) = best_at {
                let entry = sc.ext.collisions.entries[i];
                hit = true;
                respond(db, &mut self.s.velocity, entry.normal, best);
                let tm = entry
                    .object
                    .and_then(|o| host.world().object(o))
                    .map_or(0, |view| view.type_mask);
                solid = (tm & masks.item_solid != 0) && solid;
                if let Some(o) = entry.object {
                    let h = RayHit {
                        object: o,
                        point: entry.point,
                        normal: entry.normal,
                        type_mask: tm,
                    };
                    self.queue_shape_hit(&h, now, host);
                }
            }
            sub += 1;
            if sub >= MAX_ITEM_SUBSTEPS {
                self.s.velocity = [0.0; 3];
                break;
            }
        }

        // The end of the move.
        let mut m: Matrix = self.s.scene.obj_to_world;
        m[3] = p[0];
        m[7] = p[1];
        m[11] = p[2];
        self.s.scene.set_matrix(&m);
        host.moved(&mut self.s);
        host.enable_collision(id);
        if let Some(e) = self.s.exempt {
            host.enable_collision(e);
        }
        host.update_container(&mut self.s, db);
        if hit {
            let set = if !solid {
                true
            } else {
                let v = self.s.velocity;
                let speed = dot(v, v).sqrt();
                // Slower than the stop speed (ordered) stops the item; a faster one, or a NaN, keeps moving.
                if speed < STOP_SPEED {
                    let s = &mut *self.s;
                    s.velocity = [0.0; 3];
                    s.at_rest = true;
                    s.rest_ticks = 0;
                    true
                } else {
                    // moving: the position goes out only for an item that was marked at rest
                    self.s.at_rest
                }
            };
            if set {
                self.s.set_mask(net_mask::POSITION);
            }
        }
        // The server tail: the queued collisions' scripts, then the liquid callbacks (only if the item survived them).
        if self.notify_collision(now, host) {
            let (cov, ty) = (self.s.water_coverage, self.s.liquid_type);
            if !self.s.in_liquid {
                // A compare of the coverage against zero: equal skips, a NaN counts as not equal.
                if cov != 0.0 {
                    host.on_enter_liquid(&mut self.s, cov, ty);
                    self.s.in_liquid = true;
                }
            } else if cov == 0.0 {
                host.on_leave_liquid(&mut self.s, ty);
                self.s.in_liquid = false;
            }
        }
        Ok(())
    }

    /// A hit on a ShapeBase (`ShapeBaseObjectType`) queues a collision with the item's POST-response velocity relative to the other object's velocity.
    fn queue_shape_hit(&mut self, h: &RayHit, now: u32, host: &mut dyn ItemHost) {
        if h.type_mask & types::SHAPE_BASE == 0 {
            return;
        }
        let w = host.object_velocity(h.object);
        let v = self.s.velocity;
        let rel = std::array::from_fn(|i| v[i] - w[i]);
        queue_collision(&mut self.s.collisions, now, h.object, rel);
    }

    /// `ShapeBase::notifyCollision`: each entry queued this tick (expiry `now + 250`) with an object runs the two
    /// `onCollision`s and loses its object. False when the item was deleted by a callback.
    fn notify_collision(&mut self, now: u32, host: &mut dyn ItemHost) -> bool {
        let stamp = now.wrapping_add(250);
        let mut i = 0;
        while i < self.s.collisions.len() {
            let e = self.s.collisions[i];
            if e.expiry == stamp
                && let Some(obj) = e.object
            {
                self.s.collisions[i].object = None;
                if !host.on_collision(&mut self.s, obj, e.vec) {
                    return false;
                }
            }
            i += 1;
        }
        true
    }
}
