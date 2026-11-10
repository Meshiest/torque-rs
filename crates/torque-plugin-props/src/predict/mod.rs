//! Prop prediction on a client: the props its own Player holds, stands on or pushes run ahead in a small physics world of the
//! client's, so they are drawn where they will be; the rest are drawn between the server's updates.
//!
//! [`PropPredictionPlugin`] registers [`PropPart`] with the prediction slot (`engine.net().prediction_part(..)`): a part that
//! runs after whichever predictor the game picks. Each client tick the part
//!
//! - picks the **predicted set**: the props tied to the client's Player by a joint (held), the props under its feet, and the
//!   props it touched in the last [`PredictConfig::touch_ticks`] ticks, each with the props jointed to it
//!   ([`crate::JointBook::connected`], counted), at most [`PredictConfig::max_predicted`] (8 by default, at most
//!   [`MAX_PREDICTED`]);
//! - keeps them in its own [`PropWorld`] with the map pieces near them ([`torque_client::predict::slot::TickStep::statics_near`])
//!   and the props around them as fixed boxes, the other players and vehicles near them as boxes at their ghosts' latest
//!   records ([`torque_client::predict::slot::TickStep::around`]), and the joints between them as the server's joints (a hinge is held as it lies);
//! - steps them with the client's Player, move by move, inside the slot's move loop: for each move, the Player's push
//!   ([`crate::world::Pusher`]) as the move left it, the held prop's pull the Player's step just made, the predicted impulses
//!   tagged with that move, and one step. When the server sent new states, or the Player's moves run again after a correction,
//!   each is first put back to the server's state exactly (the wire's precision is the server's own), and the moves already
//!   run since the ack are replayed with what each did; the Player and the props then run the rest together, so a correction
//!   of the Player is a correction of what it holds too;
//! - writes the predicted states into the client's book ([`crate::client::ClientProp::predicted`]), which the client's Player
//!   meets through the props' collision source and a grapple hangs from.
//!
//! At each view the predicted props are drawn between their last two ticks, through a **smoothed correction**: the simulated
//! state takes the server's at once, and the drawn state eases the jump out (it keeps [`PredictConfig::keep`] of it a tick; a
//! jump past [`PredictConfig::snap_distance`] is drawn at once). A prop that leaves the set fades from where it was drawn to its
//! interpolated pose over [`PredictConfig::fade_ticks`] ticks; one that joins eases from the server's state.
//!
//! Prediction is not bit-exact with the server (its world holds contact and warm-start caches the client never sees): it is
//! for drawing props where they belong.
//!
//! **Predicted impulses** ([`PropPrediction::impulse`], [`PropPrediction::blast`], the client functions
//! `propPredictImpulse(%ghost, %point, %impulse)` and `propPredictBlast(%point, %radius, %impulse)`): a push the client shows at
//! once on a predicted prop, tagged with the move it was made at. They are the client's own and never sent: the server's
//! impulse for the same cause (a projectile's hit, an explosion, a game's own push) comes back in its state, and the restore
//! takes it; one the server never made is gone from the next restore and eases out. Only props in the predicted set take one;
//! each tick takes at most [`MAX_IMPULSES_PER_TICK`], each held to [`crate::hits::MAX_HIT_IMPULSE`].
//!
//! **Shots.** A projectile the client's own Player fired is followed from its first record, put ahead by the moves the
//! server has not acknowledged, and a predicted prop it meets takes the server's hit impulse ([`crate::hits`]) at once, and
//! every predicted prop in its explosion's reach its area impulse. The projectile datablock's `impulse`, `areaImpulse` and
//! `areaImpulseRadius` are read where the client's datablock carries them, else [`PredictConfig::shot`].
//!
//! **Predicted spawns** ([`PropPrediction::spawn`], `propPredictSpawn(%data, %transform)`): a prop the client shows at once
//! under a predict id ([`torque_engine::predict_ids`], the scheme ropes use), which the game sends with its own spawn command
//! (the server's `propSpawn(.., %owner, %predictId)`). The server's ghost that carries the id adopts it (drawn from where the
//! client showed it); one never confirmed within two seconds fades out. A game draws the waiting ones
//! ([`PropPrediction::spawns`]): they have no ghost yet.

mod part;

use std::cell::RefCell;
use std::rc::Rc;

use torque_engine::args::vec3;
use torque_engine::predict_ids::{PREDICT_FADE, PredictIds};
use torque_engine::{EngineBuilder, Plugin, PluginId, Requires, Res, function};

pub use part::PropPart;

use crate::client::{ClientProps, params_of};
use crate::hits::{MAX_HIT_IMPULSE, ShotPush};
use crate::joints::client::ClientJoints;
use crate::quant::{self, Quantized};
use crate::world::PropWorld;

/// The id the part is registered under.
pub const PART_ID: &str = "torque.props";

/// The most props a client predicts at once (the config is held to it).
pub const MAX_PREDICTED: usize = 32;

/// The most predicted impulses one tick takes (more are refused).
pub const MAX_IMPULSES_PER_TICK: usize = 8;

/// The most moves a restore replays (an older ack replays the newest this many).
pub const MAX_REPLAY: u32 = 64;

/// How the client predicts props.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PredictConfig {
    /// The most props predicted at once (1 to [`MAX_PREDICTED`]).
    pub max_predicted: usize,
    /// How long a prop the Player touched stays predicted (ticks).
    pub touch_ticks: u32,
    /// The ticks a prop that leaves the set fades to its interpolated pose over.
    pub fade_ticks: u32,
    /// The fraction of a drawn correction kept each tick (0: snap; at most 0.98).
    pub keep: f32,
    /// The longest correction eased out (units); a longer one is drawn at once.
    pub snap_distance: f32,
    /// The sub-steps of the client's physics world (the server's `$Pref::Props::Substeps`, 1 to 8).
    pub substeps: u8,
    /// What a shot pushes with when the client's projectile datablock does not say ([`crate::hits`]).
    pub shot: ShotPush,
    /// The most physics work the part does in one tick (work units), within the slot's own budget for parts; past it the
    /// tick draws the props as interpolated.
    pub max_work: u64,
    /// Whether the other players and vehicles near the predicted props are boxes in the client's world, as they are on the
    /// server (on by default; off, a prop pushed toward one runs on through it).
    pub bystanders: bool,
}

impl Default for PredictConfig {
    /// 8 props, 16 ticks after a touch, a 4-tick fade, 0.75 of a correction kept a tick, snaps past 5 units, 4 sub-steps,
    /// shots by their speed.
    fn default() -> PredictConfig {
        PredictConfig {
            max_predicted: 8,
            touch_ticks: 16,
            fade_ticks: 4,
            keep: 0.75,
            snap_distance: 5.0,
            substeps: 4,
            shot: ShotPush::default(),
            max_work: torque_client::predict::slot::PART_TICK_WORK,
            bystanders: true,
        }
    }
}

impl PredictConfig {
    /// Every value in its range.
    #[must_use]
    pub fn held(self) -> PredictConfig {
        let finite = |v: f32, lo: f32, hi: f32, d: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { d }
        };
        PredictConfig {
            max_predicted: self.max_predicted.clamp(1, MAX_PREDICTED),
            touch_ticks: self.touch_ticks.min(1024),
            fade_ticks: self.fade_ticks.clamp(1, 64),
            keep: finite(self.keep, 0.0, 0.98, 0.0),
            snap_distance: finite(self.snap_distance, 0.0, 1000.0, 0.0),
            substeps: self
                .substeps
                .clamp(1, torque_physics_rapier::limits::MAX_SUBSTEPS),
            shot: ShotPush::of(
                self.shot.impulse,
                Some(self.shot.area),
                Some(self.shot.radius),
            ),
            max_work: self.max_work,
            bystanders: self.bystanders,
        }
    }
}

/// Values kept by move index, newest last, at most `N`: a move run again replaces its value, the oldest go past the cap.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MoveRing<T, const N: usize> {
    items: std::collections::VecDeque<(u32, T)>,
}

impl<T, const N: usize> Default for MoveRing<T, N> {
    fn default() -> Self {
        MoveRing {
            items: std::collections::VecDeque::new(),
        }
    }
}

impl<T, const N: usize> MoveRing<T, N> {
    /// Keeps `value` for move `index`, in place of what it had.
    pub(crate) fn put(&mut self, index: u32, value: T) {
        if let Some(slot) = self.items.iter_mut().find(|(i, _)| *i == index) {
            slot.1 = value;
            return;
        }
        if self.items.len() >= N {
            self.items.pop_front();
        }
        self.items.push_back((index, value));
    }

    /// The value of move `index`.
    pub(crate) fn get(&self, index: u32) -> Option<&T> {
        self.items
            .iter()
            .rev()
            .find_map(|(i, v)| (*i == index).then_some(v))
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }
}

/// A predicted impulse waiting for the next tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Push {
    /// On one prop (its ghost), at a world point.
    At {
        ghost: u32,
        point: [f32; 3],
        impulse: [f32; 3],
    },
    /// On every predicted prop within `radius` of `at`, away from it, `impulse` falling off to nothing at the radius.
    Blast { at: [f32; 3], push: ShotPush },
}

/// A predicted spawn waiting for its ghost: the datablock and the pose it was shown at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spawn {
    pub data: u32,
    pub q: Quantized,
}

/// What the part did, for a game's display and the tests.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PredictStats {
    /// Restores from the server's states, moves replayed and physics steps taken.
    pub restores: u64,
    pub replayed: u64,
    pub steps: u64,
    /// Corrections (each prop each restore: how far its replay put it from where it was predicted, 0 for a prediction the
    /// server's state bore out) and their sizes in all (units), and the largest.
    pub corrections: u64,
    pub correction_sum: f64,
    pub correction_max: f32,
    /// Corrections past a centimeter.
    pub over_cm: u64,
    /// Predicted impulses taken, and refused (not predicted, not finite, or past the tick's cap).
    pub impulses: u64,
    pub refused: u64,
    /// Shots that hit a predicted prop.
    pub shot_hits: u64,
    /// Ticks whose work ran out (the props were drawn as the client shows them, and restored at the next).
    pub over_budget: u64,
}

impl PredictStats {
    /// The mean correction (units), 0 with none.
    pub fn mean_correction(&self) -> f64 {
        if self.corrections == 0 {
            0.0
        } else {
            self.correction_sum / self.corrections as f64
        }
    }
}

/// What the part and the client functions share.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    pub(crate) config: PredictConfig,
    /// The props predicted after the last tick, in ghost order.
    pub(crate) members: Vec<u32>,
    pub(crate) pushes: Vec<Push>,
    pub(crate) spawns: PredictIds<Spawn>,
    /// Spawns that were never confirmed, fading: the spawn and the seconds left.
    pub(crate) fading: Vec<(Spawn, f32)>,
    pub(crate) stats: PredictStats,
    /// Where each predicted prop was drawn at the last view (ghost, position, rotation), in ghost order.
    pub(crate) drawn: Vec<(u32, [f32; 3], [f32; 4])>,
    /// The collision hulls each predicted prop of `collision = "shape"` is stepped with, by ghost.
    pub(crate) hulls: std::collections::BTreeMap<u32, Rc<crate::hulls::PropHulls>>,
}

/// A client's prop prediction: the engine resource the part and the client functions share ([module docs](self)). One client
/// per engine, as every plugin's client state.
#[derive(Debug, Clone, Default)]
pub struct PropPrediction(pub(crate) Rc<RefCell<Shared>>);

impl PropPrediction {
    /// A prediction of `config` (held to its ranges).
    pub fn new(config: PredictConfig) -> PropPrediction {
        PropPrediction(Rc::new(RefCell::new(Shared {
            config: config.held(),
            ..Shared::default()
        })))
    }

    pub fn config(&self) -> PredictConfig {
        self.0.borrow().config
    }

    /// The props predicted now, in ghost order.
    pub fn members(&self) -> Vec<u32> {
        self.0.borrow().members.clone()
    }

    /// The collision hulls the predicted prop of ghost `ghost` is stepped with, when its datablock asked for its shape's and the
    /// client built them from its own copy of the shape; none for a prop that is not predicted or collides as a box.
    pub fn hulls(&self, ghost: u32) -> Option<Rc<crate::hulls::PropHulls>> {
        self.0.borrow().hulls.get(&ghost).cloned()
    }

    /// Whether the prop of ghost `ghost` is predicted now.
    pub fn is_predicted(&self, ghost: u32) -> bool {
        self.0.borrow().members.binary_search(&ghost).is_ok()
    }

    pub fn stats(&self) -> PredictStats {
        self.0.borrow().stats.clone()
    }

    /// Where each predicted prop was drawn at the last view: (ghost, position, rotation `x, y, z, w`), in ghost order.
    pub fn drawn(&self) -> Vec<(u32, [f32; 3], [f32; 4])> {
        self.0.borrow().drawn.clone()
    }

    /// Pushes the predicted prop of ghost `ghost` by `impulse` at the world point `point` from the next tick, tagged with the
    /// move made then; never sent ([module docs](self)). False (nothing changes) for a prop that is not predicted, a value that
    /// is not finite or past the world, or past the tick's [`MAX_IMPULSES_PER_TICK`]; the impulse is held to
    /// [`MAX_HIT_IMPULSE`].
    pub fn impulse(&self, ghost: u32, point: [f32; 3], impulse: [f32; 3]) -> bool {
        let mut s = self.0.borrow_mut();
        let finite = point
            .iter()
            .all(|v| v.is_finite() && v.abs() <= quant::MAX_COORD)
            && impulse.iter().all(|v| v.is_finite());
        if !finite
            || s.members.binary_search(&ghost).is_err()
            || s.pushes.len() >= MAX_IMPULSES_PER_TICK
        {
            s.stats.refused += 1;
            return false;
        }
        let size = crate::world::length(impulse);
        let impulse = if size > MAX_HIT_IMPULSE {
            impulse.map(|v| v / size * MAX_HIT_IMPULSE)
        } else {
            impulse
        };
        s.pushes.push(Push::At {
            ghost,
            point,
            impulse,
        });
        true
    }

    /// Pushes every predicted prop within `radius` of `at` away from it, `impulse` falling off to nothing at the radius
    /// ([`crate::hits::blast_impulse`]), from the next tick; never sent. False for a value that is not finite, a blast that
    /// pushes nothing, or past the tick's [`MAX_IMPULSES_PER_TICK`].
    pub fn blast(&self, at: [f32; 3], radius: f32, impulse: f32) -> bool {
        let mut s = self.0.borrow_mut();
        let push = ShotPush::of(None, Some(impulse), Some(radius));
        if !at
            .iter()
            .all(|v| v.is_finite() && v.abs() <= quant::MAX_COORD)
            || !push.blasts()
            || s.pushes.len() >= MAX_IMPULSES_PER_TICK
        {
            s.stats.refused += 1;
            return false;
        }
        s.pushes.push(Push::Blast { at, push });
        true
    }

    /// Shows a prop of the client's datablock `data` at once at a position and a unit quaternion `x, y, z, w`, under a new
    /// predict id (1 to 255) for the game to send with its spawn command; None when 16 wait already or the pose is not finite
    /// or past the world.
    pub fn spawn(&self, data: u32, (position, rotation): ([f32; 3], [f32; 4])) -> Option<u8> {
        if !position
            .iter()
            .chain(&rotation)
            .all(|v| v.is_finite() && v.abs() <= quant::MAX_COORD)
        {
            return None;
        }
        let q = Quantized::of(&torque_physics_rapier::BodyState {
            position,
            rotation,
            ..Default::default()
        });
        self.0.borrow_mut().spawns.issue(Spawn { data, q })
    }

    /// The predicted spawns a game draws (they have no ghost yet): the predict id (0 for one fading out), the spawn and how
    /// opaque it is (1, down to 0 as one never confirmed fades).
    pub fn spawns(&self) -> Vec<(u8, Spawn, f32)> {
        let s = self.0.borrow();
        let waiting = s.spawns.iter().map(|(id, sp)| (id, *sp, 1.0));
        let fading = s
            .fading
            .iter()
            .map(|(sp, left)| (0, *sp, (left / PREDICT_FADE).clamp(0.0, 1.0)));
        waiting.chain(fading).collect()
    }
}

/// Adds prop prediction to a client ([module docs](self)): the part on the prediction slot, the [`PropPrediction`] resource
/// and the client functions. It needs [`crate::PropsPlugin`] (added when the engine has none) and sends nothing new, so it has
/// no network version.
#[derive(Debug, Clone, Copy, Default, PluginId)]
#[plugin(id = "torque.props-prediction")]
pub struct PropPredictionPlugin {
    pub config: PredictConfig,
}

impl Plugin for PropPredictionPlugin {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<crate::PropsPlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        use torque_client::predict::slot::NetPrediction;
        let book = engine
            .resource::<ClientProps>()
            .cloned()
            .unwrap_or_default();
        let joints = engine
            .resource::<ClientJoints>()
            .cloned()
            .unwrap_or_default();
        let shared = PropPrediction::new(self.config);
        engine.insert_resource(shared.clone());
        engine
            .net()
            .prediction_part(PropPart::new(book, joints, shared));
        engine.client_script().natives(declared());
    }
}

/// shows a push on a prop the client predicts at once: `ghost` is the prop's ghost index, `point` and `impulse` are `"x y z"`
///
/// The push is the client's own and is never sent: the server's push for the same cause comes back in its state. Only a prop
/// the client predicts takes one (`propPredicted()` lists them), at most 8 a tick, each held to 5,000. Returns whether it was
/// taken.
#[function(name = "propPredictImpulse")]
fn prop_predict_impulse(
    prediction: Res<PropPrediction>,
    ghost: i32,
    point: &str,
    impulse: &str,
) -> bool {
    let (Ok(point), Ok(impulse), Ok(ghost)) = (
        vec3(point, "point"),
        vec3(impulse, "impulse"),
        u32::try_from(ghost),
    ) else {
        return false;
    };
    prediction.impulse(ghost, point, impulse)
}

/// shows an explosion's push on every prop the client predicts within `radius` of `point` at once, `impulse` at the center
/// falling off to nothing at the radius
///
/// Never sent, as `propPredictImpulse`; at most 8 pushes a tick. Returns whether it was taken.
#[function(name = "propPredictBlast")]
fn prop_predict_blast(
    prediction: Res<PropPrediction>,
    point: &str,
    radius: f32,
    impulse: f32,
) -> bool {
    let Ok(point) = vec3(point, "point") else {
        return false;
    };
    prediction.blast(point, radius, impulse)
}

/// the ghost indexes of the props the client predicts now, separated by spaces
#[function(name = "propPredicted")]
fn prop_predicted(prediction: Res<PropPrediction>) -> String {
    prediction
        .members()
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// shows a prop of the client's `PropData` datablock at once at `transform` (`"x y z [ax ay az angle]"`) and returns the predict
/// id the game sends with its own spawn command (the server's `propSpawn(.., %owner, %predictId)`)
///
/// The server's ghost that carries the id is the prop; one never confirmed within two seconds fades out. At most 16 wait at
/// once: past that, for a datablock that is not a `PropData` or a transform that is not finite, the result is 0.
#[function(name = "propPredictSpawn")]
fn prop_predict_spawn(
    vm: torque_engine::VmMut,
    prediction: Res<PropPrediction>,
    data: &str,
    transform: &str,
) -> i32 {
    let Some(db) = vm.objects().find(data.as_bytes()) else {
        return 0;
    };
    if params_of(&vm, db).is_none() {
        return 0;
    }
    let Ok(pose) = crate::server::parse_transform(transform) else {
        return 0;
    };
    prediction.spawn(db, pose).map_or(0, i32::from)
}

pub(crate) fn declared() -> Vec<torque_engine::script::NativeDecl> {
    torque_engine::natives![
        prop_predict_impulse,
        prop_predict_blast,
        prop_predicted,
        prop_predict_spawn
    ]
}

/// The client's world for prediction: a [`PropWorld`] with the server's default limits.
pub(crate) fn world(config: &PredictConfig) -> PropWorld {
    PropWorld::new(config.substeps, crate::world::PropLimits::default())
}
