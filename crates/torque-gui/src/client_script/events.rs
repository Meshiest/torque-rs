//! The events the player's input on the realm's controls makes, and their encoding as the format's `ui` events.

use torque_client_script_format::ui::Event;
use torque_client_script_format::{Malformed, Tier, Value, Writer, event};

/// What a `change` event carries.
#[derive(Clone, Debug, PartialEq)]
pub enum Changed {
    /// A toggle's state.
    Bool(bool),
    /// A text field's text.
    Text(String),
    /// A slider's value.
    Number(f64),
}

/// An event for the realm, by the control's handle.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// The player clicked a panel, text or image.
    Click(u32),
    /// The pointer entered (`true`) or left a panel or image.
    Hover(u32, bool),
    /// The player pressed a button.
    Press(u32),
    /// A toggle, text field or slider changed.
    Change(u32, Changed),
    /// The player pressed Enter in a text field.
    Submit(u32, String),
    /// The player picked a list row (1-based; 0 none).
    Select(u32, u32),
    /// A scroll area moved.
    Scroll(u32, f64, f64),
    /// The server area's size and the UI scale.
    Screen(f64, f64, f64),
    /// Escape, a game dialog or the player dropped the realm's cursor and focus.
    Closed,
}

impl UiEvent {
    /// The control event this is, for the events that come from a control.
    fn control_event(&self) -> Option<Event> {
        match self {
            UiEvent::Click(_) => Some(Event::Click),
            UiEvent::Hover(..) => Some(Event::Hover),
            UiEvent::Press(_) => Some(Event::Press),
            UiEvent::Change(..) => Some(Event::Change),
            UiEvent::Submit(..) => Some(Event::Submit),
            UiEvent::Select(..) => Some(Event::Select),
            UiEvent::Scroll(..) => Some(Event::Scroll),
            UiEvent::Screen(..) | UiEvent::Closed => None,
        }
    }

    /// Whether the event counts as the player acting on the realm's controls, which lets `ui.focus` through for a second.
    pub fn is_activation(&self) -> bool {
        self.control_event().is_some_and(Event::activates)
    }

    /// Whether the event is the player's input on a control (not the screen size or a close).
    pub fn is_input(&self) -> bool {
        self.control_event().is_some()
    }

    fn kind(&self) -> u8 {
        match self {
            UiEvent::Click(_) => event::ui::CLICK,
            UiEvent::Hover(..) => event::ui::HOVER,
            UiEvent::Press(_) => event::ui::PRESS,
            UiEvent::Change(..) => event::ui::CHANGE,
            UiEvent::Submit(..) => event::ui::SUBMIT,
            UiEvent::Select(..) => event::ui::SELECT,
            UiEvent::Scroll(..) => event::ui::SCROLL,
            UiEvent::Screen(..) => event::ui::SCREEN,
            UiEvent::Closed => event::ui::CLOSED,
        }
    }

    /// Appends the event as one record.
    ///
    /// # Errors
    /// When the record is longer than the format allows.
    pub fn encode(&self, out: &mut Writer) -> Result<(), Malformed> {
        let mark = out.begin(Tier::Ui.id(), self.kind());
        match self {
            UiEvent::Click(h) | UiEvent::Press(h) => out.u32(*h),
            UiEvent::Hover(h, on) => {
                out.u32(*h);
                out.bool(*on);
            }
            UiEvent::Change(h, value) => {
                out.u32(*h);
                let value = match value {
                    Changed::Bool(b) => Value::Bool(*b),
                    Changed::Text(t) => Value::String(t.clone().into_bytes()),
                    Changed::Number(n) => Value::Number(*n),
                };
                value
                    .encode(out)
                    .map_err(|_| Malformed("a change value with no encoding"))?;
            }
            UiEvent::Submit(h, text) => {
                out.u32(*h);
                out.text(text);
            }
            UiEvent::Select(h, row) => {
                out.u32(*h);
                out.u32(*row);
            }
            UiEvent::Scroll(h, x, y) => {
                out.u32(*h);
                out.f64(*x);
                out.f64(*y);
            }
            UiEvent::Screen(w, h, scale) => {
                out.f64(*w);
                out.f64(*h);
                out.f64(*scale);
            }
            UiEvent::Closed => {}
        }
        out.end(mark)
    }
}

/// The events as one batch the realm's frame takes (at most [`torque_client_script_format::MAX_EVENTS`] records; the caller
/// keeps the rest for the next frame).
pub fn encode_events(events: &[UiEvent]) -> Vec<u8> {
    let mut out = Writer::new();
    for event in events.iter().take(torque_client_script_format::MAX_EVENTS) {
        let at = out.len();
        if event.encode(&mut out).is_err() {
            out.truncate(at);
        }
    }
    out.into_bytes()
}
