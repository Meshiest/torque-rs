use super::*;

#[test]
fn the_tick_is_one_length_in_both_units() {
    assert_eq!(TICK_MS as f32 / 1000.0, TICK_SECONDS);
}
