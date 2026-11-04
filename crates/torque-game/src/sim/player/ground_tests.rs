use super::*;

/// A cycling, path-making sequence of a microsecond plays a million loops in its one second: the walk is capped at
/// [`SIM_GROUND_LOOPS`] and every displacement is charged, so a refused budget ends it.
#[test]
fn a_hostile_duration_is_capped_and_charged() {
    let mut m = shapes::synth::hull_shape([-1.0, -1.0, 0.0, 1.0, 1.0, 2.0], &[], &[], &["run"]);
    let s = &mut m.shape;
    s.ground_translations = vec![[0.0, 1.0, 0.0]];
    s.ground_rotations = vec![[0, 0, 0, 32767]];
    let seq = &mut s.sequences[0];
    seq.duration = 1e-6;
    seq.flags |= CYCLIC | MAKE_PATH;
    seq.first_ground_frame = 0;
    seq.num_ground_frames = 1;
    let mut spent = 0u64;
    let info = ground_info(&m.shape, b"run", 0, [0.0, 1.0, 0.0], true, &mut |u| {
        spent += u;
        true
    });
    assert!(info.is_some());
    assert!(
        spent <= ACTION_WORK + GROUND_WORK * (SIM_GROUND_LOOPS as u64 + 1),
        "{spent}"
    );
    assert!(spent > GROUND_WORK * 100, "the loops are charged: {spent}");
    let mut left = 50u64;
    let refused = ground_info(&m.shape, b"run", 0, [0.0, 1.0, 0.0], true, &mut |u| {
        let ok = u <= left;
        left = left.saturating_sub(u);
        ok
    });
    assert!(refused.is_none(), "a spent budget ends the walk");
}
