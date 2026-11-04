//! The keyboard keys and mouse buttons a game or a test names ([`Key`], [`MouseButton`]), and the window message each one posts
//! to the platform layer ([`key_message`], [`button_message`]): a key is the virtual key, scan code and extended bit Windows gives
//! it on a US keyboard, so [`super::Win32`] makes of it the same event TGE makes of the real message.
//!
//! ```
//! use torque_client::input::{Key, WM_KEYDOWN, key_message};
//!
//! let (msg, vk, _) = key_message(Key::W, true, false, false);
//! assert_eq!((msg, vk), (WM_KEYDOWN, u32::from(b'W')));
//! ```
//!
//! Credit: follows TGE's window procedure as Torque3D carries it (MIT: `platformWin32/winWindow.cpp`, Copyright (c) 2012
//! GarageGames, LLC).

use gui::keys;

use super::{
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

/// The keys, each with its virtual key and whether Windows sets the extended bit for it (`ext`).
macro_rules! key_table {
    ($($(#[doc = $doc:literal])* $name:ident = $vk:literal $($ext:ident)?;)*) => {
        /// A keyboard key, by its place on a US keyboard (the key, not the character a layout gives it).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Key {
            $($(#[doc = $doc])* $name,)*
        }

        impl Key {
            /// Every key, in table order.
            pub const ALL: &'static [Key] = &[$(Key::$name,)*];

            /// The Windows virtual key (`VK_*`) of the key.
            pub const fn virtual_key(self) -> u8 {
                match self {
                    $(Key::$name => $vk,)*
                }
            }

            /// Whether Windows sets the extended bit (lParam bit 24) on the key's messages.
            pub const fn extended(self) -> bool {
                match self {
                    $(Key::$name => key_table!(@ext $($ext)?),)*
                }
            }
        }
    };
    (@ext ext) => { true };
    (@ext) => { false };
}

key_table! {
    A = 0x41; B = 0x42; C = 0x43; D = 0x44; E = 0x45; F = 0x46; G = 0x47; H = 0x48; I = 0x49; J = 0x4a; K = 0x4b; L = 0x4c;
    M = 0x4d; N = 0x4e; O = 0x4f; P = 0x50; Q = 0x51; R = 0x52; S = 0x53; T = 0x54; U = 0x55; V = 0x56; W = 0x57; X = 0x58;
    Y = 0x59; Z = 0x5a;
    Digit0 = 0x30; Digit1 = 0x31; Digit2 = 0x32; Digit3 = 0x33; Digit4 = 0x34; Digit5 = 0x35; Digit6 = 0x36; Digit7 = 0x37;
    Digit8 = 0x38; Digit9 = 0x39;
    F1 = 0x70; F2 = 0x71; F3 = 0x72; F4 = 0x73; F5 = 0x74; F6 = 0x75; F7 = 0x76; F8 = 0x77; F9 = 0x78; F10 = 0x79; F11 = 0x7a;
    F12 = 0x7b; F13 = 0x7c; F14 = 0x7d; F15 = 0x7e; F16 = 0x7f; F17 = 0x80; F18 = 0x81; F19 = 0x82; F20 = 0x83; F21 = 0x84;
    F22 = 0x85; F23 = 0x86; F24 = 0x87;
    ArrowLeft = 0x25 ext; ArrowUp = 0x26 ext; ArrowRight = 0x27 ext; ArrowDown = 0x28 ext;
    Home = 0x24 ext; End = 0x23 ext; PageUp = 0x21 ext; PageDown = 0x22 ext; Insert = 0x2d ext; Delete = 0x2e ext;
    Backspace = 0x08; Tab = 0x09;
    /// Return (the main keyboard's Enter).
    Enter = 0x0d;
    Escape = 0x1b; Space = 0x20;
    ShiftLeft = 0x10; ShiftRight = 0x10; ControlLeft = 0x11; ControlRight = 0x11 ext; AltLeft = 0x12; AltRight = 0x12 ext;
    /// The left Windows (or Command) key.
    SuperLeft = 0x5b ext;
    /// The right Windows (or Command) key.
    SuperRight = 0x5c ext;
    /// The menu (Apps) key.
    ContextMenu = 0x5d ext;
    CapsLock = 0x14; NumLock = 0x90 ext; ScrollLock = 0x91; PrintScreen = 0x2c ext; Pause = 0x13;
    Numpad0 = 0x60; Numpad1 = 0x61; Numpad2 = 0x62; Numpad3 = 0x63; Numpad4 = 0x64; Numpad5 = 0x65; Numpad6 = 0x66;
    Numpad7 = 0x67; Numpad8 = 0x68; Numpad9 = 0x69; NumpadMultiply = 0x6a; NumpadAdd = 0x6b; NumpadSubtract = 0x6d;
    NumpadDecimal = 0x6e; NumpadDivide = 0x6f ext; NumpadEnter = 0x0d ext;
    /// `-` and `_`.
    Minus = 0xbd;
    /// `=` and `+`.
    Equals = 0xbb;
    /// `[` and `{`.
    BracketLeft = 0xdb;
    /// `]` and `}`.
    BracketRight = 0xdd;
    /// `\` and `|`.
    Backslash = 0xdc;
    /// `;` and `:`.
    Semicolon = 0xba;
    /// `'` and `"`.
    Apostrophe = 0xde;
    /// `,` and `<`.
    Comma = 0xbc;
    /// `.` and `>`.
    Period = 0xbe;
    /// `/` and `?`.
    Slash = 0xbf;
    /// `` ` `` and `~`.
    Grave = 0xc0;
    /// The extra key next to the left Shift on ISO keyboards (`\` and `|` on a US layout).
    IntlBackslash = 0xe2;
}

impl Key {
    /// The scan code Windows gives the key (the US layout's, `MapVirtualKey`; the right Shift's own 0x36, the ISO key's 0x56,
    /// which the platform layer's own table leaves out so its key code keeps its name).
    pub fn scan_code(self) -> u8 {
        match self {
            Key::ShiftRight => 0x36,
            Key::IntlBackslash => 0x56,
            k => keys::map_virtual_key(k.virtual_key()),
        }
    }

    /// The key that types `c` on a US layout and whether Shift must be held for it (`caps`: Caps Lock is on), as TGE's `ToAscii`
    /// reads the keys ([`keys::to_ascii`]); `'\n'` is Enter. The main keys come before the numpad's. None: no key types it.
    pub fn for_char(c: char, caps: bool) -> Option<(Key, bool)> {
        let c = if c == '\n' { '\r' } else { c };
        let c = u8::try_from(c).ok().filter(|c| *c != 0)?;
        let types = |k: Key, shift: bool| {
            let down = |vk: u8| shift && vk == Key::ShiftLeft.virtual_key();
            keys::to_ascii(k.virtual_key(), &down, caps) == c
        };
        [false, true].into_iter().find_map(|numpad| {
            [false, true].into_iter().find_map(|shift| {
                Key::ALL
                    .iter()
                    .find(|k| k.numpad() == numpad && types(**k, shift))
                    .map(|&k| (k, shift))
            })
        })
    }

    /// Whether the key is on the numeric keypad.
    pub const fn numpad(self) -> bool {
        matches!(
            self,
            Key::Numpad0
                | Key::Numpad1
                | Key::Numpad2
                | Key::Numpad3
                | Key::Numpad4
                | Key::Numpad5
                | Key::Numpad6
                | Key::Numpad7
                | Key::Numpad8
                | Key::Numpad9
                | Key::NumpadMultiply
                | Key::NumpadAdd
                | Key::NumpadSubtract
                | Key::NumpadDecimal
                | Key::NumpadDivide
                | Key::NumpadEnter
        )
    }
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// The first side button (`XBUTTON1`).
    Back,
    /// The second side button (`XBUTTON2`).
    Forward,
}

/// The message of a key: `(message, wParam, lParam)` as Windows posts it (the scan code, the extended bit, the previous-state bit of a
/// repeat, the up bits). With Alt held (and for F10, and Alt itself) Windows sends a system key.
pub fn key_message(key: Key, down: bool, repeat: bool, alt_held: bool) -> (u32, u32, u32) {
    let vk = key.virtual_key();
    let mut l = 1 | (u32::from(key.scan_code()) << 16);
    if key.extended() {
        l |= 1 << 24;
    }
    if !down {
        l |= (1 << 30) | (1 << 31);
    } else if repeat {
        l |= 1 << 30;
    }
    let system = alt_held || matches!(key, Key::AltLeft | Key::AltRight | Key::F10);
    let msg = match (down, system) {
        (true, false) => WM_KEYDOWN,
        (false, false) => WM_KEYUP,
        (true, true) => WM_SYSKEYDOWN,
        (false, true) => WM_SYSKEYUP,
    };
    (msg, u32::from(vk), l)
}

/// The message of a mouse button at the client position `at` (a side button's wParam names it: `XBUTTON1` or `XBUTTON2`).
pub fn button_message(button: MouseButton, down: bool, at: (i32, i32)) -> (u32, u32, u32) {
    let (msg, wparam) = match (button, down) {
        (MouseButton::Left, true) => (WM_LBUTTONDOWN, 0),
        (MouseButton::Left, false) => (WM_LBUTTONUP, 0),
        (MouseButton::Right, true) => (WM_RBUTTONDOWN, 0),
        (MouseButton::Right, false) => (WM_RBUTTONUP, 0),
        (MouseButton::Middle, true) => (WM_MBUTTONDOWN, 0),
        (MouseButton::Middle, false) => (WM_MBUTTONUP, 0),
        (MouseButton::Back, true) => (WM_XBUTTONDOWN, 1 << 16),
        (MouseButton::Back, false) => (WM_XBUTTONUP, 1 << 16),
        (MouseButton::Forward, true) => (WM_XBUTTONDOWN, 2 << 16),
        (MouseButton::Forward, false) => (WM_XBUTTONUP, 2 << 16),
    };
    (
        msg,
        wparam,
        ((at.1 as u32 & 0xffff) << 16) | (at.0 as u32 & 0xffff),
    )
}

#[cfg(test)]
#[path = "input_key_tests.rs"]
mod tests;
