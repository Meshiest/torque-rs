use super::name_opacity;

fn close(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-5)
}

#[test]
fn distance_fade_holds_the_name_whole_then_fades_it_to_the_visible_distance() {
    // Torque3D's: full up to visible * distanceFade, 0 at the visible distance, none past it or at the camera.
    let at = |d: f32| name_opacity(d, 1000.0, 1000.0, 400.0, Some(0.1));
    assert_eq!(at(0.0), None, "at the camera");
    assert!(close(at(5.0), 1.0));
    assert!(close(at(99.9), 1.0));
    assert!(close(at(100.0), 1.0));
    assert!(close(at(550.0), 0.5));
    assert!(close(at(1000.0), 0.0));
    assert_eq!(at(1000.5), None, "out of range");
    // The range is the nearer of the visible and name distances; the fade still ends at the visible distance.
    assert_eq!(name_opacity(600.0, 500.0, 1000.0, 400.0, Some(0.1)), None);
    // A negative share is 0 (a fade from the camera); a share of 1 or more draws it whole, then 0 at the edge.
    assert!(close(
        name_opacity(250.0, 1000.0, 1000.0, 400.0, Some(-1.0)),
        0.75
    ));
    assert!(close(
        name_opacity(999.0, 1000.0, 1000.0, 400.0, Some(1.0)),
        1.0
    ));
    assert!(close(
        name_opacity(1000.0, 1000.0, 1000.0, 400.0, Some(1.0)),
        0.0
    ));
    assert!(close(
        name_opacity(500.0, 1000.0, 1000.0, 400.0, Some(2.0)),
        1.0
    ));
}

#[test]
fn without_distance_fade_the_name_fades_from_the_fog_or_its_own_distance() {
    // The name distance's own fade: full up to 80% of it (here past the fog, so from the fog's 400).
    let at = |d: f32| name_opacity(d, 1000.0, 1000.0, 400.0, None);
    assert!(close(at(0.0), 1.0));
    assert!(close(at(399.0), 1.0));
    assert!(close(at(700.0), 0.5));
    assert_eq!(at(1001.0), None);
    // A short name distance: full to 5 less than it (more than 80% of it).
    let near = |d: f32| name_opacity(d, 100.0, 1000.0, 400.0, None);
    assert!(close(near(94.0), 1.0));
    assert!(close(near(97.5), 0.5));
}
