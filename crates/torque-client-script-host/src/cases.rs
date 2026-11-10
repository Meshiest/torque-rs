//! The cases every runner passes: the same list natively under Wasmtime (`tests/cases.rs`) and on the web under Node and
//! headless Chrome (tests/web/client-script-check, the justfile's `client-script-web-check`). A case is a list of [`Action`]s
//! over realms in numbered slots, run by a [`CaseRun`] that never blocks: while a web runner's entry is in flight,
//! [`CaseRun::step`] returns [`Step::Waiting`] and its caller waits for the runner before stepping again.
//!
//! Realm time is the run's own: every frame moves it 1/60 s on, so timers, rates and the strike rule's window are the
//! same on every runner. Deadlines and budgets are wall-clock time, as they are in a game.

pub mod parity;

use std::time::Duration;

use torque_client_script_format::{Grants, Records, Tier, Value, Writer, op};
use web_time::Instant;

use crate::{
    CallRoute, ClientMod, Effects, FRAME_STEPS, FrameBudget, GrantSet, InstanceLimits, LogLevel,
    Runner, STEP_NANOS, ServerContent, Session, Settings, Stop, Trap, Unstarted, Violation,
};
use parity::Trace;

/// A frame's length in the realm's time.
pub const FRAME_US: u64 = 16_667;
/// The frame time the budget is taken from.
const TARGET: Duration = Duration::from_micros(FRAME_US);
/// The mod every case's scripts belong to.
const MOD: &str = "m";

/// The guest of a realm.
#[derive(Clone, Copy, Debug)]
pub enum Guest {
    /// The realm's guest, `torque_lua_client.wasm`.
    Realm,
    /// A hand-written guest: its text.
    Wat(fn() -> String),
}

/// What makes realms for a case: one kind of runner.
pub trait Runners {
    /// A runner over a fresh instance of `guest`, or the trap that kept it from starting.
    ///
    /// # Errors
    ///
    /// The trap, when the instance could not be made.
    fn open(&mut self, guest: Guest, limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap>;

    /// The instances this maker's runners hold.
    fn live(&self) -> usize;

    /// The settings of this kind of runner.
    fn settings(&self) -> Settings;
}

/// One step of a case.
pub enum Action {
    /// Makes a realm in `slot`.
    Open {
        slot: usize,
        guest: Guest,
        grants: Grants,
        /// The mod's scripts, the first its entry: paths within the mod and their text.
        scripts: &'static [(&'static str, &'static str)],
        /// Changes the runner's settings for this realm.
        tune: fn(&mut Settings),
        /// What applying each `ui` piece costs the engine (to try the budget's rules).
        apply_cost: Duration,
    },
    /// Runs `count` frames of the realm in `slot` (fewer once it stopped).
    Frames { slot: usize, count: u32 },
    /// The player revokes `tier` of the realm in `slot`.
    Revoke { slot: usize, tier: Tier },
    /// Closes the realm in `slot` (a disconnect).
    Close { slot: usize },
    /// A check over the run so far.
    Check(fn(&World) -> Result<(), String>),
}

impl Action {
    /// [`Action::Open`] with the defaults: no tuning, applying free.
    fn open(
        slot: usize,
        guest: Guest,
        grants: Grants,
        scripts: &'static [(&'static str, &'static str)],
    ) -> Action {
        Action::Open {
            slot,
            guest,
            grants,
            scripts,
            tune: |_| {},
            apply_cost: Duration::ZERO,
        }
    }
}

/// A case: its name and its steps.
pub struct Case {
    /// The case's name.
    pub name: &'static str,
    /// Its steps.
    pub actions: fn() -> Vec<Action>,
}

impl Case {
    /// Whether the case runs the realm's guest (natively it is skipped when that guest was not built).
    pub fn needs_realm_guest(&self) -> bool {
        (self.actions)().iter().any(|a| {
            matches!(
                a,
                Action::Open {
                    guest: Guest::Realm,
                    ..
                }
            )
        })
    }
}

/// What a realm did to the engine, as a case sees it.
#[derive(Debug, Default)]
pub struct Recorder {
    /// Its log lines.
    pub logs: Vec<String>,
    /// The log lines that were script errors.
    pub errors: Vec<String>,
    /// The `ui` records it applied.
    pub ui_records: usize,
    /// The controls and roots it holds (made, not yet torn down or revoked).
    pub objects: usize,
    /// The `ui` pieces applied after `ui` was revoked.
    pub ui_after_revoke: usize,
    /// The tiers revoked.
    pub revoked: Vec<Tier>,
    /// How often its effects were torn down.
    pub teardowns: u32,
    /// Its messages.
    pub sends: Vec<Value>,
    /// Its calls' names.
    pub calls: Vec<String>,
    /// Everything above as one number ([`parity`]).
    pub trace: Trace,
    apply_cost: Duration,
}

impl Effects for Recorder {
    fn log(&mut self, level: LogLevel, line: &str) {
        self.trace.put(1 + level as u8, line.as_bytes());
        if level == LogLevel::Error {
            self.errors.push(line.to_string());
        }
        self.logs.push(line.to_string());
    }

    fn check_ui(&mut self, records: &[u8]) -> Result<(), Violation> {
        Records::new(records, usize::MAX, usize::MAX)
            .and_then(Records::collect_all)
            .map(|_| ())
            .map_err(|_| Violation::Ui("ui records the host should not have passed"))
    }

    fn apply_ui(&mut self, bytes: &[u8]) -> Result<(), Violation> {
        let started = Instant::now();
        self.trace.put(4, bytes);
        let records = Records::new(bytes, usize::MAX, usize::MAX)
            .and_then(Records::collect_all)
            .map_err(|_| Violation::Ui("ui records the host should not have passed"))?;
        self.ui_records += records.len();
        self.objects += records
            .iter()
            .filter(|r| matches!(r.kind, op::ui::ROOT | op::ui::CREATE))
            .count();
        if self.revoked.contains(&Tier::Ui) {
            self.ui_after_revoke += 1;
        }
        while started.elapsed() < self.apply_cost {
            std::hint::spin_loop();
        }
        Ok(())
    }

    fn send(&mut self, message: Value) {
        self.trace
            .put(5, &message.to_bytes().expect("a message that was checked"));
        self.sends.push(message);
    }

    fn call(&mut self, id: u32, name: &str, args: Value) -> CallRoute {
        self.trace.put(6, &id.to_le_bytes());
        self.trace.put(6, name.as_bytes());
        self.trace
            .put(6, &args.to_bytes().expect("arguments that were checked"));
        self.calls.push(name.to_string());
        CallRoute::Refused
    }

    fn revoke(&mut self, tier: Tier) {
        self.trace.put(7, &[tier.id()]);
        self.revoked.push(tier);
        if tier == Tier::Ui {
            self.objects = 0;
        }
    }

    fn teardown(&mut self) {
        self.trace.put(8, &[]);
        self.teardowns += 1;
        self.objects = 0;
    }

    fn stopped(&mut self, why: &Stop) {
        self.trace.put(9, format!("{why:?}").as_bytes());
    }
}

/// One realm of a case.
pub struct Slot {
    /// The realm, until it is closed.
    pub session: Option<Session<Recorder>>,
    /// What it did, once it is closed.
    closed: Option<Recorder>,
    /// What it runs with.
    pub settings: Settings,
}

/// The run so far, for checks.
pub struct World {
    slots: Vec<Option<Slot>>,
    live: usize,
    now_us: u64,
}

impl World {
    /// The realm in `slot`.
    ///
    /// # Panics
    ///
    /// When no realm was made there (a case's own mistake).
    pub fn slot(&self, slot: usize) -> &Slot {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .expect("the case made a realm in this slot")
    }

    /// The instances that exist.
    pub fn live(&self) -> usize {
        self.live
    }

    /// What each realm did, in numbers: for the message of a stream that came to another number.
    fn summary(&self) -> String {
        self.slots
            .iter()
            .flatten()
            .map(|slot| {
                let e = slot.effects();
                format!(
                    "[logs {} errors {} ui {} sends {} calls {} revoked {:?} teardowns {} stop {:?}]",
                    e.logs.len(),
                    e.errors.len(),
                    e.ui_records,
                    e.sends.len(),
                    e.calls.len(),
                    e.revoked,
                    e.teardowns,
                    slot.stop().map(|(why, _)| why)
                )
            })
            .collect()
    }

    /// The realms' streams as one number.
    fn digest(&self) -> u64 {
        let mut all = Trace::default();
        for slot in self.slots.iter().flatten() {
            all.merge(slot.effects().trace);
        }
        all.digest()
    }

    fn slot_mut(&mut self, slot: usize) -> &mut Slot {
        self.slots
            .get_mut(slot)
            .and_then(Option::as_mut)
            .expect("the case made a realm in this slot")
    }
}

impl Slot {
    /// What the realm did.
    ///
    /// # Panics
    ///
    /// Never: a slot holds a session until it is closed, and its effects after.
    pub fn effects(&self) -> &Recorder {
        self.session
            .as_ref()
            .map(Session::effects)
            .or(self.closed.as_ref())
            .expect("a slot holds its realm or what it did")
    }

    /// Why the realm stopped, and after how long.
    pub fn stop(&self) -> Option<(&Stop, Duration)> {
        self.session.as_ref().and_then(|s| s.stop())
    }

    /// `Ok` when the realm stopped with `want` within twice its deadline.
    pub fn stopped_within_twice_the_deadline(&self, want: &Stop) -> Result<(), String> {
        let Some((stop, after)) = self.stop() else {
            return Err(format!("the realm runs on (wanted {want})"));
        };
        if stop != want {
            return Err(format!(
                "the realm stopped because {stop}, not because {want}"
            ));
        }
        let limit = 2 * self.settings.deadline;
        if after > limit {
            return Err(format!("stopped after {after:?}, more than {limit:?}"));
        }
        Ok(())
    }
}

/// Where a run is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A runner's entry is in flight: wait for it, then step again.
    Waiting,
    /// The case is over: `Err` with what failed.
    Done(Result<(), String>),
}

/// A case being run.
pub struct CaseRun {
    name: &'static str,
    digest: Option<u64>,
    summary: String,
    actions: Vec<Action>,
    at: usize,
    /// Frames done of the current [`Action::Frames`].
    frames: u32,
    /// Calls of the current [`Action::Frames`] that started no frame (a web runner spends them on startup entries).
    idle_calls: u32,
    world: World,
    runners: Box<dyn Runners>,
}

impl CaseRun {
    /// A run of `case` with `runners`.
    pub fn new(case: &Case, runners: Box<dyn Runners>) -> CaseRun {
        CaseRun {
            name: case.name,
            digest: None,
            summary: String::new(),
            actions: (case.actions)(),
            at: 0,
            frames: 0,
            idle_calls: 0,
            world: World {
                slots: Vec::new(),
                live: 0,
                now_us: 0,
            },
            runners,
        }
    }

    /// Runs the case on until it waits for a runner or is over.
    pub fn step(&mut self) -> Step {
        while let Some(action) = self.actions.get(self.at) {
            match action {
                &Action::Open {
                    slot,
                    guest,
                    grants,
                    scripts,
                    tune,
                    apply_cost,
                } => self.open(slot, guest, grants, scripts, tune, apply_cost),
                &Action::Frames { slot, count } => {
                    // A frame is a frame entry of the guest, so the realm's time and the cases' counts mean the same on every
                    // runner: natively the startup entries and the first frame are one call of `Session::frame`, on the web
                    // each entry is a call of its own, and those calls pass no time.
                    while self.frames < count && self.idle_calls < 64 + 4 * count {
                        if !self.settle(slot) {
                            return Step::Waiting;
                        }
                        let now = self.world.now_us + FRAME_US;
                        let slot = self.world.slot_mut(slot);
                        let Some(session) = slot.session.as_mut() else {
                            break;
                        };
                        if session.stop().is_some() {
                            break;
                        }
                        let before = session.frames_started();
                        let report = session.frame(now, TARGET);
                        if session.frames_started() > before || report.sat_out {
                            self.world.now_us = now;
                            self.frames += 1;
                        } else {
                            self.idle_calls += 1;
                        }
                    }
                    if !self.settle(slot) {
                        return Step::Waiting;
                    }
                    self.frames = 0;
                    self.idle_calls = 0;
                }
                &Action::Revoke { slot, tier } => {
                    let slot = self.world.slot_mut(slot);
                    if let Some(session) = slot.session.as_mut() {
                        session.revoke(tier);
                    }
                }
                &Action::Close { slot } => {
                    let slot = self.world.slot_mut(slot);
                    if let Some(session) = slot.session.take() {
                        slot.closed = Some(session.close());
                    }
                }
                Action::Check(check) => {
                    self.world.live = self.runners.live();
                    if let Err(error) = check(&self.world) {
                        return Step::Done(Err(format!(
                            "{}: step {}: {error}",
                            self.name,
                            self.at + 1
                        )));
                    }
                }
            }
            self.at += 1;
        }
        let digest = self.world.digest();
        let summary = self.world.summary();
        self.digest = Some(digest);
        self.summary.clone_from(&summary);
        // The realms end with the case.
        self.world.slots.clear();
        if parity::RUNNER_DEPENDENT.contains(&self.name) {
            return Step::Done(Ok(()));
        }
        match parity::expected(self.name) {
            Some(want) if want == digest => Step::Done(Ok(())),
            Some(want) => Step::Done(Err(format!(
                "{}: its stream came to {digest:016x}, not {want:016x} (parity.txt): another runner saw something else, or the case changed {summary}",
                self.name
            ))),
            None => Step::Done(Err(format!(
                "{}: no number in parity.txt (it would be {digest:016x})",
                self.name
            ))),
        }
    }

    /// The number the finished case's stream came to ([`parity`]): `None` until it is over.
    pub fn digest(&self) -> Option<u64> {
        self.digest
    }

    /// What the realms of the finished case did, in numbers (empty until it is over).
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// [`CaseRun::digest`] for a case that has a number in `parity.txt`.
    pub fn parity(&self) -> Option<u64> {
        self.digest
            .filter(|_| !parity::RUNNER_DEPENDENT.contains(&self.name))
    }

    fn open(
        &mut self,
        slot: usize,
        guest: Guest,
        grants: Grants,
        scripts: &'static [(&'static str, &'static str)],
        tune: fn(&mut Settings),
        apply_cost: Duration,
    ) {
        let mut settings = self.runners.settings();
        tune(&mut settings);
        let runner = self
            .runners
            .open(guest, settings.instance())
            .unwrap_or_else(|trap| Box::new(Unstarted(trap)));
        let content = ServerContent::new(
            scripts
                .iter()
                .map(|(path, source)| (format!("mods/{MOD}/{path}"), source.as_bytes().into())),
        );
        let mods: Vec<ClientMod> = scripts
            .first()
            .map(|(entry, _)| ClientMod {
                id: MOD.into(),
                entry: (*entry).into(),
            })
            .into_iter()
            .collect();
        let effects = Recorder {
            apply_cost,
            ..Recorder::default()
        };
        let session = Session::open(
            runner,
            effects,
            GrantSet::unreviewed(grants),
            &content,
            &mods,
            settings,
        );
        if self.world.slots.len() <= slot {
            self.world.slots.resize_with(slot + 1, || None);
        }
        self.world.slots[slot] = Some(Slot {
            session: Some(session),
            closed: None,
            settings,
        });
    }

    /// Collects the outcome of the realm's entry in flight: `false` while it is still running.
    fn settle(&mut self, slot: usize) -> bool {
        let slot = self.world.slot_mut(slot);
        let Some(session) = slot.session.as_mut() else {
            return true;
        };
        session.poll();
        !session.is_waiting()
    }
}

/// Every case.
pub const CASES: &[Case] = &[
    Case {
        name: "the_deadline_stops_a_long_shared_prefix_compare",
        actions: || gate(SHARED_PREFIX),
    },
    Case {
        name: "the_deadline_stops_concatenating_onto_a_mib",
        actions: || gate(CONCATENATION),
    },
    Case {
        name: "the_deadline_stops_iterating_a_sparse_table",
        actions: || gate(SPARSE_TABLE),
    },
    Case {
        name: "the_deadline_stops_pattern_backtracking",
        actions: || gate(BACKTRACKING),
    },
    Case {
        name: "the_deadline_stops_a_loop_without_imports",
        actions: the_deadline_stops_a_loop_without_imports,
    },
    Case {
        name: "a_guest_past_its_memory_traps_and_only_it_stops",
        actions: a_guest_past_its_memory_traps_and_only_it_stops,
    },
    Case {
        name: "an_op_of_an_ungranted_tier_stops_the_realm",
        actions: an_op_of_an_ungranted_tier_stops_the_realm,
    },
    Case {
        name: "an_op_of_a_revoked_tier_stops_the_realm_once_it_was_told",
        actions: an_op_of_a_revoked_tier_stops_the_realm_once_it_was_told,
    },
    Case {
        name: "revoking_removes_the_tier_in_the_same_frame",
        actions: revoking_removes_the_tier_in_the_same_frame,
    },
    Case {
        name: "three_limit_stops_in_ten_seconds_turn_the_realm_off",
        actions: three_limit_stops_in_ten_seconds_turn_the_realm_off,
    },
    Case {
        name: "limit_stops_further_apart_do_not",
        actions: limit_stops_further_apart_do_not,
    },
    Case {
        name: "closing_leaves_no_instance_and_nothing_applied",
        actions: closing_leaves_no_instance_and_nothing_applied,
    },
    Case {
        name: "a_realm_over_budget_too_often_runs_every_other_frame",
        actions: a_realm_over_budget_too_often_runs_every_other_frame,
    },
    Case {
        name: "os_clock_is_the_instances_own",
        actions: os_clock_is_the_instances_own,
    },
    Case {
        name: "a_call_of_an_rpc_not_marked_for_the_realm_is_answered_refused",
        actions: a_call_of_an_rpc_not_marked_for_the_realm_is_answered_refused,
    },
    Case {
        name: "a_guest_at_its_step_caps_every_frame_is_struck_off",
        actions: a_guest_at_its_step_caps_every_frame_is_struck_off,
    },
    Case {
        name: "a_guest_holding_its_entries_past_the_long_entry_time_is_struck_off",
        actions: a_guest_holding_its_entries_past_the_long_entry_time_is_struck_off,
    },
    Case {
        name: "the_ui_functions_cost_steps_for_what_they_handle",
        actions: the_ui_functions_cost_steps_for_what_they_handle,
    },
    Case {
        name: "nan_prints_the_same_on_every_runner",
        actions: nan_prints_the_same_on_every_runner,
    },
    Case {
        name: "numbers_print_and_compute_the_same_on_every_runner",
        actions: parity::numbers,
    },
    Case {
        name: "strings_patterns_and_sorting_come_out_the_same_on_every_runner",
        actions: parity::strings,
    },
    Case {
        name: "a_ui_tree_and_its_timers_come_out_the_same_on_every_runner",
        actions: parity::ui_tree,
    },
    Case {
        name: "a_layout_tree_comes_out_the_same_on_every_runner",
        actions: parity::layout_tree,
    },
    Case {
        name: "messages_and_calls_come_out_the_same_on_every_runner",
        actions: parity::net,
    },
    Case {
        name: "the_step_caps_cut_where_they_do_on_every_runner",
        actions: parity::steps,
    },
    Case {
        name: "a_revocation_lands_in_the_same_place_on_every_runner",
        actions: parity::revoked,
    },
];

/// The longest a frame's entry of a realm at its step caps may take: a frame budget (2 ms of steps) and room for a
/// loaded machine.
const AT_CAPS: Duration = Duration::from_millis(8);

/// The base, `ui` and `net`.
const UI_NET: Grants = Grants(0b111);

/// No step limit worth the name: only the deadline can stop these.
fn unlimited_steps(settings: &mut Settings) {
    settings.callback_steps = u32::MAX;
    settings.frame_steps = u32::MAX;
}

/// A deadline case: a script whose work Luau's step count barely sees, stopped by the wall-clock deadline within
/// twice the deadline.
fn gate(scripts: &'static [(&'static str, &'static str)]) -> Vec<Action> {
    vec![
        Action::Open {
            slot: 0,
            guest: Guest::Realm,
            grants: Grants(1),
            scripts,
            tune: |s| {
                unlimited_steps(s);
                // Only the deadline: no entry here is a strike for its length.
                s.long_entry = s.deadline;
                // Room for the sparse table's 2^20 slots.
                s.lua_memory = 64 << 20;
            },
            apply_cost: Duration::ZERO,
        },
        Action::Frames { slot: 0, count: 40 },
        Action::Check(|w| {
            w.slot(0)
                .stopped_within_twice_the_deadline(&Stop::Trapped(Trap::Deadline))
        }),
        Action::Check(|w| expect(w.live() == 0, "the stopped instance is gone")),
    ]
}

const SHARED_PREFIX: &[(&str, &str)] = &[(
    "main.lua",
    r#"
local a = string.rep("a", 1048576) .. "b"
local b = string.rep("a", 1048576) .. "c"
local n = 0
while true do if a < b then n = n + 1 end end
"#,
)];

const CONCATENATION: &[(&str, &str)] = &[(
    "main.lua",
    r#"
local s = string.rep("x", 1048576)
while true do s = s .. "x" end
"#,
)];

// Filled and emptied a piece a frame (so no single entry nears the deadline), then iterated: each pass walks 2^20
// empty slots inside one instruction.
const SPARSE_TABLE: &[(&str, &str)] = &[(
    "main.lua",
    r#"
local t, n, stage = {}, 0, "fill"
local SLOTS, PIECE = 1048576, 131072
realm.onFrame(function()
  if stage == "fill" then
    for i = n + 1, n + PIECE do t[i + 0.5] = true end
    n = n + PIECE
    if n >= SLOTS then stage, n = "empty", 0 end
  elseif stage == "empty" then
    for i = n + 1, math.min(n + PIECE, SLOTS - 1) do t[i + 0.5] = nil end
    n = n + PIECE
    if n >= SLOTS - 1 then stage = "spin" end
  else
    while true do for k, v in t do end end
  end
end)
"#,
)];

// Six quantifiers, the most a pattern may have: one call that backtracks for hours.
const BACKTRACKING: &[(&str, &str)] = &[(
    "main.lua",
    r#"
string.find(string.rep("a", 200), "a*a*a*a*a*a*b")
"#,
)];

/// `Ok` when `ok`, else `Err(what)`.
fn expect(ok: bool, what: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("not so: {what}"))
    }
}

/// A hand-written guest with every export of the format: `frame` runs `frame_body` and returns 0, and its flush is
/// `ops`.
fn fake_guest(frame_body: &str, ops: &[u8]) -> String {
    let data: String = ops.iter().map(|b| format!("\\{b:02x}")).collect();
    format!(
        r#"(module
  (memory (export "memory") 1)
  (data (i32.const 16) "{data}")
  (func (export "torque_realm_format") (result i32) i32.const 2)
  (func (export "torque_realm_tiers") (result i32) i32.const 7)
  (func (export "torque_realm_alloc") (param i32) (result i32) i32.const 4096)
  (func (export "torque_realm_free") (param i32 i32))
  (func (export "torque_realm_new") (param i32 i32) (result i32) i32.const 0)
  (func (export "torque_realm_add_script") (param i32 i32 i32 i32 i32 i32) (result i32) i32.const 0)
  (func (export "torque_realm_run") (param i64 i32 i32 i32 i32) (result i32) i32.const 0)
  (func (export "torque_realm_frame") (param i64 i32 i32) (result i32) {frame_body} i32.const 0)
  (func (export "torque_realm_ops") (result i32) i32.const 16)
  (func (export "torque_realm_ops_len") (result i32) i32.const {len})
  (func (export "torque_realm_pending") (result i32) i32.const 0))"#,
        len = ops.len(),
    )
}

/// A flush of one `ui.root` op.
fn ui_root_flush() -> Vec<u8> {
    let mut out = Writer::new();
    let mark = out.begin(Tier::Ui.id(), op::ui::ROOT);
    out.u32(1);
    out.text(MOD);
    out.end(mark).expect("a short record");
    out.into_bytes()
}

fn the_deadline_stops_a_loop_without_imports() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Wat(|| fake_guest("(loop $l (br $l))", &[])),
            Grants(1),
            &[],
        ),
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| {
            w.slot(0)
                .stopped_within_twice_the_deadline(&Stop::Trapped(Trap::Deadline))
        }),
    ]
}

fn a_guest_past_its_memory_traps_and_only_it_stops() -> Vec<Action> {
    // Grows 1 MiB at a time; a refused growth is a trap (natively at the growth, on the web at the `unreachable`).
    let grow = || {
        fake_guest(
            "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) unreachable",
            &[],
        )
    };
    vec![
        Action::open(0, Guest::Wat(grow), Grants(1), &[]),
        Action::open(1, Guest::Wat(|| fake_guest("", &[])), Grants(1), &[]),
        Action::Frames { slot: 0, count: 10 },
        Action::Frames { slot: 1, count: 10 },
        Action::Check(|w| {
            w.slot(0)
                .stopped_within_twice_the_deadline(&Stop::Trapped(Trap::Memory))
        }),
        Action::Check(|w| expect(w.slot(1).stop().is_none(), "the other realm runs on")),
        Action::Check(|w| expect(w.live() == 1, "only the other instance is left")),
    ]
}

fn an_op_of_an_ungranted_tier_stops_the_realm() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Wat(|| fake_guest("", &ui_root_flush())),
            Grants(1),
            &[],
        ),
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| {
            let want = Stop::Violation(Violation::Ungranted);
            expect(
                w.slot(0).stop().is_some_and(|(s, _)| *s == want),
                "stopped for the ui op",
            )?;
            expect(w.slot(0).effects().ui_records == 0, "nothing of it applied")
        }),
    ]
}

fn an_op_of_a_revoked_tier_stops_the_realm_once_it_was_told() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Wat(|| fake_guest("", &ui_root_flush())),
            UI_NET,
            &[],
        ),
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| {
            expect(
                w.slot(0).effects().ui_records >= 1,
                "its ui ops were applied",
            )
        }),
        Action::Revoke {
            slot: 0,
            tier: Tier::Ui,
        },
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| {
            let want = Stop::Violation(Violation::Revoked);
            expect(
                w.slot(0).stop().is_some_and(|(s, _)| *s == want),
                "stopped once told",
            )?;
            expect(
                w.slot(0).effects().ui_after_revoke == 0,
                "nothing applied after the revocation",
            )
        }),
    ]
}

fn revoking_removes_the_tier_in_the_same_frame() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Realm,
            UI_NET,
            &[(
                "main.lua",
                r#"
ui.new("panel", {})
realm.on("revoked", function(name) realm.log("revoked " .. name .. " " .. tostring(ui.new("panel", {}))) end)
realm.every(0, function() ui.new("panel", {}) end)
"#,
            )],
        ),
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| expect(w.slot(0).effects().objects >= 3, "it made panels")),
        Action::Revoke {
            slot: 0,
            tier: Tier::Ui,
        },
        // Before another frame runs.
        Action::Check(|w| {
            let e = w.slot(0).effects();
            expect(
                e.revoked == [Tier::Ui] && e.objects == 0,
                "its controls are gone",
            )
        }),
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| {
            let slot = w.slot(0);
            expect(slot.stop().is_none(), "the realm runs on")?;
            expect(
                slot.effects().ui_after_revoke == 0,
                "no ui op applied after",
            )?;
            expect(
                slot.effects().logs.iter().any(|l| l == "revoked ui false"),
                "the guest was told and its ui functions answer false",
            )?;
            expect(
                !slot
                    .session
                    .as_ref()
                    .is_some_and(|s| s.grants().has(Tier::Ui)),
                "ui is gone",
            )
        }),
    ]
}

fn three_limit_stops_in_ten_seconds_turn_the_realm_off() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Realm,
            Grants(1),
            &[(
                "main.lua",
                "realm.every(0, function() while true do end end)",
            )],
        ),
        Action::Frames { slot: 0, count: 30 },
        Action::Check(|w| {
            expect(
                w.slot(0).stop().is_some_and(|(s, _)| *s == Stop::Strikes),
                "turned off",
            )?;
            expect(w.live() == 0, "its instance is gone")
        }),
    ]
}

fn limit_stops_further_apart_do_not() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Realm,
            Grants(1),
            &[(
                "main.lua",
                "realm.every(6, function() while true do end end)",
            )],
        ),
        // 25 s: a stop every 6 s, never three within 10.
        Action::Frames {
            slot: 0,
            count: 1500,
        },
        Action::Check(|w| {
            let slot = w.slot(0);
            expect(slot.stop().is_none(), "the realm runs on")?;
            expect(
                slot.effects()
                    .logs
                    .iter()
                    .filter(|l| l.contains("steps"))
                    .count()
                    >= 3,
                "its callbacks were stopped at least three times",
            )
        }),
    ]
}

fn closing_leaves_no_instance_and_nothing_applied() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Realm,
            UI_NET,
            &[("main.lua", r#"for i = 1, 3 do ui.new("panel", {}) end"#)],
        ),
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| {
            expect(
                w.slot(0).effects().objects >= 3 && w.live() == 1,
                "it made panels",
            )
        }),
        Action::Close { slot: 0 },
        Action::Check(|w| {
            let e = w.slot(0).effects();
            expect(w.live() == 0, "no instance")?;
            expect(e.teardowns == 1 && e.objects == 0, "nothing of it left")
        }),
    ]
}

fn a_realm_over_budget_too_often_runs_every_other_frame() -> Vec<Action> {
    vec![
        Action::Open {
            slot: 0,
            guest: Guest::Realm,
            grants: UI_NET,
            scripts: &[(
                "main.lua",
                r#"realm.every(0, function() ui.new("panel", {}):destroy() end)"#,
            )],
            tune: |s| {
                s.budget = match s.budget {
                    FrameBudget::Shared { share, .. } => FrameBudget::Shared {
                        max: Duration::from_millis(2),
                        share,
                    },
                    other => other,
                };
            },
            apply_cost: Duration::from_millis(3),
        },
        Action::Frames {
            slot: 0,
            count: 100,
        },
        Action::Check(|w| {
            let session = w.slot(0).session.as_ref().expect("open");
            expect(session.is_slow(), "the realm is slow")?;
            expect(session.stop().is_none(), "and still runs")
        }),
    ]
}

fn os_clock_is_the_instances_own() -> Vec<Action> {
    vec![
        Action::Open {
            slot: 0,
            guest: Guest::Realm,
            grants: Grants(1),
            scripts: &[(
                "main.lua",
                r#"local t = os.clock() while os.clock() - t < 0.15 do end realm.log("a " .. os.clock())"#,
            )],
            tune: unlimited_steps,
            apply_cost: Duration::ZERO,
        },
        Action::Frames { slot: 0, count: 10 },
        Action::open(
            1,
            Guest::Realm,
            Grants(1),
            &[("main.lua", r#"realm.log("b " .. os.clock())"#)],
        ),
        Action::Frames { slot: 1, count: 10 },
        Action::Check(|w| {
            let clock = |slot: usize, tag: &str| {
                w.slot(slot)
                    .effects()
                    .logs
                    .iter()
                    .find_map(|l| l.strip_prefix(tag)?.parse::<f64>().ok())
            };
            let (Some(a), Some(b)) = (clock(0, "a "), clock(1, "b ")) else {
                return Err("both realms logged their clocks".into());
            };
            expect(
                a >= 0.15 && b < a,
                "the second realm's clock starts at its own instance",
            )
        }),
    ]
}

fn a_call_of_an_rpc_not_marked_for_the_realm_is_answered_refused() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Realm,
            UI_NET,
            &[(
                "main.lua",
                r#"net.call("nope", {}, function(ok, v) realm.log("reply " .. tostring(ok) .. " " .. tostring(v)) end)"#,
            )],
        ),
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| {
            let e = w.slot(0).effects();
            expect(e.calls == ["nope"], "the call reached the engine")?;
            expect(
                e.logs.iter().any(|l| l == "reply false refused"),
                "and was answered refused",
            )
        }),
    ]
}

fn a_guest_at_its_step_caps_every_frame_is_struck_off() -> Vec<Action> {
    vec![
        // Each frame's callback would take 190,000 steps (about 20 ms): the caps stop it at a frame's budget.
        Action::open(
            0,
            Guest::Realm,
            Grants(1),
            &[(
                "main.lua",
                "realm.onFrame(function() local x = 0 for i = 1, 190000 do x = x + i end end)",
            )],
        ),
        Action::Frames { slot: 0, count: 40 },
        Action::Check(|w| {
            let slot = w.slot(0);
            expect(
                slot.stop().is_some_and(|(s, _)| *s == Stop::Strikes),
                "struck off",
            )?;
            expect(
                slot.effects().logs.iter().any(|l| l.contains("steps")),
                "for running out of steps",
            )?;
            let longest = slot
                .session
                .as_ref()
                .map_or(Duration::MAX, Session::longest_entry);
            expect(
                longest <= AT_CAPS,
                &format!("its entries stayed short (the longest {longest:?})"),
            )
        }),
    ]
}

fn a_guest_holding_its_entries_past_the_long_entry_time_is_struck_off() -> Vec<Action> {
    vec![
        // Comparing 512 KiB strings is a few steps of work each: 40 ms of them is well within the step caps.
        Action::open(
            0,
            Guest::Realm,
            Grants(1),
            &[(
                "main.lua",
                r#"
local a = string.rep("a", 524288) .. "b"
local b = string.rep("a", 524288) .. "c"
realm.onFrame(function()
  local t = os.clock()
  while os.clock() - t < 0.04 do local _ = a < b end
end)
"#,
            )],
        ),
        Action::Frames { slot: 0, count: 20 },
        Action::Check(|w| {
            let slot = w.slot(0);
            expect(
                slot.stop().is_some_and(|(s, _)| *s == Stop::Strikes),
                "struck off",
            )?;
            expect(
                !slot.effects().logs.iter().any(|l| l.contains("steps")),
                "for its entries' length, not its steps",
            )
        }),
    ]
}

fn the_ui_functions_cost_steps_for_what_they_handle() -> Vec<Action> {
    vec![
        // A full list's rows read back, set and made again, each in a loop a frame: the steps they are charged stop each
        // loop at the frame's budget, within ten times the time those steps of plain Luau take.
        Action::open(
            0,
            Guest::Realm,
            UI_NET,
            &[(
                "main.lua",
                r#"
local rows = {}
for i = 1, 1024 do rows[i] = string.format("row %011d", i) end
local l = ui.new("list", { rows = rows })
local n = 0
realm.onFrame(function()
  n = n + 1
  if n % 3 == 1 then
    while true do l:get("rows") end
  elseif n % 3 == 2 then
    while true do pcall(l.set, l, { rows = rows }) end
  else
    while true do pcall(ui.new, "list", { rows = rows }) end
  end
end)
"#,
            )],
        ),
        Action::Frames { slot: 0, count: 40 },
        Action::Check(|w| {
            let slot = w.slot(0);
            expect(
                slot.stop().is_some_and(|(s, _)| *s == Stop::Strikes),
                "struck off",
            )?;
            expect(
                slot.effects().logs.iter().any(|l| l.contains("steps")),
                "for running out of steps",
            )?;
            let longest = slot
                .session
                .as_ref()
                .map_or(Duration::MAX, Session::longest_entry);
            let most = Duration::from_nanos(10 * STEP_NANOS * u64::from(FRAME_STEPS));
            expect(
                longest <= most,
                &format!("its entries stayed within {most:?} (the longest {longest:?})"),
            )
        }),
    ]
}

fn nan_prints_the_same_on_every_runner() -> Vec<Action> {
    vec![
        Action::open(
            0,
            Guest::Realm,
            Grants(1),
            &[(
                "main.lua",
                r#"local z = 0 realm.log(tostring(z / z) .. " " .. tostring(-(z / z)) .. " " .. tostring(math.huge - math.huge))"#,
            )],
        ),
        Action::Frames { slot: 0, count: 10 },
        Action::Check(|w| {
            let logs = &w.slot(0).effects().logs;
            expect(
                logs.iter().any(|l| l == "nan nan nan"),
                &format!("NaN printed as {logs:?}"),
            )
        }),
    ]
}
