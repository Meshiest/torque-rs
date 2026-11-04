use super::*;

fn list(keys: &[ObjectKey]) -> ProcessList<()> {
    let mut l = ProcessList::new();
    for k in keys {
        assert!(l.add(*k, "T", (), None));
    }
    l
}

fn visited(l: &mut ProcessList<()>) -> Vec<ObjectKey> {
    l.order_list_if_dirty();
    l.begin_pass();
    let mut out = Vec::new();
    while let Some(v) = l.next_in_pass() {
        if v.ticks {
            out.push(v.key);
        }
    }
    out
}

#[test]
fn creation_order_is_the_order_and_a_clean_list_is_not_sorted() {
    let mut l = list(&[5, 3, 9, 1]);
    assert_eq!(l.order(), [5, 3, 9, 1]);
    assert!(!l.is_dirty());
    assert_eq!(visited(&mut l), [5, 3, 9, 1]);
    assert_eq!(l.order(), [5, 3, 9, 1]);
    assert!(!l.add(3, "T", (), None));
}

#[test]
fn a_followed_object_is_pulled_in_front_and_only_when_dirty() {
    let mut l = list(&[1, 2, 3, 4]);
    l.process_after(1, Some(3));
    assert!(l.is_dirty());
    // 1 follows 3: the order is 3 1 (pulled), then 2, 4.
    assert_eq!(visited(&mut l), [3, 1, 2, 4]);
    assert!(!l.is_dirty());
    // Not dirty again: the order stays whatever the sort made.
    assert_eq!(visited(&mut l), [3, 1, 2, 4]);
}

#[test]
fn a_chain_is_laid_out_dependencies_first() {
    let mut l = list(&[1, 2, 3, 4]);
    l.process_after(1, Some(2));
    l.process_after(2, Some(4));
    assert_eq!(visited(&mut l), [4, 2, 1, 3]);
}

#[test]
fn a_target_placed_earlier_stays_where_it_is() {
    let mut l = list(&[1, 2, 3]);
    l.process_after(3, Some(1));
    assert_eq!(visited(&mut l), [1, 2, 3]);
    let mut l = list(&[1, 2, 3]);
    l.process_after(2, Some(1));
    assert_eq!(visited(&mut l), [1, 2, 3]);
}

#[test]
fn two_loops_are_let_go_and_longer_loops_end_at_the_tagged_object() {
    let mut l = list(&[1, 2]);
    l.process_after(1, Some(2));
    l.process_after(2, Some(1));
    // The second call let 1's edge go: 2 follows 1.
    assert_eq!(l.after_of(1), None);
    assert_eq!(visited(&mut l), [1, 2]);
    let mut l = list(&[1, 2, 3]);
    l.process_after(1, Some(2));
    l.process_after(2, Some(3));
    l.process_after(3, Some(1));
    // Visit 1: pull 2, pull 3, 3's target 1 is tagged: the chain is 3 2 1.
    assert_eq!(visited(&mut l), [3, 2, 1]);
    let mut l = list(&[1]);
    l.process_after(1, Some(1));
    assert_eq!(visited(&mut l), [1]);
}

#[test]
fn clearing_an_edge_does_not_dirty_and_a_removed_target_is_no_edge() {
    let mut l = list(&[1, 2, 3]);
    l.process_after(1, Some(3));
    l.order_list();
    assert_eq!(l.order(), [3, 1, 2]);
    l.clear_after(1);
    assert!(!l.is_dirty());
    l.process_after(2, Some(3));
    l.remove(3);
    // 3 comes back as a new object under the same id: the old edge is not to it.
    assert!(l.add(3, "T", (), None));
    assert_eq!(l.after_of(2), None);
    assert_eq!(visited(&mut l), [1, 2, 3]);
}

#[test]
fn an_object_made_in_a_pass_is_not_visited_and_sits_ahead_of_the_unvisited() {
    let mut l = list(&[1, 2, 3]);
    l.begin_pass();
    assert_eq!(l.next_in_pass().unwrap().key, 1);
    l.add(10, "T", (), None);
    assert_eq!(l.next_in_pass().unwrap().key, 2);
    assert_eq!(l.next_in_pass().unwrap().key, 3);
    assert!(l.next_in_pass().is_none());
    assert_eq!(l.order(), [1, 10, 2, 3]);
    assert_eq!(visited(&mut l), [1, 10, 2, 3]);
}

#[test]
fn an_object_removed_in_a_pass_is_skipped_and_a_reused_id_is_a_new_object() {
    let mut l = list(&[1, 2, 3]);
    l.begin_pass();
    assert_eq!(l.next_in_pass().unwrap().key, 1);
    l.remove(2);
    l.remove(3);
    l.add(3, "T", (), None);
    assert!(l.next_in_pass().is_none());
    assert_eq!(l.order(), [1, 3]);
    assert_eq!(l.len(), 2);
}

#[test]
fn the_payload_goes_out_for_a_tick_and_comes_back() {
    let mut l: ProcessList<u8> = ProcessList::new();
    l.add(1, "T", 7, None);
    l.begin_pass();
    let v = l.next_in_pass().unwrap();
    assert_eq!(v.payload, Some(7));
    l.restore(v.key, v.serial, 8);
    l.begin_pass();
    let v = l.next_in_pass().unwrap();
    assert_eq!(v.payload, Some(8));
    assert!(l.next_in_pass().is_none());
    // A payload for an object that left (and came back under its id) is dropped.
    l.remove(v.key);
    l.add(1, "T", 1, None);
    l.restore(1, v.serial, 99);
    l.begin_pass();
    assert_eq!(l.next_in_pass().unwrap().payload, Some(1));
}

#[test]
fn an_object_with_the_process_tick_flag_clear_is_relinked_but_not_visited() {
    let mut l = list(&[1, 2, 3]);
    l.set_ticks(2, false);
    assert_eq!(visited(&mut l), [1, 3]);
    assert_eq!(l.order(), [1, 2, 3]);
    // The pass still hands it out (a control object runs its moves whatever the flag says), marked as not ticking.
    l.begin_pass();
    let seen: Vec<(ObjectKey, bool)> = std::iter::from_fn(|| l.next_in_pass())
        .map(|v| (v.key, v.ticks))
        .collect();
    assert_eq!(seen, [(1, true), (2, false), (3, true)]);
}

#[test]
fn a_pass_that_stops_early_loses_nobody() {
    let mut l = list(&[1, 2, 3, 4]);
    l.begin_pass();
    l.next_in_pass();
    l.finish_pass();
    assert_eq!(l.order(), [1, 2, 3, 4]);
}

#[test]
fn a_hostile_number_of_adds_and_removes_stays_linear() {
    let mut l: ProcessList<()> = ProcessList::new();
    for k in 0..20_000 {
        l.add(k, "T", (), None);
    }
    for k in (0..20_000).step_by(2) {
        l.remove(k);
    }
    assert_eq!(l.len(), 10_000);
    assert_eq!(l.order().len(), 10_000);
    for k in 0..5_000 {
        l.process_after(k * 2 + 1, Some(19_999 - k * 2));
    }
    l.order_list();
    assert_eq!(l.order().len(), 10_000);
}
