//! `Player::updatePos(dt)` as TGE 1.4's `player.cc` writes it: up to [`MAX_POS_ITERATIONS`] sweeps of the box along the
//! velocity, each ending at the first hit, where the Player steps up a ledge or slides along the surface; the early out when
//! nothing is near the end; the position, the container and the collision callbacks at the end.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::{Box3, F3, IDENTITY, mul_box};
use torque_collide::{Exhausted, types, work};

use super::contact::{box_planes, step_up};
use super::data::PlayerData;
use super::ext::{PlayerClass, TorquePlayer};
use super::host::PlayerHost;
use super::update_move::len3;
use super::{MAX_POS_ITERATIONS, Player, container, net_mask, shape_base, transform};

/// The sweep's margin around the swept box.
const SWEEP_PAD: f32 = 0.1;
/// The gap the Player backs off from a hit by.
const BACK_OFF: f32 = 0.01;
/// A face is a step candidate when its normal's z is within this of level (`sVerticalStepDot`, 80 degrees).
const VERTICAL_STEP_DOT: f32 = 0.173;
/// The extra speed taken out along the normal of a hit surface (`sNormalElasticity`).
const NORMAL_ELASTICITY: f32 = 0.01;

impl Player {
    /// `updatePos(dt)`, through the [`library`](crate::library) table.
    pub(super) fn update_pos(
        &mut self,
        db: &PlayerData,
        dt: f32,
        host: &mut dyn PlayerHost,
    ) -> Result<(), Exhausted> {
        (crate::library::get().player_update_pos)(self, db, dt, host)
    }

    /// `updatePos(dt)` in plain arithmetic: [`PLAIN`](crate::library::PLAIN)'s entry.
    pub fn plain_update_pos(
        &mut self,
        db: &PlayerData,
        dt: f32,
        host: &mut dyn PlayerHost,
    ) -> Result<(), Exhausted> {
        let move_mask = host.type_masks().player_move_collide;
        self.s.delta_pos = self.s.position();
        // Mounted: the mount's velocity and node.
        if let Some(mount) = self.s.mount {
            self.s.velocity = host.object_velocity(mount);
            let mt = host.mount_transform(mount, self.s.mount_node);
            let rot = self.s.rot;
            transform::set_position(&mut self.s, [0.0; 3], rot, mt, host);
            self.s.set_mask(net_mask::MOVE);
            self.s.delta_pos = [0.0; 3];
            return Ok(());
        }
        let class = host.player_class();
        let class: &dyn PlayerClass = class.as_deref().unwrap_or(&TorquePlayer);
        let initial = self.s.position();
        let mut start = initial;
        let mut first_normal: F3 = [0.0; 3];
        let mut max_step = db.max_step_height;
        let mut time = dt;
        let mut count: u32 = 0;
        let now = host.sim_time();
        while count < MAX_POS_ITERATIONS {
            let v = self.s.velocity;
            let o = self.s.pending_offset;
            let has_offset = o[0] != 0.0 || o[1] != 0.0;
            if len3(v) == 0.0 && !has_offset {
                break;
            }
            let mut end = [
                start[0] + v[0] * time,
                start[1] + v[1] * time,
                start[2] + v[2] * time,
            ];
            if has_offset {
                // The death animation's root offset, turned into world space.
                self.s.pending_offset = [0.0; 3];
                let m = self.s.obj_to_world;
                for (k, e) in end.iter_mut().enumerate() {
                    *e += m[4 * k] * o[0] + m[4 * k + 1] * o[1] + m[4 * k + 2] * o[2];
                }
            }
            let distance = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
            if class.early_out_fits(&self.s, distance) {
                // Nothing at the end position: move there.
                let b = self.s.scaled_box;
                let w_box = Box3::new(
                    std::array::from_fn(|k| b.min[k] + end[k]),
                    std::array::from_fn(|k| b.max[k] + end[k]),
                );
                let early = &mut self.scratch.early;
                early.clear();
                early.normal = [0.0; 3];
                early.planes.clear();
                early
                    .planes
                    .extend_from_slice(&box_planes(w_box.min, w_box.max));
                for c in self.working.iter() {
                    let world = host.world();
                    if !world.charge(work::CONVEX) {
                        return Err(Exhausted);
                    }
                    let Some(view) = world.object(c.object) else {
                        continue;
                    };
                    if view.type_mask & move_mask == 0 {
                        continue;
                    }
                    let Some(bb) = c.bounding_box(world) else {
                        continue;
                    };
                    if w_box.overlaps(&bb) {
                        c.poly_list(world, &mut *early)?;
                    }
                }
                if !early.early_out {
                    start = end;
                    break;
                }
            }
            // The swept box.
            let mut m = IDENTITY;
            m[3] = start[0];
            m[7] = start[1];
            m[11] = start[2];
            let sb = self.s.scaled_box;
            let sc = &mut *self.scratch;
            sc.hedron.build_box(&m, &sb);
            let old = mul_box(&m, &sb);
            let plist_box = Box3::new(
                std::array::from_fn(|k| old.min[k].min(old.min[k] + v[k] * time - SWEEP_PAD)),
                std::array::from_fn(|k| old.max[k].max(old.max[k] + v[k] * time + SWEEP_PAD)),
            );
            sc.ground.extrude(&sc.hedron, distance);
            sc.ground.set_velocity(v);
            sc.ground.collisions.reset();
            sc.zone.extrude(&sc.hedron, distance);
            sc.zone.set_velocity(v);
            sc.zone.collisions.reset();
            for c in self.working.iter() {
                let world = host.world();
                if !world.charge(work::CONVEX) {
                    return Err(Exhausted);
                }
                let Some(view) = world.object(c.object) else {
                    continue;
                };
                let tm = view.type_mask;
                if tm & move_mask == 0 {
                    continue;
                }
                let Some(bb) = c.bounding_box(world) else {
                    continue;
                };
                if !plist_box.overlaps(&bb) {
                    continue;
                }
                if tm & types::PHYSICAL_ZONE != 0 {
                    c.poly_list(world, &mut sc.zone)?;
                } else {
                    c.poly_list(world, &mut sc.ground)?;
                }
            }
            // The physical zones' velocity multipliers.
            for e in sc.zone.collisions.as_slice() {
                if let Some(obj) = e.object
                    && let Some(k) = host.zone_velocity_mod(obj)
                {
                    let s = &mut *self.s;
                    s.velocity = std::array::from_fn(|i| s.velocity[i] * k);
                }
            }
            let gl = &sc.ground.collisions;
            if gl.count == 0 || !(gl.t < 1.0) {
                start = end;
                break;
            }
            // To the hit, then back off.
            let v = self.s.velocity;
            let vel_len = len3(v);
            let hit_dt = time * gl.t.min(1.0);
            start = std::array::from_fn(|k| start[k] + v[k] * hit_dt);
            time -= hit_dt;
            self.s.falling = false;
            if vel_len > 0.0 {
                let new_t = (BACK_OFF / vel_len).min(hit_dt);
                start = std::array::from_fn(|k| start[k] - v[k] * new_t);
            }
            // A step up a near-vertical face.
            let max_height = gl.max_height;
            if max_height < start[2] + db.max_step_height * self.s.scale[2] {
                let vertical = gl
                    .as_slice()
                    .iter()
                    .any(|c| c.normal[2].abs() < VERTICAL_STEP_DOT);
                if vertical && step_up(self, db, &mut start, &mut max_step, time, host)? {
                    count += 1;
                    continue;
                }
            }
            // The surface most parallel to the face that was hit.
            let entries = self.scratch.ground.collisions.as_slice();
            let mut best = entries[0];
            for e in &entries[1..] {
                if e.face_dot > best.face_dot {
                    best = *e;
                }
            }
            let n = best.normal;
            let s = &mut *self.s;
            let bd = -(s.velocity[0] * n[0] + s.velocity[1] * n[1] + s.velocity[2] * n[2]);
            if bd > db.min_impact_speed && s.impact_block == 0 {
                host.on_impact(s, best.object, [n[0] * bd, n[1] * bd, n[2] * bd]);
            }
            if bd > db.min_impact_speed / 3.0 {
                s.impact_flag = 1;
                s.set_mask(net_mask::IMPACT);
            }
            // The velocity into the surface taken out.
            let k = bd + NORMAL_ELASTICITY;
            let dv = [n[0] * k, n[1] * k, n[2] * k];
            s.velocity = std::array::from_fn(|i| s.velocity[i] + dv[i]);
            if count == 0 {
                first_normal = n;
            } else if count == 1 {
                // Along the crease of the two surfaces.
                let fnl = first_normal;
                let dot = |a: F3, b: F3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                if dot(dv, fnl) < 0.0 && dot(n, fnl) < 0.0 {
                    let nv = [
                        n[1] * fnl[2] - n[2] * fnl[1],
                        n[2] * fnl[0] - n[0] * fnl[2],
                        n[0] * fnl[1] - n[1] * fnl[0],
                    ];
                    let mut nvl = len3(nv);
                    if nvl != 0.0 {
                        if dot(nv, s.velocity) < 0.0 {
                            nvl = -nvl;
                        }
                        let k = len3(s.velocity) / nvl;
                        s.velocity = [nv[0] * k, nv[1] * k, nv[2] * k];
                    }
                }
            }
            if let Some(hit) = best.object
                && host
                    .world()
                    .object(hit)
                    .is_some_and(|view| view.type_mask & types::SHAPE_BASE != 0)
            {
                let hv = host.object_velocity(hit);
                let vec = std::array::from_fn(|i| s.velocity[i] - hv[i]);
                shape_base::queue_collision(s, now, hit, vec);
            }
            count += 1;
        }
        if count == MAX_POS_ITERATIONS {
            // Failed to move.
            start = initial;
            self.s.velocity = [0.0; 3];
        }
        let rot = self.s.rot;
        transform::set_position(&mut self.s, start, rot, None, host);
        let s = &mut *self.s;
        s.set_mask(net_mask::MOVE);
        let sc = &mut *self.scratch;
        container::update_container(s, db, &mut sc.zones, &mut sc.contact, host);
        shape_base::notify_collision(s, now, host);
        Ok(())
    }
}
