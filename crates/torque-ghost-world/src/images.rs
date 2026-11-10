//! The mounted images a ShapeBase ghost carries, as TGE's client keeps them.
//!
//! **What travels** (`ShapeBase::unpackUpdate`'s image block): per slot the datablock, the skin's net string, the five flags (wet,
//! ammo, loaded, target, triggerDown, in that wire order), the fire count (3 bits) and, on a ghost's first record only, whether the
//! image is firing. The client writes the five flags into the slot, then calls `setImage` when the datablock or the skin changed,
//! with `(loaded, ammo, triggerDown)` (`target` false); `setImage` on the image the slot already has only reskins it. On a ghost not
//! yet added (its first record) it takes the fire count and, when the image is firing, enters the fire state (`setImageState`,
//! forced); on an added ghost a new fire count enters the fire state (forced), then `updateImageState(slot, 0)` runs. The fire count
//! is the slot's, not the image's: `setImage` never writes it, so an image mounted after any shot in that slot arrives with the count
//! the client already holds and enters its first state, not the fire state. An added ghost's empty slot still takes the count. The
//! caller keeps the four counts across mounts ([`apply`]'s `counts`).
//!
//! **The client's machine** (`setImageState` and `updateImageState` on a ghost): the states are the ones the datablock's `packData`
//! sent, and the fire state is the first state with `stateFire`. Entering a state checks the loaded, ammo, target, wet and trigger
//! transitions in that order and goes on at once on the first that is not "none"; else the delay is the state's timeout,
//! `stateLoaded` sets or clears `loaded`, the state's sound plays, its emitter starts, the spin thread's speed follows
//! `stateSpinThread`, and a zero delay follows the timeout transition. Entering the state the slot is in only restarts its timer. A
//! client never enters the fire state through a transition (only forced). Each tick (`ShapeBase::processTick`, every ghost,
//! `TickSec`) the delay falls and the same five transitions, then the timeout, are checked (all only once the delay ran out when the
//! state waits for its timeout).
//!
//! **What the client draws of it** (the renderer's): a state's `stateSequence` travels as the shape's sequence index
//! (`writeSignedInt(16)`: a sign flag, then 15 bits of magnitude; a record keeps the 16 bits as they came, so the index is decoded
//! here). The image's animation thread exists when a state names a sequence (`isAnimated`); `setImage` makes it on sequence 0 at its
//! start, speed 0, and each state that names a sequence puts it there (`setImageState`, for a state without a flash sequence): at
//! position 0 forward (`stateDirection`) or 1 backward, at speed 1 or, with `stateScaleAnimation` and a timeout, the sequence's
//! duration over the timeout, negative backward; a state without one leaves the thread as it is. `updateImageAnimation` advances it
//! every frame. The other threads a client image has are the shape's `ambient` sequence (played from the mount at speed 1) and its
//! `spin` sequence (made by `setImage`), whose speed the state sets (none 0, full 1, spin up `1 - delay / timeout`, spin down
//! `delay / timeout`, ignore unchanged). So the machine here keeps the state, the delay, the animation thread's sequence, speed and
//! time, and the time each of the other two threads has run. Not drawn: a flash state's visibility thread (`stateFlashSequence`).
//!
//! **Recoil** (`onImageRecoil`): a state with `stateRecoil` (light, medium, heavy) counts a recoil on the slot ([`ImageRecoil`]); the
//! drawer restarts its owner's recoil thread on each new one (a Player's `light_recoil`, `medium_recoil` or `heavy_recoil`).
//!
//! **Not modeled** (nothing the frame shows): the state's eject shell, the client's energy drain of a
//! `usesEnergy` image, the light of a firing image, and the reset of a cyclic animation sequence to its first frame on a state
//! change (the world loads no shapes).
//!
//! **A game's rules**: where a game's client runs the machine otherwise, its [`super::set::ClientSet::image_rules`] says so
//! ([`ImageRules`]; [`TgeImageRules`] is TGE's).
//!
//! **Engine approximations and caps**: the client world does not load shapes, so the spin thread is taken to exist when any state
//! of the datablock sets `stateSpinThread` (TGE makes it only for a shape with a `spin` sequence; the difference is the delay a
//! spin-up/down state takes over from the other, which TGE adjusts only with the thread). An image datablock the client does not
//! have empties the slot (TGE drops the connection: "Invalid packet (mounted images)"). The ticks run on the world's clock (the
//! feed's datagram times) from the mount, `TickMs` apart, on the stored slots each time the clock moves (`ClientWorld::set_clock`; a
//! read runs nothing). Each clock step may run [`TICK_BUDGET`] ticks across every slot of every ghost, starting after the ghost the
//! last step reached; a slot the budget does not reach is deferred to a later step, one a record is about to change skips what the
//! budget leaves; a gap of more than [`MAX_CATCH_UP_TICKS`] skips its oldest ticks (a skip runs the threads on at their last speed
//! and not the machine). The world counts what was run, skipped and deferred (`TickStats`). A state chain of one entry stops after
//! [`MAX_STATE_CHAIN`] transitions (TGE recurses without end on a cycle of zero timeouts; a hostile datablock's).
//!
//! Credit: the machine is TGE's, as Torque3D (MIT) carries it in `T3D/shapeImage.cpp` (Copyright (c) 2012 GarageGames, LLC) and
//! `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.).

use ghosts::{NetStr, Value};

/// A mounted image slot as an update sent it: the datablock, the skin, the five flags (wet, ammo, loaded, target, triggerDown, in
/// wire order), the fire count (3 bits) and, on the first update, whether the image is firing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageSlot {
    pub datablock: Option<u32>,
    pub skin: NetStr,
    pub flags: [bool; 5],
    pub fire_count: u32,
    pub firing: bool,
}

/// `ShapeBase::MaxMountedImages`.
pub const IMAGE_SLOTS: usize = 4;
/// States per `ShapeBaseImageData` (`MaxStates`).
pub const MAX_STATES: usize = 31;
/// The client's tick: `TickMs` and `TickSec` (`TickMs / 1000.0f`).
pub const TICK_MS: u32 = 32;
pub const TICK_SEC: f32 = TICK_MS as f32 / 1000.0;
/// Engine cap: ticks run in one catch-up.
pub const MAX_CATCH_UP_TICKS: u32 = 1 << 12;
/// Engine cap: transitions of one `setImageState`.
pub const MAX_STATE_CHAIN: u32 = 64;

/// How a game's client runs its mounted images where it differs from TGE's (every answer TGE's by default). A game installs its
/// rules through [`super::set::ClientSet::image_rules`].
pub trait ImageRules: Send + Sync {
    /// Entering the state the slot is in (a timeout or a transition back into itself) also starts the state's emitter again.
    /// TGE only restarts the state's timer.
    fn same_state_emitter(&self) -> bool {
        false
    }

    /// Every state an entry settles in asks for its emitter ([`ImageEvent::Emitter`]), a state without one too. TGE starts an
    /// emitter only for a state that names one.
    fn emitter_without_datablock(&self) -> bool {
        false
    }

    /// A new skin on the image the slot already has mounts it again (state 0). TGE's `setImage` only reskins it.
    fn remount_on_skin(&self) -> bool {
        false
    }
}

/// TGE's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TgeImageRules;

impl ImageRules for TgeImageRules {}

/// `stateSpinThread`.
const SPIN_IGNORE: u32 = 0;
const SPIN_NONE: u32 = 1;
const SPIN_UP: u32 = 2;
const SPIN_DOWN: u32 = 3;
const SPIN_FULL: u32 = 4;

/// One state as the client unpacked it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateData {
    /// The transitions in wire order (NotLoaded, Loaded, NoAmmo, Ammo, NoTarget, Target, NotWet, Wet, TriggerUp, TriggerDown,
    /// Timeout): a state index or -1.
    pub trans: [i32; 11],
    pub timeout: f32,
    pub wait_for_timeout: bool,
    pub fire: bool,
    /// `stateLoaded`: 0 leave, 1 set, else clear.
    pub loaded_flag: u32,
    pub spin: u32,
    /// `stateSequence`: the shape's sequence index, or -1.
    pub sequence: i32,
    /// `stateDirection` (forward), `stateScaleAnimation`, `stateFlashSequence`.
    pub direction: bool,
    pub scale_animation: bool,
    pub flash: bool,
    /// `stateSound` (an AudioProfile's id) and `stateEmitter` (a ParticleEmitterData's id), `stateEmitterTime` (seconds) and
    /// `stateEmitterNode` (the image shape's node index the server resolved; -1 for the muzzle node).
    pub sound: Option<u32>,
    pub emitter: Option<u32>,
    pub emitter_time: f32,
    pub emitter_node: i32,
    /// `stateRecoil`: 0 none, 1 light, 2 medium, 3 heavy.
    pub recoil: u32,
}

impl Default for StateData {
    /// The constructor's state (an unused slot): no transitions, no timeout.
    fn default() -> StateData {
        StateData {
            trans: [-1; 11],
            timeout: 0.0,
            wait_for_timeout: true,
            fire: false,
            loaded_flag: 0,
            spin: SPIN_IGNORE,
            sequence: -1,
            direction: true,
            scale_animation: false,
            flash: false,
            sound: None,
            emitter: None,
            emitter_time: 0.0,
            emitter_node: -1,
            recoil: 0,
        }
    }
}

/// The datablock's states and the fire state (`fireState`, -1 when none).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageStates {
    pub states: Vec<StateData>,
    pub fire_state: i32,
    /// Engine: whether the spin thread is taken to exist (see the module docs).
    pub spins: bool,
}

/// `BitStream::readSignedInt(16)` of the 16 bits a record keeps: a sign flag, then 15 bits of magnitude.
fn signed16(raw: i64) -> i32 {
    let raw = raw as u32 & 0xffff;
    let magnitude = (raw >> 1) as i32;
    if raw & 1 != 0 { -magnitude } else { magnitude }
}

impl ImageStates {
    /// The states of an image datablock's fields (name, element, value), read in one pass over them (a record has a few hundred;
    /// the first value of a name and element counts). Elements past the 31 states are ignored.
    pub fn of_fields<'a>(
        fields: impl IntoIterator<Item = (&'a str, u32, &'a Value)>,
    ) -> ImageStates {
        let mut states = vec![StateData::default(); MAX_STATES];
        let mut used = [false; MAX_STATES];
        // Which (state, field) were set already: the first value wins.
        let mut seen = vec![[false; 26]; MAX_STATES];
        let int = |v: &Value| match v {
            Value::Int(i) => *i,
            _ => 0,
        };
        let flag = |v: &Value| matches!(v, Value::Bool(true));
        let id = |v: &Value| match v {
            Value::Id(id) => *id,
            _ => None,
        };
        for (name, index, value) in fields {
            let (i, field) = match name {
                "stateTrans" => ((index / 11) as usize, (index % 11) as usize),
                "stateUsed" => (index as usize, 11),
                "stateTimeoutValue" => (index as usize, 12),
                "stateWaitForTimeout" => (index as usize, 13),
                "stateFire" => (index as usize, 14),
                "stateLoadedFlag" => (index as usize, 15),
                "stateSpinThread" => (index as usize, 16),
                "stateSequence" => (index as usize, 17),
                "stateDirection" => (index as usize, 18),
                "stateScaleAnimation" => (index as usize, 19),
                "stateFlashSequence" => (index as usize, 20),
                "stateSound" => (index as usize, 21),
                "stateEmitter" => (index as usize, 22),
                "stateEmitterTime" => (index as usize, 23),
                "stateEmitterNode" => (index as usize, 24),
                "stateRecoil" => (index as usize, 25),
                _ => continue,
            };
            let (Some(st), Some(seen)) = (states.get_mut(i), seen.get_mut(i)) else {
                continue;
            };
            if std::mem::replace(&mut seen[field], true) {
                continue;
            }
            match field {
                // The state index plus one in 5 bits: 0 is "none".
                0..=10 => st.trans[field] = (int(value) & 0x1f) as i32 - 1,
                11 => used[i] = flag(value),
                12 => {
                    st.timeout = match *value {
                        Value::F32(x) => x,
                        _ => 0.0,
                    }
                }
                13 => st.wait_for_timeout = flag(value),
                14 => st.fire = flag(value),
                15 => st.loaded_flag = (int(value) & 7) as u32,
                16 => st.spin = (int(value) & 7) as u32,
                // The record keeps the wire's 16 bits (`readSignedInt(16)`): bit 0 the sign, the magnitude above it.
                17 => st.sequence = signed16(int(value)).max(-1),
                18 => st.direction = flag(value),
                19 => st.scale_animation = flag(value),
                20 => st.flash = flag(value),
                21 => st.sound = id(value),
                22 => st.emitter = id(value),
                23 => {
                    st.emitter_time = match *value {
                        Value::F32(x) => x,
                        _ => 0.0,
                    }
                }
                24 => st.emitter_node = int(value) as u32 as i32,
                _ => st.recoil = (int(value) & 3) as u32,
            }
        }
        // A slot not sent keeps the constructor's state.
        for (s, u) in states.iter_mut().zip(used) {
            if !u {
                *s = StateData::default();
            }
        }
        let fire_state = states.iter().position(|s| s.fire).map_or(-1, |i| i as i32);
        let spins = states.iter().any(|s| s.spin != SPIN_IGNORE);
        ImageStates {
            states,
            fire_state,
            spins,
        }
    }

    fn state(&self, i: usize) -> StateData {
        self.states.get(i).copied().unwrap_or_default()
    }
}

/// An image's animation thread as the last state that named a sequence left it (`setImageState`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageAnim {
    /// The shape's sequence index as sent.
    pub sequence: i32,
    /// `stateDirection`: from position 0 forward, else from 1 backward.
    pub forward: bool,
    /// With `stateScaleAnimation` and a timeout: the timeout the sequence's duration is spread over (speed `duration / timeout`).
    pub over: Option<f32>,
    /// Seconds the thread has run since the state set it.
    pub time: f32,
    /// The thread `setImage` made and no state has moved yet: sequence 0 at its start, speed 0.
    pub still: bool,
    /// How many times a state has set the thread since the mount (0: `setImage`'s): a drawer that plays the thread by its own
    /// frame time starts again from [`ImageAnim::time`] when this changes.
    pub entered: u32,
}

impl ImageAnim {
    /// The thread's speed for a sequence of `duration` seconds (negative backward).
    pub fn time_scale(&self, duration: f32) -> f32 {
        if self.still {
            return 0.0;
        }
        let s = match self.over {
            Some(t) => duration / t,
            None => 1.0,
        };
        if self.forward { s } else { -s }
    }

    /// The position the state put it at.
    pub fn start(&self) -> f32 {
        if self.forward { 0.0 } else { 1.0 }
    }
}

/// The recoils a slot's states asked for (`onImageRecoil`): the last one's kind (1 light, 2 medium, 3 heavy) and how many
/// there were since the mount (a drawer restarts its owner's recoil thread when the count changes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRecoil {
    pub kind: u32,
    pub count: u32,
}

/// One mounted image on the client.
#[derive(Debug, Clone, PartialEq)]
pub struct MountedImage {
    pub datablock: u32,
    pub skin: NetStr,
    pub wet: bool,
    pub ammo: bool,
    pub loaded: bool,
    pub target: bool,
    pub trigger: bool,
    pub fire_count: u32,
    /// The current state.
    pub state: usize,
    /// The time left in the state (seconds).
    pub delay: f32,
    /// The world clock (ms) the slot has been run to; `None` when the world has no clock.
    pub at_ms: Option<u32>,
    /// Seconds the `ambient` thread has run (speed 1 from the mount).
    pub ambient_time: f32,
    /// The `spin` thread's speed and the sequence time it has run (seconds at speed 1).
    pub spin_scale: f32,
    pub spin_time: f32,
    /// The animation thread (`isAnimated`): the sequence the last state that named one put it on, and the time since.
    pub anim: Option<ImageAnim>,
    /// The recoils its states asked for (none yet: `None`).
    pub recoil: Option<ImageRecoil>,
    /// How many times an engine cap ended a chain or a catch-up.
    pub cap_hits: u32,
    /// What the states entered since the world last took them asked the client to do (sounds, emitters), at most
    /// [`MAX_SLOT_EVENTS`] (the rest counted in `cap_hits`).
    pub events: Vec<ImageEvent>,
}

/// Engine cap: the events one slot keeps between two takes.
pub const MAX_SLOT_EVENTS: usize = 64;

/// What entering a state asks of the client (`setImageState` on a ghost).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageEvent {
    /// The slot's image is gone or replaced (`resetImageSlot`: `alxStop` of the slot's sound, looping or not, and its emitters
    /// deleted when empty).
    Unmount,
    /// The slot's looping state sound stops (every entry first stops a sound whose source loops: `alxStop` on the slot's handle).
    StopLoop,
    /// The state's sound starts at the object's render transform (`alxPlay`; the handle kept, and moved with the object by
    /// `updateImageAnimation`).
    Sound { profile: u32 },
    /// The state's emitter (`startImageEmitter`): the slot's emitter on that datablock and node keeps the longer of its time and the
    /// state's, else a free or the shortest of the slot's `MaxImageEmitters` takes a new one. `None` (a dead one) only where a
    /// game's [`ImageRules::emitter_without_datablock`] asks for it.
    Emitter {
        datablock: Option<u32>,
        time: f32,
        node: i32,
    },
}

/// A ghost's four slots.
pub type Slots = [Option<MountedImage>; IMAGE_SLOTS];

impl MountedImage {
    /// `setImage` on a client: a fresh slot on `datablock` entering state 0.
    fn set_image(
        rules: &dyn ImageRules,
        states: &ImageStates,
        sent: &ImageSlot,
        datablock: u32,
        now: Option<u32>,
    ) -> MountedImage {
        let [wet, ammo, loaded, _target, trigger] = sent.flags;
        let mut m = MountedImage {
            datablock,
            skin: sent.skin.clone(),
            wet,
            ammo,
            loaded,
            target: false,
            trigger,
            fire_count: 0,
            state: 0,
            delay: 0.0,
            at_ms: now,
            ambient_time: 0.0,
            spin_scale: 1.0,
            spin_time: 0.0,
            anim: None,
            recoil: None,
            cap_hits: 0,
            events: Vec::new(),
        };
        // An animated image's thread (`isAnimated`: a state names a sequence) is made here (`setImage` on a client:
        // `addThread`, which puts it on sequence 0 at its start, then speed 0), before state 0 is entered: a state that names no
        // sequence leaves the image posed at sequence 0's first frame, not at the shape's bare node pose.
        if states.states.iter().any(|s| s.sequence != -1) {
            m.anim = Some(ImageAnim {
                sequence: 0,
                forward: true,
                over: None,
                time: 0.0,
                still: true,
                entered: 0,
            });
        }
        // The slot's current state is none yet: entering 0 always runs.
        m.state = usize::MAX;
        m.set_state(rules, states, 0, true);
        m
    }

    fn event(&mut self, e: ImageEvent) {
        // Repeated stops between two takes are one.
        if e == ImageEvent::StopLoop && self.events.last() == Some(&ImageEvent::StopLoop) {
            return;
        }
        if self.events.len() >= MAX_SLOT_EVENTS {
            self.cap_hits += 1;
            return;
        }
        self.events.push(e);
    }

    /// `setImageState(slot, new, force)` on a ghost. Returns the chain's steps (the states it went through), which the
    /// caller charges to the clock step's budget past the first.
    fn set_state(
        &mut self,
        rules: &dyn ImageRules,
        states: &ImageStates,
        new: usize,
        force: bool,
    ) -> u32 {
        let mut new = new;
        let mut force = force;
        for step in 1..=MAX_STATE_CHAIN {
            // A client enters the fire state only when forced.
            if !force && new as i32 == states.fire_state && self.state != new {
                return step;
            }
            // The same state again: its timer restarts (and, by a game's rule, its emitter runs on).
            if !force && self.state == new {
                let s = states.state(new);
                self.delay = s.timeout;
                if rules.same_state_emitter() {
                    self.emitter(rules, &s);
                }
                return step;
            }
            let last = (self.state != usize::MAX).then(|| states.state(self.state));
            let last_delay = self.delay;
            self.state = new;
            self.event(ImageEvent::StopLoop);
            let s = states.state(new);
            let flag = |b: bool| usize::from(b);
            let next = [
                s.trans[flag(self.loaded)],
                s.trans[2 + flag(self.ammo)],
                s.trans[4 + flag(self.target)],
                s.trans[6 + flag(self.wet)],
                s.trans[8 + flag(self.trigger)],
            ]
            .into_iter()
            .find(|t| *t != -1);
            if let Some(t) = next {
                new = t as usize;
                force = false;
                continue;
            }
            self.delay = s.timeout;
            if let Some(profile) = s.sound {
                self.event(ImageEvent::Sound { profile });
            }
            self.emitter(rules, &s);
            if s.recoil != 0 {
                let count = self.recoil.map_or(1, |r| r.count.wrapping_add(1));
                self.recoil = Some(ImageRecoil {
                    kind: s.recoil,
                    count,
                });
            }
            // The animation thread (`isAnimated`; the flash path, a `_vis` thread, is not drawn).
            if s.sequence != -1 && !s.flash {
                let entered = self.anim.map_or(1, |a| a.entered.wrapping_add(1));
                self.anim = Some(ImageAnim {
                    sequence: s.sequence,
                    forward: s.direction,
                    over: (s.scale_animation && s.timeout != 0.0).then_some(s.timeout),
                    time: 0.0,
                    still: false,
                    entered,
                });
            }
            if s.loaded_flag != 0 {
                self.loaded = s.loaded_flag == 1;
            }
            if states.spins {
                let last_spin = last.map_or(SPIN_IGNORE, |l| l.spin);
                match s.spin {
                    SPIN_NONE => self.spin_scale = 0.0,
                    SPIN_FULL => self.spin_scale = 1.0,
                    SPIN_UP if last_spin == SPIN_DOWN => {
                        self.delay *= 1.0 - last_delay / s.timeout;
                    }
                    SPIN_DOWN if last_spin == SPIN_UP => {
                        self.delay *= 1.0 - last_delay / s.timeout;
                    }
                    _ => {}
                }
            }
            // `!delayTime` is false for any delay but zero: a NaN waits too.
            if self.delay != 0.0 || s.trans[10] == -1 {
                return step;
            }
            new = s.trans[10] as usize;
            force = false;
        }
        self.cap_hits += 1;
        MAX_STATE_CHAIN
    }

    /// The state's emitter (`startImageEmitter`), when it names one or the game's rule asks for every state's.
    fn emitter(&mut self, rules: &dyn ImageRules, s: &StateData) {
        if s.emitter.is_some() || rules.emitter_without_datablock() {
            self.event(ImageEvent::Emitter {
                datablock: s.emitter,
                time: s.emitter_time,
                node: s.emitter_node,
            });
        }
    }

    /// `updateImageState(slot, dt)` on a ghost. Returns the steps of the state chain it ran (0 when it ran none).
    fn update(&mut self, rules: &dyn ImageRules, states: &ImageStates, dt: f32) -> u32 {
        if self.state == usize::MAX {
            return 0;
        }
        self.delay -= dt;
        let s = states.state(self.state);
        let delay = self.delay;
        if delay <= 0.0 || !s.wait_for_timeout {
            let flag = |b: bool| usize::from(b);
            let next = [
                s.trans[flag(self.loaded)],
                s.trans[2 + flag(self.ammo)],
                s.trans[4 + flag(self.target)],
                s.trans[6 + flag(self.wet)],
                s.trans[8 + flag(self.trigger)],
            ]
            .into_iter()
            .find(|t| *t != -1)
            .or_else(|| (delay <= 0.0 && s.trans[10] != -1).then_some(s.trans[10]));
            if let Some(t) = next {
                return self.set_state(rules, states, t as usize, false);
            }
        }
        if states.spins {
            match s.spin {
                SPIN_UP => self.spin_scale = 1.0 - delay / s.timeout,
                SPIN_DOWN => self.spin_scale = delay / s.timeout,
                _ => {}
            }
        }
        0
    }

    /// Runs the ticks up to the world clock `now` (and the threads with them).
    /// Moves time on by `ticks` without running the machine: the threads run on at their present speed (an engine cap's skip).
    fn jump(&mut self, ticks: u32) {
        let rest = ticks as f32 * TICK_SEC;
        self.ambient_time += rest;
        self.spin_time += rest * self.spin_scale;
        if let Some(a) = self.anim.as_mut() {
            a.time += rest;
        }
    }

    /// Runs the ticks up to the world clock `now` (and the threads with them), drawing on the shared `budget`. A gap of more than
    /// [`MAX_CATCH_UP_TICKS`] skips its oldest ticks; ticks past the budget are deferred to a later call (`at_ms` stays behind) or,
    /// with `force` (a record about to apply to the slot), skipped.
    fn advance_to(
        &mut self,
        rules: &dyn ImageRules,
        states: &ImageStates,
        now: Option<u32>,
        budget: &mut u32,
        force: bool,
        stats: &mut TickStats,
    ) {
        let (Some(at), Some(now)) = (self.at_ms, now) else {
            if self.at_ms.is_none() {
                self.at_ms = now;
            }
            return;
        };
        let Some(gap) = now.checked_sub(at) else {
            return;
        };
        let ticks = gap / TICK_MS;
        let mut at = at;
        let mut left = ticks;
        if left > MAX_CATCH_UP_TICKS {
            let skip = left - MAX_CATCH_UP_TICKS;
            self.jump(skip);
            self.cap_hits += 1;
            stats.skipped += u64::from(skip);
            at += skip * TICK_MS;
            left = MAX_CATCH_UP_TICKS;
        }
        // A tick costs one unit, and a state chain it runs one more a state past the first, so a datablock of states that
        // point at each other with no timeout cannot run past the budget.
        let mut run = 0;
        while run < left && *budget > 0 {
            *budget -= 1;
            run += 1;
            let extra = self.update(rules, states, TICK_SEC).saturating_sub(1);
            *budget = budget.saturating_sub(extra);
            stats.chain += u64::from(extra);
            self.ambient_time += TICK_SEC;
            self.spin_time += TICK_SEC * self.spin_scale;
            if let Some(a) = self.anim.as_mut() {
                a.time += TICK_SEC;
            }
        }
        stats.run += u64::from(run);
        at += run * TICK_MS;
        let behind = left - run;
        if behind > 0 {
            if force {
                self.jump(behind);
                self.cap_hits += 1;
                stats.skipped += u64::from(behind);
                at += behind * TICK_MS;
            } else {
                stats.deferred += u64::from(behind);
            }
        }
        self.at_ms = Some(at);
        for t in [&mut self.ambient_time, &mut self.spin_time] {
            if !t.is_finite() {
                *t = 0.0;
            }
        }
        if let Some(a) = self.anim.as_mut()
            && !a.time.is_finite()
        {
            a.time = 0.0;
        }
    }
}

/// The image ticks a world has run, skipped (engine caps: a gap past [`MAX_CATCH_UP_TICKS`], or a record forcing a slot the budget
/// left behind) and deferred to a later clock step (the per-step budget [`TICK_BUDGET`] ran out).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickStats {
    pub run: u64,
    pub skipped: u64,
    pub deferred: u64,
    /// The state-chain steps past the first that the ticks run were charged for.
    pub chain: u64,
}

/// Engine cap: the image ticks one step of the world's clock may run, across every slot of every ghost (a clock step is a datagram
/// time; the records of that datagram draw on what is left).
pub const TICK_BUDGET: u32 = 4096;

/// Every slot of `images` run towards the clock `now` on `budget`, starting at the ghost after `cursor` (so a budget that runs out
/// does not leave the same ghosts behind every step). `cursor` is the last ghost reached.
pub fn advance_all(
    rules: &dyn ImageRules,
    images: &mut std::collections::BTreeMap<u32, Slots>,
    cursor: &mut u32,
    states: &dyn Fn(u32) -> Option<ImageStates>,
    now: Option<u32>,
    budget: &mut u32,
    stats: &mut TickStats,
) {
    let start = *cursor;
    let order: Vec<u32> = images
        .range(start.saturating_add(1)..)
        .map(|(k, _)| *k)
        .chain(images.range(..=start).map(|(k, _)| *k))
        .collect();
    for index in order {
        let Some(slots) = images.get_mut(&index) else {
            continue;
        };
        for m in slots.iter_mut().flatten() {
            if let Some(st) = states(m.datablock) {
                m.advance_to(rules, &st, now, budget, false, stats);
            }
        }
        if *budget == 0 {
            // The rest stays behind: counted when a later step reaches it.
            *cursor = index;
            return;
        }
        *cursor = index;
    }
}

/// The image block of one record applied to a ghost's slots (`unpackUpdate`'s loop): `added` is false for the ghost's creating
/// record. `states` answers a datablock id with its states (`None`: not an image datablock the client has, which TGE refuses as an
/// invalid packet; the slot is cleared here). `counts` are the slots' fire counts, kept while a slot is empty or changes image (TGE
/// keeps them in the slot, which `setImage` does not touch).
#[allow(clippy::too_many_arguments)]
pub fn apply(
    rules: &dyn ImageRules,
    slots: &mut Slots,
    counts: &mut [u32; IMAGE_SLOTS],
    sent: &[Option<ImageSlot>; IMAGE_SLOTS],
    added: bool,
    now: Option<u32>,
    states: &dyn Fn(u32) -> Option<ImageStates>,
    budget: &mut u32,
    stats: &mut TickStats,
    gone: &mut Vec<(usize, ImageEvent)>,
) {
    for (i, (slot, sent)) in slots.iter_mut().zip(sent).enumerate() {
        let Some(sent) = sent else {
            continue;
        };
        let count = sent.fire_count & 7;
        let db_states = sent.datablock.and_then(|d| Some((d, states(d)?)));
        // Run the ticks up to this record first.
        if let Some(m) = slot.as_mut()
            && let Some(s) = states(m.datablock)
        {
            m.advance_to(rules, &s, now, budget, true, stats);
        }
        let Some((db, st)) = db_states else {
            if let Some(old) = slot.take() {
                gone.extend(old.events.into_iter().map(|e| (i, e)));
                gone.push((i, ImageEvent::Unmount));
            }
            // An added ghost's empty slot takes the count too (the forced fire state then has no image to enter).
            if added {
                counts[i] = count;
            }
            continue;
        };
        let changed = match &*slot {
            Some(m) => m.datablock != db || (m.skin != sent.skin && rules.remount_on_skin()),
            None => true,
        };
        if let Some(m) = slot.as_mut() {
            [m.wet, m.ammo, m.loaded, m.target, m.trigger] = sent.flags;
            // `setImage` on the image the slot has: a reskin.
            m.skin.clone_from(&sent.skin);
        }
        if changed {
            if let Some(old) = slot.take() {
                gone.extend(old.events.into_iter().map(|e| (i, e)));
                gone.push((i, ImageEvent::Unmount));
            }
            let mut m = MountedImage::set_image(rules, &st, sent, db, now);
            // The slot's count, not a new image's 0: else every image mounted after a shot would enter its fire state.
            m.fire_count = counts[i];
            *slot = Some(m);
        }
        let Some(m) = slot.as_mut() else {
            continue;
        };
        let fire = st.fire_state.max(0) as usize;
        let mut steps = 0;
        if !added {
            m.fire_count = count;
            if sent.firing {
                steps += m.set_state(rules, &st, fire, true).saturating_sub(1);
            }
        } else {
            if count != m.fire_count {
                m.fire_count = count;
                steps += m.set_state(rules, &st, fire, true).saturating_sub(1);
            }
            steps += m.update(rules, &st, 0.0).saturating_sub(1);
        }
        *budget = budget.saturating_sub(steps);
        stats.chain += u64::from(steps);
        counts[i] = m.fire_count;
    }
}

#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
