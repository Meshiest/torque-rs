//! One realm, for one server connection: its instance, its tiers, the work it waits to do and the rules that stop it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use torque_client_script_format::{Grants, Tier, Value, Writer, abi, event};
use web_time::Instant;

use crate::content::{self, ClientMod, Refused, ServerContent};
use crate::effects::{CallRoute, Effects, Refusal, Stop};
use crate::events::EventQueue;
use crate::flush::{self, Action, LogRate, NetCheck, Tiers};
use crate::grants::GrantSet;
use crate::pace::{Pace, Strikes};
use crate::runner::{Entry, Outcome, Reply, Runner};
use crate::settings::{FrameBudget, Settings};

/// The entry that is running.
struct InFlight {
    entry: Started,
    /// The realm time it was handed.
    now_us: u64,
    /// The revocations the guest had been handed when it started.
    told: Grants,
    started: Instant,
}

/// What a started entry was, for its outcome.
enum Started {
    New,
    AddScript { mod_id: Arc<str>, path: Arc<str> },
    Run { mod_id: Arc<str>, path: Arc<str> },
    Frame,
}

impl Started {
    fn of(entry: &Entry) -> Started {
        match entry {
            Entry::New(_) => Started::New,
            Entry::AddScript { mod_id, path, .. } => Started::AddScript {
                mod_id: Arc::clone(mod_id),
                path: Arc::clone(path),
            },
            Entry::Run { mod_id, path, .. } => Started::Run {
                mod_id: Arc::clone(mod_id),
                path: Arc::clone(path),
            },
            Entry::Frame { .. } => Started::Frame,
        }
    }
}

/// What one [`Session::frame`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameReport {
    /// Entries started.
    pub entries: u32,
    /// Checked actions applied.
    pub applied: u32,
    /// Whether the realm's work went over the frame's budget.
    pub over: bool,
    /// Whether the realm sat this frame out (it is slow).
    pub sat_out: bool,
}

/// A realm's state, without the engine's side ([`Session`] holds both).
struct Core {
    runner: Option<Box<dyn Runner>>,
    settings: Settings,
    tiers: Tiers,
    /// The engine time the realm's clock starts at.
    origin_us: Option<u64>,
    /// The realm time of the latest entry: never goes back.
    now_us: u64,
    startup: VecDeque<Entry>,
    refused: Vec<Refused>,
    in_flight: Option<InFlight>,
    backlog: VecDeque<Action>,
    events: EventQueue,
    net: NetCheck,
    logs: LogRate,
    strikes: Strikes,
    pace: Pace,
    stop: Option<(Stop, Duration)>,
    /// Whether the realm was closed: its effects were torn down.
    closed: bool,
    guest_pending: u32,
    /// How many `frame` entries were started.
    frames_started: u64,
    /// The longest a script's entry (a file run or a frame) ran.
    longest: Duration,
}

impl Core {
    fn open(
        runner: Box<dyn Runner>,
        grants: GrantSet,
        content: &ServerContent,
        mods: &[ClientMod],
        settings: Settings,
    ) -> Core {
        let grants = grants.grants();
        let (scripts, refused) = content::scripts(content, mods);
        let mut startup = VecDeque::with_capacity(scripts.len() + mods.len() + 1);
        startup.push_back(Entry::New(settings.config(grants)));
        startup.extend(scripts.into_iter().map(|s| Entry::AddScript {
            mod_id: s.mod_id,
            path: s.path,
            source: s.source,
        }));
        startup.extend(
            mods.iter()
                .filter(|m| {
                    !refused.iter().any(|r| match r {
                        Refused::ModId(id) | Refused::Entry { mod_id: id, .. } => *id == m.id,
                        Refused::File { .. } => false,
                    })
                })
                .map(|m| Entry::Run {
                    now_us: 0,
                    mod_id: m.id.as_str().into(),
                    path: m.entry.as_str().into(),
                }),
        );
        Core {
            runner: Some(runner),
            settings,
            tiers: Tiers {
                granted: grants,
                revoked: Grants(0),
                told: Grants(0),
            },
            origin_us: None,
            now_us: 0,
            startup,
            refused,
            in_flight: None,
            backlog: VecDeque::new(),
            events: EventQueue::default(),
            net: NetCheck::default(),
            logs: LogRate::default(),
            strikes: Strikes::new(settings.strikes),
            pace: Pace::new(settings.slow_after),
            stop: None,
            closed: false,
            guest_pending: 0,
            frames_started: 0,
            longest: Duration::ZERO,
        }
    }

    /// The tiers the realm holds now: those granted, less those revoked.
    fn grants(&self) -> Grants {
        Grants(self.tiers.granted.0 & !self.tiers.revoked.0)
    }

    /// Why the realm stopped, and how long the entry that stopped it had run.
    fn stop(&self) -> Option<(&Stop, Duration)> {
        self.stop.as_ref().map(|(stop, after)| (stop, *after))
    }

    /// Whether the realm is over its frame budget too often, and runs every other frame (the chip says "slow").
    fn is_slow(&self) -> bool {
        self.pace.is_slow()
    }

    /// Whether an entry is running off this thread (on the web) and its outcome has not been collected.
    fn is_waiting(&self) -> bool {
        self.in_flight.is_some()
    }

    /// The events the guest keeps for a later frame (its frame budget ran out), and those the host keeps.
    fn pending(&self) -> (u32, usize) {
        (self.guest_pending, self.events.len())
    }

    /// The scripts and mods left out (a path, compiled code, a cap, an entry not among the files).
    fn refused(&self) -> &[Refused] {
        &self.refused
    }

    /// The realm time at engine time `now_us`: the microseconds since its first frame, never going back.
    fn clock(&mut self, now_us: u64) -> u64 {
        let origin = *self.origin_us.get_or_insert(now_us);
        self.now_us = self.now_us.max(now_us.saturating_sub(origin));
        self.now_us
    }

    /// One engine frame at engine time `now_us`, whose target length is `target`: the outcome of a running entry
    /// collected, its checked ops applied, and the next entries run, within the realm's frame budget. The rest waits
    /// for the next frame.
    fn frame(&mut self, now_us: u64, target: Duration, effects: &mut dyn Effects) -> FrameReport {
        let mut report = FrameReport::default();
        if self.stop.is_some() {
            return report;
        }
        let now = self.clock(now_us);
        if self.pace.sits_out() {
            report.sat_out = true;
            return report;
        }
        let budget = self.settings.budget.of(target);
        let started = Instant::now();
        let mut applying = Duration::ZERO;
        let mode = self.settings.budget;
        let spent = |applying: Duration| match mode {
            FrameBudget::Shared { .. } => started.elapsed(),
            FrameBudget::ApplyOnly { .. } => applying,
        };
        let mut framed = false;
        loop {
            if self.in_flight.is_some() && !self.collect(effects) {
                break;
            }
            while let Some(action) = self.backlog.pop_front() {
                let at = Instant::now();
                self.apply(action, effects);
                applying += at.elapsed();
                report.applied += 1;
                if self.stop.is_some() {
                    return report;
                }
                if spent(applying) >= budget {
                    break;
                }
            }
            let used = spent(applying) >= budget && (report.entries > 0 || report.applied > 0);
            if !self.backlog.is_empty() || used {
                break;
            }
            let entry = match self.startup.pop_front() {
                Some(Entry::Run { mod_id, path, .. }) => Entry::Run {
                    now_us: now,
                    mod_id,
                    path,
                },
                Some(entry) => entry,
                None if !framed => {
                    framed = true;
                    self.frames_started += 1;
                    // The batch's revocations lead it: from this entry on the guest knows of every one so far.
                    self.tiers.told = self.tiers.revoked;
                    Entry::Frame {
                        now_us: now,
                        events: self.events.take_batch(),
                    }
                }
                None => break,
            };
            self.begin(entry, now);
            report.entries += 1;
        }
        report.over = spent(applying) > budget;
        if report.entries > 0 || report.applied > 0 {
            self.pace.record(report.over);
        }
        report
    }

    /// Collects the outcome of the running entry, if it has one (on the web, between frames).
    fn poll(&mut self, effects: &mut dyn Effects) {
        if self.in_flight.is_some() {
            self.collect(effects);
        }
    }

    fn begin(&mut self, entry: Entry, now_us: u64) {
        let Some(runner) = self.runner.as_mut() else {
            return;
        };
        self.in_flight = Some(InFlight {
            entry: Started::of(&entry),
            now_us,
            told: self.tiers.told,
            started: Instant::now(),
        });
        runner.start(entry);
    }

    /// Takes the running entry's outcome: `false` while it has none.
    fn collect(&mut self, effects: &mut dyn Effects) -> bool {
        // A stopped realm has no runner: nothing more comes, so there is nothing to wait for.
        let Some(outcome) = self.runner.as_mut().and_then(|r| r.poll()) else {
            return self.runner.is_none();
        };
        let Some(flight) = self.in_flight.take() else {
            return true;
        };
        self.outcome(flight, outcome, effects);
        true
    }

    fn outcome(&mut self, flight: InFlight, outcome: Outcome, effects: &mut dyn Effects) {
        let ran = flight.started.elapsed();
        let reply = match outcome {
            Ok(reply) => reply,
            Err(trap) => return self.halt(Stop::Trapped(trap), ran, effects),
        };
        // A script's entry that held the frame too long is a strike, whatever its steps were. Making the realm and
        // keeping its files are the host's own calls, not the script's time.
        if matches!(flight.entry, Started::Run { .. } | Started::Frame) {
            self.longest = self.longest.max(reply.elapsed);
            if (reply.long || reply.elapsed > self.settings.long_entry)
                && self.strikes.strike(flight.now_us)
            {
                return self.halt(Stop::Strikes, ran, effects);
            }
        }
        let status = reply.status;
        let InFlight {
            entry,
            now_us,
            told,
            ..
        } = flight;
        let flushed = match entry {
            Started::New if status == abi::OK => false,
            Started::New => {
                return self.halt(Stop::Refused(Refusal::New), ran, effects);
            }
            Started::AddScript { mod_id, path } => {
                if status == abi::REFUSED {
                    self.refused.push(Refused::File {
                        mod_id: mod_id.to_string(),
                        path: path.to_string(),
                    });
                } else if status != abi::OK {
                    return self.halt(Stop::Refused(Refusal::Script), ran, effects);
                }
                false
            }
            Started::Run { mod_id, path } => {
                if status == abi::REFUSED {
                    self.refused.push(Refused::Entry {
                        mod_id: mod_id.to_string(),
                        path: path.to_string(),
                    });
                    false
                } else if status != abi::OK {
                    return self.halt(Stop::Refused(Refusal::Run), ran, effects);
                } else {
                    true
                }
            }
            Started::Frame if status == abi::OK => true,
            Started::Frame => return self.halt(Stop::Refused(Refusal::Frame), ran, effects),
        };
        if flushed {
            self.flushed(now_us, told, reply, ran, effects);
        }
    }

    /// Checks an entry's flush whole; its strikes count now, the rest waits to be applied.
    fn flushed(
        &mut self,
        now_us: u64,
        told: Grants,
        reply: Reply,
        ran: Duration,
        effects: &mut dyn Effects,
    ) {
        self.guest_pending = reply.pending;
        let tiers = Tiers { told, ..self.tiers };
        let checked = flush::check(
            &reply.ops,
            tiers,
            now_us,
            &mut self.net,
            &mut self.logs,
            effects,
        );
        let actions = match checked {
            Ok(actions) => actions,
            Err(violation) => return self.halt(Stop::Violation(violation), ran, effects),
        };
        for action in actions {
            if action != Action::Limit {
                self.backlog.push_back(action);
            } else if self.strikes.strike(now_us) {
                return self.halt(Stop::Strikes, ran, effects);
            }
        }
    }

    fn apply(&mut self, action: Action, effects: &mut dyn Effects) {
        match action {
            Action::Log(level, line) => effects.log(level, &line),
            Action::Limit => {}
            Action::Ui(records) => {
                if let Err(violation) = effects.apply_ui(&records) {
                    self.halt(Stop::Violation(violation), Duration::ZERO, effects);
                }
            }
            Action::Send(message) => effects.send(message),
            Action::Call { id, name, args } => {
                if effects.call(id, &name, args) == CallRoute::Refused {
                    self.answer(id, false, &Value::String(b"refused".to_vec()));
                }
            }
        }
    }

    /// Stops the realm: its instance thrown away, everything it did removed, the player told.
    fn halt(&mut self, stop: Stop, ran: Duration, effects: &mut dyn Effects) {
        self.runner = None;
        self.in_flight = None;
        self.startup.clear();
        self.backlog.clear();
        self.events.clear();
        self.net.clear();
        effects.teardown();
        effects.stopped(&stop);
        self.stop = Some((stop, ran));
    }

    /// The player took `tier` away: its effects go now, its queued events and ops are dropped, and the guest is told
    /// in its next frame. `false` when the realm does not hold it (or it is the base, which only closing ends).
    fn revoke(&mut self, tier: Tier, effects: &mut dyn Effects) -> bool {
        if self.stop.is_some() || tier == Tier::Realm || !self.grants().has(tier) {
            return false;
        }
        self.tiers.revoked = self.tiers.revoked.with(tier);
        effects.revoke(tier);
        self.backlog.retain(|action| action_tier(action) != tier);
        self.events.drop_tier(tier);
        if tier == Tier::Net {
            self.net.clear();
        }
        self.events.revoked(tier);
        true
    }

    /// Ends the realm: its instance and everything it did gone (torn down once, unless a stop did it).
    fn close(&mut self, effects: &mut dyn Effects) {
        if self.stop.is_none() && !self.closed {
            effects.teardown();
        }
        self.closed = true;
        self.runner = None;
        self.in_flight = None;
        self.backlog.clear();
        self.events.clear();
        self.net.clear();
    }

    /// The GUI's events for the realm (records of `ui` events): those that follow the format are queued, the number
    /// dropped is returned. Nothing is queued while the realm does not hold `ui`.
    fn ui_events(&mut self, batch: &[u8]) -> usize {
        if self.stop.is_some() || !self.grants().has(Tier::Ui) {
            return 0;
        }
        self.events.ui_batch(batch)
    }

    /// A free-form message from the realm's server: `false` when it is not delivered (no `net`, no encoding).
    fn message(&mut self, message: &Value) -> bool {
        self.holds_net() && self.events.net(event::net::MESSAGE, |_| {}, &[message])
    }

    /// A call of the server's to the realm's `net.handle(name)`: `false` when it is not delivered.
    fn rpc(&mut self, name: &str, value: &Value) -> bool {
        self.holds_net() && self.events.net(event::net::RPC, |w| w.text(name), &[value])
    }

    /// The server's answer to the realm's call `id`: `false` when no such call waits.
    fn reply(&mut self, id: u32, ok: bool, value: &Value) -> bool {
        self.holds_net() && self.answer(id, ok, value)
    }

    fn answer(&mut self, id: u32, ok: bool, value: &Value) -> bool {
        if !self.net.answered(id) {
            return false;
        }
        let head = |ok: bool| {
            move |w: &mut Writer| {
                w.u32(id);
                w.bool(ok);
            }
        };
        // An answer with no room in a batch still ends the call, as a failure.
        self.events.net(event::net::REPLY, head(ok), &[value])
            || self.events.net(
                event::net::REPLY,
                head(false),
                &[&Value::String(b"the answer was too large".to_vec())],
            )
    }

    fn holds_net(&self) -> bool {
        self.stop.is_none() && self.grants().has(Tier::Net)
    }
}

/// Why a session's effects are there: it holds them from [`Session::open`] until it is closed or dropped.
const HELD: &str = "a session holds its effects until it is closed";

/// One realm on one server connection, and the engine's side of it (its [`Effects`]).
///
/// Made when the player allowed the server's scripts; [`Session::close`] it on disconnect, kick, server change or the
/// player turning scripts off (dropping it does the same). Every input is checked: events against the format before
/// they are queued, every op of the guest against the format, the grants and the caps before anything of its flush is
/// applied. A stopped realm (a trap, a violation, the strike rule) runs nothing again.
pub struct Session<E: Effects> {
    core: Core,
    /// `None` only once closed.
    effects: Option<E>,
}

impl<E: Effects> Session<E> {
    /// A realm over `runner` (a fresh instance) with `grants` and the engine's `effects`, running the client scripts
    /// of `mods` from the server's verified `content`. The guest is made and the scripts handed over and run on the
    /// first frames.
    pub fn open(
        runner: Box<dyn Runner>,
        effects: E,
        grants: GrantSet,
        content: &ServerContent,
        mods: &[ClientMod],
        settings: Settings,
    ) -> Session<E> {
        Session {
            core: Core::open(runner, grants, content, mods, settings),
            effects: Some(effects),
        }
    }

    /// The engine's side of the realm.
    pub fn effects(&self) -> &E {
        self.effects.as_ref().expect(HELD)
    }

    /// The engine's side of the realm, to change.
    pub fn effects_mut(&mut self) -> &mut E {
        self.effects.as_mut().expect(HELD)
    }

    /// One engine frame at engine time `now_us`, whose target length is `target`: the outcome of a running entry
    /// collected, its checked ops applied, and the next entries run, within the realm's frame budget. The rest waits
    /// for the next frame.
    pub fn frame(&mut self, now_us: u64, target: Duration) -> FrameReport {
        let effects = self.effects.as_mut().expect(HELD);
        self.core.frame(now_us, target, effects)
    }

    /// Collects the outcome of the running entry, if it has one (on the web, between frames).
    pub fn poll(&mut self) {
        let effects = self.effects.as_mut().expect(HELD);
        self.core.poll(effects);
    }

    /// The player took `tier` away: its effects go now, its queued events and ops are dropped, and the guest is told
    /// in its next frame. `false` when the realm does not hold it (or it is the base, which only closing ends).
    pub fn revoke(&mut self, tier: Tier) -> bool {
        let effects = self.effects.as_mut().expect(HELD);
        self.core.revoke(tier, effects)
    }

    /// Ends the realm (disconnect, kick, server change, the player turning it off): its instance and everything it did
    /// gone. Hands back the engine's side.
    pub fn close(mut self) -> E {
        let mut effects = self.effects.take().expect(HELD);
        self.core.close(&mut effects);
        effects
    }

    /// The tiers the realm holds now: those granted, less those revoked.
    pub fn grants(&self) -> Grants {
        self.core.grants()
    }

    /// Why the realm stopped, and how long the entry that stopped it had run.
    pub fn stop(&self) -> Option<(&Stop, Duration)> {
        self.core.stop()
    }

    /// Whether the realm is over its frame budget too often, and runs every other frame (the chip says "slow").
    pub fn is_slow(&self) -> bool {
        self.core.is_slow()
    }

    /// Whether an entry is running off this thread (on the web) and its outcome has not been collected.
    pub fn is_waiting(&self) -> bool {
        self.core.is_waiting()
    }

    /// The events the guest keeps for a later frame (its frame budget ran out), and those the host keeps.
    pub fn pending(&self) -> (u32, usize) {
        self.core.pending()
    }

    /// How many `frame` entries the guest was started on. A call of [`Session::frame`] starts at most one; on the web it may
    /// start none, spending the call on one of the startup entries.
    pub fn frames_started(&self) -> u64 {
        self.core.frames_started
    }

    /// The longest a script's entry (a file run or a frame) ran, as its runner measured it.
    pub fn longest_entry(&self) -> Duration {
        self.core.longest
    }

    /// The scripts and mods left out (a path, compiled code, a cap, an entry not among the files).
    pub fn refused(&self) -> &[Refused] {
        self.core.refused()
    }

    /// The GUI's events for the realm (records of `ui` events): those that follow the format are queued, the number
    /// dropped is returned. Nothing is queued while the realm does not hold `ui`.
    pub fn ui_events(&mut self, batch: &[u8]) -> usize {
        self.core.ui_events(batch)
    }

    /// A free-form message from the realm's server: `false` when it is not delivered (no `net`, no encoding).
    pub fn message(&mut self, message: &Value) -> bool {
        self.core.message(message)
    }

    /// A call of the server's to the realm's `net.handle(name)`: `false` when it is not delivered.
    pub fn rpc(&mut self, name: &str, value: &Value) -> bool {
        self.core.rpc(name, value)
    }

    /// The server's answer to the realm's call `id`: `false` when no such call waits.
    pub fn reply(&mut self, id: u32, ok: bool, value: &Value) -> bool {
        self.core.reply(id, ok, value)
    }
}

impl<E: Effects> Drop for Session<E> {
    /// A session dropped without [`Session::close`] is closed: nothing it did outlives it.
    fn drop(&mut self) {
        if let Some(effects) = self.effects.as_mut() {
            self.core.close(effects);
        }
    }
}

/// The tier an action is of.
fn action_tier(action: &Action) -> Tier {
    match action {
        Action::Log(..) | Action::Limit => Tier::Realm,
        Action::Ui(_) => Tier::Ui,
        Action::Send(_) | Action::Call { .. } => Tier::Net,
    }
}
