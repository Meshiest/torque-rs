//! Movers: brush entities that move (`func_door`, `func_door_rotating`, `func_movelinear`, `func_button`) or appear and go
//! (`func_brush`), drawn with the map's brush geometry and sent to clients as the plugin ghost class `MapMover`
//! ([`MoverGhost`]).
//!
//! **Motion.** A mover runs between a closed pose (0) and an open one (1) at a constant speed: a door and a button by their own
//! size along `movedir` less `lip`, `func_movelinear` by `movedistance`, a rotating door by `distance` degrees about z (x with
//! spawnflag 64, y with 128; spawnflag 2 the other way). A speed of 0 or less moves at once. The brush geometry is in the
//! model's own space at the entity's origin; a translating mover adds its offset, a rotating one turns about the origin.
//!
//! **Blocking.** Each tick a moving mover's volumes at the next pose are tested against the touchers' boxes by the separating
//! axis test (torque-collide). When one overlaps there and not at the current pose (a toucher already inside does not block it,
//! or it could never get out), the mover does not move that tick and fires `OnBlockedOpening` or
//! `OnBlockedClosing` once (the blocker as the activator); a door then reverses unless `forceclosed` is set, the others wait.
//! A door turned back by a block does not open by touch for a second. When it moves again it fires `OnUnblockedOpening` or `OnUnblockedClosing`. A host's own movement collides with
//! [`crate::MapLogic::mover_hit`].
//!
//! | Class | Inputs | Outputs |
//! |---|---|---|
//! | doors | `Open`, `Close`, `Toggle`, `Lock`, `Unlock`, `SetSpeed`, `Use` (spawnflag 256) | `OnOpen`, `OnClose`, `OnFullyOpen`, `OnFullyClosed`, the blocked ones, `OnLockedUse` |
//! | `func_movelinear` | `Open`, `Close`, `SetPosition`, `SetSpeed` | `OnFullyOpen`, `OnFullyClosed` |
//! | `func_button` | `Press`, `PressIn`, `PressOut`, `Lock`, `Unlock`, `Use` (spawnflag 1024) | `OnPressed`, `OnIn`, `OnOut`, `OnUseLocked` |
//! | `func_brush` | `Enable`, `Disable`, `Toggle` (and `Alpha`, `Color`, taken and not drawn) | none |
//!
//! A door opens when a player touches it (spawnflag 1024, on by default) and closes again `wait` seconds after it is fully
//! open (-1 stays open; spawnflag 32, toggle, stays too); `spawnpos` 1 starts it open; spawnflag 2048 starts it locked (a
//! locked door ignores use and touch, not inputs). A button moves in on a press (`OnPressed`, then `OnIn`), and out `wait`
//! seconds later (`OnOut`; -1 stays in); spawnflag 1 does not move it, 32 makes each press toggle it, 256 presses it on
//! touch, 2048 starts it locked. `func_brush` is solid while shown for `Solidity` 0, never for 1, always for 2.

use torque_engine::GhostRecord;

use crate::bus::{Ctx, EntityId, MapLogic, TICK_US};
use crate::entities::Keys;
use crate::value::{angles_dir, atof32, vec3};
use crate::volume::{Pose, Volume};

/// Which class a mover is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoverClass {
    Door,
    RotatingDoor,
    Linear,
    Button,
    Brush,
}

/// A mover's state (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct Mover {
    pub class: MoverClass,
    /// The volumes in the model's space.
    pub local: Vec<Volume>,
    pub origin: [f32; 3],
    /// The open pose's offset (translating classes).
    pub offset: [f32; 3],
    /// The open pose's turn, Source angles (rotating doors).
    pub turn: [f32; 3],
    /// The units (or degrees) from closed to open.
    pub travel: f32,
    /// Units (or degrees) a second.
    pub speed: f32,
    /// Where it is, 0 closed to 1 open.
    pub frac: f32,
    /// Where it is going.
    pub target: f32,
    /// When it goes back (a door's close, a button's return), in microseconds.
    pub return_at: Option<u64>,
    pub wait: f32,
    pub locked: bool,
    pub blocked: bool,
    pub visible: bool,
    pub solid: bool,
    pub force_closed: bool,
    flags: u32,
    solidity: i64,
    /// No opening by touch before this time (microseconds): set when a block turns a door back.
    touch_after: u64,
    /// The fraction at the last change of motion, and when.
    since_frac: f32,
    since_us: u64,
}

fn size_along(volumes: &[Volume], dir: [f32; 3]) -> f32 {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for v in volumes {
        let (a, b) = v.bounds();
        for k in 0..3 {
            lo[k] = lo[k].min(a[k]);
            hi[k] = hi[k].max(b[k]);
        }
    }
    if volumes.is_empty() {
        return 0.0;
    }
    (0..3).map(|k| (dir[k] * (hi[k] - lo[k])).abs()).sum()
}

impl Mover {
    pub(crate) fn new(k: &mut Keys<'_>, volumes: &[Volume]) -> Mover {
        let class = match k.ent.class.as_str() {
            "func_door" => MoverClass::Door,
            "func_door_rotating" => MoverClass::RotatingDoor,
            "func_movelinear" => MoverClass::Linear,
            "func_button" => MoverClass::Button,
            _ => MoverClass::Brush,
        };
        let flags = k.ent.spawnflags;
        let dir = angles_dir(vec3(&k.str("movedir")).unwrap_or([0.0; 3]));
        let mut m = Mover {
            class,
            local: volumes.to_vec(),
            origin: k.ent.origin,
            offset: [0.0; 3],
            turn: [0.0; 3],
            travel: 0.0,
            speed: k.f32("speed"),
            frac: 0.0,
            target: 0.0,
            return_at: None,
            wait: k.f32("wait"),
            locked: flags & 2048 != 0,
            blocked: false,
            visible: true,
            solid: true,
            force_closed: k.bool("forceclosed"),
            flags,
            solidity: 0,
            touch_after: 0,
            since_frac: 0.0,
            since_us: 0,
        };
        match class {
            MoverClass::Door | MoverClass::Button => {
                let travel = (size_along(volumes, dir) - k.f32("lip")).max(0.0);
                m.travel = travel;
                m.offset = dir.map(|d| d * travel);
                if class == MoverClass::Door && k.int("spawnpos") == 1 {
                    m.frac = 1.0;
                    m.target = 1.0;
                }
                if class == MoverClass::Door {
                    m.solid = flags & 4 == 0;
                }
            }
            MoverClass::RotatingDoor => {
                let mut d = k.f32("distance");
                if flags & 2 != 0 {
                    d = -d;
                }
                let axis = if flags & 64 != 0 {
                    2
                } else if flags & 128 != 0 {
                    0
                } else {
                    1
                };
                m.turn[axis] = d;
                m.travel = d.abs();
                if k.int("spawnpos") == 1 {
                    m.frac = 1.0;
                    m.target = 1.0;
                }
                m.solid = flags & 4 == 0;
            }
            MoverClass::Linear => {
                let travel = k.f32("movedistance").max(0.0);
                m.travel = travel;
                m.offset = dir.map(|d| d * travel);
                let s = k.f32("startposition").clamp(0.0, 1.0);
                m.frac = s;
                m.target = s;
                m.solid = flags & 8 == 0;
            }
            MoverClass::Brush => {
                m.solidity = k.int("Solidity");
                m.visible = !k.bool("StartDisabled");
                m.solid = m.solid_when(m.visible);
            }
        }
        m.since_frac = m.frac;
        m
    }

    fn solid_when(&self, visible: bool) -> bool {
        match self.solidity {
            1 => false,
            2 => true,
            _ => visible,
        }
    }

    /// The pose at the fraction `f`.
    pub fn pose_at(&self, f: f32) -> Pose {
        Pose {
            origin: [0, 1, 2].map(|k| self.origin[k] + self.offset[k] * f),
            angles: self.turn.map(|a| a * f),
        }
    }

    /// The volumes where they are now.
    pub fn volumes_now(&self) -> Vec<Volume> {
        let pose = self.pose_at(self.frac);
        self.local.iter().map(|v| v.placed(&pose)).collect()
    }

    /// The fraction a second it moves.
    fn rate(&self) -> f32 {
        if self.travel <= 0.0 || self.speed.is_nan() || self.speed <= 0.0 {
            f32::INFINITY
        } else {
            self.speed / self.travel
        }
    }

    fn moving(&self) -> i8 {
        if self.target > self.frac {
            1
        } else if self.target < self.frac {
            -1
        } else {
            0
        }
    }

    fn mark(&mut self, ml: &MapLogic) {
        self.since_frac = self.frac;
        self.since_us = ml.now_us();
    }

    fn go(&mut self, ml: &MapLogic, target: f32) {
        self.target = target.clamp(0.0, 1.0);
        self.return_at = None;
        self.mark(ml);
    }

    /// What clients are sent (`model` the brush model).
    pub fn ghost(&self, model: u32) -> MoverGhost {
        let rate = if self.blocked {
            0.0
        } else {
            let r = self.rate();
            let r = if r.is_finite() { r } else { 1e6 };
            r * f32::from(self.moving())
        };
        MoverGhost {
            model,
            origin: self.origin,
            offset: self.offset,
            turn: self.turn,
            from: self.since_frac,
            to: self.target,
            rate,
            since_tick: (self.since_us / TICK_US).min(u64::from(u32::MAX)) as u32,
            visible: self.visible,
            solid: self.solid,
        }
    }

    fn open_outputs(&self) -> (&'static str, &'static str) {
        match self.class {
            MoverClass::Button => ("OnIn", "OnOut"),
            _ => ("OnFullyOpen", "OnFullyClosed"),
        }
    }

    fn arrive(&mut self, ml: &mut MapLogic, me: EntityId) {
        let (open, closed) = self.open_outputs();
        let toggle = self.flags & 32 != 0;
        if self.frac >= 1.0 {
            ml.fire(me, open, "", Some(me), Some(me));
            let returns = matches!(
                self.class,
                MoverClass::Door | MoverClass::RotatingDoor | MoverClass::Button
            );
            if returns && self.wait >= 0.0 && !toggle {
                self.return_at = Some(ml.now_us() + (f64::from(self.wait) * 1e6) as u64);
            }
        } else if self.frac <= 0.0 {
            ml.fire(me, closed, "", Some(me), Some(me));
        }
        self.mark(ml);
    }

    pub(crate) fn spawn(&mut self, ml: &mut MapLogic, _me: EntityId) {
        self.mark(ml);
    }

    fn blocker(&self, ml: &MapLogic, frac: f32) -> Option<EntityId> {
        if !self.solid || self.local.is_empty() {
            return None;
        }
        let at = |f: f32| -> Vec<Volume> {
            let pose = self.pose_at(f);
            self.local.iter().map(|v| v.placed(&pose)).collect()
        };
        let (now, next) = (at(self.frac), at(frac));
        let meets = |vols: &[Volume], t: &crate::bus::Toucher| {
            vols.iter().any(|v| v.overlaps_box(t.min, t.max))
        };
        // Something already inside the mover does not block it: it could never get out.
        ml.touchers
            .iter()
            .find(|t| meets(&next, t) && !meets(&now, t))
            .map(|t| t.id)
    }

    /// The first player whose box is within one unit of the mover now.
    fn toucher(&self, ml: &MapLogic) -> Option<EntityId> {
        if self.local.is_empty() {
            return None;
        }
        let vols = self.volumes_now();
        ml.touchers
            .iter()
            .filter(|t| t.kind == crate::bus::ToucherKind::Player)
            .find(|t| {
                let (lo, hi) = (t.min.map(|v| v - 1.0), t.max.map(|v| v + 1.0));
                vols.iter().any(|v| v.overlaps_box(lo, hi))
            })
            .map(|t| t.id)
    }

    pub(crate) fn think(&mut self, ml: &mut MapLogic, me: EntityId) {
        if self.class == MoverClass::Brush {
            return;
        }
        // The volume tests this tick may make (touch and the two poses of the blocking test), paid first; a tick with no
        // work left moves nothing.
        let tests = (ml.touchers.len() as u64)
            .saturating_mul(self.local.len() as u64)
            .saturating_mul(3)
            .saturating_add(self.local.len() as u64);
        if !ml.has_work() || !ml.spend(tests) {
            return;
        }
        // Touch opens a door; touch presses a button with spawnflag 256.
        let touch_opens = match self.class {
            MoverClass::Door | MoverClass::RotatingDoor => self.flags & 1024 != 0,
            MoverClass::Button => self.flags & 256 != 0,
            _ => false,
        };
        if touch_opens
            && !self.locked
            && ml.now_us() >= self.touch_after
            && self.moving() <= 0
            && self.frac < 1.0
            && let Some(who) = self.toucher(ml)
        {
            if self.class == MoverClass::Button {
                self.press(ml, me, Some(who));
            } else {
                self.open(ml, me, Some(who));
            }
        }
        if let Some(t) = self.return_at
            && ml.now_us() >= t
            && self.moving() == 0
        {
            self.return_at = None;
            self.close(ml, me, Some(me));
        }
        let dir = self.moving();
        if dir == 0 {
            return;
        }
        let step = self.rate() * (TICK_US as f32 / 1e6);
        let next = if dir > 0 {
            (self.frac + step).min(self.target)
        } else {
            (self.frac - step).max(self.target)
        };
        if let Some(who) = self.blocker(ml, next) {
            if !self.blocked {
                self.blocked = true;
                let out = if dir > 0 {
                    "OnBlockedOpening"
                } else {
                    "OnBlockedClosing"
                };
                ml.fire(me, out, "", Some(who), Some(me));
                self.mark(ml);
            }
            let door = matches!(self.class, MoverClass::Door | MoverClass::RotatingDoor);
            if door && !self.force_closed {
                // Back the way it came, and no reopening by touch for a second.
                self.target = if dir > 0 { 0.0 } else { 1.0 };
                self.touch_after = ml.now_us() + 1_000_000;
                self.blocked = false;
                self.mark(ml);
            }
            return;
        }
        if self.blocked {
            self.blocked = false;
            let out = if dir > 0 {
                "OnUnblockedOpening"
            } else {
                "OnUnblockedClosing"
            };
            ml.fire(me, out, "", Some(me), Some(me));
            self.mark(ml);
        }
        self.frac = next;
        if self.frac == self.target {
            self.arrive(ml, me);
        }
    }

    fn open(&mut self, ml: &mut MapLogic, me: EntityId, activator: Option<EntityId>) {
        if self.frac >= 1.0 && self.moving() == 0 {
            // Already open: a door that closes by itself waits again from now.
            if self.return_at.is_some() {
                self.return_at = Some(ml.now_us() + (f64::from(self.wait.max(0.0)) * 1e6) as u64);
            }
            return;
        }
        if self.moving() > 0 {
            return;
        }
        self.go(ml, 1.0);
        if matches!(self.class, MoverClass::Door | MoverClass::RotatingDoor) {
            ml.fire(me, "OnOpen", "", activator, Some(me));
        }
    }

    fn close(&mut self, ml: &mut MapLogic, me: EntityId, activator: Option<EntityId>) {
        if self.frac <= 0.0 && self.moving() == 0 {
            return;
        }
        if self.moving() < 0 {
            return;
        }
        self.go(ml, 0.0);
        if matches!(self.class, MoverClass::Door | MoverClass::RotatingDoor) {
            ml.fire(me, "OnClose", "", activator, Some(me));
        }
    }

    fn press(&mut self, ml: &mut MapLogic, me: EntityId, activator: Option<EntityId>) {
        if self.locked {
            ml.fire(me, "OnUseLocked", "", activator, Some(me));
            return;
        }
        let toggle = self.flags & 32 != 0;
        if toggle {
            ml.fire(me, "OnPressed", "", activator, Some(me));
            if self.target >= 1.0 {
                self.go(ml, 0.0);
            } else {
                self.go(ml, 1.0);
            }
        } else {
            if self.moving() != 0 || self.frac > 0.0 {
                return;
            }
            ml.fire(me, "OnPressed", "", activator, Some(me));
            self.go(ml, 1.0);
        }
        if self.flags & 1 != 0 {
            // A button that does not move arrives at once.
            self.frac = self.target;
            self.arrive(ml, me);
        }
    }

    pub(crate) fn input(&mut self, ml: &mut MapLogic, ctx: Ctx, input: &str, value: &str) -> bool {
        let me = ctx.me;
        let act = ctx.activator;
        match (self.class, input) {
            (MoverClass::Brush, "enable") => {
                self.visible = true;
                self.solid = self.solid_when(true);
            }
            (MoverClass::Brush, "disable") => {
                self.visible = false;
                self.solid = self.solid_when(false);
            }
            (MoverClass::Brush, "toggle") => {
                self.visible = !self.visible;
                self.solid = self.solid_when(self.visible);
            }
            (_, "alpha" | "color") => {}
            (MoverClass::Brush, _) => return false,
            (MoverClass::Button, "press") => self.press(ml, me, act),
            (MoverClass::Button, "use") => {
                if self.flags & 1024 != 0 {
                    self.press(ml, me, act);
                }
            }
            (MoverClass::Button, "pressin") => {
                ml.fire(me, "OnPressed", "", act, Some(me));
                self.go(ml, 1.0);
            }
            (MoverClass::Button, "pressout") => self.go(ml, 0.0),
            (MoverClass::Linear, "open") => self.go(ml, 1.0),
            (MoverClass::Linear, "close") => self.go(ml, 0.0),
            (MoverClass::Linear, "setposition") => self.go(ml, atof32(value)),
            (_, "open") => self.open(ml, me, act),
            (_, "close") => self.close(ml, me, act),
            (_, "toggle") => {
                if self.target >= 1.0 {
                    self.close(ml, me, act);
                } else {
                    self.open(ml, me, act);
                }
            }
            (_, "use") => {
                if self.locked {
                    ml.fire(me, "OnLockedUse", "", act, Some(me));
                } else if self.flags & 256 != 0 {
                    if self.flags & 32 != 0 && self.target >= 1.0 {
                        self.close(ml, me, act);
                    } else {
                        self.open(ml, me, act);
                    }
                }
            }
            (_, "lock") => self.locked = true,
            (_, "unlock") => self.locked = false,
            (_, "setspeed") => {
                self.speed = atof32(value);
                self.mark(ml);
            }
            _ => return false,
        }
        true
    }
}

/// What a client knows of a mover: its brush model, the closed pose's origin, the open pose's offset and turn, and its motion
/// since a tick (the fraction `from` then, moving at `rate` a second toward `to`). A client derives the pose at any time
/// from these ([`MoverGhost::fraction_at`]), so a state change costs one update, not one per tick.
#[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
pub struct MoverGhost {
    #[net(bits = 16)]
    pub model: u32,
    pub origin: [f32; 3],
    pub offset: [f32; 3],
    pub turn: [f32; 3],
    pub from: f32,
    pub to: f32,
    pub rate: f32,
    pub since_tick: u32,
    pub visible: bool,
    pub solid: bool,
}

/// The largest coordinate or offset a received mover keeps (the map's units).
pub const MAX_GHOST_COORD: f32 = 65_536.0;
/// The largest turn a received mover keeps (degrees, each axis).
pub const MAX_GHOST_TURN: f32 = 3_600.0;
/// The largest rate a received mover keeps (fractions a second).
pub const MAX_GHOST_RATE: f32 = 1e6;

impl MoverGhost {
    /// The record held to what a client can use, as a client takes it from the wire: None when its brush model is not one of
    /// the map's `models`; every number finite (a NaN reads as 0) and clamped (the coordinates and offset to
    /// [`MAX_GHOST_COORD`], the turn to [`MAX_GHOST_TURN`], the rate to [`MAX_GHOST_RATE`], `from` and `to` to 0..1).
    pub fn sanitized(&self, models: u32) -> Option<MoverGhost> {
        if self.model == 0 || self.model >= models {
            return None;
        }
        let held = |v: f32, max: f32| if v.is_nan() { 0.0 } else { v.clamp(-max, max) };
        Some(MoverGhost {
            model: self.model,
            origin: self.origin.map(|v| held(v, MAX_GHOST_COORD)),
            offset: self.offset.map(|v| held(v, MAX_GHOST_COORD)),
            turn: self.turn.map(|v| held(v, MAX_GHOST_TURN)),
            from: held(self.from, 1.0).max(0.0),
            to: held(self.to, 1.0).max(0.0),
            rate: held(self.rate, MAX_GHOST_RATE),
            since_tick: self.since_tick,
            visible: self.visible,
            solid: self.solid,
        })
    }

    /// The fraction at `tick` (a fraction of a tick between two).
    pub fn fraction_at(&self, tick: f64) -> f32 {
        let dt = ((tick - f64::from(self.since_tick)) * TICK_US as f64 / 1e6).max(0.0) as f32;
        let f = self.from + self.rate * dt;
        let f = if self.rate > 0.0 {
            f.min(self.to)
        } else if self.rate < 0.0 {
            f.max(self.to)
        } else {
            self.from
        };
        if f.is_finite() {
            f.clamp(0.0, 1.0)
        } else {
            self.to.clamp(0.0, 1.0)
        }
    }

    /// The pose at `tick`.
    pub fn pose_at(&self, tick: f64) -> Pose {
        let f = self.fraction_at(tick);
        Pose {
            origin: [0, 1, 2].map(|k| self.origin[k] + self.offset[k] * f),
            angles: self.turn.map(|a| a * f),
        }
    }
}
