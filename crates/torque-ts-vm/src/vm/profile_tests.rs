#[test]
fn a_leave_with_no_enter_is_ignored() {
    // The profile turned on inside a call sees the call's leave first.
    let mut p = super::Profiler::default();
    p.leave();
    assert_eq!(p.take().calls, 0);
}
