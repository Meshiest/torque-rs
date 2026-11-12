//! `Projectile::processTick`, `explode` and the bounce.
//!
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/projectile.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::vec3::{add_scaled, dot};
use torque_collide::math::{F3, IDENTITY, Matrix};

use super::{
    DELETE_WAIT_MS, Projectile, ProjectileData, ProjectileHost, SOURCE_TIMEOUT_TICKS, net_mask,
};
use crate::scene::{DT, RayHit};

/// Projectile gravity, m/s².
const GRAVITY: f32 = 9.81;
/// How far a bounce moves the next step's start out along the surface normal.
const BOUNCE_OUT: f32 = 0.05;
/// How far the explosion position is moved out along the normal.
const EXPLOSION_OUT: f32 = 0.01;

fn identity_at(p: F3) -> Matrix {
    let mut m = IDENTITY;
    m[3] = p[0];
    m[7] = p[1];
    m[11] = p[2];
    m
}

impl Projectile {
    /// `Projectile::processTick` for a server object. The move is ignored.
    pub fn process_tick(&mut self, db: &ProjectileData, host: &mut dyn ProjectileHost) {
        let id = self.s.id;
        self.s.age = self.s.age.wrapping_add(1);
        if self.s.source.is_some() && self.s.age > SOURCE_TIMEOUT_TICKS {
            self.s.source = None;
            self.s.source_id = 0;
        }
        if self.s.age >= db.lifetime {
            host.delete_object(id);
            return;
        }
        if self.s.exploded {
            return;
        }

        let old = self.s.position;
        if db.is_ballistic {
            let vz = self.s.velocity[2];
            self.s.velocity[2] = vz - GRAVITY * db.gravity_mod * DT;
        }
        let mut new = add_scaled(old, self.s.velocity, DT);

        // The source is exempt from the ray while it may still overlap its own shot.
        if let Some(src) = self.s.source {
            host.set_source_collision(src, true);
        }
        let masks = *host.type_masks();
        if let Some(hit) = host.cast_ray(old, new, masks.projectile_ray_players) {
            if hit.type_mask & masks.projectile_solid == 0 {
                self.s.set_mask(net_mask::BOUNCE);
            }
            if self.s.age > db.arming_delay {
                self.impact(&hit, host);
            } else if db.is_ballistic {
                new = self.bounce(db, &hit, old);
            }
        }
        if let Some(src) = self.s.source {
            host.set_source_collision(src, false);
        }

        let s = &mut *self.s;
        s.render_base = new;
        s.render_delta = std::array::from_fn(|i| s.position[i] - new[i]);
        s.position = new;
        s.scene.set_matrix(&identity_at(new));
        host.moved(s);
    }

    /// A hit past the arming delay: the projectile stops at the hit point, `onCollision` runs (with the type mask read
    /// before it, in case the script deletes what was hit), then `explode`.
    fn impact(&mut self, hit: &RayHit, host: &mut dyn ProjectileHost) {
        self.s.scene.set_matrix(&identity_at(hit.point));
        host.moved(&mut self.s);
        self.s.position = hit.point;
        self.s.velocity = [0.0; 3];
        let object_type = hit.type_mask;
        if let Some(src) = self.s.source {
            host.set_source_collision(src, false);
        }
        host.on_collision(&mut self.s, hit.object, hit.point, hit.normal);
        self.explode(hit.point, hit.normal, object_type, host);
    }

    /// A ballistic hit before the arming delay: the velocity is reflected about the normal, loses `bounceFriction` of its
    /// part along the surface and is scaled by `bounceElasticity`. The step ends just off the surface; the rest of the tick is
    /// whole milliseconds of the unused fraction, which is zero unless the hit was at the ray's start (the ray's fraction is 0
    /// exactly when the hit point is the start), where it is one millisecond.
    fn bounce(&mut self, db: &ProjectileData, hit: &RayHit, start: F3) -> F3 {
        let n = hit.normal;
        let v = self.s.velocity;
        let k = dot(v, n) * 2.0;
        let b: F3 = std::array::from_fn(|i| v[i] - n[i] * k);
        let along = dot(b, n);
        let mut nv = b;
        for i in 0..3 {
            let tangent = b[i] - n[i] * along;
            nv[i] -= tangent * db.bounce_friction;
        }
        for c in nv.iter_mut() {
            *c *= db.bounce_elasticity;
        }
        self.s.velocity = nv;
        let time_left_ms: u32 = if hit.point == start { 1 } else { 0 };
        let out = add_scaled(hit.point, n, BOUNCE_OUT);
        let rest = (time_left_ms as f32 / 1000.0) * 32.0;
        add_scaled(out, nv, rest)
    }

    /// `explode(pos, normal, collideType)`, server half: once; the explosion position is `pos + normal * 0.01`, the normal
    /// and type kept; `onExplode` with the position and the fade; the explosion bit; and an `ObjectDeleteEvent`
    /// [`DELETE_WAIT_MS`] later on the sim clock (read after the script returns).
    pub fn explode(
        &mut self,
        pos: F3,
        normal: F3,
        collide_type: u32,
        host: &mut dyn ProjectileHost,
    ) {
        if self.s.exploded {
            return;
        }
        self.s.exploded = true;
        {
            let s = &mut *self.s;
            s.explosion_pos = add_scaled(pos, normal, EXPLOSION_OUT);
            s.explosion_normal = normal;
            s.explosion_type = collide_type;
        }
        let (ep, fade) = (self.s.explosion_pos, self.s.fade);
        host.on_explode(&mut self.s, ep, fade);
        self.s.set_mask(net_mask::EXPLOSION);
        let at = host.sim_time().wrapping_add(DELETE_WAIT_MS);
        host.schedule_delete(self.s.id, at);
    }
}
