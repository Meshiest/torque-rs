use super::*;
use crate::screen::TgeScreen;

#[test]
fn full_screen_takes_the_monitors_mode_of_that_size_nearest_the_rate() {
    let mode = ScreenMode {
        width: 1280,
        height: 1024,
        bpp: 32,
        hz: 60,
        full_screen: true,
        borderless: false,
    };
    let modes = [
        (1920, 1080, 32, 60_000),
        (1280, 1024, 32, 75_025),
        (1280, 1024, 16, 60_000),
        (1280, 1024, 32, 59_940),
        (1280, 1024, 32, 60_000),
    ];
    assert_eq!(exclusive_mode(&mode, &modes), Some(4));
    assert_eq!(exclusive_mode(&mode, &modes[..4]), Some(3));
    assert_eq!(exclusive_mode(&mode, &modes[..1]), None, "borderless then");
}

#[test]
fn a_fresh_profile_starts_at_720p_and_a_saved_resolution_is_kept() {
    let on = |size| Monitor {
        size,
        video_modes: vec![size],
    };
    let canvas = (800, 600);
    assert_eq!(
        start_size(true, canvas, Some(&on((2560, 1440))), &TgeScreen),
        (1280, 720)
    );
    // A saved resolution, or no monitor known: the canvas's (the pref's or the default).
    assert_eq!(
        start_size(false, canvas, Some(&on((2560, 1440))), &TgeScreen),
        canvas
    );
    assert_eq!(start_size(true, canvas, None, &TgeScreen), canvas);
}
