use super::*;

/// The ground of a gentle slope (z = 0.1 x), in squares of 2 units, cut into triangles.
fn slope(at: [f32; 3], reach: f32) -> Vec<Triangle> {
    let h = |x: f32, y: f32| [x, y, 0.1 * x];
    let mut out = Vec::new();
    let (x0, y0) = (
        ((at[0] - reach) / 2.0).floor() as i32,
        ((at[1] - reach) / 2.0).floor() as i32,
    );
    let (x1, y1) = (
        ((at[0] + reach) / 2.0).ceil() as i32,
        ((at[1] + reach) / 2.0).ceil() as i32,
    );
    for i in x0..x1 {
        for j in y0..y1 {
            let (x, y) = (i as f32 * 2.0, j as f32 * 2.0);
            out.push([h(x, y), h(x + 2.0, y), h(x + 2.0, y + 2.0)]);
            out.push([h(x, y), h(x + 2.0, y + 2.0), h(x, y + 2.0)]);
        }
    }
    out
}

fn wheel(x: f32, y: f32, contact: bool) -> TrackWheel {
    TrackWheel {
        key: (1, 0),
        contact,
        point: [x, y, 0.1 * x],
    }
}

fn rules() -> TrackRules {
    TrackRules {
        width: 0.4,
        spacing: 0.5,
        texture: "track.png".to_string(),
        lifetime: 10.0,
        fade: 4.0,
        max_segments: 20,
    }
}

#[test]
fn tracks_are_laid_on_contact_on_the_ground_fade_and_are_capped() {
    let mut t = TireTracks::new(rules());
    // Off the ground: nothing.
    for k in 0..10 {
        t.update(&[wheel(0.0, k as f32, false)], &slope, 0.032);
    }
    assert!(t.is_empty());
    // On the ground along a curve over the slope: a segment each half unit, every point on the slope.
    for k in 0..12 {
        let a = k as f32 * 0.05;
        t.update(
            &[wheel(
                3.0 * a.sin(),
                3.0 * a.cos() - 3.0 + k as f32 * 0.5,
                true,
            )],
            &slope,
            0.032,
        );
    }
    assert!(t.len() >= 6, "{}", t.len());
    for s in t.segments() {
        for tri in &s.triangles {
            for (p, _) in tri {
                assert!((p[2] - 0.1 * p[0]).abs() < 1e-3, "{p:?} off the ground");
            }
        }
    }
    // Lifted: the strip ends, no segment across the gap.
    let n = t.len();
    t.update(&[wheel(0.0, 20.0, false)], &slope, 0.032);
    t.update(&[wheel(0.0, 20.0, true)], &slope, 0.032);
    t.update(&[wheel(0.0, 20.2, true)], &slope, 0.032);
    assert_eq!(t.len(), n);
    // Capped: the oldest go.
    for k in 0..40 {
        t.update(&[wheel(0.0, 20.0 + k as f32 * 0.6, true)], &slope, 0.032);
    }
    assert_eq!(t.len(), 20);
    // Fading over the last seconds of the lifetime, then gone.
    t.update(&[], &slope, 7.0);
    let o: Vec<f32> = t.segments().map(|s| t.opacity(s)).collect();
    assert!(o.iter().all(|&v| v > 0.0 && v < 1.0), "{o:?}");
    t.update(&[], &slope, 4.0);
    assert!(t.is_empty());
}

/// The longest a segment reaches along y (its triangles' points from its first to its last).
fn reach_y(s: &TrackSegment) -> f32 {
    let ys = s.triangles.iter().flat_map(|t| t.iter().map(|(p, _)| p[1]));
    let (lo, hi) = ys.fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
    hi - lo
}

#[test]
fn backing_up_lays_no_segment_ahead_of_the_wheel_and_none_longer_than_twice_the_spacing() {
    let mut t = TireTracks::new(TrackRules {
        max_segments: 200,
        ..rules()
    });
    let spacing = t.rules.spacing;
    // Forward 6 units, stop, back 6 units, forward again: in frames of 0.1, then a frame of 1.4 (a slow one).
    let mut y = 0.0;
    let mut path = Vec::new();
    for _ in 0..60 {
        y += 0.1;
        path.push(y);
    }
    for _ in 0..60 {
        y -= 0.1;
        path.push(y);
    }
    for _ in 0..30 {
        y += 0.1;
        path.push(y);
    }
    y += 1.4;
    path.push(y);
    for (k, &y) in path.iter().enumerate() {
        t.update(&[wheel(0.0, y, true)], &slope, 0.016);
        for s in t.segments().chain(t.heads().map(|h| h.1)) {
            assert!(
                reach_y(s) <= 2.0 * spacing + 1e-3,
                "frame {k}: a segment {} long",
                reach_y(s)
            );
        }
    }
    // Forward, back and forward again: overlapping strips, each on the way the wheel went.
    assert!(t.len() > 25, "{}", t.len());
}

#[test]
fn the_strip_runs_on_to_the_wheel_in_a_head_each_frame() {
    let mut t = TireTracks::new(rules());
    for k in 0..10 {
        t.update(&[wheel(0.0, k as f32 * 0.5, true)], &slope, 0.016);
    }
    // 0.3 past the last segment: a head from it to the wheel.
    t.update(&[wheel(0.0, 4.8, true)], &slope, 0.016);
    let heads: Vec<_> = t.heads().collect();
    assert_eq!(heads.len(), 1);
    let far = heads[0]
        .1
        .triangles
        .iter()
        .flat_map(|tr| tr.iter().map(|(p, _)| p[1]))
        .fold(f32::MIN, f32::max);
    assert!((far - 4.8).abs() < 0.01, "{far}");
    // Lifted: no head.
    t.update(&[wheel(0.0, 4.9, false)], &slope, 0.016);
    assert_eq!(t.heads().count(), 0);
}

#[test]
fn a_long_frame_bridges_what_the_wheel_covered_and_a_jump_ends_the_strip() {
    let mut t = TireTracks::new(TrackRules {
        max_segments: 200,
        ..rules()
    });
    t.update(&[wheel(0.0, 0.0, true)], &slope, 0.016);
    // A tenth of a second at 50 units a second: one unbroken strip.
    t.update(&[wheel(0.0, 5.0, true)], &slope, 0.1);
    let lines: Vec<_> = t.segments().map(|s| s.line).collect();
    assert_eq!(lines.len(), 10);
    assert!(lines.windows(2).all(|w| w[0][1] == w[1][0]));
    assert_eq!((lines[0][0][1], lines[9][1][1]), (0.0, 5.0));
    // Thirty units in a frame (a respawn): the strip ends there, nothing laid across.
    t.update(&[wheel(0.0, 35.0, true)], &slope, 0.016);
    assert_eq!(t.len(), 10);
    t.update(&[wheel(0.0, 35.5, true)], &slope, 0.016);
    assert_eq!(t.segments().last().unwrap().line[0][1], 35.0);
}

#[test]
fn a_stall_bridges_no_more_than_a_capped_frame_covers() {
    let mut t = TireTracks::new(TrackRules {
        max_segments: 100_000,
        lifetime: 100.0,
        ..rules()
    });
    t.update(&[wheel(0.0, 0.0, true)], &slope, 0.016);
    // A ten-second stall and a thousand units on: the strip ends there, nothing laid across.
    t.update(&[wheel(0.0, 1000.0, true)], &slope, 10.0);
    assert!(t.is_empty());
    // Fifteen units in a stall: bridged (within what a capped frame covers at the fastest speed).
    t.update(&[wheel(0.0, 1015.0, true)], &slope, 10.0);
    assert_eq!(t.len(), 30);
    // Past it: a new strip.
    t.update(&[wheel(0.0, 1040.0, true)], &slope, 10.0);
    assert_eq!(t.len(), 30);
}
