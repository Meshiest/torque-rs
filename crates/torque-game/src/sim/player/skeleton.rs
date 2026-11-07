//! The Player's skeleton on the host (`::sim::player::Skeleton` over the shape instance and the Player threads), and the
//! server's animation steps around the move: `advanceThreads(dt)` before it, `updateAnimation` with `pickActionAnimation` and
//! `updateActionThread(true)` after it.
//!
//! The action the server picks rides in the ghost record (`action`, mask), so the choice is made the same way every time.
//! The class picks it ([`PlayerKind::pick_action`](crate::sim_classes::PlayerKind::pick_action)); TGE's: falling plays the
//! table's fall action; without a contact for `sContactTickTime` ticks, `root`; otherwise the moving action (run, back or
//! side) whose ground direction best fits the velocity past 0.1, side also backwards (`forward` false); a class with
//! crouching actions moves 1..3 onto them while crouched. A mounted Player goes back to `root`. The fixed actions' numbers
//! are the class's action table's ([`ActionTable`](crate::scene::shapebase::ActionTable)): TGE's by default. The thread
//! rate follows the velocity along the action's direction over its speed, at most `maxTimeScale`.
//!
//! A synced Player (one with hitboxes) follows its clients' rules instead ([`synced`]): the action thread steps every tick
//! after the pick, by the ground covered for a moving action, a jump plays on the rise, a new action blends in, the spine
//! bones are let go, and it walks at walking pace when its shape has a walk (the engine's own, [`synced::walk_or`]).
//!
//! The shape work (`animate`, thread steps) is counted here and charged to the run by the tick when it next holds the `Vm`.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/player.cpp`
//! (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

// The comparisons are written so that a NaN takes a fixed side (`!(a <= b)` is "above, or unordered").
#![allow(clippy::neg_cmp_op_on_partial_ord)]
use std::rc::Rc;

use ::sim::player::{PlayerState, Skeleton, Thread};
use collide::math::{IDENTITY, Matrix, mul_v};
use shapes::instance::ThreadId;

use super::PlayerInfo;
use super::ground;
use super::synced;
use crate::scene::shapebase::{
    ActionRequest, PlayerThreads, ShapeBaseState, action_thread, update_node_masks,
};
use crate::sim_classes::ActionPick;

/// The root action's number (every table's first); falling and landing are the table's.
const ROOT: u32 = 0;
/// The root and the moving actions (`run`, `back`, `side`): a synced pose steps the moving ones by the ground covered.
const MOVES: u32 = 4;

/// The skeleton of one Player for one tick.
pub struct HostSkeleton {
    pub st: Option<Box<ShapeBaseState>>,
    pub info: Rc<PlayerInfo>,
    /// The shape work done, charged by the tick.
    pub work: u64,
    /// `isFirstPerson`: a controlling client whose control object this is, in first person.
    pub first_person: bool,
    /// The pose is synced with the clients' ([`synced`]).
    pub synced: bool,
    /// The step's seconds (`advanceThreads`').
    dt: f32,
}

impl HostSkeleton {
    pub fn new(st: Option<Box<ShapeBaseState>>, info: Rc<PlayerInfo>) -> HostSkeleton {
        HostSkeleton {
            st,
            info,
            work: 0,
            first_person: false,
            synced: false,
            dt: 0.0,
        }
    }

    /// The skeleton with its pose synced with the clients' or not ([`synced`]).
    pub fn synced(mut self, on: bool) -> HostSkeleton {
        self.synced = on;
        self
    }

    fn threads(&self) -> Option<&PlayerThreads> {
        self.st.as_ref()?.player.as_ref()
    }

    fn thread_id(&self, t: Thread) -> Option<ThreadId> {
        let p = self.threads()?;
        match t {
            Thread::Arm => p.arm,
            Thread::Head => p.head,
            Thread::HeadSide => p.head_side,
            Thread::Class(n) => p.class_threads.get(n as usize).copied().flatten(),
            Thread::Action => p.action_thread,
        }
    }

    /// The instance and its shape, for a thread operation.
    fn with_thread<R>(
        &mut self,
        t: Option<ThreadId>,
        f: impl FnOnce(&mut shapes::instance::ShapeInstance, &shapes::Shape, ThreadId) -> R,
    ) -> Option<R> {
        let t = t?;
        let body = self.st.as_mut()?.body.as_mut()?;
        let kept = Rc::clone(&body.kept);
        let shape = kept.shape.as_ref()?;
        Some(f(&mut body.instance, shape, t))
    }

    fn advance_time(&mut self, t: Option<ThreadId>, dt: f32) {
        self.work = self.work.saturating_add(1);
        self.with_thread(t, |inst, shape, t| inst.advance_time(shape, t, dt));
    }

    fn pos_of(&self, t: Option<ThreadId>) -> Option<f32> {
        let body = self.st.as_ref()?.body.as_ref()?;
        body.instance.thread(t?).map(|th| th.pos)
    }

    /// `setActionThread(action, forward, hold, wait, fsp, force false)`, keeping the state's action in step.
    fn set_action(
        &mut self,
        s: &mut PlayerState,
        action: u32,
        forward: bool,
        hold: bool,
        wait: bool,
        fsp: bool,
    ) {
        // A synced walk and run keep their place in the loop, as the clients' do.
        let now = self.threads().map(|p| (p.action as usize, p.forward));
        let walk = self.info.walk.map(|w| w as usize);
        let start = now
            .filter(|&n| self.synced && synced::keeps_phase(n, (action as usize, forward), walk))
            .and_then(|_| self.pos_of(self.threads().and_then(|p| p.action_thread)));
        // A synced action that only turns around (a walk forward into one backward) is set again, as the clients set it.
        let force = self.synced && now == Some((action as usize, !forward));
        // An action of another pose blends in slower and eased, as the clients blend it.
        let sets = &self.info.pose_sets;
        let pose_blend = now.and_then(|(a, _)| super::poses::blend_for(sets, a, action as usize));
        let Some(st) = self.st.as_mut() else {
            return;
        };
        let request = ActionRequest {
            forward,
            hold,
            wait,
            fsp,
            force,
            blend: self.synced,
            pose_blend,
            start,
        };
        // The out-of-range line needs the console; the server's own calls never ask past the table.
        let _ = action_thread(st, action as usize, request);
        if let Some(p) = st.player.as_ref() {
            s.action_anim = p.action;
        }
    }

    /// `pickActionAnimation`: the class picks the action
    /// ([`PlayerKind::pick_action`](crate::sim_classes::PlayerKind::pick_action)).
    fn pick_action(&mut self, s: &mut PlayerState) {
        if s.action_state != 1 || s.damage_state != 0 {
            return;
        }
        let table = self.info.actions;
        let action_now = self.threads().map_or(table.none, |p| p.action);
        if s.mount.is_some() {
            if action_now == ROOT || action_now as usize >= table.fixed.len() {
                return;
            }
            self.set_action(s, ROOT, true, false, false, false);
            return;
        }
        let mut moves = [None; 4];
        for (i, slot) in moves.iter_mut().enumerate().skip(1) {
            let seq = self
                .threads()
                .and_then(|p| p.actions.get(i))
                .map_or(-1, |a| a.sequence);
            *slot = self
                .info
                .motions
                .get(i)
                .filter(|m| seq != -1 && m.speed != 0.0)
                .map(|m| m.dir);
        }
        let pick = ActionPick {
            s,
            table,
            velocity: mul_v(&s.world_to_obj, s.velocity),
            moves,
            first_person: self.first_person,
            held: self.threads().is_some_and(|p| p.hold),
            synced: self.synced,
        };
        let Some((mut action, mut forward)) = self.info.class.pick_action(&pick) else {
            return;
        };
        // A synced Player walks at walking pace, by its clients' rule (the engine's own).
        if self.synced
            && let Some(walk) = self.info.walk
            && let Some(m) = self.info.motions.get(action as usize)
        {
            let along = pick.velocity[0] * m.dir[0]
                + pick.velocity[1] * m.dir[1]
                + pick.velocity[2] * m.dir[2];
            let speed_of = |i: usize| self.info.motions.get(i).map(|m| m.speed);
            let now = self
                .threads()
                .map_or((usize::MAX, true), |p| (p.action as usize, p.forward));
            let (a, f) = synced::walk_or(
                action as usize,
                forward,
                along,
                walk as usize,
                speed_of,
                now,
            );
            (action, forward) = (u32::try_from(a).unwrap_or(action), f);
        }
        // A synced Player plays the way down or up between the crouch and prone to its end first, as the clients do.
        let now = self.threads().map_or(usize::MAX, |p| p.action as usize);
        let armed = super::poses::armed(
            self.threads()
                .and_then(|p| p.actions.get(p.arm_action as usize))
                .map(|a| a.name.as_slice()),
        );
        if self.synced
            && !s.falling
            && let Some(way) = self.info.pose_sets.way(now, s.pose, armed)
        {
            self.set_action(s, way as u32, true, false, true, false);
            return;
        }
        // A pose with actions of its own picks among them (the engine's poses, [`poses`](super::poses)); falling keeps the fall.
        if !s.falling
            && let Some(set) = self.info.pose_sets.of_pose(s.pose)
        {
            let motions = &self.info.motions;
            let dir_of = |a: usize| motions.get(a).map_or([0.0; 3], |m| m.dir);
            if let Some((a, f)) = super::poses::pick_in(set, pick.velocity, dir_of) {
                (action, forward) = (u32::try_from(a).unwrap_or(action), f);
            }
        }
        self.set_action(s, action, forward, false, false, false);
    }

    /// A synced pose's jump: an upward speed passing the rise speed plays `jump` (`standjump` slow along the ground, when the
    /// shape has it) to its end, as the clients play it ([`synced::rose`]).
    fn synced_jump(&mut self, s: &mut PlayerState) {
        let Some(p) = self.threads() else {
            return;
        };
        let last = p.synced_from.map_or(0.0, |f| f.1);
        if p.hold || !synced::rose(last, s.velocity[2]) {
            return;
        }
        let table = self.info.actions;
        let has = |a: u32| p.actions.get(a as usize).is_some_and(|x| x.sequence != -1);
        let action = if synced::standing_jump(s.velocity) && has(table.stand_jump) {
            table.stand_jump
        } else {
            table.jump
        };
        self.set_action(s, action, true, false, true, true);
    }

    /// A synced pose's action thread after the pick: a moving action at the rate of the ground covered since the last step
    /// ([`synced::step_scale`]), any other at its own; then the step. The position and the upward speed are kept for the next.
    fn synced_step(&mut self, s: &mut PlayerState) {
        let position = [s.obj_to_world[3], s.obj_to_world[7], s.obj_to_world[11]];
        let dt = self.dt;
        let none = self.info.actions.none;
        let Some(p) = self.st.as_mut().and_then(|st| st.player.as_mut()) else {
            return;
        };
        let from = p.synced_from.replace((position, s.velocity[2]));
        let (action, forward, th) = (p.action, p.forward, p.action_thread);
        if action == none {
            return;
        }
        let mut scale = 1.0f32;
        let walking =
            self.info.walk == Some(action) || self.info.pose_sets.is_moving(action as usize);
        if (action < MOVES || walking)
            && let Some(m) = self.info.motions.get(action as usize)
            && (m.velocity_scale || walking)
            && m.speed != 0.0
        {
            let moved = from.map_or(s.velocity.map(|v| v * dt), |(f, _)| {
                [0, 1, 2].map(|i| position[i] - f[i])
            });
            let step = mul_v(&s.world_to_obj, moved);
            // The walk is held to no maxTimeScale (its limit keeps it near 1), as on the clients.
            let max = if walking {
                synced::WALK_MAX_TIME_SCALE
            } else {
                self.info.max_time_scale
            };
            scale = synced::step_scale(step, dt, m.dir, m.speed, max);
        }
        self.with_thread(th, |inst, _, t| {
            if let Some(th) = inst.thread_mut(t) {
                th.time_scale = if forward { scale } else { -scale };
            }
        });
        self.advance_time(th, dt);
    }

    /// The action's or the pose's own version of the arm action (the engine's poses, [`poses::pose_arm`](super::poses::pose_arm)) on the arm
    /// thread, at the place the arm is, as the clients play it.
    fn pose_arm(&mut self, s: &PlayerState) {
        let Some(st) = self.st.as_mut() else {
            return;
        };
        let (Some(body), Some(p)) = (st.body.as_mut(), st.player.as_ref()) else {
            return;
        };
        let (Some(t), Some(a)) = (p.arm, p.actions.get(p.arm_action as usize)) else {
            return;
        };
        let names = p
            .actions
            .iter()
            .map(|a| (a.name.as_slice(), a.sequence != -1));
        let action = p
            .actions
            .get(p.action as usize)
            .map_or(&[][..], |a| a.name.as_slice());
        let played =
            super::poses::pose_arm(names, &a.name, s.pose, action).unwrap_or(p.arm_action as usize);
        let Some(seq) = p
            .actions
            .get(played)
            .map(|a| a.sequence)
            .and_then(|q| usize::try_from(q).ok())
        else {
            return;
        };
        let kept = Rc::clone(&body.kept);
        let Some(shape) = kept.shape.as_ref() else {
            return;
        };
        let Some(th) = body.instance.thread(t) else {
            return;
        };
        if th.sequence != Some(seq) {
            let pos = th.pos;
            body.instance.set_sequence(shape, t, seq as i32, pos);
            self.work = self.work.saturating_add(1);
        }
    }

    /// `updateAnimation` on a server.
    fn update_animation(&mut self, s: &mut PlayerState) {
        let none = self.info.actions.none;
        let action = self.threads().map_or(none, |p| p.action);
        if action != none {
            let th = self.threads().and_then(|p| p.action_thread);
            let pos = self.pos_of(th);
            let forward = self.threads().is_some_and(|p| p.forward);
            let at_end = if forward {
                pos == Some(1.0)
            } else {
                pos.is_none_or(|p| p == 0.0)
            };
            if let Some(p) = self.st.as_mut().and_then(|st| st.player.as_mut()) {
                p.at_end = at_end;
            }
        }
        // The spawn guard: one less each tick, gone while mounted.
        if s.impact_block != 0 {
            s.impact_block = if s.mount.is_none() {
                s.impact_block - 1
            } else {
                0
            };
        }
        if s.impact_block == 0
            && let Some(p) = self.st.as_mut().and_then(|st| st.player.as_mut())
        {
            p.action_counter = p.action_counter.wrapping_sub(1);
        }
        let (wait, at_end) = self
            .threads()
            .map_or((false, false), |p| (p.wait, p.at_end));
        let mut action = action;
        let land = self.info.actions.land;
        if action == none || action == land || !wait || at_end {
            self.pick_action(s);
            action = self.threads().map_or(none, |p| p.action);
            if action == land || action == none {
                return;
            }
        }
        // The thread's rate: 1, or the velocity along the action's direction over its speed (at most `maxTimeScale`); backwards
        // negated.
        let mut scale = 1.0f32;
        if let Some(m) = self.info.motions.get(action as usize)
            && (m.velocity_scale || self.info.pose_sets.is_moving(action as usize))
            && m.speed != 0.0
        {
            let v = mul_v(&s.world_to_obj, s.velocity);
            scale = (crate::library::get().action_rate)(v, m.dir, m.speed);
            if self.info.max_time_scale < scale {
                scale = self.info.max_time_scale;
            }
        }
        let forward = self.threads().is_some_and(|p| p.forward);
        if !forward {
            scale = f32::from_bits(scale.to_bits() ^ 0x8000_0000);
        }
        let th = self.threads().and_then(|p| p.action_thread);
        self.with_thread(th, |inst, _, t| {
            if let Some(th) = inst.thread_mut(t) {
                th.time_scale = scale;
            }
        });
    }
}

impl Skeleton for HostSkeleton {
    fn has_shape(&self) -> bool {
        self.st.as_ref().is_some_and(|s| s.body.is_some())
    }

    fn animate(&mut self) {
        let Some(body) = self.st.as_mut().and_then(|s| s.body.as_mut()) else {
            return;
        };
        let kept = Rc::clone(&body.kept);
        let Some(shape) = kept.shape.as_ref() else {
            return;
        };
        let mut spent = 0u64;
        let _ = body.instance.animate(shape, 0, None, &mut |units| {
            spent = spent.saturating_add(units);
            true
        });
        self.work = self.work.saturating_add(spent);
    }

    fn node_transform(&self, node: i32) -> Option<Matrix> {
        let body = self.st.as_ref()?.body.as_ref()?;
        body.instance
            .node_transform(usize::try_from(node).ok()?)
            .copied()
    }

    fn eye_node(&self) -> i32 {
        self.st
            .as_ref()
            .and_then(|s| s.body.as_ref())
            .and_then(|b| b.kept.facts.as_ref())
            .map_or(-1, |f| f.eye_node)
    }

    fn mount_node(&self, point: u32) -> i32 {
        self.st
            .as_ref()
            .and_then(|s| s.body.as_ref())
            .and_then(|b| b.kept.facts.as_ref())
            .and_then(|f| f.mount_nodes.get(point as usize).copied())
            .unwrap_or(-1)
    }

    fn has_thread(&self, t: Thread) -> bool {
        self.thread_id(t).is_some()
    }

    fn thread_pos(&self, t: Thread) -> f32 {
        self.pos_of(self.thread_id(t)).unwrap_or(0.0)
    }

    fn advance_thread(&mut self, t: Thread, delta: f32) {
        let id = self.thread_id(t);
        self.work = self.work.saturating_add(1);
        self.with_thread(id, |inst, shape, t| inst.advance_pos(shape, t, delta));
    }

    fn thread_scale(&self, t: Thread) -> f32 {
        let Some(body) = self.st.as_ref().and_then(|s| s.body.as_ref()) else {
            return 0.0;
        };
        self.thread_id(t)
            .and_then(|t| body.instance.thread(t))
            .map_or(0.0, |th| th.time_scale)
    }

    fn set_thread_scale(&mut self, t: Thread, scale: f32) {
        let id = self.thread_id(t);
        self.with_thread(id, |inst, _, t| {
            if let Some(th) = inst.thread_mut(t) {
                th.time_scale = scale;
            }
        });
    }

    fn set_thread_sequence(&mut self, t: Thread, sequence: i32, pos: f32) {
        let id = self.thread_id(t);
        self.with_thread(id, |inst, shape, t| {
            inst.set_sequence(shape, t, sequence, pos)
        });
    }

    fn thread_in_transition(&self, t: Thread) -> bool {
        // TGE's server never starts one (`setActionThread`'s transition branch is a ghost's); a synced pose does.
        let Some(body) = self.st.as_ref().and_then(|s| s.body.as_ref()) else {
            return false;
        };
        self.thread_id(t)
            .and_then(|t| body.instance.thread(t))
            .is_some_and(|th| th.transition.in_transition)
    }

    fn root_displacement(&mut self, from: f32, to: f32) -> Matrix {
        self.work = self.work.saturating_add(ground::GROUND_WORK);
        let th = self.thread_id(Thread::Action);
        let r = self.with_thread(th, |inst, shape, t| {
            let seq = inst.thread(t).and_then(|th| th.sequence)?;
            let seq = shape.sequences.get(seq)?;
            Some(ground::displacement(shape, seq, from, to, &IDENTITY))
        });
        r.flatten().unwrap_or(IDENTITY)
    }

    fn set_action_thread(&mut self, s: &mut PlayerState, anim: u32) {
        self.set_action(s, anim, true, false, true, true);
    }

    /// `advanceThreads(dt)` on a server: the action thread steps only while its first-person flag is set (a synced pose steps it
    /// after the pick instead); then the class's own threads
    /// ([`PlayerKind::advance_class_threads`](crate::sim_classes::PlayerKind::advance_class_threads)).
    fn advance_before(&mut self, _s: &mut PlayerState, dt: f32) {
        self.dt = dt;
        let Some(p) = self.threads() else {
            return;
        };
        let (fsp, action) = (p.fsp, p.action_thread);
        if fsp && !self.synced {
            self.advance_time(action, dt);
        }
        let class = Rc::clone(&self.info.class);
        if let Some(st) = self.st.as_mut()
            && let (Some(body), Some(p)) = (st.body.as_mut(), st.player.as_ref())
        {
            let work = class.advance_class_threads(body, &p.class_threads, dt);
            self.work = self.work.saturating_add(work);
        }
    }

    /// `updateAnimation` and `updateActionThread(true)`.
    fn advance_after(&mut self, s: &mut PlayerState) {
        if self.synced {
            self.synced_jump(s);
        }
        self.update_animation(s);
        if self.synced {
            self.synced_step(s);
        }
        self.pose_arm(s);
        let fsp = self.threads().is_some_and(|p| p.fsp) || self.synced;
        if let Some(body) = self.st.as_mut().and_then(|st| st.body.as_mut()) {
            update_node_masks(body, fsp);
        }
    }
}
