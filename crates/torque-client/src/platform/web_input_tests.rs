use super::*;

#[test]
fn held_keys_follow_presses_and_releases() {
    let mut held = HeldKeys::default();
    held.key(Key::Space, true);
    held.key(Key::W, true);
    // a key's repeat is no second hold
    held.key(Key::Space, true);
    held.key(Key::W, false);
    held.key(Key::A, true);
    assert_eq!(held.take(), vec![Key::Space, Key::A]);
    assert!(held.is_empty());
    assert_eq!(held.take(), Vec::<Key>::new());
}

#[test]
fn a_release_of_a_key_never_seen_down_holds_nothing() {
    let mut held = HeldKeys::default();
    held.key(Key::Space, false);
    assert!(held.is_empty());
}

#[test]
fn a_lost_lock_is_the_games_escape_only_while_it_holds_the_mouse() {
    assert!(lock_loss_is_escape(true, None, 10_000));
    // the game let go of the mouse itself (its menu is open): no Esc
    assert!(!lock_loss_is_escape(false, None, 10_000));
}

#[test]
fn an_escape_that_reached_the_game_is_not_run_again() {
    assert!(!lock_loss_is_escape(true, Some(9_800), 10_000));
    assert!(!lock_loss_is_escape(
        true,
        Some(10_000 - ESCAPE_WINDOW_MS + 1),
        10_000
    ));
    assert!(lock_loss_is_escape(
        true,
        Some(10_000 - ESCAPE_WINDOW_MS),
        10_000
    ));
}
