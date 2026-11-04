use super::*;

#[test]
fn a_run_stays_while_its_count_does_and_a_new_one_goes_after() {
    let mut r = Runs::default();
    let (a, moved) = r.place(&[(1, 10), (2, 20)]);
    assert!(moved);
    assert_eq!((a[0].start, a[1].start), (0, 10));
    r.set(1, Some(7), Some(7));
    let (b, moved) = r.place(&[(2, 20), (1, 10)]);
    assert!(!moved);
    assert_eq!((b[0].start, b[1].start), (10, 0));
    assert_eq!(b[1].written, Some(7));
    // A count changed: a new run after the last.
    let (c, moved) = r.place(&[(1, 12), (2, 20)]);
    assert!(!moved);
    assert_eq!(c[0].start, 30);
    assert_eq!(c[0].written, None);
    // Past the room: laid out again.
    let (d, moved) = r.place(&[(1, 12), (2, 20), (3, 5000)]);
    assert!(moved);
    assert_eq!((d[0].start, d[1].start, d[2].start), (0, 12, 32));
    assert!(d.iter().all(|r| r.written.is_none()));
    assert!(r.capacity >= 5032);
}

#[test]
fn spans_join_runs_side_by_side_and_short_held_gaps() {
    let run = |start, len| Run {
        start,
        len,
        written: None,
        previous: None,
        seen: 0,
    };
    let runs = [
        run(0, 10),
        run(10, 10),
        run(20, 10),
        run(40, 10),
        run(50, BRIDGE + 1),
        run(51 + BRIDGE, 5),
    ];
    let order: Vec<usize> = (0..runs.len()).collect();
    let need = [true, false, true, true, false, true];
    // 0 and 2 with the held 1 between; 3 apart (a hole before it); 5 apart (the held gap too long).
    assert_eq!(
        spans(&order, |i| runs[i], &need),
        vec![vec![0, 1, 2], vec![3], vec![5]]
    );
    assert!(spans(&order, |i| runs[i], &[false; 6]).is_empty());
}

#[test]
fn a_run_not_asked_for_is_let_go() {
    let mut r = Runs::default();
    r.place(&[(1, 10)]);
    for _ in 0..KEEP_UPLOADS + 1 {
        r.place(&[(2, 10)]);
    }
    assert!(!r.runs.contains_key(&1));
    // Uploads with no stamped object keep the runs.
    let mut s = Runs::default();
    s.place(&[(1, 10)]);
    s.set(1, Some(3), None);
    for _ in 0..100 {
        s.place(&[]);
    }
    assert_eq!(s.place(&[(1, 10)]).0[0].written, Some(3));
}
