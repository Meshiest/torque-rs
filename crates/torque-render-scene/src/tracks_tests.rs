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
