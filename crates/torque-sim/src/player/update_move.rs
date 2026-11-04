//! The Player's move step: the look and turn, the wish, the contact, the run surface (or the air), the jump, the subclass's
//! thrust, swimming, the integration, then the dismount and liquid callbacks. `w` is the wish vector, `acc` the acceleration
//! and `cap` the speed cap.
//!
//! The step's shape follows TGE 1.4's `Player::updateMove` (`player.cc`), and its run-surface, jump, resistance and drag
//! rules are the same in Torque3D: the underwater speeds from a coverage of 0.9 on (no swimming pose), buoyancy as a lift
//! applied after the resistances, no stroke in water. A subclass ([`PlayerClass`]) may replace a section.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::Exhausted;
use torque_collide::math::{F3, set_euler};

use super::data::PlayerData;
use super::ext::{PlayerClass, TorquePlayer};
use super::host::{PlayerHost, Thread};
use super::{DT, MAX_YAW_WRAPS, Player, PlayerState, net_mask};
use crate::Move;

/// Half a turn and a whole turn, in radians.
const HALF_TURN: f32 = std::f32::consts::PI;
const TURN: f32 = std::f32::consts::TAU;
/// The gravity, in units per second squared.
const GRAVITY: f32 = -20.0;
/// The object-space vertical speed below which a Player off the ground is falling (`sFallingThreshold`).
const FALL_SPEED: f32 = -10.0;
/// The extra push out of the ground that lets a Player rest on it, and the acceleration left over below which it is cleared.
const REST_PUSH: f32 = 0.002;
const REST_CLEAR: f32 = 0.0001;
/// The water coverage from which the underwater speeds apply (`>= 0.9`, compared as `f64`).
const UNDERWATER: f64 = 0.9;
/// The contacts a Player may miss and still jump (`JumpSkipContactsMax`).
pub const JUMP_SKIP_CONTACTS_MAX: u32 = 8;

/// A vector's length (`Point3F::len`).
pub(super) fn len3(v: F3) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `v` held to `[lo, hi]` (`mClampF`: the high bound first, then the low one, which also takes a NaN).
fn held(v: f32, lo: f32, hi: f32) -> f32 {
    let below_hi = if v > hi { hi } else { v };
    if below_hi > lo { below_hi } else { lo }
}

/// The larger of two floats: `a` when `a > b`, else `b`.
fn larger(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// A turn input past half a turn is taken as the negative turn the other way.
fn signed_turn(a: f32) -> f32 {
    if a > HALF_TURN { a - TURN } else { a }
}

/// A turn difference taken the short way round (within half a turn either side).
fn short_way(a: f32) -> f32 {
    if a > HALF_TURN {
        a - TURN
    } else if a < -HALF_TURN {
        a + TURN
    } else {
        a
    }
}

/// The yaw brought into `[0, 2 pi]` a whole turn at a time; each direction stops after [`MAX_YAW_WRAPS`] turns.
fn into_one_turn(mut yaw: f32) -> f32 {
    for _ in 0..MAX_YAW_WRAPS {
        if !(yaw < 0.0) {
            break;
        }
        yaw += TURN;
    }
    for _ in 0..MAX_YAW_WRAPS {
        if !(yaw > TURN) {
            break;
        }
        yaw -= TURN;
    }
    yaw
}

/// Vector sum, difference and scaling.
fn add(a: F3, b: F3) -> F3 {
    std::array::from_fn(|i| a[i] + b[i])
}

fn sub(a: F3, b: F3) -> F3 {
    std::array::from_fn(|i| a[i] - b[i])
}

fn scale(a: F3, k: f32) -> F3 {
    std::array::from_fn(|i| a[i] * k)
}

/// The dot product (`mDot`).
fn dot(a: F3, b: F3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The cross product `a x b` (`mCross`).
fn cross(a: F3, b: F3) -> F3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `v` shortened to `max` when it is longer (`len` its length).
fn at_most(v: F3, len: f32, max: f32) -> F3 {
    if len > max { scale(v, max / len) } else { v }
}

impl Player {
    /// `updateMove`, through the [`library`](crate::library) table.
    pub(super) fn update_move(
        &mut self,
        db: &PlayerData,
        mv: &Move,
        host: &mut dyn PlayerHost,
    ) -> Result<(), Exhausted> {
        (crate::library::get().player_update_move)(self, db, mv, host)
    }

    /// `updateMove` in plain arithmetic: [`PLAIN`](crate::library::PLAIN)'s entry.
    pub fn plain_update_move(
        &mut self,
        db: &PlayerData,
        mv: &Move,
        host: &mut dyn PlayerHost,
    ) -> Result<(), Exhausted> {
        // The last move and the image triggers 0 and 1.
        self.s.last_move = *mv;
        if self.s.damage_state == 0 {
            host.set_image_trigger(&mut self.s, 0, mv.triggers[0]);
            host.set_image_trigger(&mut self.s, 1, mv.triggers[1]);
        }
        // The subclass's pose.
        let class = host.player_class();
        let class: &dyn PlayerClass = class.as_deref().unwrap_or(&TorquePlayer);
        class.update_pose(self, db, mv, host)?;
        // Head and rotation.
        if self.s.damage_state == 0 {
            class.head_and_rotation(&mut self.s, db, mv, host);
        }
        // The yaw matrix and the wish.
        let m = set_euler([0.0, 0.0, self.s.rot[2]]);
        let mut w: F3 = [0.0; 3];
        let mut cap = 0.0f32;
        if self.s.action_state == 1 && self.s.damage_state == 0 {
            // Column 0 times x plus column 1 times y.
            w = std::array::from_fn(|i| m[4 * i] * mv.x + m[4 * i + 1] * mv.y);
            cap = class.move_speed(&self.s, db, mv);
            if self.s.raw_axes {
                w = [mv.x, mv.y, mv.z];
            }
            // A move stops landing and every action past the table.
            let a = db.actions;
            if w[0] + w[1] + w[2] != 0.0
                && (self.s.action_anim >= a.table || self.s.action_anim == a.land)
            {
                self.s.action_anim = a.none;
            }
        }
        // Gravity and the contact.
        let gz = GRAVITY * self.s.gravity_mod * DT;
        let mut acc: F3 = [0.0, 0.0, gz];
        let mut run = false;
        let mut jump = false;
        let mut normal: F3 = [0.0; 3];
        if self.s.mount.is_none() {
            class.find_contact(self, db, &mut run, &mut jump, &mut normal, host)?;
            if jump {
                self.s.jump_normal = normal;
            }
        }
        if run {
            class.run_surface(&mut self.s, db, &w, cap, &mut acc, normal);
        } else {
            self.s.contact_timer = self.s.contact_timer.wrapping_add(1);
            class.air_move(&mut self.s, db, &mut w, cap, &mut acc, gz);
        }
        // The jump.
        jump_step(&mut self.s, db, mv, &w, &mut acc, jump, class, host);
        // The subclass's thrust.
        class.thrust(&mut self.s, db, mv, &w, &mut acc);
        // Swimming.
        self.s.ceiling = false;
        class.swim(&mut self.s, db, mv, &mut w, &mut acc);
        // Integrate, resist, buoyancy, drag.
        class.integrate(&mut self.s, db, mv, &acc, run);
        // The dismount and liquid callbacks (server).
        if mv.triggers[class.dismount_trigger()] && self.s.mount.is_some() {
            host.do_dismount(&mut self.s);
        }
        if !self.s.in_liquid {
            if self.s.water_coverage != 0.0 {
                let (cov, ty) = (self.s.water_coverage, self.s.liquid_type);
                host.on_enter_liquid(&mut self.s, cov, ty);
                self.s.in_liquid = true;
            }
        } else if self.s.water_coverage == 0.0 {
            let ty = self.s.liquid_type;
            host.on_leave_liquid(&mut self.s, ty);
            self.s.in_liquid = false;
        }
        Ok(())
    }
}

/// The look and turn: the pitch is added and held to the look angles. With free look, a Player mounted on node 0 or seen
/// from the third person turns its head (held to the free-look angle); otherwise the body turns, the head's side angle eases
/// halfway back to the front each tick, and so does its pitch while the Player controls another object. The yaw is kept in
/// one turn, and the deltas record the tick's change.
pub(super) fn head_and_rotation(s: &mut PlayerState, db: &PlayerData, mv: &Move) {
    let yaw_before = s.rot[2];
    let head_before = s.head;
    let pitch = signed_turn(mv.pitch);
    s.head[0] = held(s.head[0] + pitch, db.min_look_angle, db.max_look_angle);
    let turn = signed_turn(mv.yaw);
    let third_person = s.client.is_some_and(|c| !c.is_first_person());
    let head_turns = mv.free_look && ((s.mount.is_some() && s.mount_node == 0) || third_person);
    if head_turns {
        let limit = db.max_freelook_angle;
        s.head[2] = held(s.head[2] + turn, -limit, limit);
    } else {
        s.rot[2] += turn;
        s.head[2] *= 0.5;
        if s.control_object.is_some() {
            s.head[0] *= 0.5;
        }
    }
    s.rot[2] = into_one_turn(s.rot[2]);
    s.delta_rot = s.rot;
    s.delta_rot_vec = [0.0, 0.0, short_way(yaw_before - s.rot[2])];
    s.delta_head = s.head;
    s.delta_head_vec = sub(head_before, s.head);
}

/// The wish's speed cap: the forward (or, for a backward or no move, the backward) speed times the move's `|y|`, or the side
/// speed times its `|x|`, whichever is larger; the underwater set from a coverage of 0.9 on.
pub(super) fn move_speed(s: &PlayerState, db: &PlayerData, mv: &Move) -> f32 {
    let [ahead, behind, side] = if f64::from(s.water_coverage) >= UNDERWATER {
        [
            db.max_underwater_forward_speed,
            db.max_underwater_backward_speed,
            db.max_underwater_side_speed,
        ]
    } else {
        [
            db.max_forward_speed,
            db.max_backward_speed,
            db.max_side_speed,
        ]
    };
    let sideways = side * mv.x.abs();
    let along = if mv.y > 0.0 {
        ahead * mv.y
    } else {
        behind * mv.y.abs()
    };
    larger(along, sideways)
}

/// The wish turned parallel to a surface with normal `n`: the normal's part along the horizontal across the wish is taken
/// out, and the wish loses its part along what is left. A zero wish stays zero.
fn along_surface(wish: F3, n: F3) -> F3 {
    let len = len3(wish);
    if len == 0.0 {
        return wish;
    }
    let across = scale(cross(wish, [0.0, 0.0, 1.0]), 1.0 / len);
    let tilt = sub(n, scale(across, dot(across, n)));
    sub(wish, scale(tilt, dot(wish, tilt)))
}

/// On a run surface with normal `n`: the gravity into the surface is taken back with a small extra push so the Player rests
/// (and a leftover too small to matter is cleared); with the run energy the wish (drained when moving), else nothing, is
/// turned along the surface and scaled to the cap; the run force accelerates toward it, at most `runForce / mass` per tick
/// (scaled while recovering). A jump action ends.
pub(super) fn run_surface(
    s: &mut PlayerState,
    db: &PlayerData,
    wish: &F3,
    cap: f32,
    acc: &mut F3,
    n: F3,
) {
    s.contact_timer = 0;
    let into = -dot(*acc, n);
    if into > 0.0 {
        *acc = add(*acc, scale(n, into + REST_PUSH));
        if len3(*acc) < REST_CLEAR {
            *acc = [0.0; 3];
        }
    }
    let mut target: F3 = [0.0; 3];
    if s.energy >= db.min_run_energy {
        if cap != 0.0 {
            s.energy -= db.run_energy_drain;
        }
        target = along_surface(*wish, n);
    }
    let len = len3(target);
    if len != 0.0 {
        target = scale(target, cap / len);
    }
    let push = sub(target, add(s.velocity, *acc));
    let mut most = (db.run_force / s.mass) * DT;
    if s.action_state == 2 {
        most *= db.recover_run_force_scale;
    }
    *acc = add(*acc, at_most(push, len3(push), most));
    if s.action_anim == db.actions.jump || s.action_anim == db.actions.stand_jump {
        s.action_anim = db.actions.none;
    }
}

/// The speed resistance: above `resist` a speed (no more than `max`) loses `factor * dt` of what it has over `resist`.
fn resisted(speed: f32, max: f32, resist: f32, factor: f32) -> f32 {
    let speed = if speed > max { max } else { speed };
    speed - factor * DT * (speed - resist)
}

/// The integration: the applied force (its acceleration over a tick) and `acc` into the velocity; the horizontal and upward
/// speed resistances; buoyancy as a lift, except for a Player with a buoyancy of 1 or less standing still on a run surface;
/// the container drag, `v * drag` over a tick; and the falling flag.
pub(super) fn integrate(s: &mut PlayerState, db: &PlayerData, acc: &F3, run: bool) {
    let pushed: F3 = std::array::from_fn(|i| (s.applied_force[i] / s.mass) * DT);
    s.velocity = add(s.velocity, add(*acc, pushed));
    let [vx, vy, _] = s.velocity;
    let flat = (vx * vx + vy * vy).sqrt();
    if flat > db.horiz_resist_speed {
        let kept = resisted(
            flat,
            db.horiz_max_speed,
            db.horiz_resist_speed,
            db.horiz_resist_factor,
        );
        let k = kept / flat;
        s.velocity[0] *= k;
        s.velocity[1] *= k;
    }
    if s.velocity[2] > db.up_resist_speed {
        s.velocity[2] = resisted(
            s.velocity[2],
            db.up_max_speed,
            db.up_resist_speed,
            db.up_resist_factor,
        );
    }
    if s.buoyancy != 0.0 {
        let still = torque_collide::math::is_zero(s.velocity);
        if s.buoyancy > 1.0 || !still || !run {
            s.velocity[2] -= s.buoyancy * GRAVITY * s.gravity_mod * DT;
        }
    }
    s.velocity = sub(s.velocity, scale(scale(s.velocity, s.drag), DT));
    // The vertical speed in object space.
    let wt = s.world_to_obj;
    let v = s.velocity;
    let up = wt[8] * v[0] + wt[9] * v[1] + wt[10] * v[2];
    s.falling = !run && up < FALL_SPEED;
}

/// `Player::canJump`: in the move state, enabled, not mounted, no jump delay, the energy for it, and a jump surface touched
/// within the last [`JUMP_SKIP_CONTACTS_MAX`] contacts.
pub fn can_jump(s: &PlayerState, db: &PlayerData) -> bool {
    s.action_state == 1
        && s.damage_state == 0
        && s.mount.is_none()
        && s.jump_delay == 0
        && s.energy >= db.min_jump_energy
        && s.jump_surface_last < JUMP_SKIP_CONTACTS_MAX
}

/// The jump: with the jump trigger and [`PlayerClass::can_jump`], an impulse of `jumpForce / mass` scaled down as the vertical
/// speed nears `maxJumpSpeed`, up the jump surface's normal (and along the wish when it faces away from the surface); else the
/// jump delay counts down on a jump surface and the contacts missed count up off one.
#[allow(clippy::too_many_arguments)]
fn jump_step(
    s: &mut PlayerState,
    db: &PlayerData,
    mv: &Move,
    w: &F3,
    acc: &mut F3,
    jump_surface: bool,
    class: &dyn PlayerClass,
    host: &mut dyn PlayerHost,
) {
    if s.jump_sound > 0 {
        s.jump_sound -= 1;
    }
    if mv.triggers[2] && s.mount.is_none() && class.can_jump(s, db) {
        let vz = s.velocity[2];
        if !(vz <= db.max_jump_speed) {
            return;
        }
        let z_speed_scale = if vz <= db.min_jump_speed {
            1.0
        } else {
            1.0 - (vz - db.min_jump_speed) / (db.max_jump_speed - db.min_jump_speed)
        };
        let mut pv = *w;
        let len = len3(pv);
        if len > 0.0 {
            pv = scale(pv, 1.0 / len);
        }
        // A smaller Player jumps higher in proportion to its size than a larger one.
        let scale_z = s.scale[2] * 0.25 + 0.75;
        let n = s.jump_normal;
        let dot = dot(pv, n);
        let impulse = db.jump_force / s.mass;
        if dot > 0.0 {
            acc[0] += pv[0] * impulse * dot;
            acc[1] += pv[1] * impulse * dot;
        }
        acc[2] += n[2] * scale_z * impulse * z_speed_scale;
        s.jump_delay = db.jump_delay;
        s.energy -= db.jump_energy_drain;
        let anim = if len3(s.velocity) < 0.5 { 9 } else { 8 };
        host.skeleton().set_action_thread(s, anim);
        let sk = host.skeleton();
        if sk.has_shape() && sk.has_thread(Thread::Action) {
            let pos = sk.thread_pos(Thread::Action);
            sk.advance_thread(Thread::Action, -pos);
        }
        s.jump_surface_last = JUMP_SKIP_CONTACTS_MAX;
        s.set_mask(net_mask::MOVE | net_mask::JUMP);
        return;
    }
    if jump_surface {
        if s.jump_delay > 0 {
            s.jump_delay -= 1;
        }
        s.jump_surface_last = 0;
    } else {
        s.jump_surface_last = s.jump_surface_last.wrapping_add(1);
    }
}
