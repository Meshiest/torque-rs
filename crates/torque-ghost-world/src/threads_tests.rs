use super::*;

fn t(sequence: u32, state: u32, forward: bool) -> [Option<Thread>; 4] {
    [
        None,
        Some(Thread {
            sequence,
            state,
            forward,
            at_end: false,
        }),
        None,
        None,
    ]
}

#[test]
fn a_new_sequence_starts_at_zero_and_the_same_one_runs_on_from_where_it_is() {
    let mut s: Slots = Default::default();
    // A new sequence, stopped: put on it at position 0 and held.
    apply(&mut s, &t(5, 1, false), Some(100));
    let k = s[1].unwrap();
    assert_eq!(
        (k.sequence, k.state, k.time_scale()),
        (5, ThreadState::Stop, 0.0)
    );
    assert_eq!(k.position(Some(1100)), 0.0);
    // Play forward: runs from 0.
    apply(&mut s, &t(5, 0, true), Some(1000));
    let k = s[1].unwrap();
    assert_eq!((k.time_scale(), k.seconds(Some(2000))), (1.0, 1.0));
    // Pause one second later: held at 1 s.
    apply(&mut s, &t(5, 2, true), Some(2000));
    let k = s[1].unwrap();
    assert_eq!((k.time_scale(), k.position(Some(9000))), (0.0, 1.0));
    // Play backward: runs down from 1 s, not from the start.
    apply(&mut s, &t(5, 0, false), Some(3000));
    let k = s[1].unwrap();
    assert_eq!(k.time_scale(), -1.0);
    assert!((k.position(Some(3500)) - 0.5).abs() < 1e-6);
    // Stop: back to 0.
    apply(&mut s, &t(5, 1, false), Some(4000));
    assert_eq!(s[1].unwrap().position(Some(5000)), 0.0);
    // A sequence past `MaxSequenceIndex` and a state of 3 change nothing.
    let before = s[1];
    apply(&mut s, &t(63, 0, true), Some(6000));
    apply(&mut s, &t(6, 3, true), Some(6000));
    assert_eq!(s[1], before);
}

#[test]
fn a_thread_at_its_end_is_held() {
    let mut s: Slots = Default::default();
    let mut sent = t(2, 0, true);
    if let Some(th) = sent[1].as_mut() {
        th.at_end = true;
    }
    apply(&mut s, &sent, Some(0));
    let k = s[1].unwrap();
    assert!(k.at_end);
    assert_eq!((k.time_scale(), k.seconds(Some(500))), (0.0, 0.0));
}
