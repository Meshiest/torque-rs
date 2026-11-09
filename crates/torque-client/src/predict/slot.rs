//! The prediction slot: the predictor a client runs its control object with, one active per game, picked like a renderer.
//!
//! A predictor ([`Prediction`]) sees the three moments of the client's prediction and decides what each does:
//!
//! - **a server head** ([`Prediction::head`], a [`HeadStep`]): the move ack and the control section the server sent (its state
//!   when the checksum differed);
//! - **a tick** ([`Prediction::tick`], a [`TickStep`]): the local moves, run by the control object;
//! - **a view** ([`Prediction::view`], a [`ViewStep`]): the state to draw for the frame ([`Drawn`]), between the last two ticks.
//!
//! Each step can run TGE's scheme for that moment ([`HeadStep::run_tge`] and the others: replay the moves the server has not
//! acknowledged, compare checksums, snap to the server's state on a mismatch) and read or change what it left. Every step
//! also reads the predicted object ([`PredictState::body`]), the counters and the body solver the game picks per vehicle
//! datablock ([`PredictState::body_solver`]). The default, [`TgePrediction`], runs TGE's scheme and nothing else: what a TGE
//! server expects, bit for bit.
//!
//! **Choosing one.** A plugin registers predictors on the engine's network registry ([`NetPrediction`]):
//! `engine.net().prediction(P)` makes `P` the game's predictor (one plugin may; a second fails the build), and
//! `engine.net().add_prediction(P)` registers one that a script or the game can swap to. [`TgePrediction`] is always
//! registered, as [`TGE_PREDICTION`]. An engine that registers none has no slot: its clients run TGE's scheme and get no new
//! console functions. With one, each client keeps a [`PredictionSlot`] (a script engine extension) and has
//! `setPredictor(id)` and `getPredictor()`; a swap takes effect at the client's next tick, with a fresh predictor (the
//! simulation the TGE scheme keeps stays as it is).
//!
//! **What travels.** A predictor that only changes what is drawn (smoothing) sends TGE's bytes, so it works against any server.
//! One that needs more state on the wire registers plugin ghost classes or bulk channels, which list it in the handshake
//! manifest, so a peer without it is refused before play and TGE peers keep TGE's bytes.
//!
//! **Predicting more** (nearby vehicles and props run ahead and rolled back to the server's state, rather than interpolated)
//! fits the same three moments: the head step keeps the server's states of the predicted ghosts, the tick step restores them
//! and runs the ticks since (within a work budget per frame, each class's step deterministic on native and wasm32), and the
//! view step draws them. The engine's own simulation is reached through [`PredictState::predictor`] (the client's
//! `Predictor<W>` for its world type), and the vehicles' solvers through [`PredictState::body_solver`]. Such a predictor
//! would add per-class snapshots and their own determinism tests.
//!
//! **Parts.** A plugin that predicts objects of its own beside the control object (a props plugin's rigid bodies) registers a
//! [`PredictionPart`] (`engine.net().prediction_part(P)`): every client runs each part after its predictor at each of the three
//! moments, whichever predictor the game picks, and a part's `run_tge` does nothing (TGE's scheme ran already). A part reads the
//! moves and acks as a predictor does, and what moves the predicted object makes ([`PredictState::motion`]), the map pieces
//! near a point ([`TickStep::statics_near`]), the projectile ghosts ([`PredictState::shots`]) and a datablock's numbers
//! ([`PredictState::datablock_number`]); at a view it draws ghosts where it puts them ([`Drawn::ghosts`]), starting from where
//! the client shows them ([`ViewStep::shown`]). A part whose objects move with the control object (a prop it pushes or a
//! grapple holds) steps them inside the predictor's tick, in TGE's move loop ([`PredictionPart::moves_begin`],
//! [`PredictionPart::moved`]): after a correction the control object and those objects run the moves again together, in the
//! server's order, and the control object meets the shapes a part moved ([`MoveStep::shape_moved`]) as the next move
//! begins. The loop runs only through TGE's tick ([`TickStep::run_tge`]: a predictor that does not call it runs no part's
//! moves) and never for a Camera control object.

use std::any::Any;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use collide::convex::ObjectView;
use collide::map::MapShape;
use collide::math::F3;
use ghost_world::world::{Eye, Pose};
use net::control::{ControlData, ControlSection};
use net::moves::MoveQueue;
use torque_engine::net::NetRegistry;
use torque_engine::{ClientStage, Engine, PluginId};
use ts_vm::vm::{Native, Vm};

use super::{ClientGlobals, PredictStats, Predictor, game::PredictWorld};

/// The id [`TgePrediction`] is registered under.
pub const TGE_PREDICTION: &str = "tge";

/// A client's predictor (see the [module docs](self)). Every step runs TGE's scheme by default.
///
/// A predictor is made fresh for each connection and each swap ([`Prediction::clone_box`] of the one registered), and is
/// cloned with the client's prediction.
pub trait Prediction: Any {
    /// The id it is registered and chosen under (ASCII letters, digits, `_`, `-`, `.` and `:`).
    fn id(&self) -> &str;

    /// A server head: its move ack and control section.
    fn head(&mut self, step: &mut dyn HeadStep) {
        step.run_tge();
    }

    /// The client's tick: the moves the control object has not run.
    fn tick(&mut self, step: &mut dyn TickStep) {
        step.run_tge();
    }

    /// A view: the state drawn for the frame (or for the tick, on a client without frames).
    fn view(&mut self, step: &mut dyn ViewStep) {
        step.run_tge();
    }

    /// A copy of this predictor with its state.
    fn clone_box(&self) -> Box<dyn Prediction>;
}

impl Clone for Box<dyn Prediction> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

impl fmt::Debug for dyn Prediction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Prediction({:?})", self.id())
    }
}

/// A plugin's own prediction beside the client's predictor ([module docs](self)): run after the predictor at each moment, made
/// fresh for each connection and cloned with the client's prediction. Each step's `run_tge` does nothing for a part.
pub trait PredictionPart: Any {
    /// The id it is registered under (as a predictor's).
    fn id(&self) -> &str;

    /// A server head, after the predictor's.
    fn head(&mut self, step: &mut dyn HeadStep) {
        let _ = step;
    }

    /// The control object is about to run its moves `first` to before `end` in TGE's scheme (after a correction, `first` is the
    /// server's ack and the moves run again from its state): a part whose objects move with the control object restores them
    /// here and steps them with each move ([`PredictionPart::moved`]), so the two are replayed together, in the server's
    /// order. Nothing by default.
    fn moves_begin(&mut self, step: &mut MoveStep<'_>, first: u32, end: u32) {
        let _ = (step, first, end);
    }

    /// The control object ran move `index` (its state is the move's end): what the server's tick does after the control
    /// object's move, the part does here. Nothing by default.
    fn moved(&mut self, step: &mut MoveStep<'_>, index: u32) {
        let _ = (step, index);
    }

    /// The client's tick, after the predictor's.
    fn tick(&mut self, step: &mut dyn TickStep) {
        let _ = step;
    }

    /// A view, after the predictor's.
    fn view(&mut self, step: &mut dyn ViewStep) {
        let _ = step;
    }

    /// A copy of this part (it may start over: a part restores its objects from the server's states at the next update).
    fn clone_box(&self) -> Box<dyn PredictionPart>;
}

impl Clone for Box<dyn PredictionPart> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

impl fmt::Debug for dyn PredictionPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PredictionPart({:?})", self.id())
    }
}

/// What a part's per-move step reads ([`PredictionPart::moves_begin`], [`PredictionPart::moved`]): how the control object
/// moves, which Player that is, and the tick's budget for parts.
#[derive(Debug)]
pub struct MoveStep<'a> {
    /// How the predicted object moves at this point.
    pub motion: BodyMotion,
    /// The predicted Player's ghost while it is on foot (none while it drives a vehicle).
    pub player: Option<u32>,
    work: &'a mut u64,
    moved: &'a mut Vec<(&'static str, u32)>,
}

impl<'a> MoveStep<'a> {
    /// A step at `motion` of the Player `player`, charging `work`, the shapes the part moves noted in `moved`.
    pub fn new(
        motion: BodyMotion,
        player: Option<u32>,
        work: &'a mut u64,
        moved: &'a mut Vec<(&'static str, u32)>,
    ) -> MoveStep<'a> {
        MoveStep {
            motion,
            player,
            work,
            moved,
        }
    }

    /// Charges `units` to the tick's budget for parts ([`PART_TICK_WORK`], as [`TickStep::charge`]): false once it is spent.
    pub fn charge(&mut self, units: u64) -> bool {
        *self.work = self.work.saturating_add(units);
        *self.work <= PART_TICK_WORK
    }

    /// The part moved the shape keyed `key` of the collision source `source`: the control object meets it where it is from the
    /// next move on, the read charged to the tick's budget for parts ([`SHAPE_READ_WORK`] a shape). At most
    /// [`MAX_MOVED_SHAPES`] a move are read.
    pub fn shape_moved(&mut self, source: &'static str, key: u32) {
        if self.moved.len() < MAX_MOVED_SHAPES {
            self.moved.push((source, key));
        }
    }
}

/// The most shapes the parts may say they moved in one move ([`MoveStep::shape_moved`]).
pub const MAX_MOVED_SHAPES: usize = 64;

/// The work of reading one plugin collision shape again inside the move loop ([`MoveStep::shape_moved`]).
pub const SHAPE_READ_WORK: u64 = collide::work::CANDIDATE * 16;

/// The index of the first move of `len` (from `first`) the control object runs when it has run every one before
/// `last_client_move` (a correction sets that back to the server's ack): at most `max` from the end.
pub fn replay_from(last_client_move: u32, first: u32, len: usize, max: usize) -> usize {
    let start = last_client_move.wrapping_sub(first);
    let start = if (start as i32) < 0 {
        0
    } else {
        (start as usize).min(len)
    };
    start + len.saturating_sub(start).saturating_sub(max)
}

/// The parts told that the moves `begin` to before `end` are about to run, the shapes they moved noted in `moved`.
pub fn parts_begin(
    parts: &mut [Box<dyn PredictionPart>],
    motion: BodyMotion,
    player: Option<u32>,
    work: &mut u64,
    moved: &mut Vec<(&'static str, u32)>,
    (begin, end): (u32, u32),
) {
    for part in parts {
        part.moves_begin(&mut MoveStep::new(motion, player, work, moved), begin, end);
    }
}

/// The parts told the control object ran move `index`, the shapes they moved noted in `moved`.
pub fn parts_moved(
    parts: &mut [Box<dyn PredictionPart>],
    motion: BodyMotion,
    player: Option<u32>,
    work: &mut u64,
    moved: &mut Vec<(&'static str, u32)>,
    index: u32,
) {
    for part in parts {
        part.moved(&mut MoveStep::new(motion, player, work, moved), index);
    }
}

/// How the predicted object moves, as the last tick left it: what a part's objects meet (a prop the Player walks into).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyMotion {
    /// Its world box.
    pub min: F3,
    pub max: F3,
    /// Its velocity, and the one its move asks for (a Player's: the move's x and y at its top forward speed, turned by its
    /// rotation; a vehicle's own).
    pub velocity: F3,
    pub wish: F3,
    pub mass: f32,
    /// The way it faces across the ground: its transform's forward axis, x and y (what a prop it holds is carried along).
    pub forward: [f32; 2],
}

/// A map piece near a point, as the client's collision world has it ([`TickStep::statics_near`]): a terrain or an interior by
/// its collision, or another static object by its box.
#[derive(Debug, Clone)]
pub struct StaticView {
    /// Its id in the client's collision world (stable from tick to tick).
    pub id: u32,
    pub view: ObjectView,
    pub map: Option<Rc<MapShape>>,
}

/// A Projectile ghost as its records left it ([`PredictState::shots`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub ghost: u32,
    pub datablock: Option<u32>,
    /// Where its first record put it, its velocity there and the ticks it had lived then.
    pub position: F3,
    pub velocity: F3,
    pub tick: u32,
    /// The ghost of the object that fired it, when the record named one.
    pub source: Option<u32>,
    /// Where it exploded, once its record said so.
    pub exploded: Option<F3>,
}

/// The work the parts may do in one client tick ([`TickStep::charge`]), on the scale of the physics world's work units.
pub const PART_TICK_WORK: u64 = 4_000_000;

/// The most map pieces [`TickStep::statics_near`] gives, and the most shots [`PredictState::shots`] gives.
pub const MAX_STATIC_VIEWS: usize = 64;
pub const MAX_SHOTS: usize = 256;

/// TGE's client-side prediction: replay the moves the server has not acknowledged from its state, compare checksums, snap to
/// the server's state on a mismatch, and draw between the last two ticks. The default predictor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TgePrediction;

impl Prediction for TgePrediction {
    fn id(&self) -> &str {
        TGE_PREDICTION
    }

    fn clone_box(&self) -> Box<dyn Prediction> {
        Box::new(*self)
    }
}

/// The object the client predicts and draws: the vehicle it drives, else its Player (none for a Camera control object).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PredictedBody {
    /// Its ghost index.
    pub ghost: u32,
    /// Its ghost class's name (`Player`, `WheeledVehicle`, a game's class).
    pub class: &'static str,
    /// Where it was at the last tick, and at the tick before (what a view draws at `back` 0 and 1).
    pub position: [f32; 3],
    pub previous: [f32; 3],
    /// Whether it is a vehicle the Player drives.
    pub vehicle: bool,
}

/// What every step reads of the prediction.
pub trait PredictState {
    /// The prediction's counters (resends, corrections, moves stepped).
    fn stats(&self) -> PredictStats;
    /// TGE's `mLastClientMove`: the index of the next move the control object has not run.
    fn last_client_move(&self) -> u32;
    /// The object predicted, as the last tick left it.
    fn body(&self) -> Option<PredictedBody>;
    /// How far back between the last tick (0) and the one before (1) a view is drawn.
    fn back(&self) -> f32;
    /// The client's virtual time at the last tick or view (ms).
    fn sim_ms(&self) -> u32;
    /// The body solver the game picks for vehicles of datablock `datablock` ([`super::game::PredictGame::body_solver`]); `None` is
    /// TGE's, the vehicle's own. The client's vehicles step with it.
    fn body_solver(&self, datablock: u32) -> Option<Box<dyn vehicle::BodySolver>>;
    /// The client's prediction itself: a [`Predictor<W>`] for the client's world type `W` (downcast it), for a predictor that
    /// runs more than TGE's scheme does.
    fn predictor(&mut self) -> &mut dyn Any;
    /// How the predicted object moves (none for a Camera control object).
    fn motion(&self) -> Option<BodyMotion>;
    /// The Projectile ghosts the client holds, in ghost order, at most [`MAX_SHOTS`] (none when the game's records give none).
    fn shots(&self) -> Vec<Shot>;
    /// Field `name` of datablock `id` as a number, when the client's datablock carries it.
    fn datablock_number(&self, id: u32, name: &str) -> Option<f32>;
}

/// A server head ([`Prediction::head`]).
pub trait HeadStep: PredictState {
    /// The move ack (the move list already took it).
    fn ack(&self) -> u32;
    /// The control section: the server's state when its checksum differed.
    fn control(&self) -> &ControlSection<Box<dyn ControlData>>;
    /// The client's moves.
    fn moves(&self) -> &MoveQueue;
    /// TGE's handling of the head: on a resend, the server's state and a replay of every move it has not acknowledged at the
    /// next tick.
    fn run_tge(&mut self);
}

/// The client's tick ([`Prediction::tick`]).
pub trait TickStep: PredictState {
    /// The client's moves.
    fn moves(&self) -> &MoveQueue;
    /// TGE's tick: every move from `mLastClientMove` to the end of the list run by the control object, then the vehicles the
    /// client runs on its own.
    fn run_tge(&mut self);
    /// The map pieces (terrains, interiors, static shapes) whose boxes meet `min..max` in the client's collision world, in its
    /// order, at most [`MAX_STATIC_VIEWS`]; the work is charged to the client's tick budget.
    fn statics_near(&mut self, min: F3, max: F3) -> Vec<StaticView>;
    /// Charges `units` of a part's work to the tick's budget for parts ([`PART_TICK_WORK`], shared by every part): false once
    /// it is spent, and the part leaves the rest of its work for a later tick.
    fn charge(&mut self, units: u64) -> bool;
}

/// A view ([`Prediction::view`]).
pub trait ViewStep: PredictState {
    /// TGE's view: the control object, its vehicle and the eye drawn between the last two ticks.
    fn run_tge(&mut self);
    /// What the view draws of the predicted object, to change after [`ViewStep::run_tge`].
    fn drawn(&mut self) -> Drawn<'_>;
    /// Where the client shows ghost `index` this frame when nothing draws it elsewhere: its record's pose, between the server's
    /// updates for a class shown so.
    fn shown(&self, index: u32) -> Option<Pose>;
}

/// What a view draws of the predicted object: each part the view set (`None`: not drawn this frame).
#[derive(Debug)]
pub struct Drawn<'a> {
    /// The control Player's pose.
    pub control: Option<&'a mut Pose>,
    /// The pose of the vehicle it drives.
    pub vehicle: Option<&'a mut Pose>,
    /// The control object's eye point (what the listener and first-person effects follow).
    pub eye: Option<&'a mut [f32; 3]>,
    /// The camera the world is seen through.
    pub view: Option<&'a mut Eye>,
    /// Other ghosts drawn where a predictor or a part puts them this frame (by ghost index; emptied before each view). The
    /// predicted object and its vehicle are drawn by the fields above.
    pub ghosts: &'a mut BTreeMap<u32, Pose>,
}

impl Drawn<'_> {
    /// Moves every part drawn by `offset`: the body, its vehicle, the eye and the camera together.
    pub fn offset(&mut self, offset: [f32; 3]) {
        let shift = |p: &mut [f32; 3]| {
            for (k, d) in offset.iter().enumerate() {
                p[k] += d;
            }
        };
        for pose in [self.control.as_deref_mut(), self.vehicle.as_deref_mut()]
            .into_iter()
            .flatten()
        {
            if let Some(p) = pose_position_mut(pose) {
                shift(p);
            }
        }
        if let Some(e) = self.eye.as_deref_mut() {
            shift(e);
        }
        if let Some(v) = self.view.as_deref_mut() {
            shift(&mut v.position);
        }
    }

    /// The drawn position of the body: its vehicle's when it drives one, else its own.
    pub fn position(&self) -> Option<[f32; 3]> {
        pose_position(self.vehicle.as_deref().or(self.control.as_deref())?)
    }
}

/// The position of a pose of the kinds a predicted object is drawn with (a Player's, a vehicle's, an Item's).
pub fn pose_position(pose: &Pose) -> Option<[f32; 3]> {
    match pose {
        Pose::Euler { position, .. }
        | Pose::Quat { position, .. }
        | Pose::Item { position, .. } => Some(*position),
        _ => None,
    }
}

fn pose_position_mut(pose: &mut Pose) -> Option<&mut [f32; 3]> {
    match pose {
        Pose::Euler { position, .. }
        | Pose::Quat { position, .. }
        | Pose::Item { position, .. } => Some(position),
        _ => None,
    }
}

// ---- the steps over the client's prediction

/// The predicted body of `sim` in `world`.
fn body_of<W: PredictWorld>(sim: &Predictor<W>, world: &W) -> Option<PredictedBody> {
    use ghost_world::set::ClientSet;
    if sim.camera.is_some() {
        return None;
    }
    let pr = sim.predicted.as_ref()?;
    let class = |index: u32| {
        world
            .ghosts
            .get(&index)
            .and_then(|g| g.class)
            .map_or("", <W::Set as ClientSet>::class_name)
    };
    if let Some(v) = pr.vehicle.as_ref().filter(|_| pr.player.s.mount.is_some()) {
        let s = &v.v.s;
        return Some(PredictedBody {
            ghost: v.index,
            class: class(v.index),
            position: s.delta_pos,
            previous: [0, 1, 2].map(|k| s.delta_pos[k] + s.delta_pos_vec[k]),
            vehicle: true,
        });
    }
    let s = &pr.player.s;
    Some(PredictedBody {
        ghost: pr.index,
        class: class(pr.index),
        position: s.position(),
        previous: s.delta_pos,
        vehicle: false,
    })
}

/// How the predicted object of `sim` moves.
fn motion_of<W: PredictWorld>(sim: &Predictor<W>) -> Option<BodyMotion> {
    if sim.camera.is_some() {
        return None;
    }
    Some(motion_of_predicted(sim.predicted.as_ref()?))
}

/// The predicted Player's ghost while it is on foot.
pub(super) fn player_of_predicted(pr: &super::Predicted) -> Option<u32> {
    (pr.vehicle.is_none() || pr.player.s.mount.is_none()).then_some(pr.index)
}

/// How the predicted Player (or the vehicle it drives) moves.
pub(super) fn motion_of_predicted(pr: &super::Predicted) -> BodyMotion {
    if let Some(v) = pr.vehicle.as_ref().filter(|_| pr.player.s.mount.is_some()) {
        let s = &v.v.s;
        return BodyMotion {
            min: s.world_box.min,
            max: s.world_box.max,
            velocity: s.rigid.lin_vel,
            wish: s.rigid.lin_vel,
            mass: s.mass,
            forward: [s.obj_to_world[1], s.obj_to_world[5]],
        };
    }
    let s = &pr.player.s;
    let m = s.obj_to_world;
    let speed = pr.data.max_forward_speed;
    let (x, y) = (s.last_move.x, s.last_move.y);
    let wish = [0, 1].map(|r| (m[r * 4] * x + m[r * 4 + 1] * y) * speed);
    BodyMotion {
        min: s.world_box.min,
        max: s.world_box.max,
        velocity: s.velocity,
        wish: [wish[0], wish[1], 0.0],
        mass: s.mass,
        forward: [m[1], m[5]],
    }
}

/// The Projectile ghosts of `world`, in ghost order.
fn shots_of<W: PredictWorld>(world: &W) -> Vec<Shot> {
    use crate::records::EngineRecord;
    world
        .ghosts
        .iter()
        .filter_map(|(&ghost, g)| {
            let r = g.record.as_ref()?.shot()?;
            Some(Shot {
                ghost,
                datablock: g.datablock(),
                position: r.start.or(g.position)?,
                velocity: r.velocity,
                tick: r.tick,
                source: r.source,
                exploded: r.exploded,
            })
        })
        .take(MAX_SHOTS)
        .collect()
}

/// Field `name` of datablock `id` of `world` as a number.
fn number_of<W: PredictWorld>(world: &W, id: u32, name: &str) -> Option<f32> {
    use crate::records::EngineDatablock;
    use ghosts::Value;
    match world.datablocks.get(&id)?.value(name, 0)? {
        Value::F32(v) => Some(*v),
        Value::Int(v) => Some(*v as f32),
        Value::Bool(b) => Some(f32::from(u8::from(*b))),
        _ => None,
    }
}

macro_rules! predict_state {
    ($ty:ident) => {
        impl<W: PredictWorld> PredictState for $ty<'_, W> {
            fn stats(&self) -> PredictStats {
                self.sim.stats
            }

            fn last_client_move(&self) -> u32 {
                self.sim.last_client_move
            }

            fn body(&self) -> Option<PredictedBody> {
                body_of(self.sim, &*self.world)
            }

            fn back(&self) -> f32 {
                self.sim.back
            }

            fn sim_ms(&self) -> u32 {
                self.sim.sim_ms
            }

            fn body_solver(&self, datablock: u32) -> Option<Box<dyn vehicle::BodySolver>> {
                self.sim.game().body_solver(&*self.world, datablock)
            }

            fn predictor(&mut self) -> &mut dyn Any {
                &mut *self.sim
            }

            fn motion(&self) -> Option<BodyMotion> {
                motion_of(self.sim)
            }

            fn shots(&self) -> Vec<Shot> {
                shots_of(&*self.world)
            }

            fn datablock_number(&self, id: u32, name: &str) -> Option<f32> {
                number_of(&*self.world, id, name)
            }
        }
    };
}

pub(super) struct HeadCx<'a, W: PredictWorld> {
    pub(super) sim: &'a mut Predictor<W>,
    pub(super) ack: u32,
    pub(super) control: &'a ControlSection<Box<dyn ControlData>>,
    pub(super) world: &'a W,
    pub(super) moves: &'a MoveQueue,
    /// TGE's scheme ran already (a part's step): `run_tge` does nothing.
    pub(super) ran: bool,
}

predict_state!(HeadCx);

impl<W: PredictWorld> HeadStep for HeadCx<'_, W> {
    fn ack(&self) -> u32 {
        self.ack
    }

    fn control(&self) -> &ControlSection<Box<dyn ControlData>> {
        self.control
    }

    fn moves(&self) -> &MoveQueue {
        self.moves
    }

    fn run_tge(&mut self) {
        if !std::mem::replace(&mut self.ran, true) {
            self.sim
                .tge_on_head(self.ack, self.control, self.world, self.moves);
        }
    }
}

pub(super) struct TickCx<'a, W: PredictWorld> {
    pub(super) sim: &'a mut Predictor<W>,
    pub(super) moves: &'a MoveQueue,
    pub(super) world: &'a W,
    pub(super) globals: &'a ClientGlobals,
    pub(super) ran: bool,
}

predict_state!(TickCx);

impl<W: PredictWorld> TickStep for TickCx<'_, W> {
    fn moves(&self) -> &MoveQueue {
        self.moves
    }

    fn run_tge(&mut self) {
        if !std::mem::replace(&mut self.ran, true) {
            self.sim.tge_process(self.moves, self.world, self.globals);
        }
    }

    fn statics_near(&mut self, min: F3, max: F3) -> Vec<StaticView> {
        self.sim.statics_near(self.world, min, max)
    }

    fn charge(&mut self, units: u64) -> bool {
        self.sim.part_work = self.sim.part_work.saturating_add(units);
        self.sim.part_work <= PART_TICK_WORK
    }
}

pub(super) struct ViewCx<'a, W: PredictWorld> {
    pub(super) sim: &'a mut Predictor<W>,
    pub(super) world: &'a mut W,
    pub(super) ran: bool,
}

predict_state!(ViewCx);

impl<W: PredictWorld> ViewStep for ViewCx<'_, W> {
    fn run_tge(&mut self) {
        if !std::mem::replace(&mut self.ran, true) {
            self.sim.tge_update_view(self.world);
        }
    }

    fn shown(&self, index: u32) -> Option<Pose> {
        let world = &**self.world;
        let pose = world.ghosts.get(&index)?.pose()?;
        Some(world.warped_pose(index, pose))
    }

    fn drawn(&mut self) -> Drawn<'_> {
        let driven = body_of(self.sim, &*self.world)
            .filter(|b| b.vehicle)
            .map(|b| b.ghost);
        let world = &mut **self.world;
        Drawn {
            control: world.control_render.as_mut(),
            vehicle: driven.and_then(|i| world.vehicle_render.get_mut(&i)),
            eye: world.control_eye.as_mut(),
            view: world.view.as_mut(),
            ghosts: &mut world.ghost_render,
        }
    }
}

// ---- the slot

/// A predictor registered on the engine: what each client's slot makes its predictors from.
#[derive(Clone)]
pub struct PredictionEntry {
    id: String,
    prototype: Rc<dyn Prediction>,
}

impl PredictionEntry {
    /// The entry of `prediction` under its id.
    pub fn new<P: Prediction>(prediction: P) -> PredictionEntry {
        PredictionEntry {
            id: prediction.id().to_owned(),
            prototype: Rc::new(prediction),
        }
    }

    /// Its id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// A fresh predictor of it.
    pub fn make(&self) -> Box<dyn Prediction> {
        self.prototype.clone_box()
    }
}

impl fmt::Debug for PredictionEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PredictionEntry")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// A part registered on the engine ([`NetPrediction::prediction_part`]): what each client's slot makes its parts from.
#[derive(Clone)]
pub struct PartEntry {
    id: String,
    prototype: Rc<dyn PredictionPart>,
}

impl PartEntry {
    /// The entry of `part` under its id.
    pub fn new<P: PredictionPart>(part: P) -> PartEntry {
        PartEntry {
            id: part.id().to_owned(),
            prototype: Rc::new(part),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// A fresh part of it.
    pub fn make(&self) -> Box<dyn PredictionPart> {
        self.prototype.clone_box()
    }
}

impl fmt::Debug for PartEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartEntry")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Why a swap of predictors was refused: no predictor has the id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownPrediction {
    /// The id asked for.
    pub id: String,
    /// The ids registered, in the slot's order.
    pub registered: Vec<String>,
}

impl fmt::Display for UnknownPrediction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no predictor {:?} (registered: {})",
            self.id,
            self.registered.join(", ")
        )
    }
}

impl std::error::Error for UnknownPrediction {}

/// One client's predictors: [`TgePrediction`] and those its engine registered, and the one it runs. A client's script engine
/// keeps it as an extension ([`PredictionSlot::of`]); the client's prediction follows it from its next tick.
#[derive(Debug)]
pub struct PredictionSlot {
    entries: Vec<PredictionEntry>,
    parts: Vec<PartEntry>,
    current: Cell<usize>,
    /// Counts the swaps (from 1: a prediction that never saw the slot takes its predictor).
    generation: Cell<u64>,
}

impl PredictionSlot {
    /// A slot with [`TgePrediction`] (unless an entry has its id) and `entries` in order, running `start` when it is one of
    /// them, else TGE's. An invalid or repeated id is left out, and the second value says so for each.
    pub fn new(entries: &[PredictionEntry], start: Option<&str>) -> (PredictionSlot, Vec<String>) {
        let mut kept: Vec<PredictionEntry> = Vec::new();
        let mut warnings = Vec::new();
        if !entries.iter().any(|e| e.id == TGE_PREDICTION) {
            kept.push(PredictionEntry::new(TgePrediction));
        }
        for e in entries {
            if !PluginId::is_valid(&e.id) {
                warnings.push(format!(
                    "a predictor's id {:?} is not valid: left out",
                    e.id
                ));
            } else if kept.iter().any(|k| k.id == e.id) {
                warnings.push(format!(
                    "a second predictor registered as {:?}: the first is kept",
                    e.id
                ));
            } else {
                kept.push(e.clone());
            }
        }
        let current = match start {
            Some(id) => kept.iter().position(|e| e.id == id).unwrap_or_else(|| {
                warnings.push(format!(
                    "the game's predictor {id:?} is not registered: TGE's runs"
                ));
                0
            }),
            None => kept
                .iter()
                .position(|e| e.id == TGE_PREDICTION)
                .unwrap_or(0),
        };
        let slot = PredictionSlot {
            entries: kept,
            parts: Vec::new(),
            current: Cell::new(current),
            generation: Cell::new(1),
        };
        (slot, warnings)
    }

    /// The slot with `parts` too, in order: a part whose id is not valid, or repeats one, is left out, and the second value says
    /// so for each.
    pub fn with_parts(mut self, parts: &[PartEntry]) -> (PredictionSlot, Vec<String>) {
        let mut warnings = Vec::new();
        for p in parts {
            if !PluginId::is_valid(&p.id) {
                warnings.push(format!(
                    "a prediction part's id {:?} is not valid: left out",
                    p.id
                ));
            } else if self.parts.iter().any(|k| k.id == p.id) {
                warnings.push(format!(
                    "a second prediction part registered as {:?}: the first is kept",
                    p.id
                ));
            } else {
                self.parts.push(p.clone());
            }
        }
        (self, warnings)
    }

    /// The parts' ids, in the order they run.
    pub fn part_ids(&self) -> impl ExactSizeIterator<Item = &str> + '_ {
        self.parts.iter().map(PartEntry::id)
    }

    /// Fresh parts, in the order they run.
    pub fn make_parts(&self) -> Vec<Box<dyn PredictionPart>> {
        self.parts.iter().map(PartEntry::make).collect()
    }

    /// The registered predictors' ids, in the slot's order.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = &str> + '_ {
        self.entries.iter().map(PredictionEntry::id)
    }

    /// Whether a predictor has the id `id`.
    pub fn has(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// The id of the predictor the client runs.
    pub fn current(&self) -> &str {
        self.entries[self.current.get()].id()
    }

    /// Runs the predictor `id` from the client's next tick. Err: no predictor has the id (the one running stays).
    pub fn switch(&self, id: &str) -> Result<(), UnknownPrediction> {
        let Some(index) = self.entries.iter().position(|e| e.id == id) else {
            return Err(UnknownPrediction {
                id: id.to_owned(),
                registered: self.ids().map(str::to_owned).collect(),
            });
        };
        if index != self.current.get() {
            self.current.set(index);
            self.generation.set(self.generation.get() + 1);
        }
        Ok(())
    }

    /// Counts the swaps: a client's prediction makes a new predictor when it changed.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// A fresh predictor of the one the client runs.
    pub fn make(&self) -> Box<dyn Prediction> {
        self.entries[self.current.get()].make()
    }

    /// The slot of the client whose script engine is `vm` (None: its engine registered no predictor, so it runs TGE's).
    pub fn of(vm: &Vm) -> Option<Rc<PredictionSlot>> {
        vm.extension::<PredictionSlot>()
    }

    /// Gives `vm` the slot and the console functions `setPredictor(id)` and `getPredictor()`.
    pub fn install(self, vm: &mut Vm) {
        vm.set_extension(Rc::new(self));
        vm.register(
            "setPredictor",
            2,
            2,
            "(id) - Runs the client's control object with the predictor registered as id, from the next tick.",
            |vm, c| {
                let id = String::from_utf8_lossy(crate::arg(c, 1)).trim().to_owned();
                let Some(slot) = PredictionSlot::of(vm) else {
                    return Native::Bool(false);
                };
                match slot.switch(&id) {
                    Ok(()) => Native::Bool(true),
                    Err(e) => {
                        vm.warn(format!("setPredictor: {e}"));
                        Native::Bool(false)
                    }
                }
            },
        );
        vm.register(
            "getPredictor",
            1,
            1,
            "() - The id of the predictor the client's control object runs with.",
            |vm, _| {
                let id = PredictionSlot::of(vm)
                    .map_or_else(|| TGE_PREDICTION.to_owned(), |s| s.current().to_owned());
                Native::Str(id.into_bytes())
            },
        );
    }
}

/// The console functions a client's slot gives its script engine: any engine that registers a predictor or a part has them.
pub const SLOT_NATIVES: [&str; 2] = ["setPredictor", "getPredictor"];

/// The game's predictor ([`NetPrediction::prediction`]).
struct StartPrediction(String);

/// Set once the slot's client recipe is recorded.
struct SlotRecipe;

/// The network registry's calls that register predictors ([module docs](self)).
pub trait NetPrediction {
    /// Registers `prediction` and makes it the game's predictor: each client starts with it. One plugin may set it; a second
    /// fails the build.
    fn prediction<P: Prediction>(&mut self, prediction: P) -> &mut Self;

    /// Registers `prediction` for a script or the game to swap to (`setPredictor(id)`, [`PredictionSlot::switch`]).
    fn add_prediction<P: Prediction>(&mut self, prediction: P) -> &mut Self;

    /// Registers `part`: every client runs it after its predictor ([`PredictionPart`]).
    fn prediction_part<P: PredictionPart>(&mut self, part: P) -> &mut Self;
}

impl NetPrediction for NetRegistry<'_> {
    fn prediction<P: Prediction>(&mut self, prediction: P) -> &mut Self {
        let id = prediction.id().to_owned();
        self.add_prediction(prediction);
        self.builder().set_slot(Rc::new(StartPrediction(id)));
        self
    }

    fn add_prediction<P: Prediction>(&mut self, prediction: P) -> &mut Self {
        let builder = self.builder();
        builder.push(PredictionEntry::new(prediction));
        recipe(builder);
        self
    }

    fn prediction_part<P: PredictionPart>(&mut self, part: P) -> &mut Self {
        let builder = self.builder();
        builder.push(PartEntry::new(part));
        recipe(builder);
        self
    }
}

/// Records the slot's client recipe once.
fn recipe(builder: &mut torque_engine::EngineBuilder) {
    if builder.resource::<SlotRecipe>().is_none() {
        builder.insert_resource(SlotRecipe);
        builder.client(ClientStage::Natives, install_slot);
    }
}

/// The client recipe: the slot of the engine's predictors on the client's script engine.
fn install_slot(vm: &mut Vm, engine: &Engine) -> Result<(), String> {
    let start = engine.slot::<StartPrediction>();
    let (slot, mut warnings) = PredictionSlot::new(
        engine.list::<PredictionEntry>(),
        start.as_deref().map(|s| s.0.as_str()),
    );
    let (slot, more) = slot.with_parts(engine.list::<PartEntry>());
    warnings.extend(more);
    for warning in warnings {
        vm.warn(warning);
    }
    slot.install(vm);
    Ok(())
}

#[cfg(test)]
#[path = "slot_tests.rs"]
mod tests;
