//! The part the prediction slot runs ([`PropPart`]): the predicted set, the client's physics world, the restore and replay, the
//! smoothed drawing and the shots ([module docs](super)).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use torque_client::predict::slot::{
    BodyBox, BodyMotion, HeadStep, MoveStep, PredictionPart, Shot, StaticView, TickStep, ViewStep,
};
use torque_collide::math::vec3::sub;
use torque_collide::math::{
    Box3, QUAT_IDENTITY, affine_inverse, interpolate, mul_p, plain_quat_normalize,
};
use torque_collide::source::segment_box;
use torque_engine::predict_ids::PREDICT_FADE;
use torque_ghost_world::world::Pose;
use torque_physics_rapier::TICK;

use super::{MAX_REPLAY, MoveRing, PART_ID, PredictConfig, PropPrediction, Push, Spawn};
use crate::client::{ClientProp, ClientProps, PropBook};
use crate::data::{PropCollision, PropParams};
use crate::hits::{self, ShotPush};
use crate::hulls::{Choice, HullCaps, HullDigest, Made, PropHulls, Refusal, choose};
use crate::joints::client::{ClientJoints, JointBook};
use crate::joints::records::EndWire;
use crate::joints::{End, FarEnd, Joint, Kind, KindId, Reach, Spring};
use crate::quant::{self, Quantized, conjugate, quat_mul, quat_nlerp};
use crate::records::xf_of;
use crate::world::{
    PUSH_MARGIN, PlayerAt, PropWorld, Pusher, Rest, SpawnArgs, StaticPiece, length, world_half,
};

/// The ids the client's world gives the map pieces and the props around the predicted ones (fixed boxes), above the props'
/// and joints' ghost indexes. The map's are the lower: the world keeps the lowest ids when it has too many fixed pieces, so no
/// pile of props crowds the floor out.
const MAP_BASE: u32 = 0x4000_0000;
const NEIGHBOR_BASE: u32 = 0x8000_0000;

/// The most props around the predicted ones taken as fixed boxes (the nearest).
const MAX_NEIGHBORS: usize = 64;

// Every map piece's id sorts before every neighbor's, and the world keeps them all.
const _: () = assert!(MAP_BASE + (MAP_BASE - 1) < NEIGHBOR_BASE);
const _: () = assert!(
    MAX_NEIGHBORS + torque_client::predict::slot::MAX_STATIC_VIEWS <= crate::world::MAX_STATICS
);

/// How far around the predicted props the map pieces and the props around them are taken (units), and how far the props
/// may move before the map pieces are looked for again.
const REACH: f32 = 8.0;
const REFETCH: f32 = 4.0;

/// The moves whose pushes are kept for a replay.
const HISTORY: usize = 128;

/// The most predicted pushes kept for replays (eight a move over the longest replay).
const MAX_LOG: usize = MAX_REPLAY as usize * super::MAX_IMPULSES_PER_TICK;

/// How close above a prop's top the Player's feet count as standing on it (units).
const STAND: f32 = 0.3;

/// The ticks a shot is followed at most.
const SHOT_TICKS: u32 = 160;

/// Projectile gravity (as the projectile's own step).
const GRAVITY: f32 = 9.81;

/// A pose: a position and a unit quaternion `x, y, z, w`.
type Place = ([f32; 3], [f32; 4]);

/// A predicted prop.
#[derive(Debug, Clone, Copy)]
struct Member {
    /// The book's update count at the last restore.
    updates: u32,
    /// The drawn correction: added to the simulated position, and turning the simulated rotation.
    offset: [f32; 3],
    turn: [f32; 4],
    /// Drawn at the last tick and the one before.
    cur: Place,
    prev: Place,
    /// It joined (or its prediction stopped) at the last restore: its drawn pose eases from `from`.
    from: Option<Place>,
}

/// A prop fading from where it was drawn to where the client shows it.
#[derive(Debug, Clone, Copy)]
struct Fade {
    from: Place,
    left: u32,
}

/// A shot of the client's own Player followed ahead.
#[derive(Debug, Clone, Copy)]
struct Track {
    at: [f32; 3],
    velocity: [f32; 3],
    gravity: f32,
    ticks: u32,
    /// It hit a predicted prop (its explosion's push was given then).
    hit: bool,
}

/// The tick's work for parts ran out ([`TickStep::charge`]).
struct OverBudget;

/// The predicted pushes taken, by the move each was made at (for a replay), at most [`MAX_LOG`].
#[derive(Default)]
struct PushLog {
    by_move: BTreeMap<u32, Vec<Push>>,
    len: usize,
}

impl PushLog {
    /// Drops the pushes of the moves the server has acknowledged (`ack` and after are kept).
    fn acked(&mut self, ack: u32) {
        let before = self.by_move.len();
        self.by_move
            .retain(|m, _| (m.wrapping_sub(ack) as i32) >= 0);
        if self.by_move.len() != before {
            self.len = self.by_move.values().map(Vec::len).sum();
        }
    }

    /// Keeps `pushes`, made at move `at`; the oldest go past the cap.
    fn add(&mut self, at: u32, pushes: &[Push]) {
        if pushes.is_empty() {
            return;
        }
        self.by_move
            .entry(at)
            .or_default()
            .extend_from_slice(pushes);
        self.len += pushes.len();
        while self.len > MAX_LOG {
            let Some((_, gone)) = self.by_move.pop_first() else {
                break;
            };
            self.len -= gone.len();
        }
    }

    #[cfg(test)]
    fn at(&self, m: u32) -> &[Push] {
        self.by_move.get(&m).map_or(&[], Vec::as_slice)
    }

    /// The pushes of move `m`, taken out to apply ([`PushLog::put_back`] returns them).
    fn take(&mut self, m: u32) -> Vec<Push> {
        self.by_move.remove(&m).unwrap_or_default()
    }

    fn put_back(&mut self, m: u32, pushes: Vec<Push>) {
        if !pushes.is_empty() {
            self.by_move.insert(m, pushes);
        }
    }
}

/// At most this many shapes' hulls are kept (a server has one set a datablock and draw scale; the cache starts over past it).
const MAX_HULL_SETS: usize = 256;

/// At most this many datablock and digest pairs are kept as unfit (the props cap); past it the set starts over.
const MAX_UNFIT: usize = crate::world::MAX_PROPS as usize;

/// What the part keeps between ticks.
struct State {
    world: PropWorld,
    tick: u64,
    /// The first move the server has not acknowledged, from the last head.
    ack: u32,
    members: BTreeMap<u32, Member>,
    /// The tick each prop was last touched by the Player.
    touched: BTreeMap<u32, u64>,
    history: MoveRing<BodyMotion, HISTORY>,
    log: PushLog,
    neighbors: BTreeMap<u32, Quantized>,
    statics: Vec<(u32, StaticPiece)>,
    statics_at: Option<[f32; 3]>,
    /// The client joints in the world (by joint ghost).
    joints: BTreeSet<u32>,
    fading: BTreeMap<u32, Fade>,
    shots: BTreeMap<u32, Track>,
    /// Shots done with (hit, exploded or gone).
    done: BTreeSet<u32>,
    /// The book's ghosts already looked at for a predicted spawn (each once, when it comes).
    seen: BTreeSet<u32>,
    /// The tick's work ran out: the members are drawn as the client shows them this tick.
    interpolating: bool,
    /// The physics work done this tick.
    work: u64,
    /// Where this tick's moves stand.
    run: Run,
    /// The members' states before the last move was stepped (kept from tick to tick, refilled in place).
    pre: Vec<(u32, Quantized, Rest)>,
    /// The collision hulls of the shapes the predicted props of `collision = "shape"` came from, by file and draw scale: made
    /// once from the client's own copy of the shape, a refusal kept as well.
    hulls: BTreeMap<(String, [u32; 3]), Made>,
    /// The datablocks and digests the client's hulls did not match: props of them are left to the server.
    unfit: BTreeSet<(u32, HullDigest)>,
}

/// Where a tick's moves stand.
#[derive(Debug, Default)]
enum Run {
    /// Stepped as they ran.
    #[default]
    Stepping,
    /// Put back to the server's states and replayed: where each was predicted before.
    Restored(Vec<(u32, Place)>),
    /// The tick's work ran out: it falls back to interpolation.
    Over,
}

impl State {
    fn new(config: &PredictConfig) -> State {
        State {
            world: super::world(config),
            tick: 0,
            ack: 0,
            members: BTreeMap::new(),
            touched: BTreeMap::new(),
            history: MoveRing::default(),
            log: PushLog::default(),
            neighbors: BTreeMap::new(),
            statics: Vec::new(),
            statics_at: None,
            joints: BTreeSet::new(),
            fading: BTreeMap::new(),
            shots: BTreeMap::new(),
            done: BTreeSet::new(),
            seen: BTreeSet::new(),
            interpolating: false,
            work: 0,
            run: Run::Stepping,
            pre: Vec::new(),
            hulls: BTreeMap::new(),
            unfit: BTreeSet::new(),
        }
    }
}

/// The props plugin's part of the client's prediction ([module docs](super)).
pub struct PropPart {
    book: ClientProps,
    joints: ClientJoints,
    shared: PropPrediction,
    state: Option<Box<State>>,
}

impl PropPart {
    /// A part over the client's books and the shared prediction.
    pub fn new(book: ClientProps, joints: ClientJoints, shared: PropPrediction) -> PropPart {
        PropPart {
            book,
            joints,
            shared,
            state: None,
        }
    }

    /// Runs `f` over the part's state and the client's books (nothing when a book is borrowed elsewhere).
    fn with_cx(&mut self, f: impl FnOnce(&mut Cx<'_>)) {
        let config = self.shared.config();
        let state = self
            .state
            .get_or_insert_with(|| Box::new(State::new(&config)));
        let Ok(mut book) = self.book.0.try_borrow_mut() else {
            return;
        };
        let Ok(joints) = self.joints.0.try_borrow() else {
            return;
        };
        f(&mut Cx {
            config,
            book: &mut book,
            joints: &joints,
            shared: &self.shared,
            s: state,
        });
    }
}

impl PredictionPart for PropPart {
    fn id(&self) -> &str {
        PART_ID
    }

    fn head(&mut self, step: &mut dyn HeadStep) {
        if let Some(s) = self.state.as_deref_mut() {
            s.ack = step.ack();
        }
    }

    fn moves_begin(&mut self, step: &mut MoveStep<'_>, first: u32, end: u32) {
        self.with_cx(|cx| cx.begin(step, first, end));
    }

    fn moved(&mut self, step: &mut MoveStep<'_>, index: u32) {
        self.with_cx(|cx| cx.moved(step, index));
    }

    fn tick(&mut self, step: &mut dyn TickStep) {
        self.with_cx(|cx| cx.tick(step));
    }

    fn view(&mut self, step: &mut dyn ViewStep) {
        let Some(s) = self.state.as_deref() else {
            return;
        };
        let back = step.back().clamp(0.0, 1.0);
        let fade_ticks = self.shared.config().fade_ticks as f32;
        let mut drawn = Vec::with_capacity(s.members.len());
        let mut poses = Vec::with_capacity(s.members.len() + s.fading.len());
        if !s.interpolating {
            for (&ghost, m) in &s.members {
                let (p, q) = blend(m.cur, m.prev, back);
                drawn.push((ghost, p, q));
                poses.push((ghost, (p, q)));
            }
        }
        for (&ghost, f) in &s.fading {
            let Some(shown) = step.shown(ghost).as_ref().and_then(place_of) else {
                continue;
            };
            let t = ((f.left as f32 + back) / fade_ticks).clamp(0.0, 1.0);
            poses.push((ghost, blend(shown, f.from, t)));
        }
        let scales: Vec<Option<[f32; 3]>> = poses
            .iter()
            .map(|(g, _)| match step.shown(*g) {
                Some(Pose::Affine { scale, .. }) => scale,
                _ => None,
            })
            .collect();
        let out = step.drawn();
        for ((ghost, (p, q)), scale) in poses.into_iter().zip(scales) {
            out.ghosts.insert(
                ghost,
                Pose::Affine {
                    xf: xf_of(p, q),
                    scale,
                },
            );
        }
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.drawn = drawn;
        }
    }

    fn clone_box(&self) -> Box<dyn PredictionPart> {
        // A copy starts over: it restores the props from the server's states at the next tick.
        Box::new(PropPart::new(
            self.book.clone(),
            self.joints.clone(),
            self.shared.clone(),
        ))
    }
}

/// One tick's work over the part's state and the client's books.
struct Cx<'a> {
    config: PredictConfig,
    book: &'a mut PropBook,
    joints: &'a JointBook,
    shared: &'a PropPrediction,
    s: &'a mut State,
}

impl Cx<'_> {
    /// The control object is about to run its moves `first` to before `end`: the pushes the game asked for are kept for the
    /// newest (for whichever props are predicted when it runs, a prop that joins this tick too), and when the server sent new
    /// states or the moves run again after a correction, every member is put back to the server's state and the moves from
    /// the ack to `first` replayed; the rest are stepped with the control object's own ([`Cx::moved`]).
    fn begin(&mut self, step: &mut MoveStep<'_>, first: u32, end: u32) {
        self.s.work = 0;
        self.s.run = Run::Stepping;
        let ack = self.s.ack;
        self.s.log.acked(ack);
        let pushes = self.take_pushes();
        self.s.log.add(end.wrapping_sub(1), &pushes);
        if self.s.members.is_empty() {
            return;
        }
        let stale = self
            .s
            .members
            .iter()
            .any(|(g, m)| self.book.get(*g).is_some_and(|p| p.updates != m.updates));
        let again = end.wrapping_sub(first) > 1;
        if !(stale || again) {
            return;
        }
        let (motion, player) = (Some(step.motion), step.player);
        let mut charge = |units: u64| step.charge(units);
        self.s.run = match self.restore_and_replay(&mut charge, motion, player, ack, first) {
            Ok(before) => Run::Restored(before),
            Err(OverBudget) => Run::Over,
        };
        // The moves the control object runs from here are replayed too.
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.stats.replayed += u64::from(end.wrapping_sub(first));
        }
        self.note_shapes(step);
    }

    /// The control object ran move `m`: its motion kept for a replay, and the members stepped for it (its push, the held
    /// prop's pull it just made, the pushes made at it), their states into the book for its next move.
    fn moved(&mut self, step: &mut MoveStep<'_>, m: u32) {
        self.s.history.put(m, step.motion);
        if self.s.members.is_empty() || matches!(self.s.run, Run::Over) {
            return;
        }
        let (motion, player) = (step.motion, step.player);
        let mut charge = |units: u64| step.charge(units);
        if self
            .step_move(&mut charge, m, Some(motion), player)
            .is_err()
        {
            self.s.run = Run::Over;
            return;
        }
        self.write_states();
        self.note_shapes(step);
    }

    /// The tick, after the control object's moves: the restore's corrections eased out, the predicted set chosen (a prop that
    /// joins is brought up to the moves run so far at once), the shots of the client's Player followed (a hit's push made at
    /// the move of the hit, which is stepped again with it), the map pieces brought up to date and the drawn poses settled.
    fn tick(&mut self, step: &mut dyn TickStep) {
        self.s.tick += 1;
        self.s.interpolating = false;
        let next = step.last_client_move();
        let newest = next.wrapping_sub(1);
        let player = step.body().filter(|b| !b.vehicle).map(|b| b.ghost);
        let motion = step.motion();
        self.tend_spawns();
        let wanted = match (player, motion) {
            (Some(p), Some(m)) => self.wanted(p, &m),
            _ => Vec::new(),
        };
        let mut over = match std::mem::take(&mut self.s.run) {
            Run::Restored(before) => {
                let last = self.pre_places();
                self.ease_corrections(&before, &last);
                false
            }
            Run::Over => true,
            Run::Stepping => false,
        };
        let joined = self.join_and_leave(&wanted, step);
        if !self.s.members.is_empty() {
            self.refresh_around(step);
        }
        let ack = self.s.ack;
        if joined && !over {
            let mut charge = |units: u64| step.charge(units);
            match self.restore_and_replay(&mut charge, motion, player, ack, next) {
                Ok(_) => {
                    let last = self.pre_places();
                    self.ease_corrections(&[], &last);
                }
                Err(OverBudget) => over = true,
            }
        }
        let mut pushes = Vec::new();
        let shots = step.shots();
        self.follow_shots(newest, player, &shots, &*step, &mut pushes);
        if !pushes.is_empty() {
            self.s.log.add(newest, &pushes);
            if !over && !self.s.members.is_empty() {
                let mut charge = |units: u64| step.charge(units);
                over = self.restep_newest(&mut charge, newest, player).is_err();
            }
        }
        if over {
            self.fall_back();
        } else {
            self.settle_drawn();
        }
        self.write_book();
    }

    /// The newest move stepped again from the members' states before it (a hit's push was made at it): what the server's
    /// step does, drawn in the frame of the hit.
    fn restep_newest(
        &mut self,
        charge: &mut dyn FnMut(u64) -> bool,
        newest: u32,
        player: Option<u32>,
    ) -> Result<(), OverBudget> {
        let pre = std::mem::take(&mut self.s.pre);
        for &(g, q, rest) in &pre {
            self.s.world.restore(g, q, rest);
        }
        self.s.pre = pre;
        let motion = self.motion_at(newest);
        self.step_move(charge, newest, motion, player)?;
        self.write_states();
        Ok(())
    }

    /// The props the Player holds, stands on or touched lately, each with the props jointed to it, in priority order and capped.
    fn wanted(&mut self, player: u32, m: &BodyMotion) -> Vec<u32> {
        let tick = self.s.tick;
        let held: Vec<u32> = self
            .joints
            .connected(player)
            .into_iter()
            .filter(|g| self.book.get(*g).is_some())
            .collect();
        let mut under = Vec::new();
        for (&g, p) in self.book.props() {
            let Some(params) = p.params else { continue };
            if p.frozen {
                continue;
            }
            let q = p.now();
            let c = q.position();
            let half = world_half(params.shape.half_extents(), q.rotation());
            let margin = PUSH_MARGIN + 0.05;
            let touching = (0..3).all(|k| {
                c[k] + half[k] + margin >= m.min[k] && c[k] - half[k] - margin <= m.max[k]
            });
            if touching {
                self.s.touched.insert(g, tick);
            }
            let top = c[2] + half[2];
            let over = (0..2).all(|k| c[k] + half[k] >= m.min[k] && c[k] - half[k] <= m.max[k]);
            if over && m.min[2] >= top - STAND && m.min[2] <= top + STAND {
                under.push(g);
            }
        }
        let window = u64::from(self.config.touch_ticks);
        self.s
            .touched
            .retain(|_, t| tick.saturating_sub(*t) <= window);
        // Most recent touches first, then by ghost.
        let mut recent: Vec<(u64, u32)> = self.s.touched.iter().map(|(&g, &t)| (t, g)).collect();
        recent.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let max = self.config.max_predicted;
        let mut out: Vec<u32> = Vec::new();
        for g in held
            .into_iter()
            .chain(under)
            .chain(recent.into_iter().map(|(_, g)| g))
        {
            self.pull_in(g, max, &mut out);
        }
        out.sort_unstable();
        out
    }

    /// Adds `g` and the props jointed to it (breadth first) to `out` while there is room.
    fn pull_in(&self, g: u32, max: usize, out: &mut Vec<u32>) {
        let mut queue = VecDeque::from([g]);
        while let Some(g) = queue.pop_front() {
            if out.len() >= max {
                return;
            }
            if out.contains(&g)
                || self.book.get(g).is_none_or(|p| {
                    p.frozen
                        || p.params.is_none()
                        || p.is_linked()
                        || p.data
                            .zip(p.hulls)
                            .is_some_and(|k| self.s.unfit.contains(&k))
                })
            {
                continue;
            }
            out.push(g);
            queue.extend(self.joints.connected(g));
        }
    }

    /// Makes the members `wanted`: new ones are spawned in the client's world, ones that left are taken out and fade. Whether
    /// any joined.
    fn join_and_leave(&mut self, wanted: &[u32], step: &mut dyn TickStep) -> bool {
        let gone: Vec<u32> = self
            .s
            .members
            .keys()
            .filter(|g| wanted.binary_search(g).is_err())
            .copied()
            .collect();
        for g in gone {
            if let Some(m) = self.s.members.remove(&g) {
                self.s.fading.insert(
                    g,
                    Fade {
                        from: m.cur,
                        left: self.config.fade_ticks,
                    },
                );
            }
            self.s.world.remove(g);
            self.book.set_predicted(g, None);
            if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
                shared.hulls.remove(&g);
            }
        }
        let mut joined = false;
        for &g in wanted {
            if self.s.members.contains_key(&g) {
                continue;
            }
            let Some(p) = self.book.get(g).copied() else {
                continue;
            };
            let (Some(data), Some(params)) = (p.data, p.params) else {
                continue;
            };
            // A prop of hulls waits (a tick or two) for the client to read its shape, and is left to the server when the client's
            // hulls are not the server's.
            let Some((params, hulls)) = self.fitted(data, p.hulls, params, step) else {
                continue;
            };
            let st = p.q.state();
            if self
                .s
                .world
                .spawn_with(SpawnArgs {
                    id: g,
                    data,
                    params,
                    hulls,
                    pose: (st.position, st.rotation),
                    owner: None,
                    predict: 0,
                })
                .is_err()
            {
                continue;
            }
            if let Some(hulls) = self.s.world.get(g).and_then(|p| p.hulls.clone())
                && let Ok(mut shared) = self.shared.0.try_borrow_mut()
            {
                shared.hulls.insert(g, hulls);
            }
            let shown = place_q(&p.q);
            let from = self.s.fading.remove(&g).map_or(shown, |f| {
                blend(shown, f.from, f.left as f32 / self.config.fade_ticks as f32)
            });
            self.s.members.insert(
                g,
                Member {
                    updates: p.updates.wrapping_sub(1),
                    offset: [0.0; 3],
                    turn: QUAT_IDENTITY,
                    cur: from,
                    prev: from,
                    from: Some(from),
                },
            );
            joined = true;
        }
        joined
    }

    /// The numbers and collision hulls a prop of datablock `data` is predicted with, given the digest of the hulls its server
    /// collides it as (`server`: none for the datablock's box): `params` for the box, else the hulls the client makes from its
    /// own copy of the shape (read through `step`) with the numbers fitted to them, but only if they are the server's
    /// ([`choose`]). None while the client has not read the shape yet and may, and for a prop whose client hulls are not the
    /// server's (another file, a read refused, a cap): it is not predicted, so it is never predicted as a shape the server does
    /// not have.
    fn fitted(
        &mut self,
        data: u32,
        server: Option<HullDigest>,
        params: PropParams,
        step: &mut dyn TickStep,
    ) -> Option<(PropParams, Option<Rc<PropHulls>>)> {
        let Some(digest) = server else {
            return Some((params, None));
        };
        let source = self
            .book
            .hull_source(data)
            .filter(|_| params.collision == PropCollision::Shape);
        let built = match source {
            None => Some(Err(Refusal::Unreadable)),
            Some(source) => {
                let key = (
                    source.path.to_ascii_lowercase(),
                    source.scale.map(f32::to_bits),
                );
                if !self.s.hulls.contains_key(&key) {
                    let path = source.path.as_bytes();
                    let made = match step.shape(path) {
                        // The work of making the hulls is the tick's for parts: a tick without the room for it waits.
                        Some(shape) => {
                            let mut charge = |units: u64| step.charge(units);
                            match PropHulls::of_client(
                                &shape,
                                source.scale,
                                &HullCaps::default(),
                                &mut charge,
                            ) {
                                Err(Refusal::Budget) => None,
                                made => Some(made),
                            }
                        }
                        None if step.shape_settled(path) => Some(Err(Refusal::Unreadable)),
                        None => None,
                    };
                    if let Some(made) = made {
                        // A server that names ever more shapes cannot grow the cache past this: it starts over (a shape is made
                        // again, and charged again, when a prop of it joins).
                        if self.s.hulls.len() >= MAX_HULL_SETS {
                            self.s.hulls.clear();
                        }
                        self.s.hulls.insert(key.clone(), made.map(Rc::new));
                    }
                }
                self.s.hulls.get(&key).cloned()
            }
        };
        match choose(server, built.as_ref()) {
            Choice::Hulls => {
                let hulls = built.and_then(Result::ok)?;
                Some((params.with_hulls(hulls.volume()), Some(hulls)))
            }
            Choice::Box => Some((params, None)),
            Choice::Wait => None,
            Choice::Unfit => {
                // A server that keeps sending fresh digests cannot grow the set past this: it starts over (each prop left
                // out is only looked at again).
                if self.s.unfit.len() >= MAX_UNFIT {
                    self.s.unfit.clear();
                }
                self.s.unfit.insert((data, digest));
                None
            }
        }
    }

    /// The pushes the game asked for since the last tick (each on the props predicted when its move runs).
    fn take_pushes(&mut self) -> Vec<Push> {
        let mut shared = self.shared.0.borrow_mut();
        let taken: Vec<Push> = std::mem::take(&mut shared.pushes);
        shared.stats.impulses += taken.len() as u64;
        taken
    }

    /// The shots of the client's Player followed on a tick: one that meets a member pushes it (and blasts the members in its
    /// reach) into `out`; one whose explosion came first blasts when it comes.
    fn follow_shots(
        &mut self,
        newest: u32,
        player: Option<u32>,
        shots: &[Shot],
        step: &dyn TickStep,
        out: &mut Vec<Push>,
    ) {
        let ahead = newest.wrapping_sub(self.s.ack).min(MAX_REPLAY);
        let mut hits = 0;
        for shot in shots {
            if player.is_none() || shot.source != player || self.s.done.contains(&shot.ghost) {
                continue;
            }
            let push = self.shot_push(shot, step);
            if let Some(at) = shot.exploded {
                // An explosion the client did not see coming pushes at once.
                if !self.s.shots.get(&shot.ghost).is_some_and(|t| t.hit) && push.blasts() {
                    out.push(Push::Blast { at, push });
                }
                self.s.shots.remove(&shot.ghost);
                self.s.done.insert(shot.ghost);
                continue;
            }
            let gravity = shot_gravity(shot, step);
            let track = self.s.shots.entry(shot.ghost).or_insert_with(|| {
                // Put ahead by the moves the server has not acknowledged: where it is in the client's time.
                let mut t = Track {
                    at: shot.position,
                    velocity: shot.velocity,
                    gravity,
                    ticks: 0,
                    hit: false,
                };
                for _ in 0..ahead {
                    advance(&mut t);
                }
                t
            });
            if track.hit {
                continue;
            }
            let from = track.at;
            advance(track);
            track.ticks += 1;
            let (to, velocity) = (track.at, track.velocity);
            let Some((ghost, point)) = first_member_hit(&self.s.members, self.book, from, to)
            else {
                continue;
            };
            track.hit = true;
            hits += 1;
            if let Some(impulse) = hits::hit_impulse(velocity, &push) {
                out.push(Push::At {
                    ghost,
                    point,
                    impulse,
                });
            }
            if push.blasts() {
                out.push(Push::Blast { at: point, push });
            }
        }
        let alive: BTreeSet<u32> = shots.iter().map(|s| s.ghost).collect();
        self.s
            .shots
            .retain(|g, t| alive.contains(g) && t.ticks < SHOT_TICKS);
        self.s.done.retain(|g| alive.contains(g));
        if hits > 0
            && let Ok(mut shared) = self.shared.0.try_borrow_mut()
        {
            shared.stats.shot_hits += hits;
        }
    }

    /// What `shot` pushes with: its datablock's numbers where the client has them, else the config's.
    fn shot_push(&self, shot: &Shot, step: &dyn TickStep) -> ShotPush {
        let d = self.config.shot;
        let Some(db) = shot.datablock else {
            return d;
        };
        let n = |name: &str| step.datablock_number(db, name);
        ShotPush::of(
            n("impulse").or(d.impulse),
            n("areaImpulse").or(Some(d.area)),
            n("areaImpulseRadius").or(Some(d.radius)),
        )
    }

    /// The map pieces and the props around the members, given to the client's world as fixed pieces when the members moved
    /// far enough (or a prop around them moved): the map's first, then the nearest [`MAX_NEIGHBORS`] props. The other players
    /// and vehicles around them come each tick from the same construction of the client's collision world.
    fn refresh_around(&mut self, step: &mut dyn TickStep) {
        let (lo, hi) = self.region();
        let center: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
        let refetch = self
            .s
            .statics_at
            .is_none_or(|at| length(sub(center, at)) > REFETCH);
        let pad = REACH + REFETCH;
        let statics = refetch.then(|| (lo.map(|v| v - pad), hi.map(|v| v + pad)));
        let bodies = self
            .config
            .bystanders
            .then(|| (lo.map(|v| v - REACH), hi.map(|v| v + REACH)));
        let around = step.around(statics, bodies);
        if !around.built {
            // No room left in the tick's budget for parts to look: the pieces and boxes of the last look stay, and the pieces
            // are fetched again at the next.
            return;
        }
        if refetch {
            self.s.statics = around.statics.iter().filter_map(piece_of).collect();
            self.s.statics_at = Some(center);
        }
        self.set_bystanders(&around.bodies);
        let mut near: Vec<(f32, u32, Quantized)> = self
            .book
            .props()
            .iter()
            .filter(|(g, p)| {
                !self.s.members.contains_key(g)
                    && p.params.is_some()
                    && (0..3).all(|k| {
                        let c = p.now().position()[k];
                        c >= lo[k] - REACH && c <= hi[k] + REACH
                    })
            })
            .map(|(&g, p)| (length(sub(p.now().position(), center)), g, *p.now()))
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let near: BTreeMap<u32, Quantized> = near
            .into_iter()
            .take(MAX_NEIGHBORS)
            .map(|(_, g, q)| (g, q))
            .collect();
        if !refetch && near == self.s.neighbors {
            return;
        }
        // A prop around them that moved goes out and comes back where it is.
        let kept: Vec<(u32, StaticPiece)> = self
            .pieces(&near)
            .into_iter()
            .filter(|(id, _)| {
                id.checked_sub(NEIGHBOR_BASE)
                    .is_none_or(|g| self.s.neighbors.get(&g) == near.get(&g))
            })
            .collect();
        self.s.world.sync_statics(kept);
        let all = self.pieces(&near);
        self.s.world.sync_statics(all);
        self.s.neighbors = near;
    }

    /// The other players and vehicles around the members, as the server places them: their boxes at their latest records
    /// (frictionless kinematic bodies that push nothing, [`PropWorld::set_bystanders`]), so a prop meets one it is pushed
    /// toward, in ascending ghost order. A body's push (the speed it moves at) is the server's alone, and a player in a crouch,
    /// prone or swim pose is boxed standing: the client's player data carries no pose box.
    fn set_bystanders(&mut self, bodies: &[BodyBox]) {
        let boxes = bodies
            .iter()
            .map(|b| Pusher {
                id: b.ghost,
                min: b.min,
                max: b.max,
                velocity: [0.0; 3],
                mass: 0.0,
            })
            .collect();
        self.s.world.set_bystanders(boxes);
    }

    /// The fixed pieces of the client's world: the map's and the props around the members.
    fn pieces(&self, near: &BTreeMap<u32, Quantized>) -> Vec<(u32, StaticPiece)> {
        let mut out = self.s.statics.clone();
        for (&g, q) in near {
            let Some(params) = self.book.get(g).and_then(|p| p.params) else {
                continue;
            };
            let Some(id) = NEIGHBOR_BASE.checked_add(g) else {
                continue;
            };
            out.push((
                id,
                StaticPiece::Box {
                    position: q.position(),
                    rotation: q.rotation(),
                    half: params.shape.half_extents(),
                },
            ));
        }
        out
    }

    /// The box the members' centers lie in.
    fn region(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for g in self.s.members.keys() {
            if let Some(p) = self.book.get(*g) {
                let c = p.now().position();
                for k in 0..3 {
                    lo[k] = lo[k].min(c[k]);
                    hi[k] = hi[k].max(c[k]);
                }
            }
        }
        (lo, hi)
    }

    /// Every member put back to the server's state, the client's joints made again from it, and the moves from `ack` to before
    /// `next` replayed with what each did when it ran (the control object runs the rest with the members after). Gives where
    /// each member was predicted before; stops when the tick's work runs out.
    fn restore_and_replay(
        &mut self,
        charge: &mut dyn FnMut(u64) -> bool,
        motion: Option<BodyMotion>,
        player: Option<u32>,
        ack: u32,
        next: u32,
    ) -> Result<Vec<(u32, Place)>, OverBudget> {
        let before = self.sim_places();
        for (&g, m) in self.s.members.iter_mut() {
            let Some(p) = self.book.get(g) else { continue };
            self.s.world.restore(g, p.q, rest_of(p));
            m.updates = p.updates;
        }
        self.rejoin(player);
        // After a correction the moves run again from the ack itself: nothing to replay here then.
        let count = if (next.wrapping_sub(ack) as i32) > 0 {
            next.wrapping_sub(ack).min(MAX_REPLAY)
        } else {
            0
        };
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.stats.restores += 1;
            shared.stats.replayed += u64::from(count);
        }
        self.fill_pre();
        let first = next.wrapping_sub(count);
        for k in 0..count {
            let m = first.wrapping_add(k);
            let motion = self.motion_at(m).or(motion);
            self.step_move(charge, m, motion, player)?;
        }
        self.write_states();
        Ok(before)
    }

    /// The correction of a restore: where the replay has each member at the move before the last (`last`) against where it was
    /// predicted then (`before`), counted; what is drawn eases from where it was drawn. Both lists are in ghost order.
    fn ease_corrections(&mut self, before: &[(u32, Place)], last: &[(u32, Place)]) {
        let find = |list: &[(u32, Place)], g: u32| {
            list.binary_search_by_key(&g, |e| e.0)
                .ok()
                .map(|i| list[i].1)
        };
        let (keep, snap) = (self.config.keep, self.config.snap_distance);
        let mut sizes = Vec::new();
        for (&g, m) in self.s.members.iter_mut() {
            let Some(r) = find(last, g) else {
                continue;
            };
            let drawn = match m.from.take() {
                Some(from) => from,
                None => {
                    if let Some(b) = find(before, g) {
                        sizes.push(length(sub(b.0, r.0)));
                    }
                    m.cur
                }
            };
            let off = sub(drawn.0, r.0);
            if length(off) > snap || keep == 0.0 {
                m.offset = [0.0; 3];
                m.turn = QUAT_IDENTITY;
            } else {
                m.offset = off;
                m.turn = quat_mul(drawn.1, conjugate(r.1));
            }
        }
        if sizes.is_empty() {
            return;
        }
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            let st = &mut shared.stats;
            for d in sizes {
                st.corrections += 1;
                st.correction_sum += f64::from(d);
                st.correction_max = st.correction_max.max(d);
                st.over_cm += u64::from(d > 0.01);
            }
        }
    }

    /// The tick's work ran out: the members are drawn as the client shows them for this tick, and are restored at the next,
    /// easing from the server's state.
    fn fall_back(&mut self) {
        self.s.interpolating = true;
        for (&g, m) in self.s.members.iter_mut() {
            let Some(p) = self.book.get(g) else { continue };
            m.updates = p.updates.wrapping_sub(1);
            m.from = Some(place_q(&p.q));
            m.cur = place_q(&p.q);
            m.prev = m.cur;
        }
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.stats.over_budget += 1;
        }
    }

    /// The members' joints made again in the client's world from their states now: the joints between members (or a member
    /// and the world), a hinge held as it lies, and the hold of the client's own `player` on a member (driven from where each
    /// move leaves the Player, as the server's).
    fn rejoin(&mut self, player: Option<u32>) {
        for id in std::mem::take(&mut self.s.joints) {
            self.s.world.remove_joint(id);
        }
        let member = |e: &EndWire| match *e {
            EndWire::Ghost { ghost, offset } if self.s.members.contains_key(&ghost) => {
                Some(End::Prop {
                    id: ghost,
                    local: offset,
                })
            }
            _ => None,
        };
        let mut made = Vec::new();
        for (&id, j) in self.joints.joints() {
            if j.broken {
                continue;
            }
            if j.kind == KindId::Hold {
                if let (EndWire::Ghost { ghost, offset }, Some(b)) = (j.a, member(&j.b))
                    && Some(ghost) == player
                {
                    made.push((
                        id,
                        Joint::new(Kind::Hold, End::Player { id: ghost, offset }, b, None),
                    ));
                }
                continue;
            }
            let (a, b) = match (member(&j.a), member(&j.b), j.b) {
                (Some(a), Some(b), _) => (a, b),
                (Some(a), None, EndWire::Point(p)) => (a, End::Point(p)),
                _ => continue,
            };
            let kind = match j.kind {
                KindId::Tether | KindId::Winch => Kind::Tether {
                    reach: reach_of(j.length, j.min, j.stiffness, j.damping),
                    winch: None,
                },
                // A hinge's axis is not sent: it is held as it lies.
                KindId::Weld | KindId::Hinge => {
                    let frame = self.s.world.weld_frame(a.prop(), b.prop());
                    let Some(frame) = frame else { continue };
                    Kind::Weld { frame }
                }
                // A grapple pulls its prop from the Player's own step (`step_once`); a hold is made above.
                KindId::Grapple | KindId::Hold => continue,
            };
            let b = match (kind, b) {
                (Kind::Weld { .. }, End::Point(_)) => End::World,
                (_, b) => b,
            };
            made.push((id, Joint::new(kind, a, b, None)));
        }
        for (id, joint) in made {
            if self.s.world.add_joint(id, joint).is_ok() {
                self.s.joints.insert(id);
            }
        }
    }

    /// The member at the far end of the Player's grapple.
    fn grappled(&self, player: u32) -> Option<u32> {
        self.joints
            .joints()
            .values()
            .find_map(|j| match (j.a, j.b) {
                (EndWire::Ghost { ghost: p, .. }, EndWire::Ghost { ghost, .. })
                    if j.kind == KindId::Grapple
                        && !j.broken
                        && p == player
                        && self.s.members.contains_key(&ghost) =>
                {
                    Some(ghost)
                }
                _ => None,
            })
    }

    /// One step for move `m`, the members' states before it kept: the Player's push (from its `motion` at that move), the held
    /// prop's pull from the grapple at that move (as the server's step gives it) or the hold's drive to where the move left the
    /// Player, the pushes made at that move; its work charged to the tick's.
    fn step_move(
        &mut self,
        charge: &mut dyn FnMut(u64) -> bool,
        m: u32,
        motion: Option<BodyMotion>,
        player: Option<u32>,
    ) -> Result<(), OverBudget> {
        self.fill_pre();
        let pusher = motion.map(|mo| pusher_of(player.unwrap_or(0), &mo));
        let at = player.zip(motion).map(|(p, mo)| {
            (
                p,
                PlayerAt::of_box(mo.min, mo.max, mo.forward).looking(mo.elevation),
            )
        });
        self.s.world.set_players(at.into_iter().collect());
        if let Some(p) = player
            && let Some(prop) = self.grappled(p)
            && let Some(far) = self.s.world.get(prop).map(FarEnd::of)
            && let Some(j) = self.joints.pull_at(m, far)
        {
            self.s.world.impulse(prop, j);
        }
        let pushes = self.s.log.take(m);
        for push in &pushes {
            self.apply(push);
        }
        self.s.log.put_back(m, pushes);
        let center = pusher.map(|p| feet(&p));
        let near = |at: [f32; 3]| center.map_or(0.0, |c| length(sub(at, c)));
        let report = self.s.world.tick(pusher.as_slice(), &near);
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.stats.steps += 1;
        }
        self.s.work = self.s.work.saturating_add(report.work);
        if charge(report.work) && self.s.work <= self.config.max_work && !report.over_budget {
            Ok(())
        } else {
            Err(OverBudget)
        }
    }

    /// The members' collision shapes moved: the client's Player meets them where they are from its next move.
    fn note_shapes(&self, step: &mut MoveStep<'_>) {
        for &g in self.s.members.keys() {
            if let Some(p) = self.book.get(g) {
                step.shape_moved(crate::COLLISION_NAME, u32::from(p.slot));
            }
        }
    }

    /// The members' states now into the kept list, in ghost order (refilled in place).
    fn fill_pre(&mut self) {
        let pre = &mut self.s.pre;
        pre.clear();
        for &g in self.s.members.keys() {
            if let Some(p) = self.s.world.get(g) {
                pre.push((g, p.q, p.rest));
            }
        }
    }

    /// The kept states as poses.
    fn pre_places(&self) -> Vec<(u32, Place)> {
        self.s
            .pre
            .iter()
            .map(|(g, q, _)| (*g, place_q(q)))
            .collect()
    }

    fn apply(&mut self, push: &Push) {
        match *push {
            Push::At {
                ghost,
                point,
                impulse,
            } => {
                self.s.world.impulse_at(ghost, impulse, point);
            }
            Push::Blast { at, push } => {
                let near: Vec<(u32, [f32; 3])> = self
                    .s
                    .members
                    .keys()
                    .filter_map(|&g| {
                        let c = self.s.world.get(g)?.q.position();
                        Some((g, hits::blast_impulse(at, c, &push)?))
                    })
                    .collect();
                for (g, j) in near {
                    self.s.world.impulse(g, j);
                }
            }
        }
    }

    fn motion_at(&self, m: u32) -> Option<BodyMotion> {
        self.s.history.get(m).copied()
    }

    /// The members' simulated poses now, in ghost order.
    fn sim_places(&self) -> Vec<(u32, Place)> {
        self.s
            .members
            .keys()
            .filter_map(|&g| Some((g, place_q(&self.s.world.get(g)?.q))))
            .collect()
    }

    /// The tick's drawn poses: each member's simulated pose with its correction, the correction eased by `keep`; the fades
    /// counted down.
    fn settle_drawn(&mut self) {
        let keep = self.config.keep;
        let sims = self.sim_places();
        for (g, m) in self.s.members.iter_mut() {
            let Some(&(p, q)) = sims
                .binary_search_by_key(g, |e| e.0)
                .ok()
                .map(|i| &sims[i].1)
            else {
                continue;
            };
            m.prev = m.cur;
            m.cur = (
                std::array::from_fn(|k| p[k] + m.offset[k]),
                plain_quat_normalize(&quat_mul(m.turn, q)),
            );
            m.offset = m.offset.map(|v| v * keep);
            if m.offset.iter().all(|v| v.abs() < 1.0e-4) {
                m.offset = [0.0; 3];
            }
            m.turn = quat_nlerp(QUAT_IDENTITY, m.turn, keep);
        }
        self.s.fading.retain(|_, f| {
            f.left = f.left.saturating_sub(1);
            f.left > 0
        });
    }

    /// The members' predicted states into the book (none while the tick fell back to interpolation), and the members' list
    /// into the shared state.
    fn write_book(&mut self) {
        if self.s.interpolating {
            for &g in self.s.members.keys() {
                self.book.set_predicted(g, None);
            }
        } else {
            self.write_states();
        }
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.members = self.s.members.keys().copied().collect();
        }
    }

    /// The members' simulated states into the book: what the client's Player meets and its grapple hangs from.
    fn write_states(&mut self) {
        for &g in self.s.members.keys() {
            let q = self.s.world.get(g).map(|p| p.q);
            self.book.set_predicted(g, q);
        }
    }

    /// The predicted spawns: aged (one never confirmed starts to fade), adopted by the ghost that carries the id when the ghost
    /// comes (each ghost is looked at once, so an id given again later never adopts an old ghost), drawn from where it was shown
    /// and fading into the ghost's pose.
    fn tend_spawns(&mut self) {
        let book = &*self.book;
        self.s.seen.retain(|g| book.get(*g).is_some());
        let new: Vec<(u32, bool, u8)> = book
            .props()
            .iter()
            .filter(|(g, _)| !self.s.seen.contains(g))
            .map(|(&g, p)| (g, p.mine, p.predict))
            .collect();
        self.s.seen.extend(new.iter().map(|(g, ..)| *g));
        let Ok(mut shared) = self.shared.0.try_borrow_mut() else {
            return;
        };
        let expired = shared.spawns.expire(TICK, |_| true);
        shared
            .fading
            .extend(expired.into_iter().map(|sp| (sp, PREDICT_FADE)));
        shared.fading.retain_mut(|(_, left)| {
            *left -= TICK;
            *left > 0.0
        });
        for (g, mine, id) in new {
            if !mine {
                continue;
            }
            if let Some(Spawn { q, .. }) = shared.spawns.adopt(id) {
                self.s.fading.insert(
                    g,
                    Fade {
                        from: place_q(&q),
                        left: self.config.fade_ticks,
                    },
                );
            }
        }
    }
}

/// How a book's prop rests: asleep or awake (a frozen one is not predicted).
fn rest_of(p: &ClientProp) -> Rest {
    if p.asleep { Rest::Asleep } else { Rest::Awake }
}

/// The Player's push from how it moves.
fn pusher_of(id: u32, m: &BodyMotion) -> Pusher {
    Pusher {
        id,
        min: m.min,
        max: m.max,
        velocity: m.wish,
        mass: m.mass,
    }
}

/// The point under the middle of a pusher's box (a Player's position).
fn feet(p: &Pusher) -> [f32; 3] {
    [
        (p.min[0] + p.max[0]) * 0.5,
        (p.min[1] + p.max[1]) * 0.5,
        p.min[2],
    ]
}

fn reach_of(length: f32, min: f32, stiffness: f32, damping: f32) -> Reach {
    Reach {
        min,
        max: length,
        spring: (stiffness > 0.0).then_some(Spring { stiffness, damping }),
    }
    .held()
}

/// A shot's gravity: its datablock's when the client has it ballistic.
fn shot_gravity(shot: &Shot, step: &dyn TickStep) -> f32 {
    let Some(db) = shot.datablock else {
        return 0.0;
    };
    if step
        .datablock_number(db, "isBallistic")
        .is_none_or(|b| b == 0.0)
    {
        return 0.0;
    }
    GRAVITY
        * step
            .datablock_number(db, "gravityMod")
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
}

/// A shot moved on by one tick (its gravity first, as the projectile's own step).
fn advance(t: &mut Track) {
    t.velocity[2] -= t.gravity * TICK;
    t.at = std::array::from_fn(|k| t.at[k] + t.velocity[k] * TICK);
}

/// The member the segment `from` to `to` meets first (its box at its predicted state), and the point.
fn first_member_hit(
    members: &BTreeMap<u32, Member>,
    book: &PropBook,
    from: [f32; 3],
    to: [f32; 3],
) -> Option<(u32, [f32; 3])> {
    let mut best: Option<(f32, u32)> = None;
    for &g in members.keys() {
        let Some(p) = book.get(g) else { continue };
        let Some(t) = segment_hits(p, from, to) else {
            continue;
        };
        if best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, g));
        }
    }
    let (t, g) = best?;
    Some((g, interpolate(from, to, t)))
}

/// Where along `from` to `to` the segment enters prop `p`'s box (at its predicted state), as a fraction; none when it misses
/// or a value is not a number ([`segment_box`], in the prop's own frame).
fn segment_hits(p: &ClientProp, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
    let half = p.params?.shape.half_extents();
    let q = p.now();
    let to_local = affine_inverse(&quant::matrix(q.position(), q.rotation()));
    let bx = Box3::new(half.map(|h| -h), half);
    segment_box(mul_p(&to_local, from), mul_p(&to_local, to), &bx).map(|(t, ..)| t)
}

/// A map piece of the client's collision world as a fixed piece of its physics world: a terrain or an interior by its
/// collision, a static shape by its box.
fn piece_of(v: &StaticView) -> Option<(u32, StaticPiece)> {
    let id = MAP_BASE + (v.id & (MAP_BASE - 1));
    if let Some(map) = &v.map {
        return Some((id, StaticPiece::Map(std::rc::Rc::clone(map))));
    }
    Some((id, StaticPiece::of_view(&v.view)?))
}

/// A prop's pose as the client shows it (the record's transform), for a fade.
fn place_of(pose: &Pose) -> Option<Place> {
    let Pose::Affine { xf, .. } = pose else {
        return None;
    };
    let v = xf.quat.map(|c| -c.get());
    let w2 = 1.0 - (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    let w = w2.max(0.0).sqrt();
    let w = if xf.negative_w { -w } else { w };
    Some((xf.position.map(|c| c.get()), [v[0], v[1], v[2], w]))
}

fn place_q(q: &Quantized) -> Place {
    (q.position(), q.rotation())
}

/// `a` at `t` 0 and `b` at `t` 1.
fn blend(a: Place, b: Place, t: f32) -> Place {
    (interpolate(a.0, b.0, t), quat_nlerp(a.1, b.1, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(k: u32) -> Push {
        Push::At {
            ghost: k,
            point: [0.0; 3],
            impulse: [0.0; 3],
        }
    }

    #[test]
    fn the_push_log_drops_acknowledged_moves_and_holds_its_cap_however_the_ack_stalls() {
        let mut log = PushLog::default();
        for m in 0..(MAX_REPLAY * 4) {
            log.add(m, &[push(1); super::super::MAX_IMPULSES_PER_TICK]);
            assert!(log.len <= MAX_LOG, "{}", log.len);
        }
        assert_eq!(log.len, MAX_LOG);
        let newest = MAX_REPLAY * 4 - 1;
        assert_eq!(log.at(newest).len(), super::super::MAX_IMPULSES_PER_TICK);
        assert!(log.at(0).is_empty(), "the oldest went past the cap");
        log.acked(newest);
        assert_eq!(log.len, super::super::MAX_IMPULSES_PER_TICK);
        assert!(log.at(newest - 1).is_empty());
    }

    #[test]
    fn a_move_ring_keeps_one_value_a_move_and_its_newest_moves() {
        let mut ring: MoveRing<u32, 4> = MoveRing::default();
        for m in 0..6 {
            ring.put(m, m * 10);
        }
        ring.put(5, 7);
        assert_eq!(ring.len(), 4);
        assert_eq!(
            (ring.get(1), ring.get(2), ring.get(5)),
            (None, Some(&20), Some(&7))
        );
        ring.clear();
        assert_eq!(ring.get(5), None);
    }

    #[test]
    fn a_shot_segment_that_is_not_finite_meets_no_prop() {
        let params = crate::data::PropNumbers::default().held();
        let prop = ClientProp {
            data: Some(1),
            params: Some(params),
            generation: 0,
            slot: 0,
            hulls: None,
            mine: false,
            predict: 0,
            q: Quantized::default(),
            asleep: false,
            frozen: false,
            held: false,
            updates: 0,
            predicted: None,
            place: crate::client::Placed::default(),
        };
        assert_eq!(
            segment_hits(&prop, [0.0, -5.0, 0.0], [0.0, 5.0, 0.0]).map(|t| t > 0.0),
            Some(true)
        );
        assert_eq!(
            segment_hits(&prop, [f32::NAN, -5.0, 0.0], [0.0, 5.0, 0.0]),
            None
        );
        assert_eq!(
            segment_hits(&prop, [0.0, -5.0, 0.0], [0.0, f32::INFINITY, 0.0]),
            None
        );
    }
}
