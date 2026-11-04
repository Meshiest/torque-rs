use super::*;
use crate::input::{WM_KEYDOWN, WM_KEYUP, WM_LBUTTONUP, WM_SYSKEYDOWN, WM_XBUTTONDOWN};
use winit::event::MouseButton as WinitButton;

#[test]
fn the_wheel_posts_one_message_a_whole_notch() {
    // a notch is one message of 120; parts of a notch add up; the sign is kept.
    let mut w = Wheel::default();
    let deltas = |m: Vec<(u32, u32, u32)>| -> Vec<i16> {
        m.iter().map(|x| (x.1 >> 16) as u16 as i16).collect()
    };
    assert_eq!(deltas(w.messages(1.0)), vec![120]);
    assert_eq!(deltas(w.messages(-2.0)), vec![-120, -120]);
    assert!(w.messages(0.25).is_empty());
    assert!(w.messages(0.5).is_empty());
    assert_eq!(deltas(w.messages(0.25)), vec![120]);
    assert!(w.messages(-0.75).is_empty());
    assert_eq!(deltas(w.messages(-0.25)), vec![-120]);
    assert_eq!(w.messages(1e9).len(), MAX_WHEEL_NOTCHES as usize);
    assert!(w.messages(f32::NAN).is_empty());
    assert_eq!(wheel_message(-1.0).unwrap().1 >> 16, 0xff88);
}

#[test]
fn the_window_drops_synthetic_presses_and_lets_go_of_held_buttons_on_focus_loss() {
    let mut w = WindowInput::default();
    // A synthetic press (a key found held as the focus came) posts nothing and holds nothing; its release is posted.
    assert!(w.key(Key::AltLeft, true, false, true).is_none());
    assert!(!w.alt());
    let (m, _, _) = w.key(Key::Z, true, false, false).unwrap();
    assert_eq!(m, WM_KEYDOWN, "no Alt held: a plain key");
    let (m, _, _) = w.key(Key::ControlLeft, false, false, true).unwrap();
    assert_eq!(m, WM_KEYUP);
    // A real Alt makes the next keys system keys until it is let go.
    w.key(Key::AltLeft, true, false, false).unwrap();
    assert_eq!(
        w.key(Key::Numpad8, true, false, false).unwrap().0,
        WM_SYSKEYDOWN
    );
    w.key(Key::AltLeft, false, false, false).unwrap();
    assert_eq!(
        w.key(Key::Numpad8, false, false, false).unwrap().0,
        WM_KEYUP
    );
    // The buttons held when the focus goes are let go (`WM_ACTIVATE` inactive), once.
    w.button(MouseButton::Left, true, (5, 6));
    w.button(MouseButton::Right, true, (5, 6));
    w.button(MouseButton::Right, false, (5, 6));
    let ups = w.focus_lost((7, 8));
    assert_eq!(ups, vec![(WM_LBUTTONUP, 0, (8 << 16) | 7)]);
    assert!(w.focus_lost((7, 8)).is_empty());
    // Line and pixel deltas: a notch is 120 either way.
    use winit::dpi::PhysicalPosition;
    assert_eq!(w.wheel(MouseScrollDelta::LineDelta(0.0, 1.0)).len(), 1);
    assert!(
        w.wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            0.0, -60.0
        )))
        .is_empty()
    );
    let m = w.wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
        0.0, -60.0,
    )));
    assert_eq!(m.len(), 1);
    assert_eq!((m[0].1 >> 16) as u16 as i16, -120);
}

#[test]
fn every_key_the_window_mapped_before_keeps_its_virtual_key() {
    // The window's table before the engine's keys: each winit key code with its virtual key and extended bit.
    let before: &[(KeyCode, u8, bool)] = &[
        (KeyCode::KeyA, b'A', false),
        (KeyCode::KeyB, b'B', false),
        (KeyCode::KeyC, b'C', false),
        (KeyCode::KeyD, b'D', false),
        (KeyCode::KeyE, b'E', false),
        (KeyCode::KeyF, b'F', false),
        (KeyCode::KeyG, b'G', false),
        (KeyCode::KeyH, b'H', false),
        (KeyCode::KeyI, b'I', false),
        (KeyCode::KeyJ, b'J', false),
        (KeyCode::KeyK, b'K', false),
        (KeyCode::KeyL, b'L', false),
        (KeyCode::KeyM, b'M', false),
        (KeyCode::KeyN, b'N', false),
        (KeyCode::KeyO, b'O', false),
        (KeyCode::KeyP, b'P', false),
        (KeyCode::KeyQ, b'Q', false),
        (KeyCode::KeyR, b'R', false),
        (KeyCode::KeyS, b'S', false),
        (KeyCode::KeyT, b'T', false),
        (KeyCode::KeyU, b'U', false),
        (KeyCode::KeyV, b'V', false),
        (KeyCode::KeyW, b'W', false),
        (KeyCode::KeyX, b'X', false),
        (KeyCode::KeyY, b'Y', false),
        (KeyCode::KeyZ, b'Z', false),
        (KeyCode::Digit0, b'0', false),
        (KeyCode::Digit1, b'1', false),
        (KeyCode::Digit2, b'2', false),
        (KeyCode::Digit3, b'3', false),
        (KeyCode::Digit4, b'4', false),
        (KeyCode::Digit5, b'5', false),
        (KeyCode::Digit6, b'6', false),
        (KeyCode::Digit7, b'7', false),
        (KeyCode::Digit8, b'8', false),
        (KeyCode::Digit9, b'9', false),
        (KeyCode::Backspace, 0x08, false),
        (KeyCode::Tab, 0x09, false),
        (KeyCode::Enter, 0x0d, false),
        (KeyCode::NumpadEnter, 0x0d, true),
        (KeyCode::ShiftLeft, 0x10, false),
        (KeyCode::ShiftRight, 0x10, false),
        (KeyCode::ControlLeft, 0x11, false),
        (KeyCode::ControlRight, 0x11, true),
        (KeyCode::AltLeft, 0x12, false),
        (KeyCode::AltRight, 0x12, true),
        (KeyCode::Pause, 0x13, false),
        (KeyCode::CapsLock, 0x14, false),
        (KeyCode::Escape, 0x1b, false),
        (KeyCode::Space, 0x20, false),
        (KeyCode::PageUp, 0x21, true),
        (KeyCode::PageDown, 0x22, true),
        (KeyCode::End, 0x23, true),
        (KeyCode::Home, 0x24, true),
        (KeyCode::ArrowLeft, 0x25, true),
        (KeyCode::ArrowUp, 0x26, true),
        (KeyCode::ArrowRight, 0x27, true),
        (KeyCode::ArrowDown, 0x28, true),
        (KeyCode::PrintScreen, 0x2c, true),
        (KeyCode::Insert, 0x2d, true),
        (KeyCode::Delete, 0x2e, true),
        (KeyCode::SuperLeft, 0x5b, true),
        (KeyCode::SuperRight, 0x5c, true),
        (KeyCode::ContextMenu, 0x5d, true),
        (KeyCode::Numpad0, 0x60, false),
        (KeyCode::Numpad1, 0x61, false),
        (KeyCode::Numpad2, 0x62, false),
        (KeyCode::Numpad3, 0x63, false),
        (KeyCode::Numpad4, 0x64, false),
        (KeyCode::Numpad5, 0x65, false),
        (KeyCode::Numpad6, 0x66, false),
        (KeyCode::Numpad7, 0x67, false),
        (KeyCode::Numpad8, 0x68, false),
        (KeyCode::Numpad9, 0x69, false),
        (KeyCode::NumpadMultiply, 0x6a, false),
        (KeyCode::NumpadAdd, 0x6b, false),
        (KeyCode::NumpadSubtract, 0x6d, false),
        (KeyCode::NumpadDecimal, 0x6e, false),
        (KeyCode::NumpadDivide, 0x6f, true),
        (KeyCode::F1, 0x70, false),
        (KeyCode::F2, 0x71, false),
        (KeyCode::F3, 0x72, false),
        (KeyCode::F4, 0x73, false),
        (KeyCode::F5, 0x74, false),
        (KeyCode::F6, 0x75, false),
        (KeyCode::F7, 0x76, false),
        (KeyCode::F8, 0x77, false),
        (KeyCode::F9, 0x78, false),
        (KeyCode::F10, 0x79, false),
        (KeyCode::F11, 0x7a, false),
        (KeyCode::F12, 0x7b, false),
        (KeyCode::NumLock, 0x90, true),
        (KeyCode::ScrollLock, 0x91, false),
        (KeyCode::Semicolon, 0xba, false),
        (KeyCode::Equal, 0xbb, false),
        (KeyCode::Comma, 0xbc, false),
        (KeyCode::Minus, 0xbd, false),
        (KeyCode::Period, 0xbe, false),
        (KeyCode::Slash, 0xbf, false),
        (KeyCode::Backquote, 0xc0, false),
        (KeyCode::BracketLeft, 0xdb, false),
        (KeyCode::Backslash, 0xdc, false),
        (KeyCode::BracketRight, 0xdd, false),
        (KeyCode::Quote, 0xde, false),
        (KeyCode::IntlBackslash, 0xe2, false),
    ];
    for &(code, vk, extended) in before {
        let key = Key::from_winit(code).unwrap_or_else(|| panic!("{code:?} lost"));
        assert_eq!(
            (key.virtual_key(), key.extended()),
            (vk, extended),
            "{code:?}"
        );
    }
    // Only F13..F24 are new; the media keys still post nothing.
    assert!(added_all().iter().all(|c| Key::from_winit(*c).is_some()));
    assert!(Key::from_winit(KeyCode::MediaPlayPause).is_none());
    assert!(Key::from_winit(KeyCode::BrowserBack).is_none());
    assert_eq!(Key::ALL.len(), before.len() + 12);
    // Every key has its winit code: none of the engine's keys is out of the window's reach.
    let reached: std::collections::HashSet<Key> = before
        .iter()
        .map(|b| b.0)
        .chain(added_all())
        .filter_map(Key::from_winit)
        .collect();
    assert_eq!(reached.len(), Key::ALL.len());
}

/// The keys the window's table gained with the engine's keys.
fn added_all() -> [KeyCode; 12] {
    use KeyCode as C;
    [
        C::F13,
        C::F14,
        C::F15,
        C::F16,
        C::F17,
        C::F18,
        C::F19,
        C::F20,
        C::F21,
        C::F22,
        C::F23,
        C::F24,
    ]
}

#[test]
fn the_side_buttons_post_x_button_messages() {
    let mut w = WindowInput::default();
    let back = MouseButton::from_winit(WinitButton::Back).unwrap();
    let (m, wp, _) = w.button(back, true, (1, 2));
    assert_eq!((m, wp >> 16), (WM_XBUTTONDOWN, 1));
    assert!(
        w.focus_lost((1, 2)).is_empty(),
        "the side buttons are not let go on focus loss"
    );
    assert!(MouseButton::from_winit(WinitButton::Other(7)).is_none());
}
