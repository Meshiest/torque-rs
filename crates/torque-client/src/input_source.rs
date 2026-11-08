//! Input sources: devices a plugin adds that the client polls once per client pass, and the joystick events they make.
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
//! Credit: the joystick events and their names follow TGE's DirectInput devices and `ActionMap` as Torque3D (MIT) keeps them,
//! `platformWin32/winDInputDevice.cpp` and `sim/actionMap.cpp` (each Copyright (c) 2012 GarageGames, LLC). The code is this
//! crate's own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use gui::input::InputEvent;
use gui::keys::{JOYSTICK_DEVICE, SI_BREAK, SI_BUTTON, SI_MAKE, SI_MOVE, SI_POV};
use torque_engine::EngineBuilder;
use ts_vm::vm::Vm;

use crate::conn::ConnGame;
use crate::state::ClientState;

/// The most events a source may leave in one poll (the rest are dropped), so a source that runs away cannot stall a pass.
pub const MAX_POLL_EVENTS: usize = 4_096;

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

/// A device source the client polls once per client pass.
pub trait InputSource {
    /// One poll: say what the devices did since the last one ([`InputCx::button`], [`InputCx::axis`], [`InputCx::pov`],
    /// [`InputCx::disconnect`]). Should not block.
    fn poll(&mut self, cx: &mut InputCx<'_>);
}

/// What a source is given to poll with: the script engine, to read a pref from, and the events to say.
pub struct InputCx<'a> {
    vm: &'a Vm,
    events: Vec<JoyEvent>,
}

impl<'a> InputCx<'a> {
    /// A context over `vm`.
    pub fn new(vm: &'a Vm) -> InputCx<'a> {
        InputCx {
            vm,
            events: Vec::new(),
        }
    }

    /// The client's script engine, for the prefs a source reads (`vm.prefs::<MyPrefs>()`).
    pub fn vm(&self) -> &Vm {
        self.vm
    }

    /// Says an event.
    pub fn push(&mut self, event: JoyEvent) {
        if self.events.len() < MAX_POLL_EVENTS {
            self.events.push(event);
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

    /// An axis of joystick `device` is at `value`.
    pub fn axis(&mut self, device: u32, axis: JoyAxis, value: f32) {
        self.push(JoyEvent::Axis {
            device,
            axis,
            value,
        });
    }

    /// Hat `hat` of joystick `device` is at `x`, `y`.
    pub fn pov(&mut self, device: u32, hat: u8, x: i8, y: i8) {
        self.push(JoyEvent::Pov { device, hat, x, y });
    }

    /// Joystick `device` is gone.
    pub fn disconnect(&mut self, device: u32) {
        self.push(JoyEvent::Disconnected { device });
    }

    /// The events said so far.
    pub fn into_events(self) -> Vec<JoyEvent> {
        self.events
    }
}

/// The sources a client polls, and what the devices held at the last poll.
#[derive(Default)]
pub struct InputSources {
    sources: Vec<Box<dyn InputSource>>,
    held: HashMap<u32, Held>,
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

    /// Polls every source in the order they were added: what they said, as the engine's events (see
    /// [`InputSources::translate`]).
    pub fn poll(&mut self, vm: &Vm, modifier: u8) -> Vec<InputEvent> {
        let mut said = Vec::new();
        for source in &mut self.sources {
            let mut cx = InputCx::new(vm);
            source.poll(&mut cx);
            said.extend(cx.into_events());
        }
        self.translate(&said, modifier)
    }

    /// What `said` changed, as the joystick events the action maps take, with the modifier bits `modifier`.
    pub fn translate(&mut self, said: &[JoyEvent], modifier: u8) -> Vec<InputEvent> {
        let mut out = Vec::new();
        for &event in said {
            match event {
                JoyEvent::Button {
                    device,
                    button,
                    down,
                } => {
                    if button >= MAX_BUTTONS {
                        continue;
                    }
                    let held = self.held.entry(device).or_default();
                    let bit = 1u32 << button;
                    if (held.buttons & bit != 0) == down {
                        continue;
                    }
                    held.buttons ^= bit;
                    out.push(button_event(device, button, down, modifier));
                }
                JoyEvent::Axis {
                    device,
                    axis,
                    value,
                } => {
                    let value = if value.is_nan() {
                        0.0
                    } else {
                        value.clamp(-1.0, 1.0)
                    };
                    let held = self.held.entry(device).or_default();
                    let last = held.axes.insert(axis, value).unwrap_or(0.0);
                    if last != value {
                        out.push(axis_event(device, axis.code(), value, modifier));
                    }
                }
                JoyEvent::Pov { device, hat, x, y } => {
                    if hat > 1 {
                        continue;
                    }
                    let (x, y) = (x.clamp(-1, 1), y.clamp(-1, 1));
                    let held = self.held.entry(device).or_default();
                    let last = held.hats.insert(hat, (x, y)).unwrap_or((0, 0));
                    hat_events(device, hat, last, (x, y), modifier, &mut out);
                }
                JoyEvent::Disconnected { device } => {
                    if let Some(held) = self.held.remove(&device) {
                        release(device, &held, modifier, &mut out);
                    }
                }
            }
        }
        out
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
    /// Adds a source: each client the engine makes polls its own copy of `source` once per client pass, in the order the sources
    /// were added.
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
#[derive(Clone, Default)]
pub struct FakeInput {
    queue: Rc<RefCell<Vec<JoyEvent>>>,
}

impl FakeInput {
    /// Says an event at the next poll.
    pub fn push(&self, event: JoyEvent) {
        self.queue.borrow_mut().push(event);
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
        for event in self.queue.borrow_mut().drain(..) {
            cx.push(event);
        }
    }
}

#[cfg(test)]
#[path = "input_source_tests.rs"]
mod tests;
