use super::*;

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
}

#[test]
fn the_window_drops_synthetic_presses_and_lets_go_of_held_buttons_on_focus_loss() {
    let mut w = WindowInput::default();
    // A synthetic press (a key found held as the focus came) posts nothing and holds nothing; its release is posted.
    assert!(w.key(KeyCode::AltLeft, true, false, true).is_none());
    assert!(!w.alt());
    let (m, _, _) = w.key(KeyCode::KeyZ, true, false, false).unwrap();
    assert_eq!(m, WM_KEYDOWN, "no Alt held: a plain key");
    let (m, _, _) = w.key(KeyCode::ControlLeft, false, false, true).unwrap();
    assert_eq!(m, WM_KEYUP);
    // A real Alt makes the next keys system keys until it is let go.
    w.key(KeyCode::AltLeft, true, false, false).unwrap();
    assert_eq!(
        w.key(KeyCode::Numpad8, true, false, false).unwrap().0,
        WM_SYSKEYDOWN
    );
    w.key(KeyCode::AltLeft, false, false, false).unwrap();
    assert_eq!(
        w.key(KeyCode::Numpad8, false, false, false).unwrap().0,
        WM_KEYUP
    );
    // The buttons held when the focus goes are let go (`WM_ACTIVATE` inactive), once.
    w.button(MouseButton::Left, true, (5, 6)).unwrap();
    w.button(MouseButton::Right, true, (5, 6)).unwrap();
    w.button(MouseButton::Right, false, (5, 6)).unwrap();
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
fn keys_become_the_messages_windows_posts() {
    let (m, w, l) = key_message(KeyCode::KeyW, true, false, false).unwrap();
    assert_eq!(
        (m, w, (l >> 16) & 0xff),
        (WM_KEYDOWN, u32::from(b'W'), 0x11)
    );
    let (_, _, l) = key_message(KeyCode::ShiftRight, true, false, false).unwrap();
    assert_eq!((l >> 16) & 0xff, 0x36);
    let (m, _, l) = key_message(KeyCode::NumpadEnter, false, false, false).unwrap();
    assert_eq!((m, (l >> 24) & 1, l >> 31), (WM_KEYUP, 1, 1));
    assert!(key_message(KeyCode::F13, true, false, false).is_none());
    assert_eq!(wheel_message(-1.0).unwrap().1 >> 16, 0xff88);
}
