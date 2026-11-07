use super::{MIN_BODY_DENSITY, body_density, zone_coverage};

#[test]
fn a_hostile_body_density_is_held_to_a_floor_or_the_default() {
    assert_eq!(body_density(0.85), 0.85);
    assert_eq!(body_density(0.0), MIN_BODY_DENSITY);
    assert_eq!(body_density(1.0e-30), MIN_BODY_DENSITY);
    assert_eq!(body_density(-3.0), MIN_BODY_DENSITY);
    assert_eq!(body_density(f32::INFINITY), 1.0);
    assert_eq!(body_density(f32::NEG_INFINITY), 1.0);
    assert_eq!(body_density(f32::NAN), 1.0);
    // The buoyancy it gives in water of density 1000 at full coverage stays finite.
    for d in [0.0, 1.0e-30, -3.0, f32::INFINITY, f32::NAN] {
        assert!((1000.0 / body_density(d)).is_finite(), "{d}");
    }
}

#[test]
fn coverage_follows_the_zone_callback_branches() {
    // Under the top, the feet at or above the bottom: full.
    assert_eq!(zone_coverage(0.0, 0.0, 1.0, -1.0, 3.0), 1.0);
    // Under the top, the feet below the bottom: a first zone gives 0.99f, a second (accumulator not 0.99f) promotes to 1.0, and
    // one that finds 0.99f keeps it.
    assert_eq!(zone_coverage(0.0, -2.0, 1.0, -1.0, 3.0), 0.99);
    assert_eq!(zone_coverage(0.5, -2.0, 1.0, -1.0, 3.0), 1.0);
    assert_eq!(zone_coverage(0.99, -2.0, 1.0, -1.0, 3.0), 0.99);
    // Above the top: the fraction under it, or 1.0 from a 0.99f accumulator.
    assert_eq!(zone_coverage(0.0, 0.0, 4.0, -1.0, 3.0), 0.75);
    assert_eq!(zone_coverage(0.99, 0.0, 4.0, -1.0, 3.0), 1.0);
    // A NaN extent takes the unordered side of each compare.
    assert_eq!(zone_coverage(0.0, f32::NAN, 1.0, -1.0, 3.0), 1.0);
    assert_eq!(zone_coverage(f32::NAN, -2.0, 1.0, -1.0, 3.0), 0.99);
}
