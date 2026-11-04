use super::*;

#[test]
fn the_histogram_mean_is_the_middle_of_its_range() {
    let mut h = vec![0u32; HISTOGRAM_BINS];
    let b = bin_of(0.5);
    h[b] = 100;
    assert!((histogram_mean_ev(&h) - bin_ev(b)).abs() < 1e-6);
    // Under the low percentile black: the range starts above it, so only the bright bin counts.
    h[0] = 60;
    assert!((histogram_mean_ev(&h) - bin_ev(b)).abs() < 1e-6);
    let t = target_ev(&h);
    assert!((t - (DEFAULT_KEY.log2() - bin_ev(b))).abs() < 1e-5, "{t}");
}

#[test]
fn adaptation_reaches_a_six_ev_step_within_a_tenth_in_two_seconds() {
    let mut ev = 0.0;
    for _ in 0..120 {
        ev = adapt(ev, 6.0, 1.0 / 60.0);
    }
    assert!((6.0 - ev).abs() < 0.1, "{ev}");
}

#[test]
fn the_agx_style_curve_keeps_mid_grey_and_reaches_white() {
    let g = agx_style([0.18; 3]);
    assert!((g[0] - AGX_MID_GREY_DISPLAY).abs() < 0.02, "{g:?}");
    let w = agx_style([1e4; 3]);
    assert!(w.iter().all(|v| *v > 0.99), "{w:?}");
    let b = agx_style([0.0; 3]);
    assert!(b.iter().all(|v| *v < 0.01), "{b:?}");
    // A bright saturated red goes toward white (its green and blue rise), a dim one stays red.
    let bright = agx_style([40.0, 0.5, 0.5]);
    let dim = agx_style([0.4, 0.005, 0.005]);
    assert!(bright[1] > 0.5 && dim[1] < 0.2, "{bright:?} {dim:?}");
    // Monotonic.
    let mut last = -1.0;
    for i in 0..200 {
        let v = agx_style([0.001 * 1.07f32.powi(i); 3])[0];
        assert!(v >= last);
        last = v;
    }
}

#[test]
fn the_soft_threshold_passes_nothing_dim_and_most_of_the_bright() {
    let b = BloomSettings {
        threshold: 2.0,
        knee: 1.0,
        strength: 0.35,
        mix: false,
    };
    assert_eq!(bloom_weight(0.5, &b), 0.0);
    assert!(bloom_weight(1.5, &b) > 0.0 && bloom_weight(1.5, &b) < 0.1);
    assert!((bloom_weight(20.0, &b) - 0.9).abs() < 1e-6);
    assert_eq!(
        bloom_weight(0.1, &crate::hd::ImageSettings::DEFAULT.bloom()),
        1.0
    );
}
