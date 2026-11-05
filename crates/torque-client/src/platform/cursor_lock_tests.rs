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
fn focus_loss_frees_the_cursor_and_focus_regained_holds_it_again_while_the_game_wants_it() {
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
fn focus_regained_holds_only_when_the_game_wants_the_hold() {
    let mut l = CursorLock::new();
    assert!(l.pass(true, true, (5, 5), 0).is_some());
    assert!(l.on_focus(false).is_some());
    // the game let go meanwhile (the menus): nothing to hold when the focus comes back.
    l.pass(false, true, (5, 5), 16);
    l.on_focus(true);
    assert_eq!(l.pass(false, true, (5, 5), 32), None);
    // or a free-cursor option.
    assert_eq!(l.pass(true, false, (5, 5), 48), None);
    assert!(!l.held());
}

#[test]
fn a_window_that_never_had_the_focus_holds_nothing() {
    let mut l = CursorLock::new();
    assert!(l.on_focus(false).is_none());
    for t in 0..50u64 {
        assert_eq!(l.pass(false, true, (5, 5), t * 10), None);
        assert_eq!(l.pass(true, true, (5, 5), t * 10 + 5), None);
        assert_eq!(l.on_click(), None, "a click on an unfocused window");
    }
    assert!(!l.held());
    // it holds when it gets the focus, not before.
    l.on_focus(true);
    assert!(matches!(
        l.pass(true, true, (5, 5), 1000),
        Some(Action::Hold { .. })
    ));
}

#[test]
fn a_hidden_window_lets_go_and_never_holds_until_it_is_shown_and_focused() {
    let mut l = CursorLock::new();
    assert!(l.pass(true, true, (5, 5), 0).is_some());
    assert_eq!(
        l.on_visible(false),
        Some(Action::Free {
            restore: None,
            show: true
        })
    );
    for t in 1..20u64 {
        assert_eq!(l.pass(true, true, (5, 5), t * 100), None, "hidden at {t}");
        assert_eq!(l.on_click(), None, "a click on a hidden window");
    }
    // shown but still unfocused: nothing.
    assert!(l.on_focus(false).is_none());
    assert_eq!(l.on_visible(true), None);
    assert_eq!(l.pass(true, true, (5, 5), 3000), None);
    l.on_focus(true);
    assert!(l.pass(true, true, (5, 5), 3016).is_some());
}

#[test]
fn guard_never_locks_or_moves_an_unfocused_or_hidden_window() {
    let hold = Action::Hold { keep: (1, 2) };
    let centre = Action::Centre { renew: true };
    let moved = Action::Free {
        restore: Some((1, 2)),
        show: false,
    };
    let freed = Action::Free {
        restore: None,
        show: true,
    };
    for (focused, visible) in [(false, true), (true, false), (false, false)] {
        assert_eq!(guard(hold, focused, visible), freed);
        assert_eq!(guard(centre, focused, visible), freed);
        assert_eq!(
            guard(Action::Centre { renew: false }, focused, visible),
            freed
        );
        assert_eq!(
            guard(moved, focused, visible),
            Action::Free {
                restore: None,
                show: false
            },
            "released, but the cursor is not put anywhere"
        );
    }
    // Focused and shown: the hold goes through as the lock decided it.
    for action in [hold, centre, moved, freed] {
        assert_eq!(guard(action, true, true), action);
    }
}

#[test]
fn a_browser_holds_only_right_after_a_click_and_never_on_a_timer() {
    let mut l = CursorLock::click_required();
    assert_eq!(l.pass(true, true, (5, 5), 0), None, "no click: no lock");
    assert_eq!(l.pass(true, true, (5, 5), 5000), None);
    // The click's pass holds; a click is good for one pass.
    assert_eq!(l.on_click(), None);
    assert!(matches!(
        l.pass(true, true, (5, 5), 5016),
        Some(Action::Hold { .. })
    ));
    // Held for a long while: never renewed on a timer (Esc leaves the lock and nothing asks for it again).
    for t in 1..40u64 {
        assert_eq!(
            l.pass(true, true, (5, 5), 5016 + t * RENEW_MS),
            Some(Action::Centre { renew: false }),
            "at {t}"
        );
    }
    // A click while held asks for it again, in the click's own event.
    assert_eq!(l.on_click(), Some(Action::Centre { renew: true }));
    assert_eq!(
        l.pass(true, true, (5, 5), 5016 + 41 * RENEW_MS),
        Some(Action::Centre { renew: false })
    );
    // Focus lost and back: a browser needs a click again.
    assert!(l.on_focus(false).is_some());
    l.on_focus(true);
    assert_eq!(l.pass(true, true, (5, 5), 30_000), None);
    l.on_click();
    assert!(matches!(
        l.pass(true, true, (5, 5), 30_016),
        Some(Action::Hold { .. })
    ));
}

#[test]
fn a_click_is_good_for_the_next_pass_only() {
    let mut l = CursorLock::click_required();
    l.on_click();
    assert_eq!(l.pass(false, true, (5, 5), 0), None, "the menus: free");
    assert_eq!(l.pass(true, true, (5, 5), 16), None, "a stale click");
}

#[test]
fn an_unfocused_window_never_takes_the_cursor_however_long_it_waits() {
    // the renewal (every RENEW_MS) is a held pass's; an unfocused or hidden window holds nothing.
    let mut l = CursorLock::new();
    assert!(l.pass(true, true, (5, 5), 0).is_some());
    assert!(l.on_focus(false).is_some());
    for t in 1..100u64 {
        assert_eq!(l.pass(true, true, (5, 5), t * 100), None, "at {t}");
        assert_eq!(l.on_click(), None);
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
