//! A gamepad plugin for torque-rs: the pads plugged in, as the `joystick<N>` devices a script binds.
//!
//! [`GamepadPlugin`] adds an input source ([`torque_client::input_source`]) that reads the pads connected now and says what they
//! hold: the sticks as axes, the triggers as axes and buttons, the face and shoulder buttons as `button0` to `button10`, the cross
//! as a hat. Natively the pads come from [gilrs](https://crates.io/crates/gilrs) (XInput and Windows.Gaming.Input, evdev, IOKit);
//! on the web, from the browser's Gamepad API (`navigator.getGamepads()`), so the same scripts run in both. Pads can be plugged
//! in and out while the game runs: each takes the lowest free `joystick<N>` and keeps it, and one that goes lets go of every
//! button and stick it held.
//!
//! It is not in [`DefaultPlugins`](https://docs.rs/torque-app): a game adds it.
//!
//! ```
//! use torque_engine::{EngineBuilder, Prefs};
//! use torque_plugin_gamepad::{GamepadPlugin, GamepadPrefs};
//!
//! // In the game's `main`, beside the default plugins:
//! let engine = EngineBuilder::new().add_plugins(GamepadPlugin::default()).build()?;
//! // The plugin's prefs have their defaults in every client's script engine, and a script (or `prefs.cs`) changes them.
//! let prefs = GamepadPrefs::defaults();
//! assert_eq!((prefs.dead_zone, prefs.invert_y), (0.15, false));
//! # let _ = engine;
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! The game's scripts bind the pad as they would a DirectInput joystick:
//!
//! ```text
//! PlayMap.bind(joystick0, xaxis, "S", 2.0, walkSideways);   // the left stick, scaled
//! PlayMap.bind(joystick0, ryaxis, lookUpDown);              // the right stick
//! PlayMap.bind(joystick0, button0, jump);                   // A
//! PlayMap.bind(joystick0, upov, selectPrevious);            // the cross, up
//! PlayMap.bind(joystick1, button0, jump);                   // a second pad
//! ```
//!
//! | The pad | The joystick |
//! |---|---|
//! | left stick | `xaxis`, `yaxis` |
//! | right stick | `rxaxis`, `ryaxis` |
//! | left and right triggers | `zaxis`, `rzaxis` (0 to 1), and `button11`, `button12` when pressed in half way |
//! | South, East, West, North (A, B, X, Y on an Xbox pad) | `button0` to `button3` |
//! | shoulders, Select, Start, the sticks pressed, the logo button | `button4` to `button10` |
//! | the cross | `xpov`, `ypov` and `upov`, `dpov`, `lpov`, `rpov` |
//!
//! Y grows downward, as the joystick axes and the mouse's do: a stick pushed up reads -1.
//!
//! # Prefs
//!
//! [`GamepadPrefs`]: `$Pref::Gamepad::DeadZone` (0 to 0.9, default 0.15) takes the middle out of each stick and spreads the rest
//! over the full range; `$Pref::Gamepad::InvertY` swaps up and down on both sticks; `$Pref::Gamepad::PollRate` (30 to 2000,
//! default 500) is how often the reading thread reads the pads; `$Pref::Gamepad::WebFastPoll` (default off) adds a timer in a
//! browser. They are ordinary `$Pref` globals, so a game that exports its prefs saves them, and a settings screen sets them. The
//! source reads them at every poll.
//!
//! # Reading apart from the frame rate
//!
//! Natively the pads are read on a thread of their own (`PadThread`), [`poll_rate`](GamepadPrefs::poll_rate) times a second,
//! and each change is left in a queue stamped with the time it was read. The client drains the queue at each poll (a frame's,
//! or a client pass's where nothing is drawn) and gives each stick as its mean over the time since the last poll, weighted by
//! how long each value held; buttons and the cross keep the order they were pressed in. A slow frame loses nothing, and a fast
//! one is not limited to what the stick read at the instant it was drawn. The thread starts at the first poll of a client (an
//! engine that is built and never run opens no device) and is joined when the client goes. gilrs's `Gilrs` is `Send` (it
//! asserts so on every desktop target), but the backend is made on the thread anyway, so a platform's device handles never move
//! between threads. `GamepadPlugin::without_thread` reads at each poll instead, with no stamps. In a browser the Gamepad API
//! exists on the page's thread only, so the pads are read at each poll; [`web_fast_poll`](GamepadPrefs::web_fast_poll) adds a
//! timer there.
//!
//! # Tests
//!
//! A test plugs pads in and out with [`FakePads`] (`GamepadPlugin::with_backend`) and never reads a real one; this crate's
//! `tests/` bind a script to them.

mod pad;
mod sampler;
mod source;

#[cfg(not(target_arch = "wasm32"))]
mod gilrs_backend;
#[cfg(not(target_arch = "wasm32"))]
mod thread;
#[cfg(target_arch = "wasm32")]
mod web_backend;
#[cfg(target_arch = "wasm32")]
mod web_timer;

use std::rc::Rc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use torque_client::input_source::InputBuilderExt;
use torque_engine::{EngineBuilder, Plugin, PluginId};

#[cfg(not(target_arch = "wasm32"))]
pub use gilrs_backend::GilrsBackend;
pub use pad::{FakePads, PadBackend, PadButton, PadState};
pub use sampler::{HARD_MAX_SAMPLES, MAX_SAMPLES, PadSample, PadSamples, Sampler};
pub use source::{GamepadPrefs, GamepadSource, MAX_PADS, StartReader, apply_dead_zone};
#[cfg(not(target_arch = "wasm32"))]
pub use thread::{MakeBackend, PadThread, RATES, RESTARTS};
#[cfg(target_arch = "wasm32")]
pub use web_backend::WebBackend;

/// The backend a build reads pads with: gilrs natively, the browser's Gamepad API on the web.
pub fn system_backend() -> Box<dyn PadBackend> {
    #[cfg(not(target_arch = "wasm32"))]
    return Box::new(GilrsBackend::default());
    #[cfg(target_arch = "wasm32")]
    return Box::new(WebBackend);
}

/// How a plugin's clients get their pads.
#[derive(Clone)]
enum Pads {
    /// A backend made on the client's thread and read at each poll.
    Local(Rc<dyn Fn() -> Box<dyn PadBackend>>),
    /// A backend made and read on a thread of its own, at [`GamepadPrefs::poll_rate`].
    #[cfg(not(target_arch = "wasm32"))]
    Threaded(MakeBackend),
}

/// Adds the gamepad input source and its prefs to the engine's clients. Not in `DefaultPlugins`.
///
/// Natively the pads are read on a thread of their own, 500 times a second by default (`$Pref::Gamepad::PollRate`), and the
/// client averages each stick over the span since its last poll, so the sticks do not depend on the frame rate. The thread
/// starts when a client first polls (an engine built and never run opens no device) and is joined when the client goes. In a
/// browser the pads are read at each poll, as `navigator.getGamepads()` allows only on the page's own thread; see
/// [`GamepadPrefs::web_fast_poll`] for the timer that reads them more often.
#[derive(Clone, PluginId)]
#[plugin(id = "torque.gamepad")]
pub struct GamepadPlugin {
    pads: Pads,
}

impl Default for GamepadPlugin {
    /// The plugin over the system's pads ([`system_backend`]): read on a thread natively, at each poll in a browser.
    fn default() -> GamepadPlugin {
        #[cfg(not(target_arch = "wasm32"))]
        return GamepadPlugin {
            pads: Pads::Threaded(Arc::new(system_backend)),
        };
        #[cfg(target_arch = "wasm32")]
        return GamepadPlugin {
            pads: Pads::Local(Rc::new(system_backend)),
        };
    }
}

impl GamepadPlugin {
    /// The plugin over pads of another backend, made once for each client and read at each poll on the client's thread: a
    /// scripted one in a test ([`FakePads`]), or a game's own source of pads.
    pub fn with_backend<B: PadBackend + 'static>(make: impl Fn() -> B + 'static) -> GamepadPlugin {
        GamepadPlugin {
            pads: Pads::Local(Rc::new(move || Box::new(make()))),
        }
    }

    /// The plugin over pads of another backend, made on the thread that reads it (once for each client) and read at
    /// [`GamepadPrefs::poll_rate`]. Natively only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_threaded_backend<B: PadBackend + 'static>(
        make: impl Fn() -> B + Send + Sync + 'static,
    ) -> GamepadPlugin {
        GamepadPlugin {
            pads: Pads::Threaded(Arc::new(move || Box::new(make()))),
        }
    }

    /// The same pads, read at each poll on the client's thread, with no thread of their own (as before the thread existed).
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn without_thread(mut self) -> GamepadPlugin {
        if let Pads::Threaded(make) = self.pads {
            self.pads = Pads::Local(Rc::new(move || make()));
        }
        self
    }
}

impl Plugin for GamepadPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client_script().prefs::<GamepadPrefs>();
        match self.pads.clone() {
            Pads::Local(make) => {
                engine.add_input_source_with(move || GamepadSource::new(make()));
            }
            #[cfg(not(target_arch = "wasm32"))]
            Pads::Threaded(make) => {
                engine.add_input_source_with(move || {
                    let make = Arc::clone(&make);
                    let start: StartReader = Box::new(move |samples, rate| {
                        Box::new(PadThread::spawn(make, samples.clone(), Arc::clone(rate)))
                    });
                    GamepadSource::sampled(PadSamples::default(), Some(start))
                });
            }
        }
    }
}
