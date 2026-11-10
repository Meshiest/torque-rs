//! The native backend: gilrs reads the pads (XInput or Windows.Gaming.Input on Windows, evdev on Linux, IOKit on macOS).

use std::collections::BTreeMap;

use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};

use crate::pad::{PadBackend, PadButton, PadState};

/// Pads read by gilrs. It starts reading on the first poll, not when it is made, so an engine that is built and never run opens
/// no device. If gilrs cannot start, it reports no pads, and says so once on stderr.
#[derive(Default)]
pub struct GilrsBackend {
    gilrs: Option<Gilrs>,
    failed: bool,
    pads: BTreeMap<usize, PadState>,
}

impl GilrsBackend {
    fn open(&mut self) -> Option<&mut Gilrs> {
        if self.gilrs.is_none() && !self.failed {
            match Gilrs::new() {
                Ok(gilrs) => self.gilrs = Some(gilrs),
                // A platform gilrs does not support: its stand-in works, and has no pads.
                Err(gilrs::Error::NotImplemented(gilrs)) => self.gilrs = Some(gilrs),
                Err(error) => {
                    self.failed = true;
                    eprintln!("gamepad: no pads can be read: {error}");
                }
            }
            if let Some(gilrs) = &self.gilrs {
                for (id, _) in gilrs.gamepads() {
                    self.pads.entry(usize::from(id)).or_default();
                }
            }
        }
        self.gilrs.as_mut()
    }
}

impl PadBackend for GilrsBackend {
    fn poll(&mut self) -> Vec<(u32, PadState)> {
        let mut events = Vec::new();
        if let Some(gilrs) = self.open() {
            while let Some(event) = gilrs.next_event() {
                events.push(event);
            }
        }
        for Event { id, event, .. } in events {
            apply(&mut self.pads, id, event);
        }
        self.pads
            .iter()
            .map(|(id, state)| (*id as u32, *state))
            .collect()
    }
}

/// One gilrs event on the pads' states.
fn apply(pads: &mut BTreeMap<usize, PadState>, id: GamepadId, event: EventType) {
    let key = usize::from(id);
    match event {
        EventType::Connected => {
            pads.entry(key).or_default();
        }
        EventType::Disconnected => {
            pads.remove(&key);
        }
        EventType::ButtonPressed(button, _) => {
            button_event(pads.entry(key).or_default(), button, 1.0)
        }
        EventType::ButtonReleased(button, _) => {
            button_event(pads.entry(key).or_default(), button, 0.0)
        }
        EventType::ButtonChanged(button, value, _) => {
            // A button with a value: the triggers. The rest are set by their press and release.
            if matches!(button, Button::LeftTrigger2 | Button::RightTrigger2) {
                button_event(pads.entry(key).or_default(), button, value);
            }
        }
        EventType::AxisChanged(axis, value, _) => {
            axis_event(pads.entry(key).or_default(), axis, value)
        }
        _ => {}
    }
}

fn button_event(state: &mut PadState, button: Button, value: f32) {
    let down = value >= 0.5;
    let pad = match button {
        Button::South => PadButton::South,
        Button::East => PadButton::East,
        Button::West => PadButton::West,
        Button::North => PadButton::North,
        Button::LeftTrigger => PadButton::LeftShoulder,
        Button::RightTrigger => PadButton::RightShoulder,
        Button::Select => PadButton::Select,
        Button::Start => PadButton::Start,
        Button::Mode => PadButton::Guide,
        Button::LeftThumb => PadButton::LeftStick,
        Button::RightThumb => PadButton::RightStick,
        Button::DPadUp => PadButton::DPadUp,
        Button::DPadDown => PadButton::DPadDown,
        Button::DPadLeft => PadButton::DPadLeft,
        Button::DPadRight => PadButton::DPadRight,
        Button::LeftTrigger2 => {
            state.triggers.0 = value.clamp(0.0, 1.0);
            return;
        }
        Button::RightTrigger2 => {
            state.triggers.1 = value.clamp(0.0, 1.0);
            return;
        }
        _ => return,
    };
    state.set(pad, down);
}

/// gilrs reads up as positive; the pad's Y is down positive.
fn axis_event(state: &mut PadState, axis: Axis, value: f32) {
    match axis {
        Axis::LeftStickX => state.left.0 = value,
        Axis::LeftStickY => state.left.1 = -value,
        Axis::RightStickX => state.right.0 = value,
        Axis::RightStickY => state.right.1 = -value,
        // A pad that reports its cross as axes.
        Axis::DPadX => {
            state.set(PadButton::DPadLeft, value < -0.5);
            state.set(PadButton::DPadRight, value > 0.5);
        }
        Axis::DPadY => {
            state.set(PadButton::DPadUp, value > 0.5);
            state.set(PadButton::DPadDown, value < -0.5);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gilrs_events_make_a_pads_state_with_y_down_positive() {
        let mut state = PadState::default();
        axis_event(&mut state, Axis::LeftStickY, 1.0);
        axis_event(&mut state, Axis::RightStickX, 0.5);
        assert_eq!((state.left, state.right), ((0.0, -1.0), (0.5, 0.0)));
        button_event(&mut state, Button::LeftTrigger, 1.0);
        button_event(&mut state, Button::RightTrigger2, 0.75);
        axis_event(&mut state, Axis::DPadY, 1.0);
        assert!(state.pressed(PadButton::LeftShoulder));
        assert_eq!(state.triggers, (0.0, 0.75));
        assert_eq!(state.cross(), (0, -1));
        button_event(&mut state, Button::LeftTrigger, 0.0);
        assert!(!state.pressed(PadButton::LeftShoulder));
    }
}
