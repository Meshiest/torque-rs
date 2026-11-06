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

#[test]
fn the_render_scale_is_read_strictly() {
    assert_eq!(RenderScale::parse("auto"), Some(RenderScale::Auto));
    assert_eq!(RenderScale::parse("0.75"), Some(RenderScale::Factor(0.75)));
    assert_eq!(RenderScale::parse("1"), Some(RenderScale::Factor(1.0)));
    for bad in ["", "Auto", "0", "0.1", "1.5", "-1", "NaN", "inf", "75%"] {
        assert_eq!(RenderScale::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn auto_draws_every_pixel_up_to_a_1080p_screens_worth_then_keeps_the_shape() {
    assert_eq!(drawn_size((1400, 900), RenderScale::Auto), (1400, 900));
    assert_eq!(drawn_size((1920, 1080), RenderScale::Auto), (1920, 1080));
    let (w, h) = drawn_size((2560, 1080), RenderScale::Auto);
    assert!(u64::from(w) * u64::from(h) <= AUTO_PIXELS, "{w}x{h}");
    assert!(
        u64::from(w) * u64::from(h) > AUTO_PIXELS * 99 / 100,
        "{w}x{h}"
    );
    assert!((f64::from(w) / f64::from(h) - 2560.0 / 1080.0).abs() < 0.01);
    // a phone at three device pixels a CSS pixel
    let (w, h) = drawn_size((1170, 2532), RenderScale::Auto);
    assert!(u64::from(w) * u64::from(h) <= AUTO_PIXELS, "{w}x{h}");
    assert_eq!(
        drawn_size((1000, 600), RenderScale::Factor(0.5)),
        (500, 300)
    );
    assert_eq!(drawn_size((1, 1), RenderScale::Factor(0.25)), (1, 1));
    assert_eq!(drawn_size((0, 0), RenderScale::Auto), (1, 1));
}

#[test]
fn a_mouse_position_is_in_the_drawn_pixels() {
    assert_eq!(
        drawn_point((500.0, 300.0), (1000, 600), (500, 300)),
        (250, 150)
    );
    assert_eq!(drawn_point((10.0, 20.0), (800, 600), (800, 600)), (10, 20));
}
