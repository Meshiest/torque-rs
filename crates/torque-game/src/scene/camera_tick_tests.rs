use super::*;

const UP: [f32; 3] = [0.0, 0.0, 1.0];

#[test]
fn with_nothing_in_the_way_the_eye_goes_back_the_whole_distance() {
    let eye = validate_eye([0.0, 0.0, 10.0], [0.0, 1.0, 0.0], 1.0, 5.0, &mut |_, _| {
        Ok(None)
    })
    .unwrap();
    assert_eq!(eye, [0.0, -4.0, 10.0]);
}

#[test]
fn the_cast_reaches_two_and_a_half_times_the_distance() {
    let mut seen = None;
    validate_eye([0.0; 3], [0.0, 1.0, 0.0], 0.0, 2.0, &mut |from, to| {
        seen = Some((from, to));
        Ok(None)
    })
    .unwrap();
    assert_eq!(seen, Some(([0.0; 3], [0.0, -5.0, 0.0])));
}

#[test]
fn a_facing_surface_stops_the_eye_off_it_by_the_camera_radius() {
    let wall = EyeHit {
        point: [0.0, -2.0, 0.0],
        normal: [0.0, 1.0, 0.0],
    };
    let eye = validate_eye([0.0; 3], [0.0, 1.0, 0.0], 0.0, 4.0, &mut |_, _| {
        Ok(Some(wall))
    })
    .unwrap();
    assert_eq!(eye, [0.0, -(2.0 - CAMERA_RADIUS), 0.0]);
    // Past the full distance the eye stays at the full distance; behind the start it stays at the start.
    let far = EyeHit {
        point: [0.0, -9.0, 0.0],
        ..wall
    };
    let eye = validate_eye([0.0; 3], [0.0, 1.0, 0.0], 0.0, 4.0, &mut |_, _| {
        Ok(Some(far))
    })
    .unwrap();
    assert_eq!(eye, [0.0, -4.0, 0.0]);
    let behind = EyeHit {
        point: [0.0, 1.0, 0.0],
        ..wall
    };
    let eye = validate_eye([0.0; 3], [0.0, 1.0, 0.0], 0.0, 4.0, &mut |_, _| {
        Ok(Some(behind))
    })
    .unwrap();
    assert_eq!(eye, [0.0; 3]);
}

#[test]
fn a_surface_seen_edge_on_does_not_stop_the_eye() {
    let floor = EyeHit {
        point: [0.0, -1.0, 0.0],
        normal: UP,
    };
    let eye = validate_eye([0.0; 3], [0.0, 1.0, 0.0], 0.0, 3.0, &mut |_, _| {
        Ok(Some(floor))
    })
    .unwrap();
    assert_eq!(eye, [0.0, -3.0, 0.0]);
}

#[test]
fn no_orbit_distance_leaves_the_eye_at_the_centre() {
    let eye = validate_eye([1.0, 2.0, 3.0], [0.0, 1.0, 0.0], 2.0, 2.0, &mut |_, _| {
        panic!("no cast for a zero distance")
    })
    .unwrap();
    assert_eq!(eye, [1.0, 2.0, 3.0]);
}
