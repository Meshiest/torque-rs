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

/// Every key: its virtual key and the key code the engine's platform layer makes of it, by TGE's name.
fn key_table() -> Vec<(Key, u8, u16)> {
    use Key as K;
    let mut table: Vec<(Key, u8, u16)> = vec![
        (K::ArrowLeft, 0x25, named("left")),
        (K::ArrowUp, 0x26, named("up")),
        (K::ArrowRight, 0x27, named("right")),
        (K::ArrowDown, 0x28, named("down")),
        (K::Home, 0x24, named("home")),
        (K::End, 0x23, named("end")),
        (K::PageUp, 0x21, named("pageup")),
        (K::PageDown, 0x22, named("pagedown")),
        (K::Insert, 0x2d, named("insert")),
        (K::Delete, 0x2e, named("delete")),
        (K::Backspace, 0x08, named("backspace")),
        (K::Tab, 0x09, named("tab")),
        (K::Enter, 0x0d, named("return")),
        (K::Escape, 0x1b, named("escape")),
        (K::Space, 0x20, named("space")),
        (K::ShiftLeft, 0x10, named("lshift")),
        (K::ShiftRight, 0x10, named("rshift")),
        (K::ControlLeft, 0x11, named("lcontrol")),
        (K::ControlRight, 0x11, named("rcontrol")),
        (K::AltLeft, 0x12, named("lalt")),
        (K::AltRight, 0x12, named("ralt")),
        (K::SuperLeft, 0x5b, named("win_lwindow")),
        (K::SuperRight, 0x5c, named("win_rwindow")),
        (K::ContextMenu, 0x5d, named("win_apps")),
        (K::CapsLock, 0x14, named("capslock")),
        (K::NumLock, 0x90, named("numlock")),
        (K::ScrollLock, 0x91, named("scrolllock")),
        (K::PrintScreen, 0x2c, named("print")),
        (K::Pause, 0x13, named("pause")),
        (K::NumpadMultiply, 0x6a, named("numpadmult")),
        (K::NumpadAdd, 0x6b, named("numpadadd")),
        (K::NumpadSubtract, 0x6d, named("numpadminus")),
        (K::NumpadDecimal, 0x6e, named("numpaddecimal")),
        (K::NumpadDivide, 0x6f, named("numpaddivide")),
        (K::NumpadEnter, 0x0d, named("numpadenter")),
        (K::Minus, 0xbd, named("minus")),
        (K::Equals, 0xbb, named("equals")),
        (K::BracketLeft, 0xdb, named("lbracket")),
        (K::BracketRight, 0xdd, named("rbracket")),
        (K::Backslash, 0xdc, named("backslash")),
        (K::Semicolon, 0xba, named("semicolon")),
        (K::Apostrophe, 0xde, named("apostrophe")),
        (K::Comma, 0xbc, named("comma")),
        (K::Period, 0xbe, named("period")),
        (K::Slash, 0xbf, named("slash")),
        (K::Grave, 0xc0, named("tilde")),
        (K::IntlBackslash, 0xe2, named("lessthan")),
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
        table.push((k, b'A' + i as u8, KEY_A + i as u16));
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
        table.push((digits[i], b'0' + i as u8, KEY_0 + i as u16));
        table.push((numpad[i], 0x60 + i as u8, named(&format!("numpad{i}"))));
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
        table.push((k, 0x70 + i as u8, KEY_F1 + i as u16));
    }
    table
}

#[test]
fn every_key_is_its_virtual_key_and_has_a_key_code_of_its_own() {
    let table = key_table();
    assert_eq!(table.len(), Key::ALL.len(), "every key in the table");
    let mut codes = std::collections::HashMap::new();
    for (key, vk, code) in table {
        assert_eq!(key.virtual_key(), vk, "{key:?}");
        let mut w = Win32::default();
        let ev = event(&mut w, key, true);
        assert_eq!(ev.obj_inst, code, "{key:?}");
        assert_eq!(event(&mut w, key, false).obj_inst, code, "{key:?} up");
        if let Some(other) = codes.insert(code, key) {
            panic!("{key:?} and {other:?} share {code:#x}");
        }
    }
}

#[test]
fn tge_s_key_codes_keep_its_window_procedure_s_quirks() {
    // (key, TGE's code by name; None: one TGE reads no key code for). The other keys keep the engine's code.
    let mut quirks: Vec<(Key, Option<u16>)> = vec![
        // Its scan code is Num Lock's (the scan code table reads it as `numlock`).
        (Key::Pause, Some(named("numlock"))),
        // Its scan code is the slash's (the extended bit tells them apart, and the scan code table reads none).
        (Key::NumpadDivide, Some(named("slash"))),
        // `print` is TGE's code for `VK_PRINT` (0x2a), not for this key's `VK_SNAPSHOT`.
        (Key::PrintScreen, None),
        (Key::SuperLeft, None),
        (Key::SuperRight, None),
        (Key::ContextMenu, None),
    ];
    // F16 and up have scan codes the scan code table does not read.
    quirks.extend(
        [
            Key::F16,
            Key::F17,
            Key::F18,
            Key::F19,
            Key::F20,
            Key::F21,
            Key::F22,
            Key::F23,
            Key::F24,
        ]
        .map(|k| (k, None)),
    );
    for (key, _, engine) in key_table() {
        let want = quirks
            .iter()
            .find(|q| q.0 == key)
            .map_or(Some(engine), |q| q.1);
        let mut w = Win32 {
            tge_key_codes: true,
            ..Win32::default()
        };
        let ev = event(&mut w, key, true);
        assert_eq!((ev.obj_inst != 0).then_some(ev.obj_inst), want, "{key:?}");
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
