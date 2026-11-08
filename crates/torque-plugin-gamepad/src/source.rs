//! The input source: the pads a backend reports, as joysticks.

use torque_client::input_source::{InputCx, InputSource, JoyAxis};
use torque_engine::{Prefs, VmPrefs};

use crate::pad::{PadBackend, PadButton, PadState};

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

/// The input source over a backend's pads.
///
/// Each pad takes the lowest `joystick<N>` that is free when it is first seen and keeps it while it is plugged in; an unplugged
/// pad lets go of everything it held and frees its number for the next pad. Each poll says what every pad holds now; the client
/// passes on only what changed.
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
    backend: Box<dyn PadBackend>,
    /// By joystick number: the backend's id of the pad that has it.
    slots: Vec<Option<u32>>,
}

impl GamepadSource {
    /// A source over `backend`.
    pub fn new(backend: Box<dyn PadBackend>) -> GamepadSource {
        GamepadSource {
            backend,
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
}

impl InputSource for GamepadSource {
    fn poll(&mut self, cx: &mut InputCx<'_>) {
        let prefs: GamepadPrefs = cx.vm().prefs();
        let pads = self.backend.poll();
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
            if self.slots.contains(&Some(id)) {
                continue;
            }
            if let Some(free) = self.slots.iter_mut().find(|slot| slot.is_none()) {
                *free = Some(id);
            } else if self.slots.len() < MAX_PADS {
                self.slots.push(Some(id));
            }
        }
        for (id, state) in &pads {
            if let Some(joystick) = self.joystick_of(*id) {
                say(cx, joystick, state, &prefs);
            }
        }
    }
}

/// Says what one pad holds.
fn say(cx: &mut InputCx<'_>, joystick: u32, state: &PadState, prefs: &GamepadPrefs) {
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
    for button in BUTTONS {
        if let Some(number) = button.joystick_button() {
            cx.button(joystick, number, state.pressed(button));
        }
    }
    cx.button(
        joystick,
        LEFT_TRIGGER_BUTTON,
        state.triggers.0 >= TRIGGER_PRESSED,
    );
    cx.button(
        joystick,
        RIGHT_TRIGGER_BUTTON,
        state.triggers.1 >= TRIGGER_PRESSED,
    );
    let (x, y) = state.cross();
    cx.pov(joystick, 0, x, y);
    let flip = if prefs.invert_y { -1.0 } else { 1.0 };
    let stick = |value: f32| apply_dead_zone(value, prefs.dead_zone);
    cx.axis(joystick, JoyAxis::X, stick(state.left.0));
    cx.axis(joystick, JoyAxis::Y, flip * stick(state.left.1));
    cx.axis(joystick, JoyAxis::Rx, stick(state.right.0));
    cx.axis(joystick, JoyAxis::Ry, flip * stick(state.right.1));
    cx.axis(joystick, JoyAxis::Z, state.triggers.0);
    cx.axis(joystick, JoyAxis::Rz, state.triggers.1);
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
