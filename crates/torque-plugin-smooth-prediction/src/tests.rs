use super::*;

#[test]
fn settings_are_clamped() {
    let p = SmoothPrediction::default().keep(2.0).snap_distance(-1.0);
    assert_eq!((p.keep, p.snap_distance), (0.98, 0.0));
    let p = SmoothPrediction::default()
        .keep(f32::NAN)
        .snap_distance(f32::INFINITY);
    assert_eq!((p.keep, p.snap_distance), (0.0, 0.0));
}

#[test]
fn the_offset_is_drawn_between_the_last_two_ticks() {
    let p = SmoothPrediction {
        last: [1.0, 0.0, -2.0],
        current: [0.5, 0.0, -1.0],
        ..SmoothPrediction::default()
    };
    assert_eq!(p.offset(0.0), [0.5, 0.0, -1.0]);
    assert_eq!(p.offset(1.0), [1.0, 0.0, -2.0]);
    assert_eq!(p.offset(0.5), [0.75, 0.0, -1.5]);
    assert_eq!(p.offset(7.0), p.offset(1.0));
}

#[test]
fn classes_opt_in() {
    let p = SmoothPrediction::default().classes(["Player"]);
    let body = |class| PredictedBody {
        ghost: 1,
        class,
        position: [0.0; 3],
        previous: [0.0; 3],
        vehicle: false,
    };
    assert!(p.smooths(&body("Player")));
    assert!(!p.smooths(&body("WheeledVehicle")));
}
