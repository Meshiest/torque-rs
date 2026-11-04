use super::*;

#[test]
fn the_least_recently_used_go_first_and_both_caps_hold() {
    let mut l: Lru<u32, u32> = Lru::new(3, 100);
    for k in 0..3 {
        assert!(l.insert(k, k, 10));
    }
    // 0 used again: 1 is now the oldest.
    assert_eq!(l.get(&0), Some(&0));
    assert!(l.insert(3, 3, 10));
    assert!(!l.contains(&1) && l.contains(&0) && l.contains(&2) && l.contains(&3));
    // The byte cap: a heavy entry drops as many as it needs.
    assert!(l.insert(4, 4, 90));
    assert!(l.bytes() <= 100 && l.len() <= 3);
    assert!(l.contains(&4));
    // One above the cap is not kept, and what was there stays.
    assert!(!l.insert(5, 5, 101));
    assert!(l.contains(&4) && !l.contains(&5));
    assert_eq!(l.remove(&4), Some(4));
    assert_eq!(l.bytes(), l.values().count() * 10);
}

#[test]
fn what_a_frame_used_stays_while_it_lasts() {
    let mut l: Lru<u32, ()> = Lru::new(10, 30);
    for k in 0..3 {
        assert!(l.insert(k, (), 10));
    }
    l.begin_frame();
    assert!(l.get(&0).is_some() && l.get(&1).is_some() && l.get(&2).is_some());
    // Nothing unpinned to drop: refused, and the frame's entries kept.
    assert!(!l.fits(10));
    assert!(!l.insert(3, (), 10));
    assert_eq!(l.len(), 3);
    // The next frame uses only 2: the others may go.
    l.begin_frame();
    assert!(l.get(&2).is_some());
    assert!(l.fits(20));
    assert!(l.insert(3, (), 10) && l.insert(4, (), 10));
    assert!(l.contains(&2) && l.contains(&3) && l.contains(&4));
    assert!(!l.contains(&0) && !l.contains(&1));
}
