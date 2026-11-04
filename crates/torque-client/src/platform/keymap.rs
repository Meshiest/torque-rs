//! The window's keys and buttons as the Windows messages TGE's platform layer reads: a winit key is the engine's [`Key`]
//! ([`Key::from_winit`]), whose message ([`key_message`]) is the virtual key, scan code and extended bit Windows would give it on
//! a US keyboard, so the engine's [`crate::input::Win32`] turns it into the same event TGE makes of the real message.
//!
//! Credit: follows TGE's window procedure as Torque3D carries it (MIT: `platformWin32/winWindow.cpp`, Copyright (c) 2012
//! GarageGames, LLC).

use crate::input::{Key, MouseButton, WM_MOUSEWHEEL, button_message, key_message};
use winit::event::MouseScrollDelta;
use winit::keyboard::KeyCode;

impl Key {
    /// The key of a winit key code (None: one the platform layer has no key for, such as the media keys).
    pub fn from_winit(code: KeyCode) -> Option<Key> {
        use KeyCode as C;
        Some(match code {
            C::KeyA => Key::A,
            C::KeyB => Key::B,
            C::KeyC => Key::C,
            C::KeyD => Key::D,
            C::KeyE => Key::E,
            C::KeyF => Key::F,
            C::KeyG => Key::G,
            C::KeyH => Key::H,
            C::KeyI => Key::I,
            C::KeyJ => Key::J,
            C::KeyK => Key::K,
            C::KeyL => Key::L,
            C::KeyM => Key::M,
            C::KeyN => Key::N,
            C::KeyO => Key::O,
            C::KeyP => Key::P,
            C::KeyQ => Key::Q,
            C::KeyR => Key::R,
            C::KeyS => Key::S,
            C::KeyT => Key::T,
            C::KeyU => Key::U,
            C::KeyV => Key::V,
            C::KeyW => Key::W,
            C::KeyX => Key::X,
            C::KeyY => Key::Y,
            C::KeyZ => Key::Z,
            C::Digit0 => Key::Digit0,
            C::Digit1 => Key::Digit1,
            C::Digit2 => Key::Digit2,
            C::Digit3 => Key::Digit3,
            C::Digit4 => Key::Digit4,
            C::Digit5 => Key::Digit5,
            C::Digit6 => Key::Digit6,
            C::Digit7 => Key::Digit7,
            C::Digit8 => Key::Digit8,
            C::Digit9 => Key::Digit9,
            C::F1 => Key::F1,
            C::F2 => Key::F2,
            C::F3 => Key::F3,
            C::F4 => Key::F4,
            C::F5 => Key::F5,
            C::F6 => Key::F6,
            C::F7 => Key::F7,
            C::F8 => Key::F8,
            C::F9 => Key::F9,
            C::F10 => Key::F10,
            C::F11 => Key::F11,
            C::F12 => Key::F12,
            C::F13 => Key::F13,
            C::F14 => Key::F14,
            C::F15 => Key::F15,
            C::F16 => Key::F16,
            C::F17 => Key::F17,
            C::F18 => Key::F18,
            C::F19 => Key::F19,
            C::F20 => Key::F20,
            C::F21 => Key::F21,
            C::F22 => Key::F22,
            C::F23 => Key::F23,
            C::F24 => Key::F24,
            C::ArrowLeft => Key::ArrowLeft,
            C::ArrowUp => Key::ArrowUp,
            C::ArrowRight => Key::ArrowRight,
            C::ArrowDown => Key::ArrowDown,
            C::Home => Key::Home,
            C::End => Key::End,
            C::PageUp => Key::PageUp,
            C::PageDown => Key::PageDown,
            C::Insert => Key::Insert,
            C::Delete => Key::Delete,
            C::Backspace => Key::Backspace,
            C::Tab => Key::Tab,
            C::Enter => Key::Enter,
            C::Escape => Key::Escape,
            C::Space => Key::Space,
            C::ShiftLeft => Key::ShiftLeft,
            C::ShiftRight => Key::ShiftRight,
            C::ControlLeft => Key::ControlLeft,
            C::ControlRight => Key::ControlRight,
            C::AltLeft => Key::AltLeft,
            C::AltRight => Key::AltRight,
            C::SuperLeft => Key::SuperLeft,
            C::SuperRight => Key::SuperRight,
            C::ContextMenu => Key::ContextMenu,
            C::CapsLock => Key::CapsLock,
            C::NumLock => Key::NumLock,
            C::ScrollLock => Key::ScrollLock,
            C::PrintScreen => Key::PrintScreen,
            C::Pause => Key::Pause,
            C::Numpad0 => Key::Numpad0,
            C::Numpad1 => Key::Numpad1,
            C::Numpad2 => Key::Numpad2,
            C::Numpad3 => Key::Numpad3,
            C::Numpad4 => Key::Numpad4,
            C::Numpad5 => Key::Numpad5,
            C::Numpad6 => Key::Numpad6,
            C::Numpad7 => Key::Numpad7,
            C::Numpad8 => Key::Numpad8,
            C::Numpad9 => Key::Numpad9,
            C::NumpadMultiply => Key::NumpadMultiply,
            C::NumpadAdd => Key::NumpadAdd,
            C::NumpadSubtract => Key::NumpadSubtract,
            C::NumpadDecimal => Key::NumpadDecimal,
            C::NumpadDivide => Key::NumpadDivide,
            C::NumpadEnter => Key::NumpadEnter,
            C::Minus => Key::Minus,
            C::BracketLeft => Key::BracketLeft,
            C::BracketRight => Key::BracketRight,
            C::Backslash => Key::Backslash,
            C::Semicolon => Key::Semicolon,
            C::Comma => Key::Comma,
            C::Period => Key::Period,
            C::Slash => Key::Slash,
            C::IntlBackslash => Key::IntlBackslash,
            C::Equal => Key::Equals,
            C::Quote => Key::Apostrophe,
            C::Backquote => Key::Grave,
            _ => return None,
        })
    }
}

impl MouseButton {
    /// The button of a winit mouse button (None: one past the two side buttons).
    pub fn from_winit(button: winit::event::MouseButton) -> Option<MouseButton> {
        use winit::event::MouseButton as B;
        Some(match button {
            B::Left => MouseButton::Left,
            B::Right => MouseButton::Right,
            B::Middle => MouseButton::Middle,
            B::Back => MouseButton::Back,
            B::Forward => MouseButton::Forward,
            B::Other(_) => return None,
        })
    }
}

/// The wheel's message for `notches` (positive away from the user), 120 a notch as Windows counts it.
pub fn wheel_message(notches: f32) -> Option<(u32, u32, u32)> {
    let delta = (notches * 120.0).round() as i32;
    if delta == 0 {
        return None;
    }
    Some((WM_MOUSEWHEEL, ((delta as u32) & 0xffff) << 16, 0))
}

/// The wheel's notches gathered into whole ones. Windows posts `WM_MOUSEWHEEL` with a delta of 120 a notch, and the platform layer
/// takes a message of at least 120 either way as a Z axis move (an action map binds `mouse0 zaxis`); a wheel or
/// touchpad that reports parts of a notch (winit hands them on as fractions) would never reach 120 in one message, so the parts are
/// added up and each whole notch posted as one message of 120.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wheel {
    part: f32,
}

impl Wheel {
    /// The messages for `notches` more (positive away from the user), one a whole notch, at most [`MAX_WHEEL_NOTCHES`] at once.
    pub fn messages(&mut self, notches: f32) -> Vec<(u32, u32, u32)> {
        if !notches.is_finite() {
            return Vec::new();
        }
        self.part = (self.part + notches).clamp(-MAX_WHEEL_NOTCHES, MAX_WHEEL_NOTCHES);
        let whole = self.part.trunc();
        self.part -= whole;
        let n = whole.abs() as usize;
        let one = if whole > 0.0 { 1.0 } else { -1.0 };
        (0..n).filter_map(|_| wheel_message(one)).collect()
    }
}

/// The most notches posted from one wheel event (a hostile or broken device's huge delta).
pub const MAX_WHEEL_NOTCHES: f32 = 32.0;

/// A wheel event's notches (positive away from the user): a line delta as it is (winit's Windows backend gives the message's
/// delta over 120), a pixel delta at 120 pixels a notch.
pub fn wheel_notches(delta: MouseScrollDelta) -> f32 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => y,
        MouseScrollDelta::PixelDelta(p) => (p.y / 120.0) as f32,
    }
}

/// The window's keys, buttons, wheel and focus as the messages it posts to the platform layer ([`crate::input::Win32`]),
/// whatever winit adds of its own:
/// - a key is its message ([`key_message`]), a system key while Alt is held;
/// - a button is its message ([`button_message`]); the side buttons (Back, Forward) are `WM_XBUTTONDOWN`/`UP`;
/// - winit's **synthetic presses** (the keys found held when the window takes the focus) post nothing: Windows sends no message
///   for them, and a press whose release goes to another window (the Alt of an Alt+Tab back, the Shift of a Win+Shift+S) leaves the
///   platform layer holding that modifier, so every modifier bind reads as another (Ctrl+Z as Ctrl+Alt+Z, which no bind has: the
///   `processAction` then takes the plain `z`). Its synthetic releases (the keys held as the focus goes) are posted;
/// - the focus lost lets go of the mouse buttons held, as TGE's window procedure does (`WM_ACTIVATE` inactive: a break
///   for each of buttons 0 to 2 still down), so a held fire trigger does not stay pressed in the game;
/// - the wheel is whole notches of 120 ([`Wheel`]).
#[derive(Debug, Clone, Default)]
pub struct WindowInput {
    alt: bool,
    wheel: Wheel,
    /// The left, right and middle buttons the window posted a down for and no up since.
    buttons: [bool; 3],
}

impl WindowInput {
    /// The message of a key event (None: nothing to post).
    pub fn key(
        &mut self,
        key: Key,
        pressed: bool,
        repeat: bool,
        synthetic: bool,
    ) -> Option<(u32, u32, u32)> {
        if synthetic && pressed {
            return None;
        }
        if matches!(key, Key::AltLeft | Key::AltRight) {
            self.alt = pressed;
        }
        Some(key_message(key, pressed, repeat, self.alt))
    }

    /// The message of a mouse button at the client position `at`.
    pub fn button(
        &mut self,
        button: MouseButton,
        pressed: bool,
        at: (i32, i32),
    ) -> (u32, u32, u32) {
        let slot = match button {
            MouseButton::Left => Some(0),
            MouseButton::Right => Some(1),
            MouseButton::Middle => Some(2),
            MouseButton::Back | MouseButton::Forward => None,
        };
        if let Some(slot) = slot {
            self.buttons[slot] = pressed;
        }
        button_message(button, pressed, at)
    }

    /// The messages of a wheel event.
    pub fn wheel(&mut self, delta: MouseScrollDelta) -> Vec<(u32, u32, u32)> {
        self.wheel.messages(wheel_notches(delta))
    }

    /// The wheel's part of a notch gathered and not yet posted (positive away from the user; the engine's smooth scrolling follows it).
    pub fn wheel_part(&self) -> f32 {
        self.wheel.part
    }

    /// The messages the focus lost posts: an up for each button still held (left, right, middle).
    pub fn focus_lost(&mut self, at: (i32, i32)) -> Vec<(u32, u32, u32)> {
        let mut out = Vec::new();
        for (slot, button) in [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
            .into_iter()
            .enumerate()
        {
            if std::mem::take(&mut self.buttons[slot]) {
                out.push(button_message(button, false, at));
            }
        }
        out
    }

    /// Whether Alt is held (the window's own reading of its key events).
    pub fn alt(&self) -> bool {
        self.alt
    }
}

#[cfg(test)]
#[path = "keymap_tests.rs"]
mod tests;
