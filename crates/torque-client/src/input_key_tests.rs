use super::*;
use crate::input::{Engine, Win32};
use gui::input::InputEvent;
use gui::keys::{KEY_0, KEY_A, KEY_F1, VIRTUAL_MAP};

/// The event the platform layer makes of a key's message.
fn event(w: &mut Win32, key: Key, down: bool) -> InputEvent {
    let (m, wp, l) = key_message(key, down, false, false);
    match w.message(m, wp, l) {
        Some(Engine::Input(ev)) => ev,
        other => panic!("{key:?}: {other:?}"),
    }
}

/// The key code of a TGE key name (`gVirtualMap`).
fn named(name: &str) -> u16 {
    VIRTUAL_MAP
        .iter()
        .find(|r| r.0 == name)
        .unwrap_or_else(|| panic!("{name}"))
        .2
}

#[test]
fn every_key_is_the_virtual_key_and_key_code_tge_uses() {
    use Key as K;
    // (key, virtual key, the key code the platform layer makes of it by TGE's name; None: one TGE reads no key code for).
    let mut table: Vec<(Key, u8, Option<u16>)> = vec![
        (K::ArrowLeft, 0x25, Some(named("left"))),
        (K::ArrowUp, 0x26, Some(named("up"))),
        (K::ArrowRight, 0x27, Some(named("right"))),
        (K::ArrowDown, 0x28, Some(named("down"))),
        (K::Home, 0x24, Some(named("home"))),
        (K::End, 0x23, Some(named("end"))),
        (K::PageUp, 0x21, Some(named("pageup"))),
        (K::PageDown, 0x22, Some(named("pagedown"))),
        (K::Insert, 0x2d, Some(named("insert"))),
        (K::Delete, 0x2e, Some(named("delete"))),
        (K::Backspace, 0x08, Some(named("backspace"))),
        (K::Tab, 0x09, Some(named("tab"))),
        (K::Enter, 0x0d, Some(named("return"))),
        (K::Escape, 0x1b, Some(named("escape"))),
        (K::Space, 0x20, Some(named("space"))),
        (K::ShiftLeft, 0x10, Some(named("lshift"))),
        (K::ShiftRight, 0x10, Some(named("rshift"))),
        (K::ControlLeft, 0x11, Some(named("lcontrol"))),
        (K::ControlRight, 0x11, Some(named("rcontrol"))),
        (K::AltLeft, 0x12, Some(named("lalt"))),
        (K::AltRight, 0x12, Some(named("ralt"))),
        // The Windows and menu keys: TGE's window procedure finds no key code for them.
        (K::SuperLeft, 0x5b, None),
        (K::SuperRight, 0x5c, None),
        (K::ContextMenu, 0x5d, None),
        (K::CapsLock, 0x14, Some(named("capslock"))),
        (K::NumLock, 0x90, Some(named("numlock"))),
        (K::ScrollLock, 0x91, Some(named("scrolllock"))),
        // `print` is TGE's code for `VK_PRINT` (0x2a), not for this key's `VK_SNAPSHOT`.
        (K::PrintScreen, 0x2c, None),
        // Its scan code is Num Lock's (the scan code table reads it as `numlock`).
        (K::Pause, 0x13, Some(named("numlock"))),
        (K::NumpadMultiply, 0x6a, Some(named("numpadmult"))),
        (K::NumpadAdd, 0x6b, Some(named("numpadadd"))),
        (K::NumpadSubtract, 0x6d, Some(named("numpadminus"))),
        (K::NumpadDecimal, 0x6e, Some(named("numpaddecimal"))),
        // Its scan code is the slash's (the extended bit tells them apart, and the scan code table reads none).
        (K::NumpadDivide, 0x6f, Some(named("slash"))),
        (K::NumpadEnter, 0x0d, Some(named("numpadenter"))),
        (K::Minus, 0xbd, Some(named("minus"))),
        (K::Equals, 0xbb, Some(named("equals"))),
        (K::BracketLeft, 0xdb, Some(named("lbracket"))),
        (K::BracketRight, 0xdd, Some(named("rbracket"))),
        (K::Backslash, 0xdc, Some(named("backslash"))),
        (K::Semicolon, 0xba, Some(named("semicolon"))),
        (K::Apostrophe, 0xde, Some(named("apostrophe"))),
        (K::Comma, 0xbc, Some(named("comma"))),
        (K::Period, 0xbe, Some(named("period"))),
        (K::Slash, 0xbf, Some(named("slash"))),
        (K::Grave, 0xc0, Some(named("tilde"))),
        (K::IntlBackslash, 0xe2, Some(named("lessthan"))),
    ];
    let letters = [
        K::A,
        K::B,
        K::C,
        K::D,
        K::E,
        K::F,
        K::G,
        K::H,
        K::I,
        K::J,
        K::K,
        K::L,
        K::M,
        K::N,
        K::O,
        K::P,
        K::Q,
        K::R,
        K::S,
        K::T,
        K::U,
        K::V,
        K::W,
        K::X,
        K::Y,
        K::Z,
    ];
    for (i, k) in letters.into_iter().enumerate() {
        table.push((k, b'A' + i as u8, Some(KEY_A + i as u16)));
    }
    let digits = [
        K::Digit0,
        K::Digit1,
        K::Digit2,
        K::Digit3,
        K::Digit4,
        K::Digit5,
        K::Digit6,
        K::Digit7,
        K::Digit8,
        K::Digit9,
    ];
    let numpad = [
        K::Numpad0,
        K::Numpad1,
        K::Numpad2,
        K::Numpad3,
        K::Numpad4,
        K::Numpad5,
        K::Numpad6,
        K::Numpad7,
        K::Numpad8,
        K::Numpad9,
    ];
    for i in 0..10 {
        table.push((digits[i], b'0' + i as u8, Some(KEY_0 + i as u16)));
        table.push((
            numpad[i],
            0x60 + i as u8,
            Some(named(&format!("numpad{i}"))),
        ));
    }
    let f = [
        K::F1,
        K::F2,
        K::F3,
        K::F4,
        K::F5,
        K::F6,
        K::F7,
        K::F8,
        K::F9,
        K::F10,
        K::F11,
        K::F12,
        K::F13,
        K::F14,
        K::F15,
        K::F16,
        K::F17,
        K::F18,
        K::F19,
        K::F20,
        K::F21,
        K::F22,
        K::F23,
        K::F24,
    ];
    for (i, k) in f.into_iter().enumerate() {
        // F16 and up have scan codes the scan code table does not read.
        let code = (i < 15).then(|| KEY_F1 + i as u16);
        table.push((k, 0x70 + i as u8, code));
    }
    assert_eq!(table.len(), Key::ALL.len(), "every key in the table");
    for (key, vk, code) in table {
        assert_eq!(key.virtual_key(), vk, "{key:?}");
        let mut w = Win32::default();
        let ev = event(&mut w, key, true);
        assert_eq!((ev.obj_inst != 0).then_some(ev.obj_inst), code, "{key:?}");
        assert_eq!(
            event(&mut w, key, false).obj_inst,
            ev.obj_inst,
            "{key:?} up"
        );
    }
}

#[test]
fn keys_become_the_messages_windows_posts() {
    let (m, w, l) = key_message(Key::W, true, false, false);
    assert_eq!(
        (m, w, (l >> 16) & 0xff),
        (WM_KEYDOWN, u32::from(b'W'), 0x11)
    );
    let (_, _, l) = key_message(Key::ShiftRight, true, false, false);
    assert_eq!((l >> 16) & 0xff, 0x36);
    let (m, _, l) = key_message(Key::NumpadEnter, false, false, false);
    assert_eq!((m, (l >> 24) & 1, l >> 31), (WM_KEYUP, 1, 1));
    let (_, _, l) = key_message(Key::A, true, true, false);
    assert_eq!((l >> 30) & 1, 1, "a repeat's previous state");
    assert_eq!(key_message(Key::F10, true, false, false).0, WM_SYSKEYDOWN);
    assert_eq!(key_message(Key::A, false, false, true).0, WM_SYSKEYUP);
    assert_eq!(
        button_message(MouseButton::Right, true, (3, 4)),
        (WM_RBUTTONDOWN, 0, (4 << 16) | 3)
    );
    let (m, w, _) = button_message(MouseButton::Forward, false, (0, 0));
    assert_eq!((m, w >> 16), (WM_XBUTTONUP, 2));
}

#[test]
fn each_character_has_the_key_that_types_it() {
    for caps in [false, true] {
        for c in (0x20u8..0x7f)
            .map(char::from)
            .chain(['\n', '\t', '\x08', '\x1b'])
        {
            let (key, shift) = Key::for_char(c, caps).unwrap_or_else(|| panic!("{c:?}"));
            let mut w = Win32::default();
            if caps {
                event(&mut w, Key::CapsLock, true);
                event(&mut w, Key::CapsLock, false);
            }
            if shift {
                event(&mut w, Key::ShiftLeft, true);
            }
            let want = if c == '\n' { b'\r' } else { c as u8 };
            assert_eq!(
                event(&mut w, key, true).ascii,
                u16::from(want),
                "{c:?} caps {caps}"
            );
            assert!(!key.numpad(), "{c:?}: the main keys first");
        }
    }
    assert_eq!(Key::for_char('Q', false), Some((Key::Q, true)));
    assert_eq!(Key::for_char('Q', true), Some((Key::Q, false)));
    assert!(Key::for_char('\u{e9}', false).is_none());
    assert!(Key::for_char('\0', false).is_none());
}
