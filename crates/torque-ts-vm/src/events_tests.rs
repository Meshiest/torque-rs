use super::*;

fn call(name: &str) -> EventKind {
    EventKind::Call {
        argv: vec![name.as_bytes().to_vec()],
        on_object: false,
    }
}

fn name_of(event: &Event) -> String {
    match &event.kind {
        EventKind::Call { argv, .. } => String::from_utf8_lossy(&argv[0]).into_owned(),
        EventKind::Delete => "delete".to_string(),
        EventKind::Native { tag, arg } => format!("native {tag} {arg}"),
    }
}

#[test]
fn events_leave_by_time_and_then_by_posting_order() {
    let mut queue = EventQueue::new();
    for (time, name) in [(5, "e"), (3, "a"), (5, "f"), (3, "b"), (4, "c"), (3, "d")] {
        queue.post(time, 7, call(name));
    }
    let mut order = Vec::new();
    while let Some(event) = queue.take_due(u32::MAX) {
        order.push(name_of(&event));
    }
    assert_eq!(order, ["a", "b", "d", "c", "e", "f"]);
    assert!(queue.is_empty());
}

#[test]
fn an_event_is_due_at_its_time_and_not_before() {
    let mut queue = EventQueue::new();
    queue.post(10, 7, call("x"));
    assert_eq!(queue.next_due(9), None);
    assert!(queue.take_due(9).is_none());
    assert_eq!(queue.next_due(10), Some(10));
    assert!(queue.take_due(10).is_some());
    assert!(queue.take_due(u32::MAX).is_none());
}

#[test]
fn ids_start_at_one_wrap_past_two_to_the_32_and_skip_zero_and_ids_in_use() {
    let mut queue = EventQueue::new();
    assert_eq!(queue.post(0, 7, call("a")), Some(1));
    assert_eq!(queue.post(0, 7, call("b")), Some(2));
    queue.next_seq = u32::MAX;
    assert_eq!(queue.post(0, 7, call("c")), Some(u32::MAX));
    // 0 is never an event; 1 and 2 are in use.
    assert_eq!(queue.post(0, 7, call("d")), Some(3));
    queue.remove(1);
    queue.next_seq = u32::MAX;
    assert_eq!(
        queue.post(0, 7, call("e")),
        Some(1),
        "u32::MAX is taken, 0 skipped, 1 is free"
    );
    assert_eq!(queue.len(), 4);
    assert_eq!(queue.get(1).map(name_of).as_deref(), Some("e"));
    assert_eq!(queue.get(u32::MAX).map(name_of).as_deref(), Some("c"));
}

#[test]
fn the_pending_cap_refuses_and_makes_room_again() {
    let mut queue = EventQueue::new();
    queue.set_max_pending(2, usize::MAX);
    assert_eq!(queue.post(0, 7, call("a")), Some(1));
    assert_eq!(queue.post(0, 7, call("b")), Some(2));
    assert_eq!(queue.post(0, 7, call("c")), None);
    assert_eq!(queue.len(), 2);
    assert!(queue.remove(1).is_some());
    assert_eq!(queue.post(0, 7, call("d")), Some(3));
    queue.set_max_pending(1, usize::MAX);
    assert_eq!(
        queue.post(0, 7, call("e")),
        None,
        "a lower cap keeps what is pending"
    );
    assert_eq!(queue.len(), 2);
}

#[test]
fn the_default_cap_is_a_million_pending_events() {
    assert_eq!(EventQueue::new().max_pending, MAX_PENDING_EVENTS);
    assert_eq!(EventLimits::default().pending, MAX_PENDING_EVENTS);
    assert_eq!(EventQueue::new().max_bytes, MAX_PENDING_BYTES);
    assert_eq!(EventLimits::default().pending_bytes, MAX_PENDING_BYTES);
    const { assert!(MAX_PENDING_EVENTS >= 10 * 100_000) };
}

#[test]
fn the_bytes_of_the_arguments_are_capped_and_counted_down_again() {
    let sized = |n: usize| EventKind::Call {
        argv: vec![vec![b'f'], vec![b'x'; n]],
        on_object: false,
    };
    let mut queue = EventQueue::new();
    queue.set_max_pending(100, 10);
    let a = queue.post(5, 1, sized(4)).unwrap();
    assert_eq!(queue.pending_bytes(), 5);
    let b = queue.post(6, 2, sized(2)).unwrap();
    assert_eq!(queue.pending_bytes(), 8);
    assert_eq!(
        queue.post(7, 1, sized(2)),
        None,
        "11 bytes do not fit under 10"
    );
    assert!(
        queue.post(7, 1, EventKind::Delete).is_some(),
        "an event without arguments does"
    );
    // Every way out gives the bytes back.
    assert!(queue.remove(a).is_some());
    assert_eq!(queue.pending_bytes(), 3);
    assert!(queue.take_due(6).is_some());
    assert_eq!(queue.pending_bytes(), 0);
    assert!(queue.get(b).is_none());
    queue.post(8, 3, sized(5)).unwrap();
    queue.post(8, 3, sized(1)).unwrap();
    assert_eq!(queue.pending_bytes(), 8);
    assert_eq!(queue.drop_dest(3), 2);
    assert_eq!(queue.pending_bytes(), 0);
}

#[test]
fn removing_and_dropping_keep_the_indexes_in_step() {
    let mut queue = EventQueue::new();
    let a = queue.post(5, 1, call("a")).unwrap();
    let b = queue.post(5, 2, call("b")).unwrap();
    let c = queue.post(6, 1, call("c")).unwrap();
    assert_eq!(queue.drop_dest(1), 2);
    assert_eq!(queue.drop_dest(1), 0);
    assert!(queue.get(a).is_none() && queue.get(c).is_none());
    assert!(queue.remove(a).is_none());
    assert_eq!(queue.remove(b).map(|e| name_of(&e)).as_deref(), Some("b"));
    assert!(queue.is_empty());
    assert!(queue.by_seq.is_empty() && queue.by_dest.is_empty());
    // Taking due events forgets them as well.
    let d = queue.post(1, 3, call("d")).unwrap();
    assert!(queue.take_due(1).is_some());
    assert!(queue.get(d).is_none());
    assert!(queue.by_seq.is_empty() && queue.by_dest.is_empty());
}

#[test]
fn an_objects_events_are_tracked_from_one_to_many_and_back() {
    let mut queue = EventQueue::new();
    let a = queue.post(1, 9, call("a")).unwrap();
    let b = queue.post(2, 9, call("b")).unwrap();
    let c = queue.post(3, 9, call("c")).unwrap();
    let other = queue.post(4, 10, call("o")).unwrap();
    assert_eq!(queue.remove(b).map(|e| name_of(&e)).as_deref(), Some("b"));
    assert!(queue.remove(b).is_none());
    assert_eq!(queue.take_due(1).map(|e| e.seq), Some(a));
    // One event of object 9 is left, held without a set.
    assert_eq!(queue.drop_dest_seqs(9), [c]);
    assert_eq!(queue.drop_dest(9), 0);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.get(other).map(name_of).as_deref(), Some("o"));
    let first = queue.post(5, 11, call("x")).unwrap();
    let second = queue.post(6, 11, call("y")).unwrap();
    assert_eq!(queue.drop_dest_seqs(11), [first, second]);
}
