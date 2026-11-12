//! What a pad is, whichever backend reads it: a button set, two sticks and two triggers ([`PadState`]), and the scripted pads
//! tests use (`FakePads`, feature `testing`).

#[cfg(any(test, feature = "testing"))]
use std::collections::BTreeMap;
#[cfg(any(test, feature = "testing"))]
use std::sync::{Arc, Mutex, PoisonError};

/// A button of the standard pad, in the order of the joystick numbers the plugin gives them (`button0` to `button10`); the four
/// directions of the pad's cross are a hat, not buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PadButton {
    /// The bottom face button (A on an Xbox pad, Cross on a PlayStation one): `button0`.
    South = 0,
    /// The right face button (B, Circle): `button1`.
    East = 1,
    /// The left face button (X, Square): `button2`.
    West = 2,
    /// The top face button (Y, Triangle): `button3`.
    North = 3,
    /// The left shoulder button: `button4`.
    LeftShoulder = 4,
    /// The right shoulder button: `button5`.
    RightShoulder = 5,
    /// Back, View or Share: `button6`.
    Select = 6,
    /// Start or Menu: `button7`.
    Start = 7,
    /// The left stick pressed in: `button8`.
    LeftStick = 8,
    /// The right stick pressed in: `button9`.
    RightStick = 9,
    /// The pad's logo button: `button10`.
    Guide = 10,
    /// The cross up (`upov`).
    DPadUp = 11,
    /// The cross down (`dpov`).
    DPadDown = 12,
    /// The cross left (`lpov`).
    DPadLeft = 13,
    /// The cross right (`rpov`).
    DPadRight = 14,
}

impl PadButton {
    /// The joystick's button number of a button that has one (`button0` to `button10`); the cross has none.
    pub const fn joystick_button(self) -> Option<u16> {
        let n = self as u8;
        if n <= 10 { Some(n as u16) } else { None }
    }

    const fn bit(self) -> u16 {
        1 << (self as u8)
    }
}

/// Everything a pad holds at one moment. Sticks are -1 to 1 with Y **down** positive (a stick pushed up reads a negative Y), as the
/// browser reports them and as the joystick axes read them; triggers are 0 (let go) to 1 (all the way in).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PadState {
    buttons: u16,
    /// The left stick: x, y.
    pub left: (f32, f32),
    /// The right stick: x, y.
    pub right: (f32, f32),
    /// The left and right triggers.
    pub triggers: (f32, f32),
}

impl PadState {
    /// Whether `button` is down.
    pub fn pressed(&self, button: PadButton) -> bool {
        self.buttons & button.bit() != 0
    }

    /// Whether `other` has the same buttons down and the same triggers in past half way: only its sticks and the depth of its
    /// triggers differ.
    pub(crate) fn same_buttons(&self, other: &PadState) -> bool {
        let pressed = |s: &PadState| (s.triggers.0 >= 0.5, s.triggers.1 >= 0.5);
        self.buttons == other.buttons && pressed(self) == pressed(other)
    }

    /// Presses or lets go of `button`.
    pub fn set(&mut self, button: PadButton, down: bool) {
        if down {
            self.buttons |= button.bit();
        } else {
            self.buttons &= !button.bit();
        }
    }

    /// The cross as a hat: x (-1 left, 1 right) and y (-1 up, 1 down).
    pub fn cross(&self) -> (i8, i8) {
        let axis = |low: PadButton, high: PadButton| {
            i8::from(self.pressed(high)) - i8::from(self.pressed(low))
        };
        (
            axis(PadButton::DPadLeft, PadButton::DPadRight),
            axis(PadButton::DPadUp, PadButton::DPadDown),
        )
    }

    /// The state a browser reports for a pad with the "standard" mapping: `buttons` as `(pressed, value)` in the standard order
    /// (0 to 3 the face buttons, 4 and 5 the shoulders, 6 and 7 the triggers, 8 and 9 Select and Start, 10 and 11 the sticks
    /// pressed, 12 to 15 the cross, 16 the logo) and `axes` (left x, left y, right x, right y). A list shorter than the standard
    /// one leaves the rest at rest; a pad with another mapping reads as if it were this one.
    pub fn from_standard(buttons: &[(bool, f64)], axes: &[f64]) -> PadState {
        // The triggers (6 and 7) are read as values, not as buttons.
        const BUTTONS: [Option<PadButton>; 17] = [
            Some(PadButton::South),
            Some(PadButton::East),
            Some(PadButton::West),
            Some(PadButton::North),
            Some(PadButton::LeftShoulder),
            Some(PadButton::RightShoulder),
            None,
            None,
            Some(PadButton::Select),
            Some(PadButton::Start),
            Some(PadButton::LeftStick),
            Some(PadButton::RightStick),
            Some(PadButton::DPadUp),
            Some(PadButton::DPadDown),
            Some(PadButton::DPadLeft),
            Some(PadButton::DPadRight),
            Some(PadButton::Guide),
        ];
        let mut state = PadState::default();
        for (i, &(pressed, value)) in buttons.iter().enumerate().take(BUTTONS.len()) {
            match (i, BUTTONS[i]) {
                (6, _) => state.triggers.0 = trigger(pressed, value),
                (7, _) => state.triggers.1 = trigger(pressed, value),
                (_, Some(button)) => state.set(button, pressed),
                (_, None) => {}
            }
        }
        let axis = |i: usize| axes.get(i).map_or(0.0, |v| stick(*v));
        state.left = (axis(0), axis(1));
        state.right = (axis(2), axis(3));
        state
    }
}

/// A trigger's value: its analog value when it has one, else all the way in when it is pressed.
fn trigger(pressed: bool, value: f64) -> f32 {
    let value = if value.is_finite() { value as f32 } else { 0.0 };
    if pressed && value == 0.0 {
        1.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// A stick's value, -1 to 1 (not a number is at rest).
fn stick(value: f64) -> f32 {
    if value.is_finite() {
        (value as f32).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Where pads come from: the pads connected now, each with an id that stays its own while it is plugged in.
pub trait PadBackend {
    /// The pads connected now, by id, and what each holds. A pad missing from the list was unplugged.
    fn poll(&mut self) -> Vec<(u32, PadState)>;
}

/// Pads a test scripts (it is `Send`, so a thread can read it): whoever holds a copy plugs pads in and out and moves their sticks, and the plugin's next poll sees it.
///
/// ```
/// use torque_plugin_gamepad::{FakePads, PadBackend, PadButton};
///
/// let pads = FakePads::default();
/// let mut backend = pads.clone();
/// pads.plug_in(7);
/// pads.press(7, PadButton::South, true);
/// pads.left_stick(7, 0.5, -1.0);
/// let seen = backend.poll();
/// assert_eq!(seen.len(), 1);
/// assert!(seen[0].1.pressed(PadButton::South));
/// pads.unplug(7);
/// assert!(backend.poll().is_empty());
/// ```
#[cfg(any(test, feature = "testing"))]
#[derive(Clone, Default)]
pub struct FakePads {
    pads: Arc<Mutex<BTreeMap<u32, PadState>>>,
}

#[cfg(any(test, feature = "testing"))]
impl FakePads {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, PadState>> {
        self.pads.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Plugs in pad `id`, at rest (a pad already plugged in keeps what it holds).
    pub fn plug_in(&self, id: u32) {
        self.lock().entry(id).or_default();
    }

    /// Unplugs pad `id`.
    pub fn unplug(&self, id: u32) {
        self.lock().remove(&id);
    }

    /// Changes what pad `id` holds (a pad not plugged in is plugged in first).
    pub fn update(&self, id: u32, change: impl FnOnce(&mut PadState)) {
        change(self.lock().entry(id).or_default());
    }

    /// Presses or lets go of a button of pad `id`.
    pub fn press(&self, id: u32, button: PadButton, down: bool) {
        self.update(id, |s| s.set(button, down));
    }

    /// Moves the left stick of pad `id`.
    pub fn left_stick(&self, id: u32, x: f32, y: f32) {
        self.update(id, |s| s.left = (x, y));
    }

    /// Moves the right stick of pad `id`.
    pub fn right_stick(&self, id: u32, x: f32, y: f32) {
        self.update(id, |s| s.right = (x, y));
    }

    /// Moves the triggers of pad `id`.
    pub fn triggers(&self, id: u32, left: f32, right: f32) {
        self.update(id, |s| s.triggers = (left, right));
    }
}

#[cfg(any(test, feature = "testing"))]
impl PadBackend for FakePads {
    fn poll(&mut self) -> Vec<(u32, PadState)> {
        self.lock().iter().map(|(id, s)| (*id, *s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cross_reads_as_a_hat() {
        let mut s = PadState::default();
        assert_eq!(s.cross(), (0, 0));
        s.set(PadButton::DPadRight, true);
        s.set(PadButton::DPadUp, true);
        assert_eq!(s.cross(), (1, -1));
        s.set(PadButton::DPadLeft, true);
        assert_eq!(s.cross(), (0, -1), "both sides cancel");
    }

    #[test]
    fn a_standard_browser_pad_reads_in_the_standard_order() {
        let mut buttons = vec![(false, 0.0); 17];
        buttons[0] = (true, 1.0); // A
        buttons[5] = (true, 1.0); // right shoulder
        buttons[6] = (false, 0.4); // left trigger, partly in
        buttons[7] = (true, 0.0); // right trigger, digital only
        buttons[9] = (true, 1.0); // start
        buttons[12] = (true, 1.0); // cross up
        buttons[15] = (true, 1.0); // cross right
        let s = PadState::from_standard(&buttons, &[0.5, -0.25, f64::NAN, 9.0]);
        assert!(s.pressed(PadButton::South) && s.pressed(PadButton::RightShoulder));
        assert!(s.pressed(PadButton::Start) && !s.pressed(PadButton::Select));
        assert_eq!(s.triggers, (0.4, 1.0));
        assert_eq!(s.cross(), (1, -1));
        assert_eq!((s.left, s.right), ((0.5, -0.25), (0.0, 1.0)));
        // A short list leaves the rest at rest.
        assert_eq!(PadState::from_standard(&[], &[]), PadState::default());
    }

    #[test]
    fn only_the_first_eleven_buttons_have_joystick_numbers() {
        assert_eq!(PadButton::South.joystick_button(), Some(0));
        assert_eq!(PadButton::Guide.joystick_button(), Some(10));
        assert_eq!(PadButton::DPadUp.joystick_button(), None);
    }
}
