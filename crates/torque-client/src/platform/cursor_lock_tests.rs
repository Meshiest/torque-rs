use super::*;

#[test]
fn the_game_taking_the_mouse_holds_the_cursor_at_the_centre_every_pass() {
    let mut l = CursorLock::new();
    assert_eq!(l.pass(false, true, (10, 20), 0), None, "the menus: free");
    assert_eq!(
        l.pass(true, true, (10, 20), 16),
        Some(Action::Hold { keep: (10, 20) })
    );
    assert!(l.held());
    // Every pass after it puts the cursor back at the centre (CheckCursorPos), the clip renewed every RENEW_MS.
    assert_eq!(
        l.pass(true, true, (400, 300), 32),
        Some(Action::Centre { renew: false })
    );
    assert_eq!(
        l.pass(true, true, (400, 300), 16 + RENEW_MS),
        Some(Action::Centre { renew: true })
    );
    // The game lets go: back where the cursor was.
    assert_eq!(
        l.pass(false, true, (400, 300), 600),
        Some(Action::Free {
            restore: Some((10, 20)),
            show: false,
        })
    );
    assert_eq!(l.pass(false, true, (10, 20), 616), None);
}

#[test]
fn focus_loss_frees_the_cursor_and_focus_regained_holds_it_again() {
    let mut l = CursorLock::new();
    assert!(matches!(
        l.pass(true, true, (5, 5), 0),
        Some(Action::Hold { .. })
    ));
    // freed at once, in the focus event, and shown.
    assert_eq!(
        l.on_focus(false),
        Some(Action::Free {
            restore: None,
            show: true
        })
    );
    assert!(!l.held());
    assert_eq!(l.pass(true, true, (5, 5), 16), None, "unfocused: nothing");
    assert_eq!(l.pass(true, true, (5, 5), 32), None, "unfocused: nothing");
    l.on_focus(true);
    // Held again at once, with the place first kept (the game never let go).
    assert_eq!(
        l.pass(true, true, (90, 90), 48),
        Some(Action::Hold { keep: (5, 5) })
    );
    // A refocus while held (a focus event pair between passes): freed, then held again at the next pass.
    assert!(l.on_focus(false).is_some());
    assert_eq!(l.on_focus(true), None);
    assert_eq!(
        l.pass(true, true, (90, 90), 64),
        Some(Action::Hold { keep: (5, 5) })
    );
}

#[test]
fn an_unfocused_window_never_takes_the_cursor_however_long_it_waits() {
    // the renewal (every RENEW_MS) is a held pass's; an unfocused or hidden window holds nothing.
    let mut l = CursorLock::new();
    assert!(l.pass(true, true, (5, 5), 0).is_some());
    assert!(l.on_focus(false).is_some());
    for t in 1..100u64 {
        assert_eq!(l.pass(true, true, (5, 5), t * 100), None, "at {t}");
        l.on_click();
        l.on_resize();
    }
    assert!(!l.held());
}

#[test]
fn a_windows_key_lets_go_until_the_focus_comes_back_or_a_click() {
    // the snipping overlay of Win+Shift+S may not take the focus.
    let mut l = CursorLock::new();
    assert!(l.pass(true, true, (5, 5), 0).is_some());
    assert_eq!(
        l.on_system_key(),
        Some(Action::Free {
            restore: None,
            show: true
        })
    );
    for t in 1..20u64 {
        assert_eq!(
            l.pass(true, true, (5, 5), t * 100),
            None,
            "focused, but let go"
        );
    }
    // A click in the game view takes it again.
    l.on_click();
    assert!(matches!(
        l.pass(true, true, (5, 5), 3000),
        Some(Action::Hold { .. })
    ));
    // Or the focus coming back (the overlay took it after all).
    assert!(l.on_system_key().is_some());
    assert!(l.on_focus(false).is_none(), "already free");
    assert_eq!(l.pass(true, true, (5, 5), 3100), None);
    l.on_focus(true);
    assert!(matches!(
        l.pass(true, true, (5, 5), 3200),
        Some(Action::Hold { .. })
    ));
    // A click while unfocused takes nothing.
    assert!(l.on_focus(false).is_some());
    l.on_click();
    assert_eq!(l.pass(true, true, (5, 5), 3300), None);
}

#[test]
fn free_cursor_never_clips_or_moves_the_cursor() {
    let mut l = CursorLock::new();
    for t in 0..10 {
        assert_eq!(l.pass(true, false, (1, 1), t * 100), None);
    }
    assert_eq!(l.pass(false, false, (1, 1), 2000), None);
    assert!(!l.held());
}
