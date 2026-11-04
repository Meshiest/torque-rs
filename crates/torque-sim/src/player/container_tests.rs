use super::zone_coverage;

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
