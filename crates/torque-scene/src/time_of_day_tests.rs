use super::*;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn an_overridden_azimuth_rises_tops_and_sets_along_it() {
    let mut d = TimeOfDay::new(DayFields {
        start_time: 0.0,
        play: false,
        ..DayFields::default()
    });
    assert_eq!(d.time, 0.0);
    let t = d.towards();
    assert!(close(t[2], 0.0), "sunrise on the horizon: {t:?}");
    assert!(close(t[0], 1f32.sin()) && close(t[1], 1f32.cos()));
    d.set_time(0.25);
    assert!(close(d.towards()[2], 1.0), "overhead at a quarter");
    d.set_time(0.5);
    let t = d.towards();
    assert!(
        close(t[2], 0.0) && close(t[0], -(1f32.sin())),
        "sets on the far side: {t:?}"
    );
    d.set_time(0.75);
    assert!(close(d.towards()[2], -1.0));
    assert!(!d.sun_up());
}

#[test]
fn the_tilted_model_tops_at_noon_short_of_overhead() {
    let mut d = TimeOfDay::new(DayFields {
        azimuth_override: 0.0,
        start_time: 0.0,
        play: false,
        ..DayFields::default()
    });
    assert!(close(d.elevation().to_degrees(), 90.0 - 23.44));
    assert!(close(d.normalized_elevation(), 90.0));
    d.set_time(0.5);
    assert!(close(d.elevation().to_degrees(), -(90.0 - 23.44)));
    assert!(close(d.normalized_elevation(), 270.0));
    // Sunrise and sunset a quarter day either side of noon, at the horizon.
    d.set_time(0.75);
    assert!(close(d.elevation(), 0.0));
    assert!(close(d.normalized_elevation(), 0.0) || close(d.normalized_elevation(), 360.0));
    d.set_time(0.25);
    assert!(close(d.normalized_elevation(), 180.0));
}

#[test]
fn the_day_plays_at_its_scales_and_fires_its_events_in_order() {
    let mut d = TimeOfDay::new(DayFields {
        start_time: 0.0,
        day_length: 100.0,
        day_scale: 1.0,
        night_scale: 2.0,
        ..DayFields::default()
    });
    d.add_event(180.0, "sunset");
    d.add_event(90.0, "noon");
    assert_eq!(d.events()[0].identifier, "noon");
    let mut fired = Vec::new();
    let mut steps = 0;
    while fired.len() < 2 {
        for n in d.advance(0.032) {
            if let DayNote::Event { identifier, .. } = n {
                fired.push((identifier, d.time));
            }
        }
        steps += 1;
    }
    assert_eq!(fired[0].0, "noon");
    assert_eq!(fired[1].0, "sunset");
    assert!(fired[0].1 >= 0.25 && fired[0].1 < 0.2504, "{fired:?}");
    // Half a day of daylight at a scale of 1: 50 seconds.
    assert!((steps as f32 * 0.032 - 50.0).abs() < 0.1, "{steps}");
    // The night runs twice as fast once the sun is 10 degrees past setting: the next quarter day in 2.78 + 11.11 seconds.
    let start = d.time;
    let mut t = 0.0f32;
    while d.time < start + 0.25 - 1e-4 {
        d.advance(0.032);
        t += 0.032;
    }
    assert!((t - 13.89).abs() < 0.1, "{t}");
}

#[test]
fn a_day_wraps_through_sunrise_and_fires_the_dawn_event() {
    let mut d = TimeOfDay::new(DayFields {
        start_time: 0.99,
        day_length: 10.0,
        ..DayFields::default()
    });
    d.add_event(0.0, "dawn");
    let notes: Vec<DayNote> = (0..10).flat_map(|_| d.advance(0.032)).collect();
    assert!(d.time < 0.05);
    assert!(
        notes
            .iter()
            .any(|n| matches!(n, DayNote::Event { identifier, .. } if identifier == "dawn")),
        "{notes:?}"
    );
}

#[test]
fn animate_runs_to_its_elevation_and_ends() {
    let mut d = TimeOfDay::new(DayFields {
        start_time: 0.0,
        play: false,
        ..DayFields::default()
    });
    assert!(!d.animate(0.0, 10.0), "already there");
    assert!(d.animate(90.0, 90.0));
    let mut done = false;
    for _ in 0..40 {
        if d.advance(0.032).contains(&DayNote::AnimateDone) {
            done = true;
            break;
        }
    }
    assert!(done);
    assert!(close(d.time, 0.25));
    assert!(d.animating().is_none());
}

#[test]
fn set_time_wraps() {
    let mut d = TimeOfDay::default();
    assert!(close(d.time, 0.5), "starts at the start time");
    d.set_time(1.25);
    assert!(close(d.time, 0.25));
    d.set_time(-0.25);
    assert!(close(d.time, 0.75));
}
