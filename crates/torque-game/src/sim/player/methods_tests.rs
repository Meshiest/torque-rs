use super::impulse_velocity;

/// `applyImpulse` adds `J / mass`, a float, to the velocity.
#[test]
fn the_impulse_quotient_is_added_to_the_velocity() {
    let v = impulse_velocity([1.0, 2.0, 3.0], [90.0, 180.0, -90.0], 90.0);
    assert_eq!(v, [2.0, 4.0, 2.0]);
    let tiny = impulse_velocity([0.0; 3], [1.0, 0.0, 0.0], 3.0);
    assert_eq!(tiny[0], 1.0f32 / 3.0);
}
