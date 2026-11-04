//! The window's keys and buttons as the Windows messages TGE's platform layer reads: a winit key is the virtual key, scan code
//! and extended bit Windows would give it on a US keyboard, so the engine's [`crate::input::Win32`] turns it into the same event
//! TGE makes of the real message.
//!
//! Credit: follows TGE's window procedure as Torque3D carries it (MIT: `platformWin32/winWindow.cpp`, Copyright (c) 2012
//! GarageGames, LLC).

use crate::input::{
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::KeyCode;

/// The virtual key and the extended bit of a key (None: one the key table has no key for).
pub fn virtual_key(code: KeyCode) -> Option<(u8, bool)> {
    use KeyCode as K;
    let letter = |c: u8| Some((c, false));
    Some(match code {
        K::KeyA => return letter(b'A'),
        K::KeyB => return letter(b'B'),
        K::KeyC => return letter(b'C'),
        K::KeyD => return letter(b'D'),
        K::KeyE => return letter(b'E'),
        K::KeyF => return letter(b'F'),
        K::KeyG => return letter(b'G'),
        K::KeyH => return letter(b'H'),
        K::KeyI => return letter(b'I'),
        K::KeyJ => return letter(b'J'),
        K::KeyK => return letter(b'K'),
        K::KeyL => return letter(b'L'),
        K::KeyM => return letter(b'M'),
        K::KeyN => return letter(b'N'),
        K::KeyO => return letter(b'O'),
        K::KeyP => return letter(b'P'),
        K::KeyQ => return letter(b'Q'),
        K::KeyR => return letter(b'R'),
        K::KeyS => return letter(b'S'),
        K::KeyT => return letter(b'T'),
        K::KeyU => return letter(b'U'),
        K::KeyV => return letter(b'V'),
        K::KeyW => return letter(b'W'),
        K::KeyX => return letter(b'X'),
        K::KeyY => return letter(b'Y'),
        K::KeyZ => return letter(b'Z'),
        K::Digit0 => (b'0', false),
        K::Digit1 => (b'1', false),
        K::Digit2 => (b'2', false),
        K::Digit3 => (b'3', false),
        K::Digit4 => (b'4', false),
        K::Digit5 => (b'5', false),
        K::Digit6 => (b'6', false),
        K::Digit7 => (b'7', false),
        K::Digit8 => (b'8', false),
        K::Digit9 => (b'9', false),
        K::Backspace => (0x08, false),
        K::Tab => (0x09, false),
        K::Enter => (0x0d, false),
        K::NumpadEnter => (0x0d, true),
        K::ShiftLeft | K::ShiftRight => (0x10, false),
        K::ControlLeft => (0x11, false),
        K::ControlRight => (0x11, true),
        K::AltLeft => (0x12, false),
        K::AltRight => (0x12, true),
        K::Pause => (0x13, false),
        K::CapsLock => (0x14, false),
        K::Escape => (0x1b, false),
        K::Space => (0x20, false),
        K::PageUp => (0x21, true),
        K::PageDown => (0x22, true),
        K::End => (0x23, true),
        K::Home => (0x24, true),
        K::ArrowLeft => (0x25, true),
        K::ArrowUp => (0x26, true),
        K::ArrowRight => (0x27, true),
        K::ArrowDown => (0x28, true),
        K::PrintScreen => (0x2c, true),
        K::Insert => (0x2d, true),
        K::Delete => (0x2e, true),
        K::SuperLeft => (0x5b, true),
        K::SuperRight => (0x5c, true),
        K::ContextMenu => (0x5d, true),
        K::Numpad0 => (0x60, false),
        K::Numpad1 => (0x61, false),
        K::Numpad2 => (0x62, false),
        K::Numpad3 => (0x63, false),
        K::Numpad4 => (0x64, false),
        K::Numpad5 => (0x65, false),
        K::Numpad6 => (0x66, false),
        K::Numpad7 => (0x67, false),
        K::Numpad8 => (0x68, false),
        K::Numpad9 => (0x69, false),
        K::NumpadMultiply => (0x6a, false),
        K::NumpadAdd => (0x6b, false),
        K::NumpadSubtract => (0x6d, false),
        K::NumpadDecimal => (0x6e, false),
        K::NumpadDivide => (0x6f, true),
        K::F1 => (0x70, false),
        K::F2 => (0x71, false),
        K::F3 => (0x72, false),
        K::F4 => (0x73, false),
        K::F5 => (0x74, false),
        K::F6 => (0x75, false),
        K::F7 => (0x76, false),
        K::F8 => (0x77, false),
        K::F9 => (0x78, false),
        K::F10 => (0x79, false),
        K::F11 => (0x7a, false),
        K::F12 => (0x7b, false),
        K::NumLock => (0x90, true),
        K::ScrollLock => (0x91, false),
        K::Semicolon => (0xba, false),
        K::Equal => (0xbb, false),
        K::Comma => (0xbc, false),
        K::Minus => (0xbd, false),
        K::Period => (0xbe, false),
        K::Slash => (0xbf, false),
        K::Backquote => (0xc0, false),
        K::BracketLeft => (0xdb, false),
        K::Backslash => (0xdc, false),
        K::BracketRight => (0xdd, false),
        K::Quote => (0xde, false),
        K::IntlBackslash => (0xe2, false),
        _ => return None,
    })
}

/// The message of a key: `(message, wParam, lParam)` as Windows posts it (the scan code the US layout's, the right Shift's 0x36,
/// the extended bit, the previous-state bit of a repeat, the up bits).
pub fn key_message(
    code: KeyCode,
    down: bool,
    repeat: bool,
    alt_held: bool,
) -> Option<(u32, u32, u32)> {
    let (vk, extended) = virtual_key(code)?;
    let scan = if code == KeyCode::ShiftRight {
        0x36
    } else {
        u32::from(gui::keys::map_virtual_key(vk))
    };
    let mut l = 1 | (scan << 16);
    if extended {
        l |= 1 << 24;
    }
    if !down {
        l |= (1 << 30) | (1 << 31);
    } else if repeat {
        l |= 1 << 30;
    }
    // Windows sends a key with Alt held (and F10, and Alt itself) as a system key.
    let system = alt_held || vk == 0x12 || vk == 0x79;
    let msg = match (down, system) {
        (true, false) => WM_KEYDOWN,
        (false, false) => WM_KEYUP,
        (true, true) => WM_SYSKEYDOWN,
        (false, true) => WM_SYSKEYUP,
    };
    Some((msg, u32::from(vk), l))
}

/// The message of a mouse button at the client position (None: a button the platform layer does not read).
pub fn button_message(button: MouseButton, down: bool, at: (i32, i32)) -> Option<(u32, u32, u32)> {
    let msg = match (button, down) {
        (MouseButton::Left, true) => WM_LBUTTONDOWN,
        (MouseButton::Left, false) => WM_LBUTTONUP,
        (MouseButton::Right, true) => WM_RBUTTONDOWN,
        (MouseButton::Right, false) => WM_RBUTTONUP,
        (MouseButton::Middle, true) => WM_MBUTTONDOWN,
        (MouseButton::Middle, false) => WM_MBUTTONUP,
        _ => return None,
    };
    Some((
        msg,
        0,
        ((at.1 as u32 & 0xffff) << 16) | (at.0 as u32 & 0xffff),
    ))
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
        code: KeyCode,
        pressed: bool,
        repeat: bool,
        synthetic: bool,
    ) -> Option<(u32, u32, u32)> {
        if synthetic && pressed {
            return None;
        }
        if matches!(code, KeyCode::AltLeft | KeyCode::AltRight) {
            self.alt = pressed;
        }
        key_message(code, pressed, repeat, self.alt)
    }

    /// The message of a mouse button at the client position `at`.
    pub fn button(
        &mut self,
        button: MouseButton,
        pressed: bool,
        at: (i32, i32),
    ) -> Option<(u32, u32, u32)> {
        let slot = match button {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
            _ => return None,
        };
        self.buttons[slot] = pressed;
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
            if std::mem::take(&mut self.buttons[slot])
                && let Some(m) = button_message(button, false, at)
            {
                out.push(m);
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
