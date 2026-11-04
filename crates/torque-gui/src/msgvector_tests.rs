use super::*;

#[test]
fn a_vector_drops_its_oldest_lines_past_its_line_and_byte_caps_in_order() {
    // `Vec::remove(0)` per line past the cap, and 65,536 lines of 64 KiB held 4 GiB.
    let mut l = Lines::default();
    let big = vec![b'x'; MAX_LINE_BYTES];
    let t = std::time::Instant::now();
    for i in 0..(2 * MAX_VECTOR_BYTES / MAX_LINE_BYTES) {
        l.insert(l.len(), big.clone(), i as i32);
        assert!(l.bytes <= MAX_VECTOR_BYTES);
    }
    assert_eq!(l.len(), MAX_VECTOR_BYTES / MAX_LINE_BYTES);
    // The oldest went first: the first line kept is the one after the last dropped.
    assert_eq!(
        l.get(0).map(|x| x.1),
        Some((MAX_VECTOR_BYTES / MAX_LINE_BYTES) as i32)
    );
    l.clear();
    for i in 0..(MAX_LINES + 1_000) {
        l.insert(l.len(), b"a".to_vec(), i as i32);
    }
    assert_eq!(l.len(), MAX_LINES);
    assert_eq!(l.get(0).map(|x| x.1), Some(1_000));
    assert_eq!(l.bytes, MAX_LINES);
    assert_eq!(l.pop_front().map(|x| x.1), Some(1_000));
    assert_eq!(l.pop_back().map(|x| x.1), Some(MAX_LINES as i32 + 999));
    assert_eq!(l.remove(l.len()), None);
    assert_eq!(l.bytes, MAX_LINES - 2);
    assert!(
        t.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        t.elapsed()
    );
}
