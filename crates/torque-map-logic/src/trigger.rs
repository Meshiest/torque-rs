//! Triggers and filters.
//!
//! **Volumes.** A trigger is its brush model's convex volumes placed at the entity's origin. Every tick each toucher's box
//! ([`crate::bus::Toucher`]) is looked up in the triggers' bins (the scene container's grid, [`torque_scene::bins`], here over
//! the map's own units: 512-unit cells), and each trigger it may meet is tested exactly: the box against each volume by the
//! separating axis test.
//!
//! **Who counts.** A toucher counts when the spawnflags take its kind (1 players, 2 NPCs, 8 physics objects, 64 everything)
//! and the trigger's filter (`filtername`, a filter entity) passes it. Only those join the touch set.
//!
//! **Outputs**, each with the toucher as the activator and the trigger as the caller, in this order each tick: for each that
//! left, `OnEndTouch`, then `OnEndTouchAll` when none is left; for each that entered, `OnStartTouch`, and `OnStartTouchAll`
//! for the first one in. Then, while anyone is inside and the trigger is not waiting: `OnTrigger` with the first of them.
//! `trigger_multiple` then waits `wait` seconds (0 waits 0.2 s; -1 removes it as a `trigger_once` does); `trigger_once`
//! removes itself after its `OnTrigger`. A disabled trigger forgets its touchers (no outputs). Inputs: `Enable`, `Disable`,
//! `Toggle`, `TouchTest` (`trigger_multiple`: `OnTouching` or `OnNotTouching`), and `StartTouch` and `EndTouch`, which fire
//! the outputs of the caller entering or leaving without changing the touch set.
//!
//! **Filters** (`filter_activator_name`, `filter_activator_class`, `filter_multi`): a test of an activator by its name or
//! class (a trailing `*` matches a prefix), or of up to five other filters (`Filter01`-`Filter05`; `filtertype` 0 all, 1 any),
//! turned over by `Negated`. `TestActivator` fires `OnPass` or `OnFail`. A filter that names a missing filter passes; filters
//! nest at most eight deep, and a filter that reaches itself fails.

use std::collections::HashMap;

use torque_scene::bins::{BinGeometry, Bins};
use torque_scene::geometry::Geometry;

use crate::bus::{Ctx, Ent, EntityId, MapLogic, ToucherKind};
use crate::entities::{Keys, Kind};
use crate::volume::{Pose, Volume};

/// The deepest chain of filters naming filters.
const MAX_FILTER_DEPTH: usize = 8;

/// A trigger's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    /// The volumes where they are (the map's space).
    pub volumes: Vec<Volume>,
    pub enabled: bool,
    pub once: bool,
    /// Seconds between `OnTrigger`s (`trigger_multiple`); negative: once.
    pub wait: f32,
    /// Until when it waits (microseconds).
    pub waiting_until: Option<u64>,
    /// Who is inside, in id order.
    pub touching: Vec<EntityId>,
    /// The filter's name (lowercase), or empty.
    pub filter: String,
}

impl Trigger {
    pub(crate) fn new(k: &mut Keys<'_>, volumes: &[Volume]) -> Trigger {
        let pose = Pose {
            origin: k.ent.origin,
            angles: k.ent.angles,
        };
        let once = k.ent.class == "trigger_once";
        Trigger {
            volumes: volumes.iter().map(|v| v.placed(&pose)).collect(),
            enabled: !k.bool("StartDisabled"),
            once,
            wait: if once { -1.0 } else { k.f32("wait") },
            waiting_until: None,
            touching: Vec::new(),
            filter: k.str("filtername").trim().to_ascii_lowercase(),
        }
    }

    /// The box around its volumes.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut out: Option<([f32; 3], [f32; 3])> = None;
        for v in &self.volumes {
            let (lo, hi) = v.bounds();
            out = Some(match out {
                None => (lo, hi),
                Some((a, b)) => (
                    [0, 1, 2].map(|k| a[k].min(lo[k])),
                    [0, 1, 2].map(|k| b[k].max(hi[k])),
                ),
            });
        }
        out
    }

    fn start_outputs(&self, ml: &mut MapLogic, me: EntityId, who: EntityId, first: bool) {
        ml.fire(me, "OnStartTouch", "", Some(who), Some(me));
        if first {
            ml.fire(me, "OnStartTouchAll", "", Some(who), Some(me));
        }
    }

    fn end_outputs(&self, ml: &mut MapLogic, me: EntityId, who: EntityId, last: bool) {
        ml.fire(me, "OnEndTouch", "", Some(who), Some(me));
        if last {
            ml.fire(me, "OnEndTouchAll", "", Some(who), Some(me));
        }
    }

    /// `OnTrigger` for `who` unless waiting; then the wait or the removal.
    fn trigger(&mut self, ml: &mut MapLogic, me: EntityId, who: EntityId) {
        if self.waiting_until.is_some_and(|t| ml.now_us() < t) {
            return;
        }
        ml.fire(me, "OnTrigger", "", Some(who), Some(me));
        if self.once || self.wait < 0.0 {
            ml.remove(me);
            return;
        }
        let wait = if self.wait <= 0.0 {
            0.2
        } else {
            f64::from(self.wait)
        };
        self.waiting_until = Some(ml.now_us() + (wait * 1e6) as u64);
    }

    pub(crate) fn think(&mut self, ml: &mut MapLogic, me: EntityId) {
        if !self.enabled {
            self.touching.clear();
            return;
        }
        let Some(e) = ml.entity(me) else { return };
        let flags = e.spawnflags;
        let candidates = ml.triggers.touchers_of(me);
        let tests = (candidates.len() as u64).saturating_mul(self.volumes.len() as u64);
        if tests > 0 && (!ml.has_work() || !ml.spend(tests)) {
            // No work left this tick: the touch set stays as it was.
            return;
        }
        let mut now: Vec<EntityId> = Vec::new();
        for t in candidates {
            let takes = flags & 64 != 0
                || match t.kind {
                    ToucherKind::Player => flags & 1 != 0,
                    ToucherKind::Npc => flags & 2 != 0,
                    ToucherKind::Physics => flags & 8 != 0,
                };
            if !takes || !self.volumes.iter().any(|v| v.overlaps_box(t.min, t.max)) {
                continue;
            }
            if !passes(ml, &self.filter, Some(t.id), 0) {
                continue;
            }
            now.push(t.id);
        }
        now.sort_unstable();
        now.dedup();
        let left: Vec<EntityId> = self
            .touching
            .iter()
            .copied()
            .filter(|i| !now.contains(i))
            .collect();
        let entered: Vec<EntityId> = now
            .iter()
            .copied()
            .filter(|i| !self.touching.contains(i))
            .collect();
        let had = !self.touching.is_empty();
        for (k, &who) in left.iter().enumerate() {
            self.end_outputs(ml, me, who, k + 1 == left.len() && now.is_empty());
        }
        for (k, &who) in entered.iter().enumerate() {
            self.start_outputs(ml, me, who, k == 0 && !had);
        }
        self.touching = now;
        if let Some(&who) = entered.first().or(self.touching.first()) {
            self.trigger(ml, me, who);
        }
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str) -> bool {
        let me = ctx.me;
        match input {
            "enable" => self.enabled = true,
            "disable" => {
                self.enabled = false;
                self.touching.clear();
            }
            "toggle" => {
                self.enabled = !self.enabled;
                if !self.enabled {
                    self.touching.clear();
                }
            }
            "touchtest" if !self.once => {
                let out = if self.touching.is_empty() {
                    "OnNotTouching"
                } else {
                    "OnTouching"
                };
                ml.fire(me, out, "", ctx.activator, Some(me));
            }
            "starttouch" => {
                if let Some(who) = ctx.caller {
                    self.start_outputs(ml, me, who, self.touching.is_empty());
                    if self.enabled {
                        self.trigger(ml, me, who);
                    }
                }
            }
            "endtouch" => {
                if let Some(who) = ctx.caller {
                    self.end_outputs(ml, me, who, self.touching.is_empty());
                }
            }
            _ => return false,
        }
        true
    }
}

/// A filter's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub by: FilterBy,
    pub negated: bool,
}

/// What a filter tests.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterBy {
    /// The activator's name (lowercase; a trailing `*` matches a prefix).
    Name(String),
    /// The activator's class.
    Class(String),
    /// Other filters by name: all (`any` false) or any.
    Multi { any: bool, filters: Vec<String> },
}

fn matches_name(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(_) => name.starts_with(pattern.trim_end_matches('*')),
        None => pattern == name,
    }
}

impl Filter {
    pub(crate) fn new(k: &mut Keys<'_>) -> Filter {
        let negated = k.bool("Negated");
        let by = match k.ent.class.as_str() {
            "filter_activator_name" => {
                FilterBy::Name(k.str("filtername").trim().to_ascii_lowercase())
            }
            "filter_activator_class" => {
                FilterBy::Class(k.str("filterclass").trim().to_ascii_lowercase())
            }
            _ => FilterBy::Multi {
                any: k.int("filtertype") == 1,
                filters: (1..=5)
                    .filter_map(|i| k.ent.key(&format!("Filter{i:02}")))
                    .map(|n| n.trim().to_ascii_lowercase())
                    .filter(|n| !n.is_empty())
                    .collect(),
            },
        };
        Filter { by, negated }
    }

    fn test(&self, ml: &MapLogic, activator: Option<EntityId>, depth: usize) -> bool {
        let ent = activator.and_then(|a| ml.entity(a));
        let raw = match &self.by {
            FilterBy::Name(n) => ent.is_some_and(|e| matches_name(n, &e.name)),
            FilterBy::Class(c) => ent.is_some_and(|e| matches_name(c, &e.class)),
            FilterBy::Multi { any, filters } => {
                if filters.is_empty() {
                    true
                } else if *any {
                    filters.iter().any(|f| passes(ml, f, activator, depth + 1))
                } else {
                    filters.iter().all(|f| passes(ml, f, activator, depth + 1))
                }
            }
        };
        raw != self.negated
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str) -> bool {
        if input != "testactivator" {
            return false;
        }
        let out = if self.test(ml, ctx.activator, 0) {
            "OnPass"
        } else {
            "OnFail"
        };
        ml.fire(ctx.me, out, "", ctx.activator, Some(ctx.me));
        true
    }
}

/// Whether the filter named `name` passes `activator` (an empty name, or one that names no filter, passes).
pub(crate) fn passes(ml: &MapLogic, name: &str, activator: Option<EntityId>, depth: usize) -> bool {
    if name.is_empty() {
        return true;
    }
    if depth >= MAX_FILTER_DEPTH {
        return false;
    }
    for id in ml.find(name) {
        match ml.kind(id) {
            Some(Kind::Filter(f)) if !f.test(ml, activator, depth) => return false,
            // The filter being tested itself: a cycle, which fails.
            Some(Kind::Busy) => return false,
            _ => {}
        }
    }
    true
}

/// The triggers' broad phase: their boxes in bins.
#[derive(Debug, Default)]
pub struct TriggerIndex {
    bins: Option<Bins>,
    boxes: HashMap<u32, ([f32; 3], [f32; 3])>,
    /// This tick's touchers by the trigger whose box they meet.
    touched: HashMap<u32, Vec<crate::bus::Toucher>>,
}

struct Boxes<'a>(&'a HashMap<u32, ([f32; 3], [f32; 3])>);

impl Geometry for Boxes<'_> {
    fn sphere(&self, object: u32) -> ([f32; 3], f32) {
        let (lo, hi) = self.0.get(&object).copied().unwrap_or_default();
        let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
        let r = (0..3)
            .map(|k| (hi[k] - lo[k]) * 0.5)
            .map(|d| d * d)
            .sum::<f32>()
            .sqrt();
        (c, r)
    }
    fn world_box(&self, object: u32) -> ([f32; 3], [f32; 3]) {
        self.0.get(&object).copied().unwrap_or_default()
    }
    fn type_mask(&self, _object: u32) -> u32 {
        1
    }
    fn collision_disabled(&self, _object: u32) -> bool {
        false
    }
}

/// The bins' grid over a map's units: 32 cells of 512 units (a map of ±16384 units fits one ring).
fn geometry() -> BinGeometry {
    BinGeometry::new(32, 512.0, u32::MAX)
}

impl TriggerIndex {
    pub(crate) fn insert(&mut self, id: EntityId, ent: &Ent) {
        let Kind::Trigger(t) = &ent.kind else { return };
        let Some(b) = t.bounds() else { return };
        if !b.0.iter().chain(b.1.iter()).all(|v| v.is_finite()) {
            return;
        }
        let bins = self
            .bins
            .get_or_insert_with(|| Bins::with_geometry(geometry()));
        if bins.insert(id.0, &b) {
            self.boxes.insert(id.0, b);
        }
    }

    pub(crate) fn remove(&mut self, id: EntityId) {
        if let Some(bins) = &mut self.bins {
            bins.remove(id.0);
        }
        self.boxes.remove(&id.0);
    }

    /// Looks each toucher's box up in the bins, once a tick.
    pub(crate) fn prepare(&mut self, touchers: &[crate::bus::Toucher]) {
        self.touched.clear();
        let Some(bins) = &self.bins else { return };
        let geom = Boxes(&self.boxes);
        for t in touchers {
            if !t.min.iter().chain(t.max.iter()).all(|v| v.is_finite()) {
                continue;
            }
            let q = [t.min[0], t.min[1], t.min[2], t.max[0], t.max[1], t.max[2]];
            let touched = &mut self.touched;
            bins.walk(q, 1, &geom, &mut |o| touched.entry(o).or_default().push(*t));
        }
    }

    /// The touchers whose boxes meet the trigger `id`'s box this tick.
    pub(crate) fn touchers_of(&self, id: EntityId) -> Vec<crate::bus::Toucher> {
        self.touched.get(&id.0).cloned().unwrap_or_default()
    }
}
