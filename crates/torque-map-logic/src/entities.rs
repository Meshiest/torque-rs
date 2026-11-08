//! The supported classes and how each is made from its keyvalues ([`make`]); each family's behavior is in its own module.
//!
//! | Class | Module |
//! |---|---|
//! | `logic_relay`, `logic_auto`, `logic_timer`, `logic_case`, `logic_compare`, `logic_branch`, `math_counter` | [`crate::logic`] |
//! | `trigger_once`, `trigger_multiple`, `filter_activator_name`, `filter_activator_class`, `filter_multi` | [`crate::trigger`] |
//! | `func_door`, `func_door_rotating`, `func_movelinear`, `func_button`, `func_brush` | [`crate::mover`] |
//! | `ambient_generic` | [`crate::sound`] |
//!
//! Point entities that only mark a place (`info_target`, the spawn points, ...) and the players are known and have no
//! behavior. Any other class keeps its name, keys and outputs and takes the generic inputs (`Kill`, `AddOutput`,
//! `FireUser1-4`, `CancelPending`); its own inputs are counted as unhandled.

use crate::bus::{Ctx, Ent, EntityId, MapLogic};
use crate::defaults::Defaults;
use crate::logic::{Auto, Branch, Case, Compare, Counter, Relay, Timer};
use crate::mover::Mover;
use crate::sound::Ambient;
use crate::trigger::{Filter, Trigger};
use crate::value::{atof, atof32, atoi, boolean};
use crate::volume::Volume;

/// An entity's behavior.
#[derive(Debug, Clone)]
pub enum Kind {
    Relay(Relay),
    Auto(Auto),
    Timer(Timer),
    Case(Case),
    Compare(Compare),
    Branch(Branch),
    Counter(Counter),
    Trigger(Trigger),
    Filter(Filter),
    Mover(Box<Mover>),
    Ambient(Ambient),
    /// A marker or a player: known, nothing to do.
    Passive,
    /// A class with no handler here.
    Plain,
    /// Taken out of its slot while it runs.
    Busy,
}

/// Classes that are known and do nothing.
const PASSIVE: &[&str] = &[
    "player",
    "info_target",
    "info_null",
    "info_landmark",
    "info_teleport_destination",
    "info_player_start",
    "info_player_deathmatch",
    "info_player_combine",
    "info_player_rebel",
    "info_player_terrorist",
    "info_player_counterterrorist",
    "info_node",
    "info_node_hint",
    "path_corner",
    "path_track",
    "worldspawn",
];

/// Reads an entity's keys with the defaults beneath them.
pub(crate) struct Keys<'a> {
    pub ent: &'a Ent,
    pub defaults: &'a mut Defaults,
}

impl Keys<'_> {
    pub fn str(&mut self, key: &str) -> String {
        match self.ent.key(key) {
            Some(v) => v.to_string(),
            None => self.defaults.key(&self.ent.class, key).unwrap_or_default(),
        }
    }
    pub fn f32(&mut self, key: &str) -> f32 {
        atof32(&self.str(key))
    }
    pub fn f64(&mut self, key: &str) -> f64 {
        atof(&self.str(key))
    }
    pub fn int(&mut self, key: &str) -> i64 {
        atoi(&self.str(key))
    }
    pub fn bool(&mut self, key: &str) -> bool {
        boolean(&self.str(key))
    }
}

/// The behavior of `ent` (its class, keys and spawnflags read), with its brush model's volumes.
pub(crate) fn make(ent: &Ent, volumes: &[Volume], defaults: &mut Defaults) -> Kind {
    let mut k = Keys { ent, defaults };
    match ent.class.as_str() {
        "logic_relay" => Kind::Relay(Relay::new(&mut k)),
        "logic_auto" => Kind::Auto(Auto::default()),
        "logic_timer" => Kind::Timer(Timer::new(&mut k)),
        "logic_case" => Kind::Case(Case::new(&mut k)),
        "logic_compare" => Kind::Compare(Compare::new(&mut k)),
        "logic_branch" => Kind::Branch(Branch::new(&mut k)),
        "math_counter" => Kind::Counter(Counter::new(&mut k)),
        "trigger_once" | "trigger_multiple" => Kind::Trigger(Trigger::new(&mut k, volumes)),
        "filter_activator_name" | "filter_activator_class" | "filter_multi" => {
            Kind::Filter(Filter::new(&mut k))
        }
        "func_door" | "func_door_rotating" | "func_movelinear" | "func_button" | "func_brush" => {
            Kind::Mover(Box::new(Mover::new(&mut k, volumes)))
        }
        "ambient_generic" => Kind::Ambient(Ambient::new(&mut k)),
        c if PASSIVE.contains(&c) => Kind::Passive,
        _ => Kind::Plain,
    }
}

impl Kind {
    /// Runs once, on the first tick.
    pub(crate) fn spawn(&mut self, ml: &mut MapLogic, me: EntityId) {
        match self {
            Kind::Relay(r) => r.spawn(ml, me),
            Kind::Auto(a) => a.spawn(ml, me),
            Kind::Timer(t) => t.spawn(ml, me),
            Kind::Counter(c) => c.spawn(ml, me),
            Kind::Mover(m) => m.spawn(ml, me),
            Kind::Ambient(a) => a.spawn(ml, me),
            _ => {}
        }
    }

    /// Runs every tick, after the first drain.
    pub(crate) fn think(&mut self, ml: &mut MapLogic, me: EntityId) {
        match self {
            Kind::Timer(t) => t.think(ml, me),
            Kind::Trigger(t) => t.think(ml, me),
            Kind::Mover(m) => m.think(ml, me),
            _ => {}
        }
    }

    /// An input (lowercase); false when the class does not take it.
    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str, value: &str) -> bool {
        match self {
            Kind::Relay(r) => r.input(ml, ctx, input),
            Kind::Auto(_) => false,
            Kind::Timer(t) => t.input(ml, ctx, input, value),
            Kind::Case(c) => c.input(ml, ctx, input, value),
            Kind::Compare(c) => c.input(ml, ctx, input, value),
            Kind::Branch(b) => b.input(ml, ctx, input, value),
            Kind::Counter(c) => c.input(ml, ctx, input, value),
            Kind::Trigger(t) => t.input(ml, ctx, input),
            Kind::Filter(f) => f.input(ml, ctx, input),
            Kind::Mover(m) => m.input(ml, ctx, input, value),
            Kind::Ambient(a) => a.input(ml, ctx, input, value),
            Kind::Passive | Kind::Plain | Kind::Busy => false,
        }
    }

    pub fn as_mover(&self) -> Option<&Mover> {
        match self {
            Kind::Mover(m) => Some(m),
            _ => None,
        }
    }
}
