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
//! whose direction best matches its velocity in its own frame (a dot product over 0.1, the back of `side` counting as `side`
//! played backwards: a step the other way), else `root`. A jump the record's move asks for while the Player leaves the ground
//! plays `jump` (`standjump` under 0.5 units a second along the ground, when the shape has one) to its end; so does one whose
//! upward speed passes [`JUMP_RISE_SPEED`] (TGE's client starts the jump as it runs the ghost's move; a record seldom carries
//! the move's jump trigger together with the take-off, and `falling` is set only once the Player is coming down fast, so the
//! rise is what the client sees of a jump). An action the server set (a record's action, one past the
//! table: a script's `setActionThread`, such as a death) plays forward from its start, or at its end when the record says the
//! server's has ended, and is played to its end before another is picked; one the server holds at its end (`holdAtEnd`) stays
//! there until the server sets another. A new action blends in over [`TRANSITION`] seconds
//! (`transitionToSequence`); an action the shape has no sequence for changes nothing. A moving action's rate is the velocity
//! along its direction over its speed, at most `maxTimeScale` ([`MAX_TIME_SCALE`]), negative backwards
//! (`updateActionThread`). A Player that gets off its mount ([`PlayerMotion::mounted`]) plays `root` forward, neither held nor
//! waited for (`onUnmount`), so its moves pick its actions again; one that gets on takes the action its record carries anew.
//!
//! **The walk** (the engine's own, not TGE's): a shape with a `walk` sequence that moves it forward (one of the actions after
//! the table, found by name: [`WALK`]) walks at walking pace instead of playing `run` slowly. Where TGE's pick chooses `run`
//! and the speed along its direction is under the walk's and the run's speeds' geometric mean ([`walk_limit`], at which both
//! play equally far from their own pace), `walk` plays forward; where it chooses `back` and the speed backward is under the
//! mean of the walk's and the back's, `walk` plays backward (a walk backward is the walk's steps in reverse). The limit has a
//! margin ([`WALK_MARGIN`]) either way so a speed hovering at it does not flip the two. A change between `walk` and `run`
//! keeps the thread's position (the walk's feet land where the run's legs reach furthest forward) and blends over
//! [`TRANSITION`] seconds like any other. The walk's rate is the speed over its own, held to
//! [`synced::WALK_MAX_TIME_SCALE`] rather than `maxTimeScale` (the limit keeps it near 1). An unheld walk a record carries
//! changes nothing: the client picks its walk itself. `side` stays TGE's at any speed, slowed by it. A shape without `walk`, or whose `walk` has no forward
//! ground motion, keeps TGE's pick. A synced Player walks by the same rule as its server ([`synced::walk_or`]), its walk
//! stepped by the ground covered.
//!
//! **Synced** ([`PlayerAnimation::set_synced`], a host's opt-in for a server whose Players have hitboxes): the rules above are the
//! ones a hitbox Player's server follows too ([`torque_game::sim::player::synced`]), and a moving action steps by the ground
//! the Player was drawn covering ([`PlayerMotion::position`]) along its direction over its speed instead of by the velocity, so
//! its phase follows the server's path rather than the client's clock.
//!
//! **Poses** (the engine's own after Torque3D's, [`torque_game::sim::player::poses`]): a Player crouched, prone, sprinting or swimming
//! ([`PlayerMotion::pose`]: the client's own as it predicts it, another's as its record's move shows it, [`record_pose`])
//! plays its pose's own actions when the shape has them (`crouch_root`, `crouch_forward`, `crouch_backward`, `crouch_side`,
//! `prone_root`, `swim_root`, ...), picked as the standing ones are; its server picks by the same rule. An action of another pose blends in
//! over a longer, eased transition ([`poses::blend_for`]), so the body lowers and rises (into or out of the water with the limbs
//! swinging, [`poses::swings`], so the arms over the head come down by the sides); between the crouch and prone the shape's
//! way down or up (`prone_down`, `prone_up`; `prone_down_armed`, `prone_up_armed` when the shape has them and the arm
//! action is not `look`) plays to its end first. A pose's moving action steps like
//! the walk (by the ground covered when synced, held to [`synced::WALK_MAX_TIME_SCALE`]). An unheld pose action a record
//! carries changes nothing (the client picks it itself).
//!
//! **Script threads** ([`PlayerAnimation::script_threads`]): the threads a server's `playThread` runs on the Player (a hit's
//! flinch, say) play over its own, each at the place its record puts it, as the server's hitboxes play them.
//!
//! **The arm thread** plays the arm action the record names (the server's `setArmThread`; `look` when the shape has it and
//! none was named), held at the head's pitch over the look range (`updateLookAnimation`): 0 looking fully up, 1 fully down.
//! A synced Player's arm moves from one arm action to the next over [`ARM_BLEND`] seconds (engine addition: TGE switches
//! at once), held at the pitch meanwhile. The arm action plays the version of the action playing, or of the pose, when the
//! shape has one ([`poses::pose_arm`]: `swim_forward_point` while the figure swims forward, `prone_hold` lying down); a
//! change of action or pose that changes the version moves the arm to it over [`TRANSITION`] seconds (engine addition).
//! **The head thread** plays the shape's `head` sequence, when it has one, at the same position (`mHeadVThread`).
//! While a death action plays (an action whose name starts with `death`), the death owns the whole body: the arm and head
//! threads stop (TGE leaves the arm and the look alone once the Player is dead) and start again, on the arm action the record
//! names, when another action plays.
//! **The recoil thread** (`mRecoilThread`, made when the shape has `light_recoil`): held at its start until a mounted image's state
//! asks for a recoil ([`PlayerAnimation::recoil`], `onImageRecoil`), then played from its start at speed 1 on the shape's
//! `light_recoil`, `medium_recoil` or `heavy_recoil` (when it has that one) to its end.
//!
//! Credit: follows TGE 1.4's `game/player.cc` (`pickActionAnimation`, `setActionThread`, `updateActionThread`,
//! `updateLookAnimation`, `onImageRecoil`, `onUnmount`, `PlayerData::preload`; Copyright (C) GarageGames.com, Inc.), whose
//! MIT-licensed successor is Torque3D's `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic,
//! Inc.). The code is this crate's own.

use collide::math::F3;
use ghosts::TgeRecord;
use ghosts::player::VELOCITY_NORMAL_BITS;
use glam::Vec3;
use shapes::Shape;
use shapes::instance::{ShapeInstance, ThreadId, find_sequence};
use sim::player::{Pose, PoseData};
use torque_game::scene::shapebase::{TGE_ACTIONS, player_actions};
use torque_game::sim::player::ground::{ActionMotion, ground_info};
use torque_game::sim::player::poses::{self, PoseSets};
use torque_game::sim::player::synced;
pub use torque_game::sim::player::synced::{JUMP_RISE_SPEED, STAND_JUMP_SPEED, TRANSITION};

/// `PlayerData::maxTimeScale`'s default: the fastest a moving action plays.
pub const MAX_TIME_SCALE: f32 = 1.5;
/// How long a synced Player's arm takes from one arm action to the next (seconds): raising a held image to the eye and
/// lowering it again.
pub const ARM_BLEND: f32 = 0.18;
/// The smallest match a moving action must beat to be chosen over `root`.
pub const MIN_MATCH: f32 = 0.1;
pub use torque_game::sim::player::synced::{WALK, WALK_MARGIN, walk_limit};

/// TGE's action numbers.
pub mod action {
    pub const ROOT: usize = 0;
    /// The moving actions are 1 to 3 (`run`, `back`, `side`).
    pub const FIRST_MOVE: usize = 1;
    pub const SIDE: usize = 3;
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

/// An action the server set, as a record carries it (`ghosts::player::Action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServerAction {
    pub action: u32,
    /// The thread holds at its end (`holdAtEnd`).
    pub hold: bool,
    /// The server's thread had reached its end when the record was written.
    pub at_end: bool,
}

impl ServerAction {
    /// The action a Player record (or an AIPlayer's) last carried, if any.
    pub fn of_record(record: &TgeRecord) -> Option<ServerAction> {
        let (TgeRecord::Player(p) | TgeRecord::AiPlayer(p)) = record else {
            return None;
        };
        p.action.map(|a| ServerAction {
            action: a.action,
            hold: a.hold_at_end,
            at_end: a.at_end,
        })
    }
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
    /// The action the server set, as the record last carried it ([`ServerAction::of_record`]).
    pub server_action: Option<ServerAction>,
    /// Where the Player is drawn (its feet): a synced animation steps a moving action by its change.
    pub position: Option<F3>,
    /// The Player is mounted on an object (its record's mount block names a ghost the client holds).
    pub mounted: bool,
    /// The Player's pose (standing unless its datablock allows others).
    pub pose: Pose,
}

/// The pose a Player record's move shows (its server's choice, with the datablock's poses `poses`); none for another record
/// or one without its move block.
pub fn record_pose(record: &TgeRecord, poses: &PoseData) -> Option<Pose> {
    let (TgeRecord::Player(p) | TgeRecord::AiPlayer(p)) = record else {
        return None;
    };
    let m = p.motion.as_ref()?;
    Some(poses.shown(&m.mv.triggers))
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
            server_action: ServerAction::of_record(record),
            position: None,
            mounted: false,
            pose: Pose::Stand,
        })
    }

    /// The velocity in the Player's own frame (`mWorldToObj.mulV`: the yaw undone; a Player of yaw `a` faces `(sin a, cos a)`).
    pub fn local_velocity(&self) -> F3 {
        self.local(self.velocity)
    }

    /// A world vector in the Player's own frame.
    pub fn local(&self, v: F3) -> F3 {
        let (s, c) = self.yaw.sin_cos();
        let [x, y, z] = v;
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
    /// The action stays at its end until the server sets another.
    hold: bool,
    /// The server's action last seen in a record.
    server_action: Option<ServerAction>,
    action_thread: ThreadId,
    arm_thread: Option<ThreadId>,
    arm_action: Option<usize>,
    head_thread: Option<(ThreadId, usize)>,
    /// The `head` sequence, kept while a death stops its thread.
    head_sequence: Option<usize>,
    /// The `look` action, the arm's when the record names none.
    look_action: Option<usize>,
    was_falling: bool,
    /// The upward speed of the last frame.
    last_rise: f32,
    /// The recoil thread, and the `light_recoil`, `medium_recoil` and `heavy_recoil` sequences.
    recoil_thread: Option<ThreadId>,
    recoil_sequences: [Option<usize>; 3],
    /// The pose follows the synced rules (see the module docs).
    synced: bool,
    /// Where the last frame drew the Player.
    last_position: Option<F3>,
    /// The Player was mounted in the last frame.
    mounted: bool,
    /// The `walk` action, when the shape has one that moves it forward (see the module docs).
    walk: Option<usize>,
    /// Each pose's own actions.
    pose_sets: PoseSets,
    /// The script threads played over the Player's own: each slot's thread and sequence.
    script: [Option<(ThreadId, usize)>; SCRIPT_SLOTS],
}

/// `ShapeBase::MaxScriptThreads`.
pub const SCRIPT_SLOTS: usize = 4;

/// A script thread to play over a Player's own ([`PlayerAnimation::script_threads`]): its sequence and its position (0 to 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptPlay {
    pub sequence: usize,
    pub pos: f32,
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
        let head_sequence = find_sequence(shape, b"head");
        let head_thread = head_sequence.and_then(|seq| {
            let t = instance.create_thread(shape)?;
            instance.set_sequence(shape, t, seq as i32, 0.5);
            Some((t, seq))
        });
        let recoil_sequences = [&b"light_recoil"[..], b"medium_recoil", b"heavy_recoil"]
            .map(|n| find_sequence(shape, n));
        let recoil_thread = recoil_sequences[0].and_then(|seq| {
            let t = instance.create_thread(shape)?;
            instance.set_sequence(shape, t, seq as i32, 0.0);
            if let Some(th) = instance.thread_mut(t) {
                th.time_scale = 0.0;
            }
            Some(t)
        });
        let root = actions
            .first()
            .and_then(|a| a.sequence)
            .or_else(|| find_sequence(shape, b"root"));
        if let Some(seq) = root {
            instance.set_sequence(shape, action_thread, seq as i32, 0.0);
        }
        let walk = walk_action(&actions);
        let pose_sets = PoseSets::of(
            actions
                .iter()
                .map(|a| (a.name.as_slice(), a.sequence.is_some(), &a.motion)),
        );
        Some(PlayerAnimation {
            actions,
            action: action::ROOT,
            forward: true,
            wait: false,
            hold: false,
            server_action: None,
            action_thread,
            arm_thread,
            arm_action,
            head_thread,
            head_sequence,
            look_action: look,
            was_falling: false,
            last_rise: 0.0,
            recoil_thread,
            recoil_sequences,
            synced: false,
            last_position: None,
            mounted: false,
            walk,
            pose_sets,
            script: [None; SCRIPT_SLOTS],
        })
    }

    /// Each pose's own actions.
    pub fn pose_sets(&self) -> &PoseSets {
        &self.pose_sets
    }

    /// The script threads to play over the Player's own threads this frame, by slot (see the module docs): a slot's thread is
    /// made on its first play, put on its sequence at its place, and let go when the slot is empty.
    pub fn script_threads(
        &mut self,
        shape: &Shape,
        instance: &mut ShapeInstance,
        slots: &[Option<ScriptPlay>; SCRIPT_SLOTS],
    ) {
        for (kept, play) in self.script.iter_mut().zip(slots) {
            match play {
                Some(p) if p.sequence < shape.sequences.len() => {
                    let t = match kept {
                        Some((t, _)) => *t,
                        None => match instance.create_thread(shape) {
                            Some(t) => t,
                            None => continue,
                        },
                    };
                    instance.set_sequence(shape, t, p.sequence as i32, p.pos.clamp(0.0, 1.0));
                    if let Some(th) = instance.thread_mut(t) {
                        th.time_scale = 0.0;
                    }
                    *kept = Some((t, p.sequence));
                }
                _ => {
                    if let Some((t, _)) = kept.take() {
                        instance.destroy_thread(shape, t);
                    }
                }
            }
        }
    }

    /// `onImageRecoil`: a mounted image's state asked for recoil `kind` (1 light, 2 medium, 3 heavy): the recoil thread plays
    /// that recoil's sequence from its start, when the shape has it.
    pub fn recoil(&mut self, shape: &Shape, instance: &mut ShapeInstance, kind: u32) {
        let (Some(t), Some(Some(seq))) = (
            self.recoil_thread,
            kind.checked_sub(1)
                .and_then(|k| self.recoil_sequences.get(k as usize)),
        ) else {
            return;
        };
        instance.set_sequence(shape, t, *seq as i32, 0.0);
        if let Some(th) = instance.thread_mut(t) {
            th.time_scale = 1.0;
        }
    }

    /// Whether the recoil thread plays (it has not reached its end since the last recoil).
    pub fn recoiling(&self, instance: &ShapeInstance) -> bool {
        self.recoil_thread
            .and_then(|t| instance.thread(t))
            .is_some_and(|t| t.time_scale != 0.0 && t.pos < 1.0)
    }

    /// Turns the synced rules on or off (see the module docs); off by default: TGE's client.
    pub fn set_synced(&mut self, on: bool) {
        self.synced = on;
    }

    /// The action list.
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// The action thread.
    pub fn action_thread(&self) -> ThreadId {
        self.action_thread
    }

    /// The action playing and whether it plays forward.
    pub fn action(&self) -> (usize, bool) {
        (self.action, self.forward)
    }

    /// The arm action playing.
    pub fn arm_action(&self) -> Option<usize> {
        self.arm_action
    }

    /// The arm thread, once the arm plays an action (its sequence the action's version for the pose and the action playing).
    pub fn arm_thread(&self) -> Option<ThreadId> {
        self.arm_thread
    }

    /// Whether a death action plays (the arm and head threads stopped).
    pub fn dying(&self) -> bool {
        self.actions
            .get(self.action)
            .is_some_and(|a| a.motion.death)
    }

    /// Whether the arm thread runs.
    pub fn arm_running(&self) -> bool {
        self.arm_thread.is_some()
    }

    /// Whether the head thread runs.
    pub fn head_running(&self) -> bool {
        self.head_thread.is_some()
    }

    /// Whether the action stays at its end until the server sets another.
    pub fn held(&self) -> bool {
        self.hold
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
        // Walking and running forward keep their place in the loop (the engine's walk; TGE starts every action anew).
        let carried =
            synced::keeps_phase((self.action, self.forward), (action, forward), self.walk)
                .then(|| instance.thread(self.action_thread).map(|t| t.pos))
                .flatten();
        // An action of another pose blends in slower and eased (the engine's poses).
        let pose_blend = poses::blend_for(&self.pose_sets, self.action, action);
        let swing = poses::swings(&self.pose_sets, self.action, action);
        self.action = action;
        self.forward = forward;
        self.wait = wait;
        self.hold = false;
        let pos = carried.unwrap_or(if forward { 0.0 } else { 1.0 });
        // The thread's direction first: a transition runs the way the thread plays when it starts, and one run against the
        // new direction would end back on the old sequence (side played backward giving way to back, say).
        if let Some(t) = instance.thread_mut(self.action_thread) {
            let speed = t.time_scale.abs().max(f32::MIN_POSITIVE);
            t.time_scale = if forward { speed } else { -speed };
        }
        match pose_blend {
            // Into or out of the water the limbs swing, so the arms come down by the sides rather than through the head.
            Some(seconds) if swing => {
                instance.transition_to_sequence_swung(shape, self.action_thread, seq, pos, seconds)
            }
            Some(seconds) => {
                instance.transition_to_sequence_eased(shape, self.action_thread, seq, pos, seconds)
            }
            None => {
                instance.transition_to_sequence(shape, self.action_thread, seq, pos, TRANSITION)
            }
        }
    }

    /// `pickActionAnimation`.
    fn pick(&mut self, shape: &Shape, instance: &mut ShapeInstance, m: &PlayerMotion) {
        if m.falling {
            self.set_action(shape, instance, action::FALL, true, false);
            return;
        }
        let vel = m.local_velocity();
        let (mut best, mut chosen, mut forward) = (MIN_MATCH, action::ROOT, true);
        let moves = self.actions.iter().enumerate().take(action::MOVES);
        for (i, a) in moves.skip(action::FIRST_MOVE) {
            if a.sequence.is_none() || a.motion.speed == 0.0 {
                continue;
            }
            let d = Vec3::from(vel).dot(a.motion.dir.into());
            if d > best {
                (best, chosen, forward) = (d, i, true);
            } else if i == action::SIDE && -d > best {
                (best, chosen, forward) = (-d, i, false);
            }
        }
        let (mut chosen, mut forward) = self.walk_or(chosen, forward, best);
        // Between the crouch and prone the way down or up plays to its end first, as the server plays it.
        let armed = poses::armed(
            m.arm_action
                .and_then(|a| self.actions.get(usize::try_from(a).ok()?))
                .map(|a| a.name.as_slice()),
        );
        if let Some(way) = self.pose_sets.way(self.action, m.pose, armed) {
            self.set_action(shape, instance, way, true, true);
            return;
        }
        // A pose with actions of its own picks among them, by the server's rule.
        if let Some(set) = self.pose_sets.of_pose(m.pose) {
            let dir_of = |a: usize| self.actions.get(a).map_or([0.0; 3], |x| x.motion.dir);
            if let Some((a, f)) = poses::pick_in(set, vel, dir_of) {
                (chosen, forward) = (a, f);
            }
        }
        self.set_action(shape, instance, chosen, forward, false);
    }

    /// The engine's walk in place of TGE's pick `(chosen, forward)` at `speed` along the chosen action's direction (see the
    /// module docs): `run` under its limit walks forward, `back` under its limit walks backward.
    fn walk_or(&self, chosen: usize, forward: bool, speed: f32) -> (usize, bool) {
        let Some(walk) = self.walk else {
            return (chosen, forward);
        };
        let speed_of = |i: usize| self.actions.get(i).map(|a| a.motion.speed);
        synced::walk_or(
            chosen,
            forward,
            speed,
            walk,
            speed_of,
            (self.action, self.forward),
        )
    }

    /// A mount began or ended. `onUnmount` plays `root` forward, neither held nor waited for, so the moves pick the actions
    /// again (the record keeps the action the server set while seated, which is not sent again). A new mount takes the action
    /// the record carries again, even the one an earlier mount set: the server sets it anew on every mount.
    fn mount_changed(&mut self, shape: &Shape, instance: &mut ShapeInstance) {
        if self.mounted {
            self.server_action = None;
            return;
        }
        self.hold = false;
        self.wait = false;
        self.set_action(shape, instance, action::ROOT, true, false);
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
        let rose = synced::rose(self.last_rise, m.velocity[2]);
        self.last_rise = m.velocity[2];
        let from = std::mem::replace(&mut self.last_position, m.position);
        if m.mounted != self.mounted {
            self.mounted = m.mounted;
            self.mount_changed(shape, instance);
        }
        if !self.hold && ((took_off && m.jumping) || rose) {
            let has = |a: usize| self.actions.get(a).is_some_and(|x| x.sequence.is_some());
            let jump = if synced::standing_jump(m.velocity) && has(action::STAND_JUMP) {
                action::STAND_JUMP
            } else {
                action::JUMP
            };
            self.set_action(shape, instance, jump, true, true);
        }
        if let Some(a) = m.server_action
            && self.server_action != Some(a)
        {
            self.server_action = Some(a);
            self.server_set(shape, instance, a);
        }
        if !self.hold && (!self.wait || self.at_end(instance)) {
            self.wait = false;
            self.pick(shape, instance, m);
        }
        let a = &self.actions[self.action.min(self.actions.len() - 1)];
        let mut scale = 1.0;
        let walking = self.walk == Some(self.action) || self.pose_sets.is_moving(self.action);
        if walking {
            if let (true, Some(from), Some(to)) = (self.synced, from, m.position) {
                let step = m.local((Vec3::from(to) - Vec3::from(from)).to_array());
                scale = synced::step_scale(
                    step,
                    dt,
                    a.motion.dir,
                    a.motion.speed,
                    synced::WALK_MAX_TIME_SCALE,
                );
            } else {
                let along = Vec3::from(m.local_velocity()).dot(a.motion.dir.into());
                scale = (along / a.motion.speed)
                    .abs()
                    .min(synced::WALK_MAX_TIME_SCALE);
            }
        } else if self.action < action::MOVES && a.motion.velocity_scale && a.motion.speed != 0.0 {
            if let (true, Some(from), Some(to)) = (self.synced, from, m.position) {
                let step = m.local((Vec3::from(to) - Vec3::from(from)).to_array());
                scale = synced::step_scale(step, dt, a.motion.dir, a.motion.speed, MAX_TIME_SCALE);
            } else {
                let along = Vec3::from(m.local_velocity()).dot(a.motion.dir.into());
                scale = (along / a.motion.speed).abs().min(MAX_TIME_SCALE);
            }
        }
        if let Some(t) = instance.thread_mut(self.action_thread) {
            t.time_scale = if self.forward { scale } else { -scale };
        }
        instance.advance_time(shape, self.action_thread, dt);
        if let Some(t) = self.recoil_thread {
            instance.advance_time(shape, t, dt);
        }

        if self.dying() {
            for t in [self.arm_thread.take(), self.head_thread.take().map(|h| h.0)]
                .into_iter()
                .flatten()
            {
                instance.destroy_thread(shape, t);
            }
            self.arm_action = None;
            return;
        }
        if self.head_thread.is_none()
            && let Some(seq) = self.head_sequence
            && let Some(t) = instance.create_thread(shape)
        {
            instance.set_sequence(shape, t, seq as i32, m.look);
            self.head_thread = Some((t, seq));
        }
        if let Some(arm) = m
            .arm_action
            .and_then(|a| usize::try_from(a).ok())
            .or(self.look_action.filter(|_| self.arm_action.is_none()))
            && self.arm_action != Some(arm)
            && let Some(seq) = self.arm_sequence(arm, m.pose)
        {
            let t = match self.arm_thread {
                Some(t) => t,
                None => match instance.create_thread(shape) {
                    Some(t) => t,
                    None => return,
                },
            };
            let switch = self.synced && self.arm_action.is_some() && self.arm_thread == Some(t);
            self.arm_thread = Some(t);
            self.arm_action = Some(arm);
            if switch {
                if let Some(th) = instance.thread_mut(t) {
                    th.time_scale = 1.0;
                }
                instance.transition_to_sequence(shape, t, seq, m.look, ARM_BLEND);
            } else {
                instance.set_sequence(shape, t, seq as i32, m.look);
            }
        } else if let Some(t) = self.arm_thread
            && instance
                .thread(t)
                .is_some_and(|th| th.transition.in_transition)
        {
            // The switch runs on by the frame's time.
            instance.advance_time(shape, t, dt);
        }
        if let (Some(t), Some(seq)) = (
            self.arm_thread,
            self.arm_action.and_then(|a| self.arm_sequence(a, m.pose)),
        ) {
            match instance.thread_mut(t) {
                // Mid-switch: the new action at the pitch, the switch's blend kept.
                Some(th) if th.transition.in_transition => {
                    th.pos = m.look.clamp(0.0, 1.0);
                    instance.set_all_dirty();
                }
                // The same arm action in another version (the action or the pose changed: `swim_point` to
                // `swim_forward_point` as the figure starts its stroke): the arm moves to it as the body blends.
                Some(th) if th.sequence.is_some_and(|q| q != seq) => {
                    th.time_scale = 1.0;
                    instance.transition_to_sequence(shape, t, seq, m.look, TRANSITION);
                }
                _ => instance.set_sequence(shape, t, seq as i32, m.look),
            }
        }
        if let Some((t, seq)) = self.head_thread {
            instance.set_sequence(shape, t, seq as i32, m.look);
        }
    }

    /// The sequence the arm action `arm` plays in `pose`: the action playing's own version when the shape has one
    /// (`swim_forward_point`), else the pose's (`prone_hold`), as the server plays it.
    fn arm_sequence(&self, arm: usize, pose: Pose) -> Option<usize> {
        let names = self
            .actions
            .iter()
            .map(|a| (a.name.as_slice(), a.sequence.is_some()));
        let action = self
            .actions
            .get(self.action)
            .map_or(&[][..], |a| a.name.as_slice());
        let played = self
            .actions
            .get(arm)
            .and_then(|a| poses::pose_arm(names, &a.name, pose, action))
            .unwrap_or(arm);
        self.actions.get(played)?.sequence
    }

    /// A new action from the server (`unpackUpdate`'s `setActionThread(action, true, hold, true, ...)`): one of the table's
    /// changes nothing (the client picks those itself), nor does an unheld walk from any server (the client picks its walk
    /// itself: a synced server by the same rule, and a server picking by TGE's rules never sends one; only a script's held
    /// walk plays as set); another plays forward to its end, from its start, or at its end already when the server's had
    /// ended.
    fn server_set(&mut self, shape: &Shape, instance: &mut ShapeInstance, a: ServerAction) {
        let action = a.action as usize;
        if a.action == ghosts::player::NULL_ANIMATION
            || a.action < ghosts::player::NUM_TABLE_ACTION_ANIMS
            || ((Some(action) == self.walk || self.pose_sets.pose_of(action) != Pose::Stand)
                && !a.hold)
            || self.actions.get(action).and_then(|x| x.sequence).is_none()
        {
            return;
        }
        if self.action != action || !self.forward {
            self.set_action(shape, instance, action, true, true);
        }
        self.wait = true;
        self.hold = a.hold;
        if a.at_end
            && let Some(seq) = self.actions[action].sequence
        {
            instance.set_sequence(shape, self.action_thread, seq as i32, 1.0);
        }
    }
}

/// The engine's walk action of `actions` (see the module docs): `walk`, after the table, with a sequence whose ground motion
/// goes forward.
fn walk_action(actions: &[Action]) -> Option<usize> {
    synced::walk_action(
        actions
            .iter()
            .map(|a| (a.name.as_slice(), a.sequence.is_some(), &a.motion)),
        TGE_ACTIONS.fixed.len(),
    )
}

#[cfg(test)]
#[path = "player_anim_tests.rs"]
mod tests;
