//! The browser backend: `navigator.getGamepads()`, read at every poll. The browser keeps a pad's index while it is plugged in and
//! reports a pad only once the page has seen it used (a button pressed), as its rules for privacy say.

use wasm_bindgen::JsCast;
use web_sys::{Gamepad, GamepadButton};

use crate::pad::{PadBackend, PadState};

/// Pads read through the Gamepad API.
#[derive(Default)]
pub struct WebBackend;

impl PadBackend for WebBackend {
    fn poll(&mut self) -> Vec<(u32, PadState)> {
        let Some(window) = web_sys::window() else {
            return Vec::new();
        };
        // `getGamepads` fails on a page the browser does not allow it (an insecure one): no pads.
        let Ok(list) = window.navigator().get_gamepads() else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|entry| entry.dyn_into::<Gamepad>().ok())
            .filter(Gamepad::connected)
            .map(|pad| {
                let buttons: Vec<(bool, f64)> = pad
                    .buttons()
                    .iter()
                    .map(|button| {
                        let button: GamepadButton = button.unchecked_into();
                        (button.pressed(), button.value())
                    })
                    .collect();
                let axes: Vec<f64> = pad
                    .axes()
                    .iter()
                    .map(|axis| axis.as_f64().unwrap_or(0.0))
                    .collect();
                (pad.index(), PadState::from_standard(&buttons, &axes))
            })
            .collect()
    }
}
