use super::*;

/// The window's loop woken every `tick_us` for a second (input, the server's clock, the timer's own lateness), each frame
/// taking `work_us` to draw: the frames drawn.
fn frames_in_a_second(fps: u32, world: bool, tick_us: u64, work_us: u64) -> u32 {
    let start = Instant::now();
    let mut cap = FrameCap::new();
    let mut drawn = 0;
    let mut t = 0u64;
    while t < 1_000_000 {
        let now = start + Duration::from_micros(t);
        if cap.wait_until(now, fps, world).is_none() {
            cap.drawn(now, fps);
            drawn += 1;
            t += work_us;
        }
        t += tick_us;
    }
    drawn
}

#[test]
fn the_menu_draws_about_the_cap_a_second() {
    for fps in [60, 120, 144, 240] {
        let n = frames_in_a_second(fps, false, 1000, 0);
        assert!(
            (fps - fps / 20..=fps + 1).contains(&n),
            "{n} frames a second at a cap of {fps}"
        );
    }
}

#[test]
fn a_frames_work_and_a_late_wake_stay_inside_the_period() {
    // 3 ms of work a frame and wakes 0.5 ms apart: at 120 the menu held about 88 a second when the period ran from a frame's
    // end; on the beat it holds the cap.
    for fps in [120, 240] {
        let n = frames_in_a_second(fps, false, 500, 3_000);
        assert!(
            n + fps / 20 >= fps && n <= fps + 1,
            "{n} frames a second at a cap of {fps} with 3 ms frames"
        );
    }
    // Work longer than the period: as fast as the work allows, no burst after it.
    let n = frames_in_a_second(240, false, 100, 6_000);
    assert!((150..=167).contains(&n), "{n} frames with 6 ms frames");
}

#[test]
fn a_world_or_no_cap_is_never_held_back() {
    assert_eq!(frames_in_a_second(120, true, 1000, 0), 1000);
    assert_eq!(frames_in_a_second(0, false, 1000, 0), 1000);
}

#[test]
fn frames_drawn_past_the_cap_never_push_the_beat_ahead() {
    // A game at 1000 frames a second for ten seconds under a cap of 60 (a world is never held), then the main menu: its first
    // frame at once, the next a period after it.
    let t = Instant::now();
    let mut cap = FrameCap::new();
    for ms in 0..10_000u64 {
        let now = t + Duration::from_millis(ms);
        assert_eq!(cap.wait_until(now, 60, true), None);
        cap.drawn(now, 60);
    }
    let menu = t + Duration::from_millis(10_001);
    let wait = cap.wait_until(menu, 60, false);
    assert!(
        wait.is_none_or(|at| at <= menu + Duration::from_secs_f64(1.0 / 60.0)),
        "the menu waits {:?}",
        wait.map(|at| at - menu)
    );
    // A redraw before the frame's due time does not push the beat either.
    let mut cap = FrameCap::new();
    cap.drawn(t, 100);
    for _ in 0..50 {
        cap.drawn(t + Duration::from_millis(1), 100);
    }
    assert_eq!(
        cap.wait_until(t + Duration::from_millis(2), 100, false),
        Some(t + Duration::from_millis(11))
    );
}

#[test]
fn a_stall_is_not_caught_up_with_a_burst() {
    let t = Instant::now();
    let mut cap = FrameCap::new();
    cap.drawn(t, 100);
    // The next frame came 50 ms late: the one after it is a period after it, not at once.
    let late = t + Duration::from_millis(60);
    assert_eq!(cap.wait_until(late, 100, false), None);
    cap.drawn(late, 100);
    assert_eq!(
        cap.wait_until(late + Duration::from_millis(1), 100, false),
        Some(late + Duration::from_millis(10))
    );
}

#[test]
fn the_wait_ends_at_the_next_frame_time() {
    let t = Instant::now();
    let mut cap = FrameCap::new();
    assert_eq!(cap.wait_until(t, 100, false), None);
    cap.drawn(t, 100);
    assert_eq!(
        cap.wait_until(t + Duration::from_millis(3), 100, false),
        Some(t + Duration::from_millis(10))
    );
    assert_eq!(
        cap.wait_until(t + Duration::from_millis(10), 100, false),
        None
    );
}

#[test]
fn the_preference_reads_as_a_cap() {
    assert_eq!(cap_of(""), Cap::Display);
    assert_eq!(cap_of("0"), Cap::None);
    assert_eq!(cap_of("-5"), Cap::None);
    assert_eq!(cap_of("60"), Cap::Fps(60));
    assert_eq!(cap_of("144.4"), Cap::Fps(144));
    assert_eq!(cap_of("3"), Cap::Fps(10));
    assert_eq!(cap_of("lots"), Cap::Display);
    assert_eq!(cap_of("nan"), Cap::Display);
}

#[test]
fn the_display_cap_follows_the_refresh_rate() {
    assert_eq!(fps_for(Cap::Display, false, Some(239_757)), 240);
    assert_eq!(fps_for(Cap::Display, false, Some(143_998)), 144);
    assert_eq!(fps_for(Cap::Display, false, Some(60_000)), 60);
    assert_eq!(fps_for(Cap::Display, false, None), 240);
    assert_eq!(fps_for(Cap::Display, false, Some(0)), 240);
    // With vertical sync the presents pace the frames: no timer over them.
    assert_eq!(fps_for(Cap::Display, true, Some(144_000)), 0);
    // A number in the preference wins either way.
    assert_eq!(fps_for(Cap::Fps(120), true, Some(240_000)), 120);
    assert_eq!(fps_for(Cap::None, false, Some(60_000)), 0);
}
