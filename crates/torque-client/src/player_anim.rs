//! A Player ghost's animation on the client, as TGE's client plays it: the action thread and the arm thread of
//! `Player::updateAnimation`.
//!
//! **The action list** ([`action_list`]) is `PlayerData::preload`'s: TGE's table (`root`, `run`, `back`, `side`, `fall`, `jump`,
//! `standjump`, `land`), each found by name, then every other sequence of the shape in its order; each action's direction and
//! speed come from its sequence's ground transform over one second (`getGroundInfo`), the table's direction at speed 1 when the
//! sequence has no ground motion. The server numbers its actions the same way, so an arm action a record carries is an index
//! into this list.
//!
//! **The action thread** ([`PlayerAnimation::update`]): unless the action playing waits for its end and has not reached it,
//! `pickActionAnimation` chooses: a falling Player plays `fall`; one on the ground the moving action (`run`, `back`, `side`)
//! whose direction best matches its velocity in its own frame (a dot product over 0.1, the back of an action counting as that
//! action played backwards), else `root`. A jump the record's move asks for while the Player leaves the ground plays `jump`
//! (`standjump` under 0.5 units a second) to its end. A new action blends in over [`TRANSITION`] seconds
//! (`transitionToSequence`); an action the shape has no sequence for changes nothing. A moving action's rate is the velocity
//! along its direction over its speed, at most `maxTimeScale` ([`MAX_TIME_SCALE`]), negative backwards
//! (`updateActionThread`).
//!
//! **The arm thread** plays the arm action the record names (the server's `setArmThread`; `look` when the shape has it and
//! none was named), held at the head's pitch over the look range (`updateLookAnimation`): 0 looking fully up, 1 fully down.
//!
//! Credit: follows TGE 1.4's `game/player.cc` (`pickActionAnimation`, `setActionThread`, `updateActionThread`,
//! `updateLookAnimation`, `PlayerData::preload`; Copyright (C) GarageGames.com, Inc.), whose MIT-licensed successor is
//! Torque3D's `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code is this
//! crate's own.

use collide::math::F3;
use ghosts::TgeRecord;
use ghosts::player::VELOCITY_NORMAL_BITS;
use shapes::Shape;
use shapes::instance::{ShapeInstance, ThreadId, find_sequence};
use torque_game::scene::shapebase::{TGE_ACTIONS, player_actions};
use torque_game::sim::player::ground::{ActionMotion, ground_info};

/// `sAnimationTransitionTime`: the seconds a new action blends in over.
pub const TRANSITION: f32 = 0.25;
/// `PlayerData::maxTimeScale`'s default: the fastest a moving action plays.
pub const MAX_TIME_SCALE: f32 = 1.5;
/// The smallest match a moving action must beat to be chosen over `root`.
pub const MIN_MATCH: f32 = 0.1;
/// Below this speed a jump is a standing jump.
pub const STAND_JUMP_SPEED: f32 = 0.5;

/// TGE's action numbers.
pub mod action {
    pub const ROOT: usize = 0;
    /// The moving actions are 1 to 3 (`run`, `back`, `side`).
    pub const FIRST_MOVE: usize = 1;
    pub const MOVES: usize = 4;
    pub const FALL: usize = 4;
    pub const JUMP: usize = 5;
    pub const STAND_JUMP: usize = 6;
    pub const LAND: usize = 7;
}

/// One action of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub name: Vec<u8>,
    /// Its sequence, `None` when the shape has none of that name.
    pub sequence: Option<usize>,
    pub motion: ActionMotion,
}

/// `PlayerData::preload`'s action list for `shape` (see the module docs).
pub fn action_list(shape: &Shape) -> Vec<Action> {
    let table = &TGE_ACTIONS;
    player_actions(shape, table)
        .into_iter()
        .enumerate()
        .map(|(i, a)| {
            let fixed = i < table.fixed.len();
            let saved = table.dirs.get(i).copied().unwrap_or([0.0; 3]);
            let sequence = usize::try_from(a.sequence).ok();
            let still = ActionMotion {
                dir: saved,
                speed: 0.0,
                velocity_scale: fixed,
                death: false,
            };
            let motion = sequence
                .and_then(|seq| ground_info(shape, &a.name, seq, saved, fixed, &mut |_| true))
                .unwrap_or(still);
            Action {
                name: a.name,
                sequence,
                motion,
            }
        })
        .collect()
}

/// What a frame of a Player's animation reads from its ghost.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlayerMotion {
    /// The velocity in the world.
    pub velocity: F3,
    /// The yaw (about z): the Player's own frame.
    pub yaw: f32,
    pub falling: bool,
    /// The jump trigger of the Player's last move.
    pub jumping: bool,
    /// The arm action the record named (`setArmThread`).
    pub arm_action: Option<u32>,
    /// The head's pitch over the look range, 0 (up) to 1 (down).
    pub look: f32,
}

impl PlayerMotion {
    /// The motion a Player record carries (`None` for another record, or one without its move block): the velocity, the
    /// yaw, the falling flag, the move's jump trigger, the arm action, and the head's pitch fraction (-1 to 1 of the
    /// datablock's `maxLookAngle`) as the look position. `yaw` is the ghost's (its pose's).
    pub fn of_record(record: &TgeRecord, yaw: f32) -> Option<PlayerMotion> {
        let (TgeRecord::Player(p) | TgeRecord::AiPlayer(p)) = record else {
            return None;
        };
        let m = p.motion.as_ref()?;
        let head = ghost_world::tge::head_fractions(record).map_or(0.0, |h| h[0]);
        Some(PlayerMotion {
            velocity: m
                .velocity
                .map_or([0.0; 3], |v| v.value(VELOCITY_NORMAL_BITS)),
            yaw,
            falling: m.falling,
            jumping: m.mv.triggers.get(2).copied().unwrap_or(false),
            arm_action: p.arm,
            look: ((head + 1.0) * 0.5).clamp(0.0, 1.0),
        })
    }

    /// The velocity in the Player's own frame (`mWorldToObj.mulV`: the yaw undone; a Player of yaw `a` faces `(sin a, cos a)`).
    pub fn local_velocity(&self) -> F3 {
        let (s, c) = self.yaw.sin_cos();
        let [x, y, z] = self.velocity;
        [x * c - y * s, x * s + y * c, z]
    }
}

/// One Player ghost's action and arm threads.
#[derive(Debug, Clone)]
pub struct PlayerAnimation {
    actions: Vec<Action>,
    action: usize,
    forward: bool,
    /// The action plays to its end before another is picked.
    wait: bool,
    action_thread: ThreadId,
    arm_thread: Option<ThreadId>,
    arm_action: Option<usize>,
    was_falling: bool,
}

impl PlayerAnimation {
    /// The threads on `instance` (a new instance of `shape`): the arm thread on `look` when the shape has it, then the action
    /// thread on `root`. `None` when the instance has no room for a thread.
    pub fn new(shape: &Shape, instance: &mut ShapeInstance) -> Option<PlayerAnimation> {
        let actions = action_list(shape);
        let look = actions
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(b"look"));
        let (arm_thread, arm_action) = match look.and_then(|i| Some((i, actions[i].sequence?))) {
            Some((i, seq)) => {
                let t = instance.create_thread(shape)?;
                instance.set_sequence(shape, t, seq as i32, 0.5);
                (Some(t), Some(i))
            }
            None => (None, None),
        };
        let action_thread = instance.create_thread(shape)?;
        let root = actions
            .first()
            .and_then(|a| a.sequence)
            .or_else(|| find_sequence(shape, b"root"));
        if let Some(seq) = root {
            instance.set_sequence(shape, action_thread, seq as i32, 0.0);
        }
        Some(PlayerAnimation {
            actions,
            action: action::ROOT,
            forward: true,
            wait: false,
            action_thread,
            arm_thread,
            arm_action,
            was_falling: false,
        })
    }

    /// The action list.
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// The action playing and whether it plays forward.
    pub fn action(&self) -> (usize, bool) {
        (self.action, self.forward)
    }

    /// The arm action playing.
    pub fn arm_action(&self) -> Option<usize> {
        self.arm_action
    }

    /// `setActionThread(action, forward)`: nothing for the action already playing that way, or one without a sequence.
    fn set_action(
        &mut self,
        shape: &Shape,
        instance: &mut ShapeInstance,
        action: usize,
        forward: bool,
        wait: bool,
    ) {
        if self.action == action && self.forward == forward {
            return;
        }
        let Some(seq) = self.actions.get(action).and_then(|a| a.sequence) else {
            return;
        };
        self.action = action;
        self.forward = forward;
        self.wait = wait;
        let pos = if forward { 0.0 } else { 1.0 };
        instance.transition_to_sequence(shape, self.action_thread, seq, pos, TRANSITION);
    }

    /// `pickActionAnimation`.
    fn pick(&mut self, shape: &Shape, instance: &mut ShapeInstance, m: &PlayerMotion) {
        if m.falling {
            self.set_action(shape, instance, action::FALL, true, false);
            return;
        }
        let vel = m.local_velocity();
        let (mut best, mut chosen, mut forward) = (MIN_MATCH, action::ROOT, true);
        for i in action::FIRST_MOVE..action::MOVES.min(self.actions.len()) {
            let a = &self.actions[i];
            if a.sequence.is_none() || a.motion.speed == 0.0 {
                continue;
            }
            let d = vel[0] * a.motion.dir[0] + vel[1] * a.motion.dir[1] + vel[2] * a.motion.dir[2];
            if d > best {
                (best, chosen, forward) = (d, i, true);
            } else if -d > best {
                (best, chosen, forward) = (-d, i, false);
            }
        }
        self.set_action(shape, instance, chosen, forward, false);
    }

    /// Whether the action thread is at its end (its start, backwards).
    fn at_end(&self, instance: &ShapeInstance) -> bool {
        instance.thread(self.action_thread).is_some_and(|t| {
            if self.forward {
                t.pos >= 1.0
            } else {
                t.pos <= 0.0
            }
        })
    }

    /// One frame of `dt` seconds with the ghost's motion `m` (see the module docs), then the threads advanced. The caller
    /// animates the instance after.
    pub fn update(
        &mut self,
        shape: &Shape,
        instance: &mut ShapeInstance,
        m: &PlayerMotion,
        dt: f32,
    ) {
        let took_off = m.falling && !self.was_falling;
        self.was_falling = m.falling;
        if took_off && m.jumping {
            let speed = m.velocity.iter().map(|v| v * v).sum::<f32>().sqrt();
            let jump = if speed < STAND_JUMP_SPEED {
                action::STAND_JUMP
            } else {
                action::JUMP
            };
            self.set_action(shape, instance, jump, true, true);
        }
        if !self.wait || self.at_end(instance) {
            self.wait = false;
            self.pick(shape, instance, m);
        }
        let a = &self.actions[self.action.min(self.actions.len() - 1)];
        let mut scale = 1.0;
        if self.action < action::MOVES && a.motion.velocity_scale && a.motion.speed != 0.0 {
            let v = m.local_velocity();
            let along = v[0] * a.motion.dir[0] + v[1] * a.motion.dir[1] + v[2] * a.motion.dir[2];
            scale = (along / a.motion.speed).abs().min(MAX_TIME_SCALE);
        }
        if let Some(t) = instance.thread_mut(self.action_thread) {
            t.time_scale = if self.forward { scale } else { -scale };
        }
        instance.advance_time(shape, self.action_thread, dt);

        if let Some(arm) = m.arm_action.and_then(|a| usize::try_from(a).ok())
            && self.arm_action != Some(arm)
            && let Some(seq) = self.actions.get(arm).and_then(|a| a.sequence)
        {
            let t = match self.arm_thread {
                Some(t) => t,
                None => match instance.create_thread(shape) {
                    Some(t) => t,
                    None => return,
                },
            };
            self.arm_thread = Some(t);
            self.arm_action = Some(arm);
            instance.set_sequence(shape, t, seq as i32, m.look);
        }
        if let (Some(t), Some(seq)) = (
            self.arm_thread,
            self.arm_action.and_then(|a| self.actions.get(a)?.sequence),
        ) {
            instance.set_sequence(shape, t, seq as i32, m.look);
        }
    }
}

#[cfg(test)]
#[path = "player_anim_tests.rs"]
mod tests;
