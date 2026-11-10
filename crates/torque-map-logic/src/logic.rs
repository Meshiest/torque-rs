//! The logic entities: server-only records with no place in the world.
//!
//! - `logic_relay`: `Trigger` fires `OnTrigger`; `Enable`, `Disable`, `Toggle`; spawnflag 1 removes it after one trigger.
//!   Unless spawnflag 2 (fast retrigger) is set, a relay whose outputs have delays is busy until the longest has passed: it
//!   posts its own `EnableRefire` after that delay and ignores `Trigger` until then. `CancelPending` drops its waiting
//!   events and ends the wait. `OnSpawn` fires on the first tick.
//! - `logic_auto`: on the first tick `OnMapSpawn`, then `OnNewGame` and `OnMultiNewMap` as [`crate::bus::Start`] says; then,
//!   with spawnflag 1 (on by default), it removes itself (its delayed outputs still run). A `globalstate` key names a game-wide
//!   state this bus does not keep, so such an entity does not fire (logged).
//! - `logic_timer`: fires `OnTimer` every `RefireTime` seconds while enabled (a random time between the bounds with
//!   `UseRandomTime`); spawnflag 1 alternates `OnTimerHigh` and `OnTimerLow` instead, high first. The interval is at least
//!   one tick. Inputs `Enable`, `Disable`, `Toggle`, `RefireTime`, `ResetTimer`, `FireTimer`, `AddToTimer`,
//!   `SubtractFromTimer`, `LowerRandomBound`, `UpperRandomBound`, `UseRandomTime`.
//! - `logic_case`: `InValue` fires the first `OnCaseNN` whose `CaseNN` equals the value (as text, or as numbers), else
//!   `OnDefault`; `PickRandom` fires one of the cases that have connections, `PickRandomShuffle` the same without repeats
//!   until each has fired once.
//! - `logic_compare`: `SetValue`, `SetValueCompare`, `SetCompareValue`, `Compare`; fires `OnLessThan`, `OnEqualTo`,
//!   `OnGreaterThan` and, when not equal, `OnNotEqualTo`, each with the value.
//! - `logic_branch`: `SetValue`, `SetValueTest`, `Toggle`, `ToggleTest`, `Test`; fires `OnTrue` or `OnFalse`.
//! - `math_counter`: `Add`, `Subtract`, `Multiply`, `Divide`, `SetValue` fire `OutValue` with the new value, held between
//!   `min` and `max` when `max` is above `min`; reaching `max` fires `OnHitMax` once until it leaves it (`min` and `OnHitMin`
//!   alike). `SetValueNoFire` sets it silently, `GetValue` fires `OnGetValue`, `SetHitMax` and `SetHitMin` move the bounds,
//!   `Enable` and `Disable`. A division by zero is ignored.

use crate::bus::{Ctx, EntityId, MapLogic, TICK_US};
use crate::entities::Keys;
use crate::value::{atof, boolean, format_number};

/// `logic_relay`'s state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relay {
    pub disabled: bool,
    pub waiting: bool,
}

impl Relay {
    pub(crate) fn new(k: &mut Keys<'_>) -> Relay {
        Relay {
            disabled: k.bool("StartDisabled"),
            waiting: false,
        }
    }

    pub(crate) fn spawn(&mut self, ml: &mut MapLogic, me: EntityId) {
        ml.fire(me, "OnSpawn", "", Some(me), Some(me));
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str) -> bool {
        match input {
            "trigger" => {
                if self.disabled || self.waiting {
                    return true;
                }
                let Some(e) = ml.entity(ctx.me) else {
                    return true;
                };
                let (once, fast) = (e.flag(1), e.flag(2));
                let longest = e
                    .outputs
                    .iter()
                    .filter(|(n, _)| n.eq_ignore_ascii_case("OnTrigger"))
                    .map(|(_, c)| c.delay)
                    .fold(0.0f32, f32::max);
                ml.fire(ctx.me, "OnTrigger", "", ctx.activator, ctx.caller);
                if once {
                    ml.remove(ctx.me);
                } else if !fast && longest > 0.0 {
                    self.waiting = true;
                    ml.post_self(ctx.me, "EnableRefire", "", longest);
                }
            }
            "enable" => self.disabled = false,
            "disable" => self.disabled = true,
            "toggle" => self.disabled = !self.disabled,
            "enablerefire" | "cancelpending" => self.waiting = false,
            _ => return false,
        }
        true
    }
}

/// `logic_auto`'s state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Auto {
    pub fired: bool,
}

impl Auto {
    pub(crate) fn spawn(&mut self, ml: &mut MapLogic, me: EntityId) {
        let Some(e) = ml.entity(me) else { return };
        if e.key("globalstate").is_some_and(|g| !g.trim().is_empty()) {
            ml.note(
                "globalstate",
                "a logic_auto waits on a game-wide state, which is not kept; it does not fire"
                    .into(),
            );
            return;
        }
        let remove = e.flag(1);
        let start = ml.start_kind();
        ml.fire(me, "OnMapSpawn", "", Some(me), Some(me));
        if start.new_game {
            ml.fire(me, "OnNewGame", "", Some(me), Some(me));
        }
        if start.multiplayer {
            ml.fire(me, "OnMultiNewMap", "", Some(me), Some(me));
        }
        self.fired = true;
        if remove {
            ml.remove(me);
        }
    }
}

/// `logic_timer`'s state.
#[derive(Debug, Clone, PartialEq)]
pub struct Timer {
    pub enabled: bool,
    pub refire: f64,
    pub random: bool,
    pub lower: f64,
    pub upper: f64,
    /// Whether the next oscillator output is `OnTimerHigh`.
    pub high: bool,
    /// When it fires next (microseconds), while enabled.
    pub next_us: Option<u64>,
}

impl Timer {
    pub(crate) fn new(k: &mut Keys<'_>) -> Timer {
        Timer {
            enabled: !k.bool("StartDisabled"),
            refire: k.f64("RefireTime"),
            random: k.bool("UseRandomTime"),
            lower: k.f64("LowerRandomBound"),
            upper: k.f64("UpperRandomBound"),
            high: true,
            next_us: None,
        }
    }

    fn interval_us(&self, ml: &mut MapLogic) -> u64 {
        let secs = if self.random {
            let (lo, hi) = (self.lower.min(self.upper), self.lower.max(self.upper));
            let r = (ml.random() >> 11) as f64 / (1u64 << 53) as f64;
            lo + (hi - lo) * r
        } else {
            self.refire
        };
        let us = if secs.is_finite() { secs * 1e6 } else { 0.0 };
        (us.clamp(TICK_US as f64, 1e12)) as u64
    }

    fn arm(&mut self, ml: &mut MapLogic) {
        self.next_us = Some(ml.now_us() + self.interval_us(ml));
    }

    pub(crate) fn spawn(&mut self, ml: &mut MapLogic, _me: EntityId) {
        if self.enabled {
            self.arm(ml);
        }
    }

    fn fire_now(&mut self, ml: &mut MapLogic, me: EntityId) {
        let osc = ml.entity(me).is_some_and(|e| e.flag(1));
        if osc {
            let out = if self.high {
                "OnTimerHigh"
            } else {
                "OnTimerLow"
            };
            self.high = !self.high;
            ml.fire(me, out, "", Some(me), Some(me));
        } else {
            ml.fire(me, "OnTimer", "", Some(me), Some(me));
        }
    }

    pub(crate) fn think(&mut self, ml: &mut MapLogic, me: EntityId) {
        if !self.enabled {
            return;
        }
        let Some(next) = self.next_us else {
            self.arm(ml);
            return;
        };
        if ml.now_us() >= next {
            self.fire_now(ml, me);
            self.arm(ml);
        }
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str, value: &str) -> bool {
        let shift = |t: &mut Timer, by: f64| {
            if let Some(n) = t.next_us {
                let d = (by * 1e6).clamp(-1e12, 1e12) as i64;
                t.next_us = Some(n.saturating_add_signed(d));
            }
        };
        match input {
            "enable" => {
                if !self.enabled {
                    self.enabled = true;
                    self.arm(ml);
                }
            }
            "disable" => {
                self.enabled = false;
                self.next_us = None;
            }
            "toggle" => {
                self.enabled = !self.enabled;
                if self.enabled {
                    self.arm(ml);
                } else {
                    self.next_us = None;
                }
            }
            "refiretime" => {
                self.refire = atof(value);
                if self.enabled {
                    self.arm(ml);
                }
            }
            "resettimer" => {
                if self.enabled {
                    self.arm(ml);
                }
            }
            "firetimer" => self.fire_now(ml, ctx.me),
            "addtotimer" => shift(self, atof(value)),
            "subtractfromtimer" => shift(self, -atof(value)),
            "lowerrandombound" => self.lower = atof(value),
            "upperrandombound" => self.upper = atof(value),
            "userandomtime" => self.random = boolean(value),
            _ => return false,
        }
        true
    }
}

/// `logic_case`'s state.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub cases: Vec<Option<String>>,
    /// The cases `PickRandomShuffle` has left.
    pub bag: Vec<usize>,
}

fn equal(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let num = |s: &str| {
        let t = s.trim();
        let ok = t
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'));
        ok.then(|| atof(t))
    };
    matches!((num(a), num(b)), (Some(x), Some(y)) if x == y)
}

impl Case {
    pub(crate) fn new(k: &mut Keys<'_>) -> Case {
        let cases = (1..=16)
            .map(|i| k.ent.key(&format!("Case{i:02}")).map(str::to_string))
            .collect();
        Case {
            cases,
            bag: Vec::new(),
        }
    }

    fn wired(ml: &MapLogic, me: EntityId) -> Vec<usize> {
        let Some(e) = ml.entity(me) else {
            return Vec::new();
        };
        (0..16)
            .filter(|i| {
                let name = format!("OnCase{:02}", i + 1);
                e.outputs.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name))
            })
            .collect()
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str, value: &str) -> bool {
        let me = ctx.me;
        match input {
            "invalue" => {
                let hit = self
                    .cases
                    .iter()
                    .position(|c| c.as_deref().is_some_and(|c| equal(c, value)));
                match hit {
                    Some(i) => ml.fire(
                        me,
                        &format!("OnCase{:02}", i + 1),
                        value,
                        ctx.activator,
                        Some(me),
                    ),
                    None => ml.fire(me, "OnDefault", value, ctx.activator, Some(me)),
                };
            }
            "pickrandom" => {
                let wired = Case::wired(ml, me);
                if !wired.is_empty() {
                    let i = wired[(ml.random() % wired.len() as u64) as usize];
                    let v = self.cases[i].clone().unwrap_or_default();
                    ml.fire(
                        me,
                        &format!("OnCase{:02}", i + 1),
                        &v,
                        ctx.activator,
                        Some(me),
                    );
                }
            }
            "pickrandomshuffle" => {
                if self.bag.is_empty() {
                    self.bag = Case::wired(ml, me);
                }
                if !self.bag.is_empty() {
                    let at = (ml.random() % self.bag.len() as u64) as usize;
                    let i = self.bag.swap_remove(at);
                    let v = self.cases[i].clone().unwrap_or_default();
                    ml.fire(
                        me,
                        &format!("OnCase{:02}", i + 1),
                        &v,
                        ctx.activator,
                        Some(me),
                    );
                }
            }
            _ => return false,
        }
        true
    }
}

/// `logic_compare`'s state.
#[derive(Debug, Clone, PartialEq)]
pub struct Compare {
    pub value: f64,
    pub compare: f64,
}

impl Compare {
    pub(crate) fn new(k: &mut Keys<'_>) -> Compare {
        Compare {
            value: k.f64("InitialValue"),
            compare: k.f64("CompareValue"),
        }
    }

    fn test(&self, ml: &mut MapLogic, ctx: Ctx) {
        let v = format_number(self.value);
        let (a, c) = (ctx.activator, ctx.caller);
        if self.value < self.compare {
            ml.fire(ctx.me, "OnLessThan", &v, a, c);
        }
        if self.value == self.compare {
            ml.fire(ctx.me, "OnEqualTo", &v, a, c);
        } else {
            ml.fire(ctx.me, "OnNotEqualTo", &v, a, c);
        }
        if self.value > self.compare {
            ml.fire(ctx.me, "OnGreaterThan", &v, a, c);
        }
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str, value: &str) -> bool {
        match input {
            "setvalue" => self.value = atof(value),
            "setvaluecompare" => {
                self.value = atof(value);
                self.test(ml, ctx);
            }
            "setcomparevalue" => self.compare = atof(value),
            "compare" => self.test(ml, ctx),
            _ => return false,
        }
        true
    }
}

/// `logic_branch`'s state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub value: bool,
}

impl Branch {
    pub(crate) fn new(k: &mut Keys<'_>) -> Branch {
        Branch {
            value: k.bool("InitialValue"),
        }
    }

    fn test(&self, ml: &mut MapLogic, ctx: Ctx) {
        let (out, v) = if self.value {
            ("OnTrue", "1")
        } else {
            ("OnFalse", "0")
        };
        ml.fire(ctx.me, out, v, ctx.activator, ctx.caller);
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str, value: &str) -> bool {
        match input {
            "setvalue" => self.value = boolean(value),
            "setvaluetest" => {
                self.value = boolean(value);
                self.test(ml, ctx);
            }
            "toggle" => self.value = !self.value,
            "toggletest" => {
                self.value = !self.value;
                self.test(ml, ctx);
            }
            "test" => self.test(ml, ctx),
            _ => return false,
        }
        true
    }
}

/// `math_counter`'s state.
#[derive(Debug, Clone, PartialEq)]
pub struct Counter {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub disabled: bool,
    hit_min: bool,
    hit_max: bool,
}

impl Counter {
    pub(crate) fn new(k: &mut Keys<'_>) -> Counter {
        let mut c = Counter {
            value: k.f64("startvalue"),
            min: k.f64("min"),
            max: k.f64("max"),
            disabled: k.bool("StartDisabled"),
            hit_min: false,
            hit_max: false,
        };
        c.value = c.held(c.value);
        c
    }

    fn bounded(&self) -> bool {
        self.max > self.min
    }

    fn held(&self, v: f64) -> f64 {
        if self.bounded() {
            v.clamp(self.min, self.max)
        } else {
            v
        }
    }

    pub(crate) fn spawn(&mut self, _ml: &mut MapLogic, _me: EntityId) {
        if self.bounded() {
            self.hit_max = self.value >= self.max;
            self.hit_min = self.value <= self.min;
        }
    }

    fn set(&mut self, ml: &mut MapLogic, ctx: Ctx, v: f64, fire: bool) {
        if !v.is_finite() {
            return;
        }
        self.value = self.held(v);
        let me = ctx.me;
        if fire {
            ml.fire(
                me,
                "OutValue",
                &format_number(self.value),
                ctx.activator,
                Some(me),
            );
        }
        if !self.bounded() {
            return;
        }
        let at_max = self.value >= self.max;
        let at_min = self.value <= self.min;
        if at_max && !self.hit_max && fire {
            ml.fire(
                me,
                "OnHitMax",
                &format_number(self.value),
                ctx.activator,
                Some(me),
            );
        }
        if at_min && !self.hit_min && fire {
            ml.fire(
                me,
                "OnHitMin",
                &format_number(self.value),
                ctx.activator,
                Some(me),
            );
        }
        self.hit_max = at_max;
        self.hit_min = at_min;
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str, value: &str) -> bool {
        let x = atof(value);
        match input {
            "enable" => self.disabled = false,
            "disable" => self.disabled = true,
            "getvalue" => {
                ml.fire(
                    ctx.me,
                    "OnGetValue",
                    &format_number(self.value),
                    ctx.activator,
                    Some(ctx.me),
                );
            }
            "sethitmax" => {
                self.max = x;
                self.value = self.held(self.value);
            }
            "sethitmin" => {
                self.min = x;
                self.value = self.held(self.value);
            }
            "add" | "subtract" | "multiply" | "divide" | "setvalue" | "setvaluenofire" => {
                if self.disabled {
                    return true;
                }
                let v = match input {
                    "add" => self.value + x,
                    "subtract" => self.value - x,
                    "multiply" => self.value * x,
                    "divide" if x == 0.0 => return true,
                    "divide" => self.value / x,
                    _ => x,
                };
                self.set(ml, ctx, v, input != "setvaluenofire");
            }
            _ => return false,
        }
        true
    }
}
