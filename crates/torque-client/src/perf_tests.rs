use super::*;

#[test]
fn the_rolling_mean_waits_for_half_a_window_and_forgets_old_frames() {
    let start = Instant::now();
    let mut r = Rolling::new(Duration::from_secs(2));
    assert_eq!(r.present(start), None);
    for k in 1..=40u64 {
        assert_eq!(r.present(start + Duration::from_millis(20 * k)), Some(20.0));
    }
    assert_eq!(r.mean(), 0.0, "0.8 s of frames: under half the window");
    for k in 41..=200u64 {
        r.present(start + Duration::from_millis(20 * k));
    }
    assert!((r.mean() - 20.0).abs() < 1e-9, "{}", r.mean());
    // Slow frames take over as the fast ones leave the window.
    let t = start + Duration::from_millis(4000);
    for k in 1..=41u64 {
        r.present(t + Duration::from_millis(50 * k));
    }
    assert!((r.mean() - 50.0).abs() < 1e-9, "{}", r.mean());
}

#[test]
fn names_are_cleaned_lists_folded_and_percentiles_ranked() {
    assert_eq!(clean(b"a\x07b", 10), "a?b");
    assert_eq!(clean(b"abcdef", 3), "abc...");
    let mut out = Vec::new();
    push_list(&mut out, "ghosts", Vec::new());
    assert_eq!(out, ["ghosts: none"]);
    out.clear();
    push_list(&mut out, "x", (0..12).map(|i| i.to_string()).collect());
    assert_eq!(out.len(), 2);
    assert!(out[1].starts_with("x (more): 6"));
    let sorted = [1.0, 2.0, 3.0, 4.0];
    assert_eq!(percentile(&sorted, 50.0), 2.0);
    assert_eq!(percentile(&sorted, 95.0), 4.0);
    assert_eq!(percentile(&[], 95.0), 0.0);
    assert_eq!(gib(1 << 30), 1.0);
}

#[test]
fn the_memory_probes_answer_is_read() {
    assert_eq!(
        parse_memory("1234 5678\r\n"),
        Some(Memory {
            working_set: 1234,
            private: 5678
        })
    );
    assert_eq!(parse_memory(""), None);
}
