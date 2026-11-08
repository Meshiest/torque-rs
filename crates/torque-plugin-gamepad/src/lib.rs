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
//! over the full range; `$Pref::Gamepad::InvertY` swaps up and down on both sticks. They are ordinary `$Pref` globals, so a game
//! that exports its prefs saves them, and a settings screen sets them. The source reads them at every poll.
//!
//! # Tests
//!
//! A test plugs pads in and out with [`FakePads`] (`GamepadPlugin::with_backend`) and never reads a real one; this crate's
//! `tests/` bind a script to them.

mod pad;
mod source;

#[cfg(not(target_arch = "wasm32"))]
mod gilrs_backend;
#[cfg(target_arch = "wasm32")]
mod web_backend;

use std::rc::Rc;

use torque_client::input_source::InputBuilderExt;
use torque_engine::{EngineBuilder, Plugin, PluginId};

#[cfg(not(target_arch = "wasm32"))]
pub use gilrs_backend::GilrsBackend;
pub use pad::{FakePads, PadBackend, PadButton, PadState};
pub use source::{GamepadPrefs, GamepadSource, MAX_PADS, apply_dead_zone};
#[cfg(target_arch = "wasm32")]
pub use web_backend::WebBackend;

/// The backend a build reads pads with: gilrs natively, the browser's Gamepad API on the web.
pub fn system_backend() -> Box<dyn PadBackend> {
    #[cfg(not(target_arch = "wasm32"))]
    return Box::new(GilrsBackend::default());
    #[cfg(target_arch = "wasm32")]
    return Box::new(WebBackend);
}

/// Adds the gamepad input source and its prefs to the engine's clients. Not in `DefaultPlugins`.
#[derive(Clone, PluginId)]
#[plugin(id = "torque.gamepad")]
pub struct GamepadPlugin {
    make: Rc<dyn Fn() -> Box<dyn PadBackend>>,
}

impl Default for GamepadPlugin {
    /// The plugin over the system's pads ([`system_backend`]).
    fn default() -> GamepadPlugin {
        GamepadPlugin {
            make: Rc::new(system_backend),
        }
    }
}

impl GamepadPlugin {
    /// The plugin over pads of another backend, made once for each client: a scripted one in a test ([`FakePads`]), or a game's
    /// own source of pads.
    pub fn with_backend<B: PadBackend + 'static>(make: impl Fn() -> B + 'static) -> GamepadPlugin {
        GamepadPlugin {
            make: Rc::new(move || Box::new(make())),
        }
    }
}

impl Plugin for GamepadPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client_script().prefs::<GamepadPrefs>();
        let make = Rc::clone(&self.make);
        engine.add_input_source_with(move || GamepadSource::new(make()));
    }
}
