use super::*;

#[test]
fn a_path_that_could_leave_the_install_is_refused() {
    assert!(path_ok(b"data/shapes/crate.png"));
    assert!(path_ok(b"mods/pack/plate.png"));
    for bad in [
        &b""[..],
        b"/etc/passwd",
        b"../x.png",
        b"data/../../x",
        b"C:/x.png",
        b"data\\x.png",
        b"a//b",
        b"a/./b",
        b"a\0b",
    ] {
        assert!(!path_ok(bad), "{bad:?}");
    }
    assert!(!path_ok("a/".repeat(600).as_bytes()));
}

#[test]
fn the_ticks_stay_on_their_grid_at_frame_steps_that_do_not_divide_a_tick() {
    // Frames of 7 ms (144 a second) and 17 ms: a tick runs at the first frame at or past its time, and falls on its time.
    for frame in [7u32, 17, 16, 32] {
        let mut last = None;
        let mut ticks = Vec::new();
        for k in 1..400u32 {
            let now = 1_000 + k * frame;
            if last.is_some_and(|t: u32| now.wrapping_sub(t) < TICK_MS) {
                continue;
            }
            let t = tick_time(last, now);
            assert!(t <= now && now - t < TICK_MS, "{frame}: {t} at {now}");
            ticks.push(t);
            last = Some(t);
        }
        assert!(
            ticks.windows(2).all(|w| w[1] - w[0] == TICK_MS),
            "{frame}: {ticks:?}"
        );
    }
    // A hitch of a tick or more starts a new grid at the frame.
    assert_eq!(tick_time(Some(1_000), 1_070), 1_070);
    assert_eq!(tick_time(None, 5), 5);
}

#[test]
fn the_connect_sequence_is_random_outside_a_run_that_must_repeat() {
    let mut vm = Vm::new();
    // The same Sim time and object: two connects still differ.
    let (a, b) = (connect_seq(&vm, 7), connect_seq(&vm, 7));
    assert_ne!(a, b);
    // With a fixed key (the trace hashes), the sequence is the seeded one, every time.
    vm.set_extension(Rc::new(EngineNonceKey([1; 16])));
    let seeded = connect_seq(&vm, 7);
    assert_eq!(seeded, connect_seq(&vm, 7));
    assert_eq!(
        seeded,
        vm.sim_time().wrapping_mul(0x9e37_79b9) ^ 7u32.wrapping_mul(0x85eb_ca6b) ^ 0x6d31_2065
    );
}
