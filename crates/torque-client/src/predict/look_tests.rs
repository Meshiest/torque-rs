use super::*;

#[test]
fn a_wheel_spins_by_its_rate_and_stays_in_one_turn() {
    let tau = std::f32::consts::TAU;
    assert!((spin(0.0, tau, 0.25) - 0.25).abs() < 1e-6);
    assert!((spin(0.9, tau, 0.25) - 0.15).abs() < 1e-5);
    assert!((spin(0.1, -tau, 0.25) - 0.85).abs() < 1e-5);
    assert_eq!(spin(0.5, f32::NAN, 0.1), 0.0);
}

#[test]
fn a_flying_ghost_warps_to_its_update_then_flies_on() {
    let w = FlyingWarp {
        from: ([0.0; 3], [0.0, 0.0, 0.0, 1.0]),
        to: ([2.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
        velocity: [10.0, 0.0, 0.0],
        at_ms: 1_000,
        warp_ms: 64.0,
    };
    assert_eq!(w.pose(1_000).0, [0.0; 3]);
    assert!((w.pose(1_032).0[0] - 1.0).abs() < 1e-5);
    assert!((w.pose(1_064).0[0] - 2.0).abs() < 1e-5);
    assert!((w.pose(1_164).0[0] - 3.0).abs() < 1e-4);
    // Never past the prediction's ticks.
    assert!((w.pose(60_000).0[0] - (2.0 + 10.0 * 0.96)).abs() < 1e-3);
}
