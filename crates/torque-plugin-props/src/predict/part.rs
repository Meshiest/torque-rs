//! The part the prediction slot runs ([`PropPart`]): the predicted set, the client's physics world, the restore and replay, the
//! smoothed drawing and the shots ([module docs](super)).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use torque_client::predict::slot::{
    BodyMotion, HeadStep, PredictionPart, Shot, StaticView, TickStep, ViewStep,
};
use torque_collide::math::{
    Box3, QUAT_IDENTITY, affine_inverse, interpolate, mul_p, plain_quat_normalize,
};
use torque_collide::source::segment_box;
use torque_engine::predict_ids::PREDICT_FADE;
use torque_ghost_world::world::Pose;
use torque_physics_rapier::TICK;

use super::{MAX_REPLAY, PART_ID, PredictConfig, PropPrediction, Push, Spawn};
use crate::client::{ClientProp, ClientProps, PropBook};
use crate::hits::{self, ShotPush};
use crate::joints::client::{ClientJoints, JointBook};
use crate::joints::records::EndWire;
use crate::joints::{End, Joint, Kind, KindId, Reach, Spring};
use crate::quant::{self, Quantized, conjugate, quat_mul, quat_nlerp};
use crate::records::xf_of;
use crate::world::{PUSH_MARGIN, PropWorld, Pusher, Rest, StaticPiece, length, world_half};

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

    fn at(&self, m: u32) -> &[Push] {
        self.by_move.get(&m).map_or(&[], Vec::as_slice)
    }
}

/// What the part keeps between ticks.
struct State {
    world: PropWorld,
    tick: u64,
    /// The first move the server has not acknowledged, from the last head.
    ack: u32,
    members: BTreeMap<u32, Member>,
    /// The tick each prop was last touched by the Player.
    touched: BTreeMap<u32, u64>,
    history: VecDeque<(u32, BodyMotion)>,
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
}

impl State {
    fn new(config: &PredictConfig) -> State {
        State {
            world: super::world(config),
            tick: 0,
            ack: 0,
            members: BTreeMap::new(),
            touched: BTreeMap::new(),
            history: VecDeque::new(),
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

    fn tick(&mut self, step: &mut dyn TickStep) {
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
        let mut cx = Cx {
            config,
            book: &mut book,
            joints: &joints,
            shared: &self.shared,
            s: state,
        };
        cx.tick(step);
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
    fn tick(&mut self, step: &mut dyn TickStep) {
        self.s.tick += 1;
        self.s.interpolating = false;
        self.s.work = 0;
        let next = step.last_client_move();
        let newest = next.wrapping_sub(1);
        let player = step.body().filter(|b| !b.vehicle).map(|b| b.ghost);
        let motion = step.motion();
        if let Some(m) = motion {
            self.s.history.retain(|(i, _)| *i != newest);
            if self.s.history.len() >= HISTORY {
                self.s.history.pop_front();
            }
            self.s.history.push_back((newest, m));
        }
        let ack = self.s.ack;
        self.s.log.acked(ack);
        self.tend_spawns();
        let wanted = match (player, motion) {
            (Some(p), Some(m)) => self.wanted(p, &m),
            _ => Vec::new(),
        };
        let joined = self.join_and_leave(&wanted);
        let mut pushes = self.take_pushes();
        let shots = step.shots();
        self.follow_shots(newest, player, &shots, &*step, &mut pushes);
        self.s.log.add(newest, &pushes);
        if !self.s.members.is_empty() {
            self.refresh_statics(step);
            let stale = self
                .s
                .members
                .iter()
                .any(|(g, m)| self.book.get(*g).is_some_and(|p| p.updates != m.updates));
            let done = if stale || joined {
                self.restore_and_replay(step, ack, next, player, motion)
            } else {
                let pusher = motion.map(|m| pusher_of(player.unwrap_or(0), &m));
                self.step_once(step, newest, pusher, player, &pushes)
            };
            if done.is_err() {
                self.fall_back();
            }
        }
        if !self.s.interpolating {
            self.settle_drawn();
        }
        self.write_book();
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
                || self
                    .book
                    .get(g)
                    .is_none_or(|p| p.frozen || p.params.is_none())
            {
                continue;
            }
            out.push(g);
            queue.extend(self.joints.connected(g));
        }
    }

    /// Makes the members `wanted`: new ones are spawned in the client's world, ones that left are taken out and fade. Whether
    /// any joined.
    fn join_and_leave(&mut self, wanted: &[u32]) -> bool {
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
            let st = p.q.state();
            if self
                .s
                .world
                .spawn(g, data, params, (st.position, st.rotation), None, 0)
                .is_err()
            {
                continue;
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

    /// The pushes the game asked for since the last tick, on members only.
    fn take_pushes(&mut self) -> Vec<Push> {
        let mut shared = self.shared.0.borrow_mut();
        let mut taken: Vec<Push> = std::mem::take(&mut shared.pushes);
        shared.stats.impulses += taken.len() as u64;
        taken.retain(|p| match p {
            Push::At { ghost, .. } => self.s.members.contains_key(ghost),
            Push::Blast { .. } => true,
        });
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
    /// far enough (or a prop around them moved): the map's first, then the nearest [`MAX_NEIGHBORS`] props.
    fn refresh_statics(&mut self, step: &mut dyn TickStep) {
        let (lo, hi) = self.region();
        let center: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
        let refetch = self
            .s
            .statics_at
            .is_none_or(|at| length(sub(center, at)) > REFETCH);
        if refetch {
            let pad = REACH + REFETCH;
            let (a, b) = (lo.map(|v| v - pad), hi.map(|v| v + pad));
            self.s.statics = step
                .statics_near(a, b)
                .iter()
                .filter_map(piece_of)
                .collect();
            self.s.statics_at = Some(center);
        }
        let mut near: Vec<(f32, u32, Quantized)> = self
            .book
            .props()
            .iter()
            .filter(|(g, p)| {
                !self.s.members.contains_key(g)
                    && p.params.is_some()
                    && (0..3).all(|k| {
                        let c = p.q.position()[k];
                        c >= lo[k] - REACH && c <= hi[k] + REACH
                    })
            })
            .map(|(&g, p)| (length(sub(p.q.position(), center)), g, p.q))
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
    /// `next` replayed; each member's jump is drawn eased out. Stops when the tick's work runs out.
    fn restore_and_replay(
        &mut self,
        step: &mut dyn TickStep,
        ack: u32,
        next: u32,
        player: Option<u32>,
        motion: Option<BodyMotion>,
    ) -> Result<(), OverBudget> {
        let before = self.sim_places();
        for (&g, m) in self.s.members.iter_mut() {
            let Some(p) = self.book.get(g) else { continue };
            let rest = if p.asleep { Rest::Asleep } else { Rest::Awake };
            self.s.world.restore(g, p.q, rest);
            m.updates = p.updates;
        }
        self.rejoin();
        let count = next.wrapping_sub(ack).min(MAX_REPLAY);
        let first = next.wrapping_sub(count);
        let mut last = None;
        for k in 0..count {
            let m = first.wrapping_add(k);
            last = Some(self.sim_places());
            let pusher = self
                .motion_at(m)
                .or(motion)
                .map(|mo| pusher_of(player.unwrap_or(0), &mo));
            let pushes = self.s.log.at(m).to_vec();
            self.step_once(step, m, pusher, player, &pushes)?;
        }
        let last = last.unwrap_or_else(|| self.sim_places());
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.stats.restores += 1;
            shared.stats.replayed += u64::from(count);
        }
        self.ease_corrections(&before, &last);
        Ok(())
    }

    /// The correction of a restore: where the replay has each member at the tick before this one (`last`) against where it was
    /// predicted then (`before`), counted; what is drawn eases from where it was drawn.
    fn ease_corrections(&mut self, before: &BTreeMap<u32, Place>, last: &BTreeMap<u32, Place>) {
        let (keep, snap) = (self.config.keep, self.config.snap_distance);
        let mut sizes = Vec::new();
        for (&g, m) in self.s.members.iter_mut() {
            let Some(r) = last.get(&g).copied() else {
                continue;
            };
            let drawn = match m.from.take() {
                Some(from) => from,
                None => {
                    if let Some(&b) = before.get(&g) {
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
    /// and the world), a hinge held as it lies.
    fn rejoin(&mut self) {
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
                // A grapple pulls its prop from the Player's own step (`step_once`).
                KindId::Grapple => continue,
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

    /// One step for move `m`: the Player's push, the held prop's pull from the grapple at that move (as the server's step
    /// gives it), the pushes made at that move; its work charged to the tick's.
    fn step_once(
        &mut self,
        step: &mut dyn TickStep,
        m: u32,
        pusher: Option<Pusher>,
        player: Option<u32>,
        pushes: &[Push],
    ) -> Result<(), OverBudget> {
        if let Some(p) = player
            && let Some(prop) = self.grappled(p)
            && let Some(j) = self.joints.pull_at(m)
        {
            self.s.world.impulse(prop, j);
        }
        for push in pushes {
            self.apply(push);
        }
        let pushers: Vec<Pusher> = pusher.into_iter().collect();
        let center = pusher.map(|p| feet(&p));
        let near = |at: [f32; 3]| center.map_or(0.0, |c| length(sub(at, c)));
        let report = self.s.world.tick(&pushers, &near);
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.stats.steps += 1;
        }
        self.s.work = self.s.work.saturating_add(report.work);
        if step.charge(report.work) && self.s.work <= self.config.max_work && !report.over_budget {
            Ok(())
        } else {
            Err(OverBudget)
        }
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
        self.s
            .history
            .iter()
            .find(|(i, _)| *i == m)
            .map(|(_, mo)| *mo)
    }

    /// The members' simulated poses now.
    fn sim_places(&self) -> BTreeMap<u32, Place> {
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
            let Some(&(p, q)) = sims.get(g) else { continue };
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
        for &g in self.s.members.keys() {
            let q = if self.s.interpolating {
                None
            } else {
                self.s.world.get(g).map(|p| p.q)
            };
            self.book.set_predicted(g, q);
        }
        if let Ok(mut shared) = self.shared.0.try_borrow_mut() {
            shared.members = self.s.members.keys().copied().collect();
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

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| a[k] - b[k])
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
    fn a_shot_segment_that_is_not_finite_meets_no_prop() {
        let params = crate::data::PropNumbers::default().held();
        let prop = ClientProp {
            data: Some(1),
            params: Some(params),
            slot: 0,
            mine: false,
            predict: 0,
            q: Quantized::default(),
            asleep: false,
            frozen: false,
            updates: 0,
            predicted: None,
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
