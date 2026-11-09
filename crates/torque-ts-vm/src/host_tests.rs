use super::{Clock, VirtualClock, pick_name, sanitize_component};

#[test]
fn a_virtual_clock_moves_one_millisecond_a_read_and_never_back() {
    let clock = VirtualClock::new();
    let first = clock.real_time_ms();
    assert_eq!(first, super::VIRTUAL_UPTIME_MS);
    let reads: Vec<u32> = (0..5).map(|_| clock.real_time_ms()).collect();
    assert_eq!(
        reads,
        [first + 1, first + 2, first + 3, first + 4, first + 5]
    );
    // A busy wait of 150 ms reads the clock 151 times, however fast the machine is.
    let start = clock.real_time_ms();
    let mut loops = 0;
    while clock.real_time_ms().wrapping_sub(start) < 150 {
        loops += 1;
    }
    assert_eq!(loops, 149);
    // The wall clock is one fixed instant, and the counter wraps like a 32-bit one.
    assert_eq!(clock.unix_time(), super::VIRTUAL_UNIX_TIME);
    assert_eq!(clock.unix_time(), clock.unix_time());
    let wrapped = VirtualClock {
        reads: std::cell::Cell::new(u32::MAX),
    };
    assert_eq!(
        (wrapped.real_time_ms(), wrapped.real_time_ms()),
        (u32::MAX, 0)
    );
}

#[test]
fn an_exact_spelling_wins_over_a_case_variant() {
    let listed = ["OK.cs", "ok.cs", "Ok.cs"];
    assert_eq!(pick_name(listed, "ok.cs"), Some("ok.cs"));
    assert_eq!(pick_name(listed, "OK.cs"), Some("OK.cs"));
    // No exact spelling: the first variant listed.
    assert_eq!(pick_name(listed, "oK.cs"), Some("OK.cs"));
    assert_eq!(pick_name(listed, "other.cs"), None);
    assert_eq!(pick_name([], "ok.cs"), None);
    // Only ASCII case folds: no other equivalence makes two names one.
    assert_eq!(pick_name(["\u{c9}.cs"], "\u{e9}.cs"), None);
}

#[test]
fn every_numbered_port_name_is_a_device() {
    for name in ["com0", "COM0.cs", "lpt0.txt", "com9", "lpt1", "com\u{b9}"] {
        assert!(!sanitize_component(name.as_bytes()), "{name}");
    }
    for name in ["com", "com10", "lpt", "computer.cs", "lpt0x"] {
        assert!(sanitize_component(name.as_bytes()), "{name}");
    }
}
