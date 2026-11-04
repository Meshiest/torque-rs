use super::*;

/// A table of `i / 128`: every lookup lands on a step.
fn steps() -> GainTable {
    GainTable::from_gains(std::array::from_fn(|i| i as f32 / 128.0))
}

#[test]
fn by_default_the_volume_is_the_gain() {
    let t = GainTable::default();
    assert!(!t.is_table());
    for v in [0.25, 0.503, 0.999] {
        assert_eq!(t.to_gain(v), v);
        assert_eq!(t.to_linear(v), v);
    }
    assert_eq!(t.to_gain(-1.0), 0.0);
    assert_eq!(t.to_gain(2.0), 1.0);
    assert_eq!(t.to_gain(f32::NAN), 0.0);
    assert_eq!(t.to_linear(1.5), 1.0);
    assert_eq!(t.to_linear(f32::NAN), 0.0);
}

#[test]
fn a_table_clamps_and_cuts_toward_zero() {
    let t = steps();
    assert!(t.is_table());
    assert_eq!(t.to_gain(-1.0), 0.0);
    assert_eq!(t.to_gain(0.0), 0.0);
    assert_eq!(t.to_gain(2.0), 1.0);
    assert_eq!(t.to_gain(f32::NAN), 0.0);
    // 0.5/128 is index 0, 1.9/128 index 1, the largest volume below 1 index 127.
    assert_eq!(t.to_gain(0.5 / 128.0), 0.0);
    assert_eq!(t.to_gain(1.9 / 128.0), 1.0 / 128.0);
    assert_eq!(t.to_gain(0.5), 0.5);
    assert_eq!(t.to_gain(0.999_999), 127.0 / 128.0);
}

#[test]
fn a_tables_inverse_finds_the_index() {
    let t = steps();
    for i in 1..128 {
        let g = i as f32 / 128.0;
        assert_eq!(t.to_linear(g), g, "{i}");
    }
    assert_eq!(t.to_linear(0.0), 0.0);
    assert_eq!(t.to_linear(1.5), 1.0);
    assert_eq!(t.to_linear(f32::NAN), 0.0);
    // Between two entries the search stops at the lower one.
    assert_eq!(t.to_linear(10.5 / 128.0), 10.0 / 128.0);
}
