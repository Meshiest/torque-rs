//! The event bus: a map's entities, their outputs, target names and the queue that delivers inputs ([`MapLogic`]).
//!
//! # Names
//!
//! A connection's target is looked up when its event is delivered, not when it is posted, so entities spawned or removed in
//! between are seen. Names compare without case. In order:
//!
//! - `!self`: the entity whose output posted the event; `!caller`: the caller the output carried (by default the entity that
//!   delivered the input that made the entity fire, so a trigger is the `!caller` of the outputs of a relay it triggered;
//!   `math_counter`, `logic_case`, the filters and every output fired by an entity on its own, such as a touch or a timer, carry
//!   the entity itself); `!activator`: the entity that began the chain (the player who touched the trigger); `!player` and
//!   `!pvsplayer`: the first player added. Any other `!` name finds nothing (counted as a missing target).
//! - A name ending in `*` matches every name that starts with what comes before the stars; only trailing stars are wildcards.
//! - Otherwise the entities of that name; when no entity has the name, the entities of that class (wildcards too).
//!
//! The matches are taken in spawn order, at most [`Limits::max_targets`].
//!
//! # Time and order
//!
//! Time is the server's tick count, 32 ms a tick ([`TICK_US`]), kept in microseconds. An event posted with a delay `d` is due
//! at `now + d` and runs in the first tick whose time has reached it, so a delay under a tick waits for the next one, and two
//! events due in one tick run in the order of their exact times. The queue is a heap on `(due time, sequence)`, the sequence
//! rising with every post and the connections of one output posting in the map's order: events due at the same time run in
//! the order they were posted (a stable choice where the format promises no order).
//!
//! **A tick** ([`MapLogic::tick`]): the time moves on one step; the spawn outputs fire on the first tick; the queue is drained
//! of everything due; the entities think in spawn order (timers fire, movers move, triggers take their touches); the queue is
//! drained again. A zero delay is due at once and so runs in the same drain, after the event that posted it (breadth first):
//! a chain of zero-delay outputs completes within its tick.
//!
//! # Limits
//!
//! A map cannot hang the server ([`Limits`]): an event posted at the current time inherits its poster's chain depth plus one
//! and is dropped past [`Limits::max_chain_depth`] (a relay that triggers itself with no delay ends there); a later event
//! starts a new chain at depth 0. At most [`Limits::max_queue`] events wait (a post is dropped when only one place is left,
//! which is kept for the rest of an event cut short) and at most
//! [`Limits::max_events_per_tick`] run in a tick (the rest stay due and run in the next tick, counted in
//! [`Stats::deferred`]). One work counter covers a tick ([`Limits::max_work_per_tick`]): deliveries, connections posted,
//! output lists scanned and the volume tests of movers and triggers. When it is spent the due events wait for the next tick
//! (an event cut short among its targets keeps the rest for then), movers and triggers skip the tick, and past
//! [`HARD_WORK_FACTOR`] times the budget posts are dropped. The connections over all entities are capped
//! ([`Limits::max_total_outputs`]), checked before anything is copied. Each drop is counted and logged once per kind.
//!
//! # Removal and canceling
//!
//! `Kill` (and a `trigger_once` or `logic_auto` that removes itself) removes the entity: it gets no more inputs and no target
//! finds it. Events it already posted still run (a `logic_auto` posts its delayed outputs and removes itself at once, and
//! those outputs must still fire). `CancelPending` drops the events the entity posted that have not run yet.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};

use torque_mapcore::scene::Entity;

use crate::connection::{Connection, parse_add_output};
use crate::defaults::Defaults;
use crate::entities::{self, Kind};
use crate::sound::SoundEvent;
use crate::volume::Volume;

/// One tick, in microseconds (the server's 32 ms step).
pub const TICK_US: u64 = 32_000;

/// An entity: its index in spawn order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId(pub u32);

/// The caps that keep a map from hanging the server (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most events waiting. A post is refused once `max_queue - 1` wait: the last place is kept for the rest of an event
    /// the work budget cut short among its targets, so the queue never holds more than `max_queue`.
    pub max_queue: usize,
    /// The most events delivered in one tick (the rest wait for the next).
    pub max_events_per_tick: usize,
    /// The deepest chain of zero-delay outputs (a post past it is dropped).
    pub max_chain_depth: u32,
    /// The most entities one event reaches (and [`MapLogic::find`] returns).
    pub max_targets: usize,
    /// The most connections one entity holds.
    pub max_outputs_per_entity: usize,
    /// The most entities a map spawns.
    pub max_entities: usize,
    /// The most lines [`MapLogic::log`] keeps.
    pub max_log: usize,
    /// The most sound events and fired outputs kept between two takes.
    pub max_pending_reports: usize,
    /// The work one tick may do ([`MapLogic::work_used`]): each delivery, each connection posted, each eighth of an output list
    /// scanned, each eighth of a target list matched and each volume test a mover or a trigger makes is one unit. Past it the
    /// due events wait for the next tick and movers and triggers skip the tick.
    pub max_work_per_tick: u64,
    /// The most connections over all entities (an `AddOutput` past it is dropped).
    pub max_total_outputs: usize,
}

/// How many times [`Limits::max_work_per_tick`] a tick may reach before posts are dropped rather than queued.
pub const HARD_WORK_FACTOR: u64 = 4;

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_queue: 65_536,
            max_events_per_tick: 8_192,
            max_chain_depth: 64,
            max_targets: 4_096,
            max_outputs_per_entity: 1_024,
            max_entities: 1 << 17,
            max_log: 512,
            max_pending_reports: 4_096,
            max_work_per_tick: 65_536,
            max_total_outputs: 262_144,
        }
    }
}

/// What the bus has done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Events delivered (each event once, whatever it matched).
    pub events: u64,
    /// Inputs an entity took (an event delivered to three entities is three).
    pub inputs: u64,
    pub outputs_fired: u64,
    pub missing_targets: u64,
    pub unhandled_inputs: u64,
    pub dropped_queue: u64,
    pub dropped_depth: u64,
    /// Due events left for the next tick by the per-tick budget.
    pub deferred: u64,
    pub canceled: u64,
    pub max_queue_seen: usize,
    pub max_depth_seen: u32,
    /// The most work one tick did.
    pub max_work_seen: u64,
    /// Posts dropped because a tick ran past the hard work limit.
    pub dropped_work: u64,
    /// `AddOutput`s and spawn outputs dropped by the caps on outputs.
    pub dropped_outputs: u64,
}

/// How a map starts: which of `logic_auto`'s outputs fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Start {
    /// `OnNewGame` (a fresh start, not a saved game).
    pub new_game: bool,
    /// `OnMultiNewMap` (a multiplayer game).
    pub multiplayer: bool,
}

impl Default for Start {
    fn default() -> Start {
        Start {
            new_game: true,
            multiplayer: true,
        }
    }
}

/// What a toucher is to a trigger's spawnflags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToucherKind {
    Player,
    Npc,
    Physics,
}

/// Something that can touch triggers and block movers this tick: an entity of the bus (a player [`MapLogic::add_player`])
/// and its box in the map's space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Toucher {
    pub id: EntityId,
    pub kind: ToucherKind,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// An output that fired (for a game's script hook): the entity, the output, the activator.
#[derive(Debug, Clone, PartialEq)]
pub struct Fired {
    pub entity: EntityId,
    pub output: String,
    pub activator: Option<EntityId>,
}

/// An input as it was delivered (the bus's trace, [`MapLogic::trace_inputs`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Delivered {
    pub tick: u64,
    pub target: EntityId,
    /// As the connection spelled it.
    pub input: String,
    pub value: String,
    pub activator: Option<EntityId>,
    pub caller: Option<EntityId>,
}

/// One entity's record.
#[derive(Debug, Clone)]
pub struct Ent {
    /// Lowercase.
    pub class: String,
    /// Lowercase; empty when it has none.
    pub name: String,
    pub keys: Vec<(String, String)>,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    /// Its brush model (`*N`).
    pub model: Option<u32>,
    pub spawnflags: u32,
    /// Output name and connection, in the map's order.
    pub outputs: Vec<(String, Connection)>,
    pub alive: bool,
    epoch: u32,
    pub(crate) kind: Kind,
}

impl Ent {
    /// The value of `key` (any case), the first if it repeats.
    pub fn key(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// Whether every bit of `flag` is set.
    pub fn flag(&self, flag: u32) -> bool {
        self.spawnflags & flag == flag
    }

    /// Whether its class has a handler here (an unsupported class keeps its name and outputs and takes the generic inputs).
    pub fn supported(&self) -> bool {
        !matches!(self.kind, Kind::Plain | Kind::Busy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Name(String),
    Entity(EntityId),
    /// The targets of an event the work budget cut short: delivered in the next tick.
    Entities(Vec<EntityId>),
}

#[derive(Debug, Clone)]
struct Event {
    due: u64,
    seq: u64,
    depth: u32,
    target: Target,
    input: String,
    value: String,
    activator: Option<EntityId>,
    caller: Option<EntityId>,
    sender: Option<EntityId>,
    epoch: u32,
}

impl PartialEq for Event {
    fn eq(&self, o: &Event) -> bool {
        self.due == o.due && self.seq == o.seq
    }
}
impl Eq for Event {}
impl PartialOrd for Event {
    fn partial_cmp(&self, o: &Event) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Event {
    /// Reversed, so the heap's top is the earliest `(due, seq)`.
    fn cmp(&self, o: &Event) -> Ordering {
        (o.due, o.seq).cmp(&(self.due, self.seq))
    }
}

/// Who an input came from: the entity it is for, the chain's activator and the input's caller (the entity whose output
/// delivered it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ctx {
    pub me: EntityId,
    pub activator: Option<EntityId>,
    pub caller: Option<EntityId>,
}

/// A seeded generator for the random choices (`PickRandom`, random timer intervals): the same map gives the same run.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// A map's logic (module docs).
pub struct MapLogic {
    ents: Vec<Ent>,
    /// Each name's and each class's living entities, in spawn order (a removal is one set removal, not a scan of the group).
    names: BTreeMap<String, BTreeSet<EntityId>>,
    classes: BTreeMap<String, BTreeSet<EntityId>>,
    queue: BinaryHeap<Event>,
    seq: u64,
    tick: u64,
    now_us: u64,
    started: bool,
    depth: u32,
    budget_used: usize,
    work_used: u64,
    total_outputs: usize,
    start: Start,
    pub limits: Limits,
    pub stats: Stats,
    log: Vec<String>,
    logged: BTreeSet<String>,
    rng: Rng,
    players: Vec<EntityId>,
    pub(crate) touchers: Vec<Toucher>,
    pub(crate) sounds: Vec<SoundEvent>,
    fired: Vec<Fired>,
    observe: bool,
    trace: Option<Vec<Delivered>>,
    pub(crate) triggers: crate::trigger::TriggerIndex,
}

impl std::fmt::Debug for MapLogic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapLogic")
            .field("entities", &self.ents.len())
            .field("tick", &self.tick)
            .field("queued", &self.queue.len())
            .field("stats", &self.stats)
            .finish()
    }
}

/// Takes `id` out of `key`'s group, and the group out of the index once it is empty (so a wildcard walk meets no empty
/// groups).
fn unindex(map: &mut BTreeMap<String, BTreeSet<EntityId>>, key: &str, id: EntityId) {
    if let Some(v) = map.get_mut(key) {
        v.remove(&id);
        if v.is_empty() {
            map.remove(key);
        }
    }
}

/// A map's brush models as volumes, in each model's own space (the entity's origin adds to it).
pub type BrushVolumes = HashMap<u32, Vec<Volume>>;

impl MapLogic {
    /// The logic of `entities` (the map's, in its order), their brush models' volumes and the defaults, with the default
    /// caps.
    pub fn new(entities: &[Entity], volumes: &BrushVolumes, defaults: &mut Defaults) -> MapLogic {
        MapLogic::with_limits(entities, volumes, defaults, Limits::default())
    }

    /// As [`MapLogic::new`] with caps of its own.
    pub fn with_limits(
        entities: &[Entity],
        volumes: &BrushVolumes,
        defaults: &mut Defaults,
        limits: Limits,
    ) -> MapLogic {
        let mut ml = MapLogic {
            ents: Vec::new(),
            names: BTreeMap::new(),
            classes: BTreeMap::new(),
            queue: BinaryHeap::new(),
            seq: 0,
            tick: 0,
            now_us: 0,
            started: false,
            depth: 0,
            budget_used: 0,
            work_used: 0,
            total_outputs: 0,
            start: Start::default(),
            limits,
            stats: Stats::default(),
            log: Vec::new(),
            logged: BTreeSet::new(),
            rng: Rng(0x5EED_0FA1_17E5_7000),
            players: Vec::new(),
            touchers: Vec::new(),
            sounds: Vec::new(),
            fired: Vec::new(),
            observe: false,
            trace: None,
            triggers: crate::trigger::TriggerIndex::default(),
        };
        for e in entities {
            if ml.ents.len() >= ml.limits.max_entities {
                ml.note(
                    "entities",
                    format!(
                        "more than {} entities; the rest are left out",
                        ml.limits.max_entities
                    ),
                );
                break;
            }
            ml.spawn_entity(e, volumes, defaults);
        }
        ml
    }

    fn spawn_entity(
        &mut self,
        e: &Entity,
        volumes: &BrushVolumes,
        defaults: &mut Defaults,
    ) -> EntityId {
        let id = EntityId(self.ents.len() as u32);
        let class = e.class.trim().to_ascii_lowercase();
        let get = |k: &str| {
            e.keys
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.as_str())
        };
        let name = get("targetname")
            .map(|n| n.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let spawnflags = match get("spawnflags") {
            Some(f) => crate::value::atoi(f).clamp(0, i64::from(u32::MAX)) as u32,
            None => defaults.spawnflags(&class),
        };
        let angles = get("angles")
            .and_then(crate::value::vec3)
            .unwrap_or([0.0; 3]);
        let model = match &e.mover {
            Some(torque_mapcore::scene::Mover::BrushModel(n)) => Some(*n),
            _ => None,
        };
        let mut outputs: Vec<(String, Connection)> = e
            .outputs
            .iter()
            .map(|o| (o.output.clone(), Connection::from_output(o)))
            .collect();
        let room = self
            .limits
            .max_total_outputs
            .saturating_sub(self.total_outputs)
            .min(self.limits.max_outputs_per_entity);
        if outputs.len() > room {
            self.stats.dropped_outputs += (outputs.len() - room) as u64;
            outputs.truncate(room);
            self.note(
                "outputs",
                "an entity has more outputs than the caps allow; the rest are left out".into(),
            );
        }
        self.total_outputs += outputs.len();
        if outputs.len() > self.limits.max_outputs_per_entity {
            outputs.truncate(self.limits.max_outputs_per_entity);
            self.note(
                "outputs",
                "an entity has more outputs than the cap; the rest are left out".into(),
            );
        }
        let mut ent = Ent {
            class: class.clone(),
            name: name.clone(),
            keys: e.keys.clone(),
            origin: e.origin,
            angles,
            model,
            spawnflags,
            outputs,
            alive: true,
            epoch: 0,
            kind: Kind::Plain,
        };
        let empty = Vec::new();
        let vols = model.and_then(|m| volumes.get(&m)).unwrap_or(&empty);
        ent.kind = entities::make(&ent, vols, defaults);
        if matches!(ent.kind, Kind::Trigger(_)) {
            self.triggers.insert(id, &ent);
        }
        self.ents.push(ent);
        if !name.is_empty() {
            self.names.entry(name).or_default().insert(id);
        }
        self.classes.entry(class).or_default().insert(id);
        id
    }

    /// Adds a player: an entity of class `player` (named `name` when not empty) that touches triggers and blocks movers when
    /// its box is given to [`MapLogic::tick`]. The first added is `!player`.
    pub fn add_player(&mut self, name: &str) -> EntityId {
        let mut keys = vec![("classname".to_string(), "player".to_string())];
        if !name.is_empty() {
            keys.push(("targetname".to_string(), name.to_string()));
        }
        let e = Entity {
            class: "player".into(),
            keys,
            origin: [0.0; 3],
            mover: None,
            outputs: Vec::new(),
        };
        let id = self.spawn_entity(&e, &BrushVolumes::new(), &mut Defaults::coded());
        self.players.push(id);
        id
    }

    /// Which of `logic_auto`'s outputs the first tick fires.
    pub fn set_start(&mut self, start: Start) {
        self.start = start;
    }

    pub(crate) fn start_kind(&self) -> Start {
        self.start
    }

    /// Keeps every fired output for [`MapLogic::take_fired`] (a game's script hook); off by default.
    pub fn observe_outputs(&mut self, on: bool) {
        self.observe = on;
    }

    // --- Reading the state.

    /// The entity `id`.
    pub fn entity(&self, id: EntityId) -> Option<&Ent> {
        self.ents.get(id.0 as usize)
    }

    /// Every entity, in spawn order (removed ones too: [`Ent::alive`]).
    pub fn entities(&self) -> impl Iterator<Item = (EntityId, &Ent)> {
        self.ents
            .iter()
            .enumerate()
            .map(|(i, e)| (EntityId(i as u32), e))
    }

    pub(crate) fn ent_mut(&mut self, id: EntityId) -> Option<&mut Ent> {
        self.ents.get_mut(id.0 as usize)
    }

    /// The living entities named `name` (any case; no wildcards), in spawn order, at most [`Limits::max_targets`].
    pub fn find(&self, name: &str) -> Vec<EntityId> {
        self.names
            .get(&name.to_ascii_lowercase())
            .map(|v| {
                v.iter()
                    .copied()
                    .filter(|&i| self.alive(i))
                    .take(self.limits.max_targets)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn alive(&self, id: EntityId) -> bool {
        self.entity(id).is_some_and(|e| e.alive)
    }

    /// The tick count (0 before the first tick).
    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    /// The time in microseconds.
    pub fn now_us(&self) -> u64 {
        self.now_us
    }

    /// The time in seconds.
    pub fn now(&self) -> f64 {
        self.now_us as f64 / 1e6
    }

    /// The events waiting.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The log: one line for each kind of drop, missing target, unhandled input and spawn note, each once.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// The sound events since the last take ([`crate::sound`]).
    pub fn take_sounds(&mut self) -> Vec<SoundEvent> {
        std::mem::take(&mut self.sounds)
    }

    /// Keeps every input delivered from now on for [`MapLogic::take_inputs`] (a trace, as a console's I/O report shows it); off by
    /// default.
    pub fn trace_inputs(&mut self, on: bool) {
        self.trace = on.then(Vec::new);
    }

    /// The inputs delivered since the last take, while tracing.
    pub fn take_inputs(&mut self) -> Vec<Delivered> {
        self.trace.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// The outputs fired since the last take, when observed ([`MapLogic::observe_outputs`]).
    pub fn take_fired(&mut self) -> Vec<Fired> {
        std::mem::take(&mut self.fired)
    }

    /// The first player, if any is alive.
    pub fn first_player(&self) -> Option<EntityId> {
        self.players.iter().copied().find(|&p| self.alive(p))
    }

    pub(crate) fn random(&mut self) -> u64 {
        self.rng.next()
    }

    /// Logs `line` once under `key`.
    pub(crate) fn note(&mut self, key: &str, line: String) {
        if self.log.len() < self.limits.max_log && self.logged.insert(key.to_string()) {
            self.log.push(line);
        }
    }

    // --- Posting.

    /// Posts `input` to `target` after `delay` seconds, as if `sender`'s output carried it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn post(
        &mut self,
        target: Target,
        input: &str,
        value: &str,
        delay: f32,
        activator: Option<EntityId>,
        caller: Option<EntityId>,
        sender: Option<EntityId>,
    ) -> bool {
        let delay_us = if delay.is_finite() && delay > 0.0 {
            (f64::from(delay) * 1e6).round() as u64
        } else {
            0
        };
        let depth = if delay_us == 0 { self.depth + 1 } else { 0 };
        if depth > self.limits.max_chain_depth {
            self.stats.dropped_depth += 1;
            self.note(
                "depth",
                format!(
                    "a chain of zero-delay outputs ran deeper than {}; its next event ({input}) was dropped",
                    self.limits.max_chain_depth
                ),
            );
            return false;
        }
        // One place stays free for the rest of an event the work budget cut short.
        if self.queue.len() + 1 >= self.limits.max_queue {
            self.stats.dropped_queue += 1;
            self.note(
                "queue",
                format!(
                    "the queue held {} events; a post was dropped",
                    self.limits.max_queue
                ),
            );
            return false;
        }
        let epoch = sender.and_then(|s| self.entity(s)).map_or(0, |e| e.epoch);
        self.seq += 1;
        self.queue.push(Event {
            due: self.now_us.saturating_add(delay_us),
            seq: self.seq,
            depth,
            target,
            input: input.to_string(),
            value: value.to_string(),
            activator,
            caller,
            sender,
            epoch,
        });
        self.stats.max_queue_seen = self.stats.max_queue_seen.max(self.queue.len());
        self.stats.max_depth_seen = self.stats.max_depth_seen.max(depth);
        true
    }

    /// Fires `id`'s output `output` with `value`: each of its connections posts an event, in the map's order; a connection
    /// with a count left fires that many more times, then is removed. The number of events posted.
    pub fn fire(
        &mut self,
        id: EntityId,
        output: &str,
        value: &str,
        activator: Option<EntityId>,
        caller: Option<EntityId>,
    ) -> usize {
        let Some(e) = self.ents.get(id.0 as usize) else {
            return 0;
        };
        let scan = 1 + e.outputs.len() as u64 / 8;
        let matching: Vec<usize> = e
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| name.eq_ignore_ascii_case(output))
            .map(|(i, _)| i)
            .collect();
        self.spend(scan);
        self.stats.outputs_fired += 1;
        if self.observe && self.fired.len() < self.limits.max_pending_reports {
            self.fired.push(Fired {
                entity: id,
                output: output.to_string(),
                activator,
            });
        }
        let mut n = 0;
        let mut spent = Vec::new();
        for i in matching {
            let c = &mut self.ents[id.0 as usize].outputs[i].1;
            if let Some(t) = &mut c.times {
                *t = t.saturating_sub(1);
                if *t == 0 {
                    spent.push(i);
                }
            }
            let (delay, empty) = (c.delay, c.target.trim().is_empty());
            if empty || !self.can_post(delay) {
                continue;
            }
            // Cloned only once the post is known to go ahead.
            let c = &self.ents[id.0 as usize].outputs[i].1;
            let target = Target::Name(c.target.clone());
            let input = c.input.clone();
            let v = if c.parameter.is_empty() {
                value.to_string()
            } else {
                c.parameter.clone()
            };
            self.spend(1);
            if self.post(target, &input, &v, delay, activator, caller, Some(id)) {
                n += 1;
            }
        }
        let outs = &mut self.ents[id.0 as usize].outputs;
        for &i in spent.iter().rev() {
            outs.remove(i);
        }
        self.total_outputs = self.total_outputs.saturating_sub(spent.len());
        n
    }

    /// Whether a post with `delay` would be taken now (the depth, the queue and the hard work limit), counting and logging
    /// the drop when not.
    fn can_post(&mut self, delay: f32) -> bool {
        let zero = !(delay.is_finite() && delay > 0.0);
        if zero && self.depth + 1 > self.limits.max_chain_depth {
            self.stats.dropped_depth += 1;
            self.note(
                "depth",
                format!(
                    "a chain of zero-delay outputs ran deeper than {}; its next event was dropped",
                    self.limits.max_chain_depth
                ),
            );
            return false;
        }
        // One place stays free for the rest of an event the work budget cut short.
        if self.queue.len() + 1 >= self.limits.max_queue {
            self.stats.dropped_queue += 1;
            self.note(
                "queue",
                format!(
                    "the queue held {} events; a post was dropped",
                    self.limits.max_queue
                ),
            );
            return false;
        }
        if self.work_used
            > self
                .limits
                .max_work_per_tick
                .saturating_mul(HARD_WORK_FACTOR)
        {
            self.stats.dropped_work += 1;
            self.note(
                "work",
                format!(
                    "a tick did more than {HARD_WORK_FACTOR} times its work budget; later posts in it were dropped"
                ),
            );
            return false;
        }
        true
    }

    /// Charges `units` of work to this tick; false once the tick's budget is spent.
    pub(crate) fn spend(&mut self, units: u64) -> bool {
        self.work_used = self.work_used.saturating_add(units);
        self.stats.max_work_seen = self.stats.max_work_seen.max(self.work_used);
        self.work_used <= self.limits.max_work_per_tick
    }

    /// Whether this tick has work left.
    pub(crate) fn has_work(&self) -> bool {
        self.work_used < self.limits.max_work_per_tick
    }

    /// The work this tick has done so far.
    pub fn work_used(&self) -> u64 {
        self.work_used
    }

    /// The connections over all entities.
    pub fn total_outputs(&self) -> usize {
        self.total_outputs
    }

    /// Fires `input` at `target` (a name as a connection names it) after `delay` seconds, from no entity: a host's or a script's
    /// own input (`ent_fire`). False when the queue refused it.
    pub fn fire_input(
        &mut self,
        target: &str,
        input: &str,
        value: &str,
        delay: f32,
        activator: Option<EntityId>,
    ) -> bool {
        let saved = self.depth;
        self.depth = 0;
        let ok = self.post(
            Target::Name(target.to_string()),
            input,
            value,
            delay,
            activator,
            None,
            None,
        );
        self.depth = saved;
        ok
    }

    /// Posts `input` to `id` itself after `delay` (an entity's own timing, such as a relay's re-arm).
    pub(crate) fn post_self(&mut self, id: EntityId, input: &str, value: &str, delay: f32) {
        self.post(
            Target::Entity(id),
            input,
            value,
            delay,
            Some(id),
            Some(id),
            Some(id),
        );
    }

    /// Removes `id`: no target finds it and it takes no more inputs (module docs: its posted events still run).
    pub fn remove(&mut self, id: EntityId) {
        let Some(e) = self.ents.get_mut(id.0 as usize) else {
            return;
        };
        if !e.alive {
            return;
        }
        e.alive = false;
        let freed = std::mem::take(&mut e.outputs).len();
        let (name, class) = (e.name.clone(), e.class.clone());
        self.total_outputs = self.total_outputs.saturating_sub(freed);
        unindex(&mut self.names, &name, id);
        unindex(&mut self.classes, &class, id);
        self.triggers.remove(id);
    }

    /// Drops the events `id` posted that have not run.
    pub fn cancel_pending(&mut self, id: EntityId) {
        if let Some(e) = self.ents.get_mut(id.0 as usize) {
            e.epoch = e.epoch.wrapping_add(1);
        }
    }

    // --- Delivering.

    fn resolve(&self, ev: &Event) -> Vec<EntityId> {
        let name = match &ev.target {
            Target::Entity(id) => {
                return if self.alive(*id) {
                    vec![*id]
                } else {
                    Vec::new()
                };
            }
            Target::Entities(v) => {
                return v.iter().copied().filter(|&i| self.alive(i)).collect();
            }
            Target::Name(n) => n.trim().to_ascii_lowercase(),
        };
        let one = |id: Option<EntityId>| -> Vec<EntityId> {
            id.filter(|&i| self.alive(i)).into_iter().collect()
        };
        match name.as_str() {
            "!self" => return one(ev.sender),
            "!caller" => return one(ev.caller),
            "!activator" => return one(ev.activator),
            "!player" | "!pvsplayer" => return one(self.first_player()),
            _ if name.starts_with('!') || name.is_empty() => return Vec::new(),
            _ => {}
        }
        let by = |map: &BTreeMap<String, BTreeSet<EntityId>>| -> Vec<EntityId> {
            let mut out: Vec<EntityId> = match name.strip_suffix('*') {
                Some(_) => {
                    let prefix = name.trim_end_matches('*');
                    map.range(prefix.to_string()..)
                        .take_while(|(k, _)| k.starts_with(prefix))
                        .flat_map(|(_, v)| v.iter().copied())
                        .take(self.limits.max_targets.saturating_mul(2))
                        .collect()
                }
                None => map
                    .get(&name)
                    .map(|v| v.iter().copied().take(self.limits.max_targets).collect())
                    .unwrap_or_default(),
            };
            out.retain(|&i| self.alive(i));
            out.sort_unstable();
            out.dedup();
            out.truncate(self.limits.max_targets);
            out
        };
        let named = by(&self.names);
        if !named.is_empty() {
            return named;
        }
        by(&self.classes)
    }

    fn dispatch(&mut self, ev: Event) {
        if let Some(s) = ev.sender
            && self.entity(s).is_some_and(|e| e.epoch != ev.epoch)
        {
            self.stats.canceled += 1;
            return;
        }
        if !matches!(ev.target, Target::Entities(_)) {
            self.stats.events += 1;
        }
        let targets = self.resolve(&ev);
        if targets.is_empty() {
            self.stats.missing_targets += 1;
            if let Target::Name(n) = &ev.target {
                let n = n.to_ascii_lowercase();
                self.note(
                    &format!("missing {n}"),
                    format!("no entity matches the target {n:?} (input {})", ev.input),
                );
            }
            return;
        }
        self.spend(targets.len() as u64 / 8);
        let saved = self.depth;
        self.depth = ev.depth;
        for (k, &t) in targets.iter().enumerate() {
            if !self.has_work() {
                // The rest of the targets wait for the next tick, first in the queue as this event was.
                self.stats.deferred += 1;
                self.queue.push(Event {
                    target: Target::Entities(targets[k..].to_vec()),
                    ..ev
                });
                break;
            }
            self.spend(1);
            let ctx = Ctx {
                me: t,
                activator: ev.activator,
                caller: ev.sender,
            };
            self.deliver(ctx, &ev.input, &ev.value);
        }
        self.depth = saved;
    }

    /// Delivers `input` to `ctx.me` now: the generic inputs, then its class's.
    pub(crate) fn deliver(&mut self, ctx: Ctx, input: &str, value: &str) {
        let i = ctx.me.0 as usize;
        if !self.ents.get(i).is_some_and(|e| e.alive) {
            return;
        }
        self.stats.inputs += 1;
        if let Some(t) = &mut self.trace
            && t.len() < self.limits.max_pending_reports
        {
            t.push(Delivered {
                tick: self.tick,
                target: ctx.me,
                input: input.to_string(),
                value: value.to_string(),
                activator: ctx.activator,
                caller: ctx.caller,
            });
        }
        let lower = input.trim().to_ascii_lowercase();
        match lower.as_str() {
            "kill" | "killhierarchy" => {
                self.remove(ctx.me);
                return;
            }
            "addoutput" => {
                self.add_output(ctx.me, value);
                return;
            }
            "fireuser1" | "fireuser2" | "fireuser3" | "fireuser4" => {
                let out = format!("OnUser{}", &lower[8..]);
                self.fire(ctx.me, &out, value, ctx.activator, ctx.caller);
                return;
            }
            "cancelpending" => {
                self.cancel_pending(ctx.me);
                // A relay's own handler also clears its wait.
            }
            _ => {}
        }
        let mut kind = std::mem::replace(&mut self.ents[i].kind, Kind::Busy);
        let handled = kind.input(self, ctx, &lower, value);
        if let Some(e) = self.ents.get_mut(i) {
            e.kind = kind;
        }
        if !handled && lower != "cancelpending" {
            self.stats.unhandled_inputs += 1;
            let class = self.ents[i].class.clone();
            self.note(
                &format!("unhandled {class}.{lower}"),
                format!("{class} does not take the input {input}"),
            );
        }
    }

    fn add_output(&mut self, id: EntityId, text: &str) {
        let cap = self.limits.max_outputs_per_entity;
        if let Some((name, c)) = parse_add_output(text) {
            let full = self.total_outputs >= self.limits.max_total_outputs;
            let Some(e) = self.ents.get_mut(id.0 as usize) else {
                return;
            };
            if e.outputs.len() >= cap || full {
                self.stats.dropped_outputs += 1;
                self.note(
                    "addoutput",
                    "AddOutput past the caps of outputs was dropped".into(),
                );
                return;
            }
            e.outputs.push((name, c));
            self.total_outputs += 1;
            return;
        }
        // `key value`: sets a keyvalue (a new name is found from then on).
        let text = text.trim();
        let (k, v) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        if k.is_empty() {
            return;
        }
        let v = v.trim().to_string();
        let Some(e) = self.ent_mut(id) else { return };
        match e.keys.iter().position(|(n, _)| n.eq_ignore_ascii_case(k)) {
            Some(i) => e.keys[i].1 = v.clone(),
            None if e.keys.len() < cap => e.keys.push((k.to_string(), v.clone())),
            None => return,
        }
        if k.eq_ignore_ascii_case("targetname") {
            let old = std::mem::replace(&mut e.name, v.to_ascii_lowercase());
            unindex(&mut self.names, &old, id);
            if !v.is_empty() {
                self.names
                    .entry(v.to_ascii_lowercase())
                    .or_default()
                    .insert(id);
            }
        }
    }

    // --- Running.

    /// Runs one tick (module docs) with `touchers` (players and others) where they are now.
    pub fn tick(&mut self, touchers: &[Toucher]) {
        self.tick += 1;
        self.now_us = self.tick.saturating_mul(TICK_US);
        self.budget_used = 0;
        self.work_used = 0;
        self.touchers = touchers.to_vec();
        self.triggers.prepare(touchers);
        if !self.started {
            self.started = true;
            for i in 0..self.ents.len() {
                self.with_kind(EntityId(i as u32), |k, ml, id| k.spawn(ml, id));
            }
        }
        self.drain();
        for i in 0..self.ents.len() {
            self.with_kind(EntityId(i as u32), |k, ml, id| k.think(ml, id));
        }
        self.drain();
    }

    /// Runs `ticks` ticks with no touchers.
    pub fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.tick(&[]);
        }
    }

    /// Calls `f` with `id`'s handler taken out of its slot (so it may change the rest of the bus).
    pub(crate) fn with_kind(
        &mut self,
        id: EntityId,
        f: impl FnOnce(&mut Kind, &mut MapLogic, EntityId),
    ) {
        let i = id.0 as usize;
        if !self.ents.get(i).is_some_and(|e| e.alive) {
            return;
        }
        let mut kind = std::mem::replace(&mut self.ents[i].kind, Kind::Busy);
        let saved = self.depth;
        self.depth = 0;
        f(&mut kind, self, id);
        self.depth = saved;
        self.ents[i].kind = kind;
    }

    fn drain(&mut self) {
        let mut deferred = false;
        while let Some(top) = self.queue.peek() {
            if top.due > self.now_us {
                break;
            }
            if self.budget_used >= self.limits.max_events_per_tick || !self.has_work() {
                deferred = true;
                break;
            }
            self.budget_used += 1;
            let Some(ev) = self.queue.pop() else { break };
            self.dispatch(ev);
        }
        if deferred {
            let due = self.queue.iter().filter(|e| e.due <= self.now_us).count();
            self.stats.deferred += due as u64;
            self.note(
                "budget",
                format!(
                    "more than {} events (or {} units of work) were due in one tick; the rest wait for the next",
                    self.limits.max_events_per_tick, self.limits.max_work_per_tick
                ),
            );
        }
    }

    /// Delivers `input` to `id` at once, as the player's use does (a button's press, a door's use): `activator` is the user.
    pub fn use_entity(&mut self, id: EntityId, activator: Option<EntityId>) {
        let ctx = Ctx {
            me: id,
            activator,
            caller: activator,
        };
        self.deliver(ctx, "Use", "");
        self.drain();
    }

    /// The handler of `id`, for reading its state.
    pub fn kind(&self, id: EntityId) -> Option<&Kind> {
        self.entity(id).map(|e| &e.kind)
    }

    /// Every class name in the map that has no handler here, with its count.
    pub fn unsupported_classes(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for e in &self.ents {
            if !e.supported() {
                *out.entry(e.class.clone()).or_insert(0) += 1;
            }
        }
        out
    }
}
