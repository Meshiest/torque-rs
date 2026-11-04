use super::*;

fn fixed() -> Display {
    TgeScreen.fixed_display()
}

fn mode(choice: Choice) -> (u32, u32, u32, u32, bool, bool) {
    match choice {
        Choice::Mode { mode: m, clamped } => {
            (m.width, m.height, m.bpp, m.hz, m.full_screen, clamped)
        }
        Choice::TooSmall => panic!("too small"),
    }
}

#[test]
fn a_window_takes_its_size_and_the_desktops_depth() {
    assert_eq!(
        mode(choose(&fixed(), 1024, 768, 16, 60, false)),
        (1024, 768, 32, 60, false, false)
    );
    assert_eq!(
        mode(choose(&fixed(), 1000, 700, 0, 60, false)),
        (1000, 700, 32, 60, false, false)
    );
}

#[test]
fn a_window_as_large_as_the_desktop_takes_the_last_mode_under_it() {
    // 1920x1080 is not narrower than the desktop: the last listed mode that is.
    assert_eq!(
        mode(choose(&fixed(), 1920, 1080, 32, 60, false)),
        (1600, 900, 32, 60, false, true)
    );
    assert_eq!(
        mode(choose(&fixed(), 1919, 1079, 32, 60, false)),
        (1919, 1079, 32, 60, false, false)
    );
    // A negative size is a huge one.
    assert!(mode(choose(&fixed(), -1i32 as u32, 600, 32, 60, false)).5);
}

#[test]
fn under_480_by_360_is_refused() {
    assert_eq!(choose(&fixed(), 479, 360, 32, 60, false), Choice::TooSmall);
    assert_eq!(choose(&fixed(), 480, 359, 32, 60, true), Choice::TooSmall);
    assert_eq!(
        mode(choose(&fixed(), 480, 360, 32, 60, false)),
        (480, 360, 32, 60, false, false)
    );
}

#[test]
fn full_screen_takes_the_listed_mode_or_the_first_nearest() {
    assert_eq!(
        mode(choose(&fixed(), 1280, 1024, 32, 60, true)),
        (1280, 1024, 32, 60, true, false)
    );
    // Another depth costs 1: the listed 32-bit mode of that size.
    assert_eq!(
        mode(choose(&fixed(), 1920, 1080, 16, 60, true)),
        (1920, 1080, 32, 60, true, false)
    );
    assert_eq!(
        mode(choose(&fixed(), 1000, 700, 32, 60, true)),
        (1024, 768, 32, 60, true, false)
    );
    assert_eq!(
        mode(choose(&fixed(), 2560, 1440, 32, 60, true)),
        (1920, 1080, 32, 60, true, false)
    );
}

#[test]
fn torques_rules_take_five_arguments_and_the_three_word_resolution() {
    let rules = TgeScreen;
    assert_eq!(rules.usage().1, 5);
    assert!(rules.call_line(800, 600, 32, false).is_none());
    let m = ScreenMode {
        width: 800,
        height: 600,
        bpp: 32,
        hz: 60,
        full_screen: false,
        borderless: false,
    };
    assert_eq!(rules.resolution_text(&m), "800 600 32");
}

fn monitor(size: (u32, u32)) -> Monitor {
    Monitor {
        size,
        video_modes: vec![(640, 480), (800, 600), (1024, 768), size],
    }
}

#[test]
fn a_monitor_lists_the_window_modes_that_fit_and_its_own_size() {
    assert_eq!(
        monitor((1366, 768)).modes(&TgeScreen),
        [(1280, 720), (1366, 768)]
    );
    // A monitor under 480x360 lists nothing and keeps the fixed list.
    assert!(monitor((400, 300)).modes(&TgeScreen).is_empty());
    assert_eq!(
        Display::of_monitor(&monitor((400, 300)), &TgeScreen),
        fixed()
    );
}

#[test]
fn the_fresh_profile_window_is_720p_or_the_largest_mode_under_the_desktop() {
    assert_eq!(monitor((1920, 1080)).start_size(&TgeScreen), (1280, 720));
    assert_eq!(monitor((1366, 768)).start_size(&TgeScreen), (1280, 720));
    assert_eq!(monitor((2560, 1440)).start_size(&TgeScreen), (1280, 720));
    // A game's rules name their own start mode.
    struct Wide;
    impl ScreenRules for Wide {
        fn start_mode(&self) -> (u32, u32) {
            (1600, 900)
        }
    }
    assert_eq!(monitor((1920, 1080)).start_size(&Wide), (1600, 900));
    // Not under the desktop: the largest listed mode that is.
    assert_eq!(monitor((1366, 768)).start_size(&Wide), (1280, 720));
    // Nothing is under it: the smallest listed mode.
    assert_eq!(monitor((1280, 720)).start_size(&TgeScreen), (1280, 720));
}
