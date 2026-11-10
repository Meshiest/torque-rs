//! The input source: the pads a backend reports, as joysticks.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use torque_client::input_source::{InputCx, InputSource, Instant, JoyAxis};
use torque_engine::{Prefs, VmPrefs};

use crate::pad::{PadBackend, PadButton, PadState};
#[cfg(target_arch = "wasm32")]
use crate::sampler::Sampler;
use crate::sampler::{PadSample, PadSamples};

/// The most pads the source gives a joystick number to (`joystick0` to `joystick15`); more are left out until one is unplugged.
pub const MAX_PADS: usize = 16;

/// The button number the left trigger makes when it is pressed in half way (`button11`), and the right one (`button12`).
pub const LEFT_TRIGGER_BUTTON: u16 = 11;
pub const RIGHT_TRIGGER_BUTTON: u16 = 12;

/// How far a trigger is pressed in before its button is down.
const TRIGGER_PRESSED: f32 = 0.5;

/// The plugin's preferences: `$Pref::Gamepad::DeadZone` and `$Pref::Gamepad::InvertY`. A script, or the `prefs.cs` a game runs,
/// sets them; the source reads them at every poll, so a change takes hold at once.
#[derive(Prefs, Debug, Clone, Copy, PartialEq)]
#[prefs(prefix = "$Pref::Gamepad")]
pub struct GamepadPrefs {
    /// How far a stick must be pushed before it counts, as a share of its travel. Inside it the stick reads 0; past it the stick
    /// is spread over the rest of its travel, so it still reaches 1 at the edge.
    #[pref(default = 0.15, range = 0.0..=0.9)]
    pub dead_zone: f32,
    /// Swaps up and down on both sticks (`yaxis` and `ryaxis`).
    #[pref(default = false)]
    pub invert_y: bool,
    /// How many times a second the pads are read on the thread that reads them on its own (30 to 2000). The client takes the
    /// mean of each stick over a frame, so a higher rate follows a fast hand more closely. Native only, and read at every poll.
    #[pref(default = 500, range = 30..=2000)]
    pub poll_rate: i32,
    /// In a browser: also read the pads from a timer about every 4 ms (the browser may stretch that, and stops timers of a
    /// hidden page), so the sticks are averaged over a frame instead of read once in it. Off by default: it keeps the page
    /// busier. Native builds ignore it.
    #[pref(default = false)]
    pub web_fast_poll: bool,
}

/// A stick axis `value` with a dead zone `dead_zone` taken out of the middle and the rest spread over the full range.
pub fn apply_dead_zone(value: f32, dead_zone: f32) -> f32 {
    let size = value.abs();
    if size <= dead_zone {
        0.0
    } else {
        value.signum() * ((size - dead_zone) / (1.0 - dead_zone)).min(1.0)
    }
}

/// Where a source gets its pads from.
enum Feed {
    /// The backend, read at each poll; what each pad holds is said whole, with no stamps.
    Polled(Box<dyn PadBackend>),
    /// Samples a thread (or a timer) left stamped, drained at each poll.
    Sampled(Sampled),
}

/// Starts a reader of pads into a queue at a rate; what it returns stops the reader when it is dropped.
pub type StartReader = Box<dyn FnOnce(&PadSamples, &Arc<AtomicU32>) -> Box<dyn Any>>;

/// A feed of stamped samples.
struct Sampled {
    samples: PadSamples,
    /// How many reads a second the reader makes (set from the pref at each poll).
    rate: Arc<AtomicU32>,
    /// Starts the reader at the first poll, so an engine that never polls opens no device.
    start: Option<StartReader>,
    /// The reader, until the source ends: dropping it stops it.
    reader: Option<Box<dyn Any>>,
    /// What each pad (by its backend id) held at the last sample drained.
    last: BTreeMap<u32, PadState>,
    /// The prefs the held states were last said with.
    said_prefs: Option<GamepadPrefs>,
    /// In a browser: the sampler a timer shares with the poll, which reads it once more at each poll.
    #[cfg(target_arch = "wasm32")]
    local: Option<std::rc::Rc<std::cell::RefCell<Sampler>>>,
}

/// The input source over a backend's pads.
///
/// Each pad takes the lowest `joystick<N>` that is free when it is first seen and keeps it while it is plugged in; an unplugged
/// pad lets go of everything it held and frees its number for the next pad. Each poll says what every pad holds now; the client
/// passes on only what changed. A source made [`GamepadSource::sampled`] is fed stamped samples instead, and says only what
/// changed between them: the client then gives each stick as its mean over the span between polls.
///
/// | The pad | The joystick |
/// |---|---|
/// | left stick | `xaxis`, `yaxis` |
/// | right stick | `rxaxis`, `ryaxis` |
/// | left and right triggers | `zaxis`, `rzaxis` (0 to 1), and `button11`, `button12` when pressed in half way |
/// | face buttons: South, East, West, North | `button0` to `button3` |
/// | shoulders, Select, Start, sticks pressed, the logo button | `button4` to `button10` |
/// | the cross | the first hat: `xpov`, `ypov`, `upov`, `dpov`, `lpov`, `rpov` |
pub struct GamepadSource {
    feed: Feed,
    /// By joystick number: the backend's id of the pad that has it.
    slots: Vec<Option<u32>>,
}

impl GamepadSource {
    /// A source over `backend`, read at each poll.
    pub fn new(backend: Box<dyn PadBackend>) -> GamepadSource {
        GamepadSource {
            feed: Feed::Polled(backend),
            slots: Vec::new(),
        }
    }

    /// A source over stamped samples. `start` is called at the first poll with the queue to fill and the reads a second (the
    /// source keeps that at the [`GamepadPrefs::poll_rate`] pref); what it returns is the reader, dropped when the source is,
    /// so it stops the thread or timer it started. `start` is `None` for samples a test pushes itself.
    pub fn sampled(samples: PadSamples, start: Option<StartReader>) -> GamepadSource {
        GamepadSource {
            feed: Feed::Sampled(Sampled {
                samples,
                rate: Arc::new(AtomicU32::new(500)),
                start,
                reader: None,
                last: BTreeMap::new(),
                said_prefs: None,
                #[cfg(target_arch = "wasm32")]
                local: None,
            }),
            slots: Vec::new(),
        }
    }

    /// The joystick number of the pad with backend id `id`, once it has been seen.
    pub fn joystick_of(&self, id: u32) -> Option<u32> {
        self.slots
            .iter()
            .position(|slot| *slot == Some(id))
            .map(|n| n as u32)
    }

    /// The joystick number for pad `id`: its own, or the lowest free (None past [`MAX_PADS`]).
    fn assign(&mut self, id: u32) -> Option<u32> {
        if let Some(joystick) = self.joystick_of(id) {
            return Some(joystick);
        }
        if let Some(free) = self.slots.iter().position(Option::is_none) {
            self.slots[free] = Some(id);
            return Some(free as u32);
        }
        if self.slots.len() < MAX_PADS {
            self.slots.push(Some(id));
            return Some(self.slots.len() as u32 - 1);
        }
        None
    }

    fn poll_backend(&mut self, cx: &mut InputCx<'_>, prefs: &GamepadPrefs) {
        let Feed::Polled(backend) = &mut self.feed else {
            return;
        };
        let pads = backend.poll();
        // Pads that went: they let go, and their numbers are free for the pads that came.
        for (joystick, slot) in self.slots.iter_mut().enumerate() {
            if let Some(id) = *slot
                && !pads.iter().any(|(pad, _)| *pad == id)
            {
                cx.disconnect(joystick as u32);
                *slot = None;
            }
        }
        for &(id, _) in &pads {
            self.assign(id);
        }
        for (id, state) in &pads {
            if let Some(joystick) = self.joystick_of(*id) {
                say(cx, None, joystick, None, state, prefs);
            }
        }
    }

    /// In a browser, `$Pref::Gamepad::WebFastPoll` moves the pads between a read at each poll and a timer that also reads them.
    #[cfg(target_arch = "wasm32")]
    fn switch_web_timer(&mut self, prefs: &GamepadPrefs) {
        use std::cell::RefCell;
        use std::rc::Rc;

        /// Stands in for the backend while it moves.
        struct NoPads;
        impl PadBackend for NoPads {
            fn poll(&mut self) -> Vec<(u32, PadState)> {
                Vec::new()
            }
        }

        let timer_on = matches!(&self.feed, Feed::Sampled(s) if s.local.is_some());
        match (&self.feed, prefs.web_fast_poll, timer_on) {
            (Feed::Polled(_), true, _) => {
                let Feed::Polled(backend) =
                    std::mem::replace(&mut self.feed, Feed::Polled(Box::new(NoPads)))
                else {
                    return;
                };
                let samples = PadSamples::default();
                let sampler = Rc::new(RefCell::new(Sampler::new(backend, samples.clone())));
                let timer_sampler = Rc::clone(&sampler);
                let start: StartReader = Box::new(move |_, _| {
                    Box::new(crate::web_timer::WebTimer::start(timer_sampler))
                });
                let mut sampled = Sampled {
                    samples,
                    rate: Arc::new(AtomicU32::new(0)),
                    start: Some(start),
                    reader: None,
                    last: BTreeMap::new(),
                    said_prefs: None,
                    local: Some(sampler),
                };
                // The pads already said stay said: the first samples are compared with nothing, so they say it all again.
                sampled.said_prefs = Some(*prefs);
                self.feed = Feed::Sampled(sampled);
            }
            (Feed::Sampled(_), false, true) => {
                let Feed::Sampled(mut sampled) =
                    std::mem::replace(&mut self.feed, Feed::Polled(Box::new(NoPads)))
                else {
                    return;
                };
                // Dropping the timer drops its hold on the sampler.
                sampled.reader = None;
                if let Some(local) = sampled.local.take()
                    && let Ok(cell) = Rc::try_unwrap(local)
                {
                    self.feed = Feed::Polled(cell.into_inner().into_backend());
                }
            }
            _ => {}
        }
    }

    fn poll_samples(&mut self, cx: &mut InputCx<'_>, prefs: &GamepadPrefs) {
        let Feed::Sampled(sampled) = &mut self.feed else {
            return;
        };
        #[cfg(target_arch = "wasm32")]
        if let Some(local) = &sampled.local {
            local.borrow_mut().sample();
        }
        sampled
            .rate
            .store(prefs.poll_rate.max(1) as u32, Ordering::Relaxed);
        if let Some(start) = sampled.start.take() {
            sampled.reader = Some(start(&sampled.samples, &sampled.rate));
        }
        // A changed pref reshapes what the pads hold now: say it all again, from the start of this poll.
        let held: Vec<(u32, PadState)> = if sampled.said_prefs.is_some_and(|p| p != *prefs) {
            sampled.last.iter().map(|(id, s)| (*id, *s)).collect()
        } else {
            Vec::new()
        };
        sampled.said_prefs = Some(*prefs);
        let samples = sampled.samples.take();
        let began = cx.now();
        for (id, state) in held {
            if let Some(joystick) = self.joystick_of(id) {
                say(cx, Some(began), joystick, None, &state, prefs);
            }
        }
        for sample in samples {
            match sample {
                PadSample::State { at, id, state } => {
                    let Some(joystick) = self.assign(id) else {
                        continue;
                    };
                    let Feed::Sampled(sampled) = &mut self.feed else {
                        return;
                    };
                    let before = sampled.last.insert(id, state);
                    say(cx, Some(at), joystick, before.as_ref(), &state, prefs);
                }
                PadSample::Gone { at, id } => {
                    let Feed::Sampled(sampled) = &mut self.feed else {
                        return;
                    };
                    sampled.last.remove(&id);
                    if let Some(joystick) = self.joystick_of(id) {
                        cx.disconnect_at(at, joystick);
                        self.slots[joystick as usize] = None;
                    }
                }
            }
        }
    }
}

impl InputSource for GamepadSource {
    fn poll(&mut self, cx: &mut InputCx<'_>) {
        let prefs: GamepadPrefs = cx.vm().prefs();
        #[cfg(target_arch = "wasm32")]
        self.switch_web_timer(&prefs);
        match self.feed {
            Feed::Polled(_) => self.poll_backend(cx, &prefs),
            Feed::Sampled(_) => self.poll_samples(cx, &prefs),
        }
    }
}

/// Says what one pad holds, at `at` (None: at the poll). With `before`, only what differs from it.
fn say(
    cx: &mut InputCx<'_>,
    at: Option<Instant>,
    joystick: u32,
    before: Option<&PadState>,
    state: &PadState,
    prefs: &GamepadPrefs,
) {
    const BUTTONS: [PadButton; 11] = [
        PadButton::South,
        PadButton::East,
        PadButton::West,
        PadButton::North,
        PadButton::LeftShoulder,
        PadButton::RightShoulder,
        PadButton::Select,
        PadButton::Start,
        PadButton::LeftStick,
        PadButton::RightStick,
        PadButton::Guide,
    ];
    let button = |cx: &mut InputCx<'_>, number: u16, down: bool| match at {
        Some(at) => cx.button_at(at, joystick, number, down),
        None => cx.button(joystick, number, down),
    };
    let axis = |cx: &mut InputCx<'_>, axis: JoyAxis, value: f32| match at {
        Some(at) => cx.axis_at(at, joystick, axis, value),
        None => cx.axis(joystick, axis, value),
    };
    for pad_button in BUTTONS {
        if let Some(number) = pad_button.joystick_button() {
            let down = state.pressed(pad_button);
            if before.is_none_or(|b| b.pressed(pad_button) != down) {
                button(cx, number, down);
            }
        }
    }
    for (number, value, was) in [
        (
            LEFT_TRIGGER_BUTTON,
            state.triggers.0,
            before.map(|b| b.triggers.0),
        ),
        (
            RIGHT_TRIGGER_BUTTON,
            state.triggers.1,
            before.map(|b| b.triggers.1),
        ),
    ] {
        let down = value >= TRIGGER_PRESSED;
        if was.is_none_or(|w| (w >= TRIGGER_PRESSED) != down) {
            button(cx, number, down);
        }
    }
    let (x, y) = state.cross();
    if before.is_none_or(|b| b.cross() != (x, y)) {
        match at {
            Some(at) => cx.pov_at(at, joystick, 0, x, y),
            None => cx.pov(joystick, 0, x, y),
        }
    }
    let flip = if prefs.invert_y { -1.0 } else { 1.0 };
    let stick = |value: f32| apply_dead_zone(value, prefs.dead_zone);
    for (name, value, was, scale) in [
        (JoyAxis::X, state.left.0, before.map(|b| b.left.0), 1.0),
        (JoyAxis::Y, state.left.1, before.map(|b| b.left.1), flip),
        (JoyAxis::Rx, state.right.0, before.map(|b| b.right.0), 1.0),
        (JoyAxis::Ry, state.right.1, before.map(|b| b.right.1), flip),
    ] {
        if was.is_none_or(|w| w != value) {
            axis(cx, name, scale * stick(value));
        }
    }
    for (name, value, was) in [
        (JoyAxis::Z, state.triggers.0, before.map(|b| b.triggers.0)),
        (JoyAxis::Rz, state.triggers.1, before.map(|b| b.triggers.1)),
    ] {
        if was.is_none_or(|w| w != value) {
            axis(cx, name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dead_zone_takes_the_middle_out_and_spreads_the_rest() {
        assert_eq!(apply_dead_zone(0.1, 0.2), 0.0);
        assert_eq!(apply_dead_zone(-0.2, 0.2), 0.0);
        assert_eq!(apply_dead_zone(1.0, 0.2), 1.0);
        assert_eq!(apply_dead_zone(-1.0, 0.2), -1.0);
        let half = apply_dead_zone(0.6, 0.2);
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        assert!((apply_dead_zone(-0.6, 0.2) + 0.5).abs() < 1e-6);
        // No dead zone leaves a stick as it is.
        assert_eq!(apply_dead_zone(0.3, 0.0), 0.3);
    }
}
