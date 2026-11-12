//! Input sources: devices a plugin adds that the client polls (once per drawn frame in a window or page, else once per client pass), and the joystick events they make. A source can stamp its events with the time they happened; the client then averages its axes over each poll's span.
//!
//! A source ([`InputSource`]) is polled with a context ([`InputCx`]) and says what its devices did: a button went down, an axis
//! moved, a hat turned ([`JoyEvent`]). The registry ([`InputSources`]) turns those into TGE's joystick events and the client host
//! routes them as it routes every engine event ([`input::route`](crate::input::route): the global action map, the canvas, the
//! other maps), so a script binds a pad's parts as it binds a DirectInput joystick's:
//!
//! | The source says | The event | A script binds it as |
//! |---|---|---|
//! | `button(0, 3, true)` | button 3 make (a break when `false`) | `bind(joystick0, button3, ...)` |
//! | `axis(0, JoyAxis::X, 0.5)` | a move of `xaxis` by 0.5 | `bind(joystick0, xaxis, ...)` |
//! | `pov(0, 0, 1, 0)` | `xpov` moves to 1, `rpov` makes | `bind(joystick0, rpov, ...)` |
//!
//! The device number is the joystick's: device 1 is `joystick1`. Axes are in -1..1 with Y increasing downward, as DirectInput's
//! and the mouse's are. The registry only passes changes on: an axis at the value it had, a button already down, a hat already
//! there make no event, and a device that goes away ([`InputCx::disconnect`]) lets go of everything it held (its buttons break,
//! its hats center, its axes return to 0), so a bound `break` or a held direction never sticks.
//!
//! A game or a plugin adds a source while the engine is built ([`InputBuilderExt::add_input_source`]); every client the engine
//! makes gets its own copy. A source has no script functions of its own. Nothing is polled, and nothing registered, in an engine
//! with no source.
//!
//! ```
//! use torque_client::input_source::{InputBuilderExt, InputCx, InputSource, InputSourceEntry, JoyAxis};
//! use torque_engine::{EngineBuilder, Plugin, PluginId};
//!
//! /// A pad that is always pushed halfway to the right.
//! #[derive(Clone)]
//! struct Push;
//!
//! impl InputSource for Push {
//!     fn poll(&mut self, cx: &mut InputCx<'_>) {
//!         cx.axis(0, JoyAxis::X, 0.5);
//!     }
//! }
//!
//! #[derive(PluginId)]
//! #[plugin(id = "example.push")]
//! struct PushPlugin;
//!
//! impl Plugin for PushPlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.add_input_source(Push);
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(PushPlugin).build()?;
//! assert_eq!(engine.list::<InputSourceEntry>().len(), 1);
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! # Timestamps
//!
//! A source that reads its devices faster than the client polls (on a thread of its own, say) stamps what it says with the time
//! it happened ([`InputCx::push_at`], [`InputCx::axis_at`]; or, from the thread, an [`InputQueue`] the source drains in `poll`).
//! The client then does not take the last sample of an axis: it gives the scripts the **time-weighted mean** of the axis over
//! the span since the last poll (a stick that sat at 0 for 5 ms and at 1 for 5 ms of a 10 ms frame reads 0.5). Buttons, hats and
//! disconnects keep the order they were said in, whatever their stamps. An event with no stamp ([`InputCx::axis`]) is the
//! device's value at the poll, as before; a source should not mix both for one axis.
//!
//! Credit: the joystick events and their names follow TGE's DirectInput devices and `ActionMap` as Torque3D (MIT) keeps them,
//! `platformWin32/winDInputDevice.cpp` and `sim/actionMap.cpp` (each Copyright (c) 2012 GarageGames, LLC). The code is this
//! crate's own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use gui::input::InputEvent;
use gui::keys::{JOYSTICK_DEVICE, SI_BREAK, SI_BUTTON, SI_MAKE, SI_MOVE, SI_POV};
use torque_engine::EngineBuilder;
use ts_vm::vm::Vm;
pub use web_time::Instant;

use crate::conn::ConnGame;
use crate::state::ClientState;

/// The most events a source may leave in one poll (the rest are dropped), so a source that runs away cannot stall a pass.
pub const MAX_POLL_EVENTS: usize = 4_096;

/// The most stamped events one poll takes (and an [`InputQueue`] holds); the rest are dropped.
pub const MAX_TIMED_EVENTS: usize = 65_536;

/// The most buttons a joystick has (`button0` to `button31`).
pub const MAX_BUTTONS: u16 = 32;

/// An axis of a joystick, by the name a script binds it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JoyAxis {
    /// `xaxis`.
    X,
    /// `yaxis`.
    Y,
    /// `zaxis`.
    Z,
    /// `rxaxis`.
    Rx,
    /// `ryaxis`.
    Ry,
    /// `rzaxis`.
    Rz,
    /// `slider`.
    Slider,
}

impl JoyAxis {
    /// The event's object type (`xaxis` is 0x20b).
    pub const fn code(self) -> u16 {
        match self {
            JoyAxis::X => 0x20b,
            JoyAxis::Y => 0x20c,
            JoyAxis::Z => 0x20d,
            JoyAxis::Rx => 0x20e,
            JoyAxis::Ry => 0x20f,
            JoyAxis::Rz => 0x210,
            JoyAxis::Slider => 0x211,
        }
    }
}

/// What a source says one of its devices did. The `device` is the joystick's number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JoyEvent {
    /// Button `button` (0 to 31) went down or up.
    Button {
        device: u32,
        button: u16,
        down: bool,
    },
    /// An axis is at `value` (-1 to 1; out of range is clamped, not a number is 0).
    Axis {
        device: u32,
        axis: JoyAxis,
        value: f32,
    },
    /// Hat `hat` (0 for `xpov` and its kin, 1 for the `...pov2` names) is at `x` (-1 left, 1 right) and `y` (-1 up, 1 down);
    /// other values are clamped to -1 and 1.
    Pov { device: u32, hat: u8, x: i8, y: i8 },
    /// The device is gone: everything it held is let go.
    Disconnected { device: u32 },
}

/// A device source the client polls once per drawn frame (once per client pass where nothing is drawn).
pub trait InputSource {
    /// One poll: say what the devices did since the last one ([`InputCx::button`], [`InputCx::axis`], [`InputCx::pov`],
    /// [`InputCx::disconnect`]). Should not block.
    fn poll(&mut self, cx: &mut InputCx<'_>);
}

/// An event a source said, and when it happened if the source knows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Said {
    pub event: JoyEvent,
    /// When it happened (monotonic); None: at the poll.
    pub at: Option<Instant>,
}

/// What a source is given to poll with: the script engine, to read a pref from, and the events to say.
pub struct InputCx<'a> {
    vm: &'a Vm,
    now: Instant,
    events: Vec<Said>,
}

impl<'a> InputCx<'a> {
    /// A context over `vm`, polling now.
    pub fn new(vm: &'a Vm) -> InputCx<'a> {
        InputCx::at(vm, Instant::now())
    }

    /// A context over `vm`, polling at `now`.
    pub fn at(vm: &'a Vm, now: Instant) -> InputCx<'a> {
        InputCx {
            vm,
            now,
            events: Vec::new(),
        }
    }

    /// The client's script engine, for the prefs a source reads (`vm.prefs::<MyPrefs>()`).
    pub fn vm(&self) -> &Vm {
        self.vm
    }

    /// When this poll began, by the clock the client stamps from.
    pub fn now(&self) -> Instant {
        self.now
    }

    fn put(&mut self, event: JoyEvent, at: Option<Instant>) {
        let cap = if at.is_some() {
            MAX_TIMED_EVENTS
        } else {
            MAX_POLL_EVENTS
        };
        if self.events.len() < cap {
            self.events.push(Said { event, at });
        }
    }

    /// Says an event.
    pub fn push(&mut self, event: JoyEvent) {
        self.put(event, None);
    }

    /// Says an event that happened at `at`. A source keeps each axis's stamps in order (a stamp earlier than the axis's last
    /// counts as the last).
    pub fn push_at(&mut self, at: Instant, event: JoyEvent) {
        self.put(event, Some(at));
    }

    /// Says everything `queue` holds, in the order it was pushed.
    pub fn drain(&mut self, queue: &InputQueue) {
        for said in queue.take() {
            self.put(said.event, said.at);
        }
    }

    /// Button `button` of joystick `device` is down or up.
    pub fn button(&mut self, device: u32, button: u16, down: bool) {
        self.push(JoyEvent::Button {
            device,
            button,
            down,
        });
    }

    /// Button `button` of joystick `device` went down or up at `at`.
    pub fn button_at(&mut self, at: Instant, device: u32, button: u16, down: bool) {
        self.push_at(
            at,
            JoyEvent::Button {
                device,
                button,
                down,
            },
        );
    }

    /// An axis of joystick `device` is at `value`.
    pub fn axis(&mut self, device: u32, axis: JoyAxis, value: f32) {
        self.push(JoyEvent::Axis {
            device,
            axis,
            value,
        });
    }

    /// An axis of joystick `device` moved to `value` at `at`.
    pub fn axis_at(&mut self, at: Instant, device: u32, axis: JoyAxis, value: f32) {
        self.push_at(
            at,
            JoyEvent::Axis {
                device,
                axis,
                value,
            },
        );
    }

    /// Hat `hat` of joystick `device` is at `x`, `y`.
    pub fn pov(&mut self, device: u32, hat: u8, x: i8, y: i8) {
        self.push(JoyEvent::Pov { device, hat, x, y });
    }

    /// Hat `hat` of joystick `device` turned to `x`, `y` at `at`.
    pub fn pov_at(&mut self, at: Instant, device: u32, hat: u8, x: i8, y: i8) {
        self.push_at(at, JoyEvent::Pov { device, hat, x, y });
    }

    /// Joystick `device` is gone.
    pub fn disconnect(&mut self, device: u32) {
        self.push(JoyEvent::Disconnected { device });
    }

    /// Joystick `device` went at `at`.
    pub fn disconnect_at(&mut self, at: Instant, device: u32) {
        self.push_at(at, JoyEvent::Disconnected { device });
    }

    /// The events said so far, without their stamps.
    pub fn into_events(self) -> Vec<JoyEvent> {
        self.events.into_iter().map(|said| said.event).collect()
    }

    /// The events said so far, with their stamps.
    pub fn into_said(self) -> Vec<Said> {
        self.events
    }
}

/// A queue of stamped events that a thread fills and a source drains in `poll` ([`InputCx::drain`]). Clones share the queue.
///
/// Stamp an event with a time read before it is pushed ([`Instant::now`]); the client reads its own clock after the drain, so no
/// stamp is ahead of it. A full queue ([`MAX_TIMED_EVENTS`]) drops new axis moves and keeps buttons, hats and disconnects.
#[derive(Clone, Default)]
pub struct InputQueue {
    inner: Arc<Mutex<Vec<Said>>>,
}

impl InputQueue {
    /// Pushes an event that happened at `at`.
    pub fn push_at(&self, at: Instant, event: JoyEvent) {
        let mut queue = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let room = if matches!(event, JoyEvent::Axis { .. }) {
            MAX_TIMED_EVENTS
        } else {
            2 * MAX_TIMED_EVENTS
        };
        if queue.len() < room {
            queue.push(Said {
                event,
                at: Some(at),
            });
        }
    }

    /// An axis moved to `value` at `at`.
    pub fn axis_at(&self, at: Instant, device: u32, axis: JoyAxis, value: f32) {
        self.push_at(
            at,
            JoyEvent::Axis {
                device,
                axis,
                value,
            },
        );
    }

    /// A button went down or up at `at`.
    pub fn button_at(&self, at: Instant, device: u32, button: u16, down: bool) {
        self.push_at(
            at,
            JoyEvent::Button {
                device,
                button,
                down,
            },
        );
    }

    /// How many events wait.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether nothing waits.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Takes everything that waits, oldest first.
    pub fn take(&self) -> Vec<Said> {
        std::mem::take(&mut *self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl fmt::Debug for InputQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputQueue")
            .field("events", &self.len())
            .finish()
    }
}

/// The sources a client polls, and what the devices held at the last poll.
#[derive(Default)]
pub struct InputSources {
    sources: Vec<Box<dyn InputSource>>,
    held: HashMap<u32, Held>,
    /// The axes said with stamps, and what each did since the last poll.
    averaged: HashMap<(u32, JoyAxis), Averaged>,
    /// Where polls read the time from (None: [`Instant::now`]).
    clock: Option<Rc<dyn Fn() -> Instant>>,
}

/// An axis said with stamps: its value now, and the area under it since `start`, to be given as a mean at the next poll.
struct Averaged {
    value: f32,
    /// The time up to which `area` is summed.
    cursor: Instant,
    /// Where the span being averaged began.
    start: Instant,
    /// The value times seconds, from `start` to `cursor`.
    area: f64,
    /// Whether no span of the axis has been given yet (the first span may be no longer than an instant).
    fresh: bool,
}

impl Averaged {
    /// The axis moved to `value` at `at` (a stamp before the last one counts as the last).
    fn moved(&mut self, at: Instant, value: f32) {
        let at = at.max(self.cursor);
        self.area += f64::from(self.value) * (at - self.cursor).as_secs_f64();
        self.cursor = at;
        self.value = value;
    }

    /// The mean of the axis over the span up to `now`; the next span starts there. A span of no time says nothing new (but for
    /// the axis's first, which says where it is).
    fn finish(&mut self, now: Instant) -> Option<f32> {
        self.moved(now, self.value);
        let span = (self.cursor - self.start).as_secs_f64();
        let mean = if span > 0.0 {
            Some((self.area / span) as f32)
        } else {
            self.fresh.then_some(self.value)
        };
        self.fresh = false;
        self.start = self.cursor;
        self.area = 0.0;
        mean
    }
}

/// What one joystick held.
#[derive(Default)]
struct Held {
    buttons: u32,
    axes: HashMap<JoyAxis, f32>,
    /// By hat: its `x` and `y`.
    hats: HashMap<u8, (i8, i8)>,
}

impl fmt::Debug for InputSources {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputSources")
            .field("sources", &self.sources.len())
            .field("devices", &self.held.len())
            .finish()
    }
}

/// The `xpov` and `ypov` object types of hat 0 and 1, then the four directions `upov`, `dpov`, `lpov`, `rpov`.
const HAT_CODES: [[u16; 6]; 2] = [
    [0x204, 0x205, 0x206, 0x207, 0x208, 0x209],
    [0x212, 0x213, 0x214, 0x215, 0x216, 0x217],
];

impl InputSources {
    /// Adds a source.
    pub fn add(&mut self, source: Box<dyn InputSource>) {
        self.sources.push(source);
    }

    /// Whether there is nothing to poll.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// How many sources there are.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Makes polls read the time from `clock` (a test's `torque_testkit::FakeClock::reader`).
    pub fn set_clock(&mut self, clock: impl Fn() -> Instant + 'static) {
        self.clock = Some(Rc::new(clock));
    }

    fn now(&self) -> Instant {
        self.clock
            .as_ref()
            .map_or_else(Instant::now, |clock| clock())
    }

    /// Polls every source in the order they were added: what they said, as the engine's events (see
    /// [`InputSources::translate_said`]).
    pub fn poll(&mut self, vm: &Vm, modifier: u8) -> Vec<InputEvent> {
        let began = self.now();
        let mut said = Vec::new();
        for source in &mut self.sources {
            let mut cx = InputCx::at(vm, began);
            source.poll(&mut cx);
            said.extend(cx.into_said());
        }
        // Read after the sources: a stamp taken before an event was pushed is never ahead of this.
        let now = self.now().max(began);
        self.translate_said(&said, now, modifier)
    }

    /// What `said` changed, as the joystick events the action maps take, with the modifier bits `modifier`. Every event is
    /// taken as said at the poll (no stamps).
    pub fn translate(&mut self, said: &[JoyEvent], modifier: u8) -> Vec<InputEvent> {
        let mut out = Vec::new();
        for &event in said {
            self.apply(event, modifier, &mut out);
        }
        out
    }

    /// What `said` changed in the span that ends at `now`: events with no stamp as [`InputSources::translate`] takes them;
    /// buttons, hats and disconnects in the order said; and each axis that was said with stamps as the mean of its value over
    /// the span since the last poll (an axis keeps its last value through a span in which it said nothing), after the rest.
    pub fn translate_said(&mut self, said: &[Said], now: Instant, modifier: u8) -> Vec<InputEvent> {
        let mut out = Vec::new();
        for &Said { event, at } in said {
            match (event, at) {
                (
                    JoyEvent::Axis {
                        device,
                        axis,
                        value,
                    },
                    Some(at),
                ) => {
                    let (at, value) = (at.min(now), clean(value));
                    self.averaged
                        .entry((device, axis))
                        .and_modify(|averaged| averaged.moved(at, value))
                        .or_insert(Averaged {
                            value,
                            cursor: at,
                            start: at,
                            area: 0.0,
                            fresh: true,
                        });
                }
                (JoyEvent::Disconnected { device }, _) => {
                    self.averaged.retain(|(d, _), _| *d != device);
                    self.apply(event, modifier, &mut out);
                }
                _ => self.apply(event, modifier, &mut out),
            }
        }
        let mut keys: Vec<_> = self.averaged.keys().copied().collect();
        keys.sort_by_key(|(device, axis)| (*device, axis.code()));
        for (device, axis) in keys {
            if let Some(averaged) = self.averaged.get_mut(&(device, axis)) {
                let Some(value) = averaged.finish(now) else {
                    continue;
                };
                self.apply(
                    JoyEvent::Axis {
                        device,
                        axis,
                        value,
                    },
                    modifier,
                    &mut out,
                );
            }
        }
        out
    }

    fn apply(&mut self, event: JoyEvent, modifier: u8, out: &mut Vec<InputEvent>) {
        match event {
            JoyEvent::Button {
                device,
                button,
                down,
            } => {
                if button >= MAX_BUTTONS {
                    return;
                }
                let held = self.held.entry(device).or_default();
                let bit = 1u32 << button;
                if (held.buttons & bit != 0) == down {
                    return;
                }
                held.buttons ^= bit;
                out.push(button_event(device, button, down, modifier));
            }
            JoyEvent::Axis {
                device,
                axis,
                value,
            } => {
                let value = clean(value);
                let held = self.held.entry(device).or_default();
                let last = held.axes.insert(axis, value).unwrap_or(0.0);
                if last != value {
                    out.push(axis_event(device, axis.code(), value, modifier));
                }
            }
            JoyEvent::Pov { device, hat, x, y } => {
                if hat > 1 {
                    return;
                }
                let (x, y) = (x.clamp(-1, 1), y.clamp(-1, 1));
                let held = self.held.entry(device).or_default();
                let last = held.hats.insert(hat, (x, y)).unwrap_or((0, 0));
                hat_events(device, hat, last, (x, y), modifier, out);
            }
            JoyEvent::Disconnected { device } => {
                if let Some(held) = self.held.remove(&device) {
                    release(device, &held, modifier, out);
                }
            }
        }
    }
}

/// An axis value in -1..1 (not a number is 0).
fn clean(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}

fn button_event(device: u32, button: u16, down: bool, modifier: u8) -> InputEvent {
    InputEvent {
        device: JOYSTICK_DEVICE,
        device_inst: device,
        obj_type: SI_BUTTON,
        obj_inst: 0x100 + button,
        action: if down { SI_MAKE } else { SI_BREAK },
        modifier,
        ascii: 0,
        value: if down { 1.0 } else { 0.0 },
    }
}

fn axis_event(device: u32, code: u16, value: f32, modifier: u8) -> InputEvent {
    InputEvent {
        device: JOYSTICK_DEVICE,
        device_inst: device,
        obj_type: code,
        action: SI_MOVE,
        modifier,
        value,
        ..InputEvent::default()
    }
}

/// A hat turned from `from` to `to`: each direction that ended breaks, its `xpov` and `ypov` move to the new values, and each
/// direction that began makes.
fn hat_events(
    device: u32,
    hat: u8,
    from: (i8, i8),
    to: (i8, i8),
    modifier: u8,
    out: &mut Vec<InputEvent>,
) {
    let codes = HAT_CODES[usize::from(hat)];
    let direction = |code: u16, down: bool| InputEvent {
        device: JOYSTICK_DEVICE,
        device_inst: device,
        obj_type: SI_POV,
        obj_inst: code,
        action: if down { SI_MAKE } else { SI_BREAK },
        modifier,
        ascii: 0,
        value: if down { 1.0 } else { 0.0 },
    };
    // Up, down, left, right.
    let of = |(x, y): (i8, i8)| [y < 0, y > 0, x < 0, x > 0];
    let (was, now) = (of(from), of(to));
    for (i, code) in codes[2..].iter().enumerate() {
        if was[i] && !now[i] {
            out.push(direction(*code, false));
        }
    }
    if from.0 != to.0 {
        out.push(axis_event(device, codes[0], f32::from(to.0), modifier));
    }
    if from.1 != to.1 {
        out.push(axis_event(device, codes[1], f32::from(to.1), modifier));
    }
    for (i, code) in codes[2..].iter().enumerate() {
        if !was[i] && now[i] {
            out.push(direction(*code, true));
        }
    }
}

/// Everything `held` of joystick `device` let go: the buttons break, the hats center, the axes move back to 0.
fn release(device: u32, held: &Held, modifier: u8, out: &mut Vec<InputEvent>) {
    for button in 0..MAX_BUTTONS {
        if held.buttons & (1 << button) != 0 {
            out.push(button_event(device, button, false, modifier));
        }
    }
    let mut hats: Vec<_> = held.hats.iter().collect();
    hats.sort_by_key(|(hat, _)| **hat);
    for (&hat, &at) in hats {
        hat_events(device, hat, at, (0, 0), modifier, out);
    }
    let mut axes: Vec<_> = held.axes.iter().filter(|(_, v)| **v != 0.0).collect();
    axes.sort_by_key(|(axis, _)| axis.code());
    for (axis, _) in axes {
        out.push(axis_event(device, axis.code(), 0.0, modifier));
    }
}

/// Polls the client's sources and gives back the events they made (`modifier`: the platform layer's modifier bits). The
/// sources are polled with no borrow of the client held. An empty list costs one borrow.
pub fn poll<G: ConnGame>(
    vm: &Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    modifier: u8,
) -> Vec<InputEvent> {
    if client.borrow().input_sources.is_empty() {
        return Vec::new();
    }
    let mut sources = std::mem::take(&mut client.borrow_mut().input_sources);
    let events = sources.poll(vm, modifier);
    client.borrow_mut().input_sources = sources;
    events
}

/// A source a plugin added: how to make one for each client.
#[derive(Clone)]
pub struct InputSourceEntry {
    make: Rc<dyn Fn() -> Box<dyn InputSource>>,
}

impl InputSourceEntry {
    /// An entry that makes a source with `make` for each client.
    pub fn new<S: InputSource + 'static>(make: impl Fn() -> S + 'static) -> InputSourceEntry {
        InputSourceEntry {
            make: Rc::new(move || Box::new(make())),
        }
    }

    /// A new source.
    pub fn make(&self) -> Box<dyn InputSource> {
        (self.make)()
    }
}

impl fmt::Debug for InputSourceEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputSourceEntry").finish_non_exhaustive()
    }
}

/// Adds input sources to an engine being built.
pub trait InputBuilderExt {
    /// Adds a source: each client the engine makes polls its own copy of `source` once per drawn frame (once per client pass where nothing is
    /// drawn), in the order the sources were added.
    fn add_input_source<S: InputSource + Clone + 'static>(&mut self, source: S) -> &mut Self;

    /// Adds a source made by `make`, called once for each client: for a source that cannot be copied, or that opens its
    /// devices when it is made.
    fn add_input_source_with<S: InputSource + 'static>(
        &mut self,
        make: impl Fn() -> S + 'static,
    ) -> &mut Self;
}

impl InputBuilderExt for EngineBuilder {
    fn add_input_source<S: InputSource + Clone + 'static>(&mut self, source: S) -> &mut Self {
        self.add_input_source_with(move || source.clone())
    }

    fn add_input_source_with<S: InputSource + 'static>(
        &mut self,
        make: impl Fn() -> S + 'static,
    ) -> &mut Self {
        self.push(InputSourceEntry::new(make))
    }
}

/// Gives `state` a copy of each source the engine's plugins added.
pub fn install<G: ConnGame>(state: &mut ClientState<G>, entries: &[InputSourceEntry]) {
    for entry in entries {
        state.input_sources.add(entry.make());
    }
}

/// A source driven by a test: whoever holds a copy says what the devices do, and the next poll passes it on.
///
/// ```
/// use torque_client::input_source::{FakeInput, InputSources, JoyAxis};
///
/// let fake = FakeInput::default();
/// let mut sources = InputSources::default();
/// sources.add(Box::new(fake.clone()));
///
/// fake.axis(0, JoyAxis::X, 0.5);
/// fake.button(1, 3, true);
/// let vm = ts_vm::vm::Vm::new();
/// let events = sources.poll(&vm, 0);
/// assert_eq!(events.len(), 2);
/// assert_eq!((events[0].device_inst, events[0].value), (0, 0.5));
/// assert_eq!((events[1].device_inst, events[1].obj_inst), (1, 0x103));
/// // Nothing new to say: no events.
/// assert!(sources.poll(&vm, 0).is_empty());
/// ```
///
/// Events stamped with the times of a `torque_testkit::FakeClock` (a dev-dependency) never wait on the real time: the registry
/// reads its time from the clock ([`InputSources::set_clock`]).
///
/// ```
/// use std::time::Duration;
/// use torque_client::input_source::{FakeInput, InputSources, JoyAxis};
/// use torque_testkit::FakeClock;
///
/// let (clock, fake) = (FakeClock::default(), FakeInput::default());
/// let mut sources = InputSources::default();
/// sources.add(Box::new(fake.clone()));
/// sources.set_clock(clock.reader());
/// let vm = ts_vm::vm::Vm::new();
///
/// // A stick at 0 for the first 5 ms of a 10 ms frame, and at 1 for the rest.
/// fake.axis_at(clock.at(0), 0, JoyAxis::X, 0.0);
/// sources.poll(&vm, 0);
/// fake.axis_at(clock.at(5), 0, JoyAxis::X, 1.0);
/// clock.advance(Duration::from_millis(10));
/// let events = sources.poll(&vm, 0);
/// assert_eq!(events[0].value, 0.5);
/// ```
#[derive(Clone, Default)]
pub struct FakeInput {
    queue: Rc<RefCell<Vec<Said>>>,
}

impl FakeInput {
    /// Says an event at the next poll.
    pub fn push(&self, event: JoyEvent) {
        self.queue.borrow_mut().push(Said { event, at: None });
    }

    /// Says an event that happened at `at` (a time of a `torque_testkit::FakeClock`) at the next poll.
    pub fn push_at(&self, at: Instant, event: JoyEvent) {
        self.queue.borrow_mut().push(Said {
            event,
            at: Some(at),
        });
    }

    /// Button `button` of joystick `device` goes down or up at the next poll.
    pub fn button(&self, device: u32, button: u16, down: bool) {
        self.push(JoyEvent::Button {
            device,
            button,
            down,
        });
    }

    /// An axis of joystick `device` moves to `value` at the next poll.
    pub fn axis(&self, device: u32, axis: JoyAxis, value: f32) {
        self.push(JoyEvent::Axis {
            device,
            axis,
            value,
        });
    }

    /// An axis of joystick `device` moved to `value` at `at`; the next poll takes it.
    pub fn axis_at(&self, at: Instant, device: u32, axis: JoyAxis, value: f32) {
        self.push_at(
            at,
            JoyEvent::Axis {
                device,
                axis,
                value,
            },
        );
    }

    /// Button `button` of joystick `device` went down or up at `at`; the next poll takes it.
    pub fn button_at(&self, at: Instant, device: u32, button: u16, down: bool) {
        self.push_at(
            at,
            JoyEvent::Button {
                device,
                button,
                down,
            },
        );
    }

    /// A hat of joystick `device` turns at the next poll.
    pub fn pov(&self, device: u32, hat: u8, x: i8, y: i8) {
        self.push(JoyEvent::Pov { device, hat, x, y });
    }

    /// Joystick `device` is unplugged at the next poll.
    pub fn disconnect(&self, device: u32) {
        self.push(JoyEvent::Disconnected { device });
    }
}

impl InputSource for FakeInput {
    fn poll(&mut self, cx: &mut InputCx<'_>) {
        for said in self.queue.borrow_mut().drain(..) {
            cx.put(said.event, said.at);
        }
    }
}

#[cfg(test)]
#[path = "input_source_tests.rs"]
mod tests;
