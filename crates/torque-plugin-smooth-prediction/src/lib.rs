//! A client predictor that never shows a snap: the simulation corrects exactly as TGE's does, and what is drawn eases the
//! correction's error out over a few ticks.
//!
//! When the server's state differs from the client's prediction, TGE's client takes the server's state and replays the moves the
//! server has not acknowledged: the control object jumps to where the replay puts it, in one frame. [`SmoothPrediction`] lets
//! that happen in the simulation (the same moves, checksums and packets: a TGE server sees nothing new) and draws the body, its
//! vehicle, the eye and the camera offset by the jump, the offset shrinking by [`SmoothPrediction::keep`] every tick and
//! interpolated between ticks like the body itself. A jump longer than [`SmoothPrediction::snap_distance`] (a teleport, a
//! respawn) is drawn at once. A resend that changes what the client controls (a mount, a dismount, a death's camera) drops
//! the offset.
//!
//! Only the classes listed are smoothed ([`SmoothPrediction::classes`]: by default `Player` and `WheeledVehicle`); any other
//! control object draws as TGE's. The rotation is drawn as the simulation has it.
//!
//! ```
//! use torque_engine::EngineBuilder;
//! use torque_plugin_smooth_prediction::{SmoothPrediction, SmoothPredictionPlugin};
//!
//! // The game's predictor: each client starts with it (and `setPredictor("tge")` swaps back).
//! let engine = EngineBuilder::new()
//!     .add_plugins(SmoothPredictionPlugin::new(SmoothPrediction::default().keep(0.7)))
//!     .build()?;
//! # let _ = engine;
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! The arithmetic is additions and products of `f32`s only, so a recorded run draws the same on native and wasm32.

use torque_client::predict::slot::{HeadStep, PredictedBody, Prediction, TickStep, ViewStep};
use torque_engine::{EngineBuilder, Plugin, PluginId};

/// The id the predictor is registered under.
pub const SMOOTH_PREDICTION: &str = "smooth";

/// The most moves between a correction and the tick that replays it that the error is measured across.
const MAX_AHEAD: u32 = 4;

/// An offset shorter than this (units) is dropped: the drawn state is the simulation's again.
const SETTLED: f32 = 1e-4;

/// What the predictor did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SmoothStats {
    /// Corrections whose jump was eased out.
    pub eased: u64,
    /// Corrections drawn at once (longer than the snap distance).
    pub snapped: u64,
    /// Resends that changed what the client controls: the offset dropped.
    pub dropped: u64,
}

/// The predicted body as a correcting head found it, before TGE's handling.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Before {
    body: PredictedBody,
    /// The index of the next move the body had not run then.
    next_move: u32,
}

/// The smoothed predictor (see the [crate docs](crate)).
#[derive(Debug, Clone, PartialEq)]
pub struct SmoothPrediction {
    keep: f32,
    snap_distance: f32,
    classes: Vec<String>,
    /// The offset drawn at the last tick (`back` 0) and at the tick before (`back` 1).
    current: [f32; 3],
    last: [f32; 3],
    /// The first correcting head since the last tick.
    pending: Option<Before>,
    /// The body the offset belongs to (ghost, vehicle).
    body: Option<(u32, bool)>,
    stats: SmoothStats,
}

impl Default for SmoothPrediction {
    /// Smooths `Player` and `WheeledVehicle`, keeps 0.75 of the error a tick (a quarter of it after about 160 ms, 5% after
    /// about 330 ms) and snaps jumps over 5 units.
    fn default() -> SmoothPrediction {
        SmoothPrediction {
            keep: 0.75,
            snap_distance: 5.0,
            classes: vec!["Player".to_owned(), "WheeledVehicle".to_owned()],
            current: [0.0; 3],
            last: [0.0; 3],
            pending: None,
            body: None,
            stats: SmoothStats::default(),
        }
    }
}

impl SmoothPrediction {
    /// The fraction of the drawn error kept after each tick, clamped to 0 (a snap, as TGE's) to 0.98.
    pub fn keep(mut self, keep: f32) -> SmoothPrediction {
        self.keep = if keep.is_finite() {
            keep.clamp(0.0, 0.98)
        } else {
            0.0
        };
        self
    }

    /// The longest jump eased out (units); a longer one is drawn at once.
    pub fn snap_distance(mut self, distance: f32) -> SmoothPrediction {
        self.snap_distance = if distance.is_finite() {
            distance.max(0.0)
        } else {
            0.0
        };
        self
    }

    /// The ghost classes smoothed (a control object or driven vehicle of another class draws as TGE's).
    pub fn classes<I: IntoIterator<Item = S>, S: Into<String>>(
        mut self,
        classes: I,
    ) -> SmoothPrediction {
        self.classes = classes.into_iter().map(Into::into).collect();
        self
    }

    /// What it did so far.
    pub fn stats(&self) -> SmoothStats {
        self.stats
    }

    /// The offset drawn `back` between the last tick (0) and the one before (1).
    pub fn offset(&self, back: f32) -> [f32; 3] {
        let b = if back.is_finite() {
            back.clamp(0.0, 1.0)
        } else {
            0.0
        };
        [0, 1, 2].map(|k| self.current[k] + (self.last[k] - self.current[k]) * b)
    }

    fn smooths(&self, body: &PredictedBody) -> bool {
        self.classes.iter().any(|c| c == body.class)
    }

    fn clear(&mut self) {
        self.current = [0.0; 3];
        self.last = [0.0; 3];
        self.pending = None;
    }

    fn settled(&self) -> bool {
        self.current == [0.0; 3] && self.last == [0.0; 3]
    }
}

impl Prediction for SmoothPrediction {
    fn id(&self) -> &str {
        SMOOTH_PREDICTION
    }

    fn head(&mut self, step: &mut dyn HeadStep) {
        let before = step.body();
        let next_move = step.last_client_move();
        let stats = step.stats();
        step.run_tge();
        let after = step.stats();
        let event = after.events > stats.events || after.event_tails > stats.event_tails;
        // A resend that is no event corrects the prediction (before the server's first match too: the client predicted it wrong).
        if after.resends > stats.resends && !event {
            if self.pending.is_none()
                && let Some(body) = before.filter(|b| self.smooths(b))
            {
                self.pending = Some(Before { body, next_move });
            }
        } else if event {
            self.clear();
            self.stats.dropped += 1;
        }
    }

    fn tick(&mut self, step: &mut dyn TickStep) {
        step.run_tge();
        let Some(body) = step.body().filter(|b| self.smooths(b)) else {
            self.clear();
            self.body = None;
            return;
        };
        if self.body != Some((body.ghost, body.vehicle)) {
            self.clear();
            self.body = Some((body.ghost, body.vehicle));
        }
        let mut last = self.current;
        if let Some(before) = self.pending.take()
            && (before.body.ghost, before.body.vehicle) == (body.ghost, body.vehicle)
        {
            // Where the old prediction stood at the time of the new previous tick: the body before the head, carried on at its
            // last tick's step for every move since but this one. The offset at `back` 1 makes the frame after this tick draw
            // where the frame before it did.
            let ahead = step
                .last_client_move()
                .wrapping_sub(before.next_move)
                .clamp(1, MAX_AHEAD)
                - 1;
            let (p, q) = (before.body.position, before.body.previous);
            let n = ahead as f32;
            let old = [0, 1, 2].map(|k| p[k] + (p[k] - q[k]) * n);
            last = [0, 1, 2].map(|k| old[k] + self.current[k] - body.previous[k]);
            let length2: f32 = last.iter().map(|d| d * d).sum();
            if length2.is_finite() && length2 <= self.snap_distance * self.snap_distance {
                self.stats.eased += 1;
            } else {
                last = [0.0; 3];
                self.stats.snapped += 1;
            }
        }
        let mut current = last.map(|d| d * self.keep);
        if current.iter().all(|d| d.abs() < SETTLED) {
            current = [0.0; 3];
        }
        if last.iter().all(|d| d.abs() < SETTLED) {
            last = [0.0; 3];
        }
        self.last = last;
        self.current = current;
    }

    fn view(&mut self, step: &mut dyn ViewStep) {
        step.run_tge();
        let back = step.back().clamp(0.0, 1.0);
        let offset = self.offset(back);
        let mut drawn = step.drawn();
        // Between a correcting head and the tick that replays from it (a frame drawn before the next tick), the simulation holds
        // the server's older state: the body is drawn where the prediction before the head would have drawn it.
        if let Some(before) = self.pending {
            if let Some(now) = drawn.position() {
                let (p, q) = (before.body.position, before.body.previous);
                drawn.offset([0, 1, 2].map(|k| p[k] + (q[k] - p[k]) * back + offset[k] - now[k]));
            }
            return;
        }
        if !self.settled() {
            drawn.offset(offset);
        }
    }

    fn clone_box(&self) -> Box<dyn Prediction> {
        Box::new(self.clone())
    }
}

/// Registers [`SmoothPrediction`] on the engine's prediction slot: as the game's predictor ([`SmoothPredictionPlugin::new`])
/// or only for a script to swap to (`setPredictor("smooth")`, [`SmoothPredictionPlugin::optional`]). It sends nothing more on
/// the wire, so it has no network version and a client using it joins any server.
#[derive(Debug, Clone, PluginId)]
#[plugin(id = "torque.smooth-prediction")]
pub struct SmoothPredictionPlugin {
    pub prediction: SmoothPrediction,
    /// Whether it is the game's predictor (each client starts with it).
    pub start: bool,
}

impl SmoothPredictionPlugin {
    /// `prediction` as the game's predictor.
    pub fn new(prediction: SmoothPrediction) -> SmoothPredictionPlugin {
        SmoothPredictionPlugin {
            prediction,
            start: true,
        }
    }

    /// `prediction` registered for a swap only: clients start with TGE's.
    pub fn optional(prediction: SmoothPrediction) -> SmoothPredictionPlugin {
        SmoothPredictionPlugin {
            prediction,
            start: false,
        }
    }
}

impl Default for SmoothPredictionPlugin {
    /// [`SmoothPrediction::default`] as the game's predictor.
    fn default() -> SmoothPredictionPlugin {
        SmoothPredictionPlugin::new(SmoothPrediction::default())
    }
}

impl Plugin for SmoothPredictionPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        use torque_client::predict::slot::NetPrediction;
        let prediction = self.prediction.clone();
        if self.start {
            engine.net().prediction(prediction);
        } else {
            engine.net().add_prediction(prediction);
        }
    }
}

#[cfg(test)]
mod tests;
