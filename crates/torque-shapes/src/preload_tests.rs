use super::*;

fn detail(name: i32) -> Detail {
    Detail {
        name,
        sub_shape: 0,
        object_detail: 0,
        size: 1.0,
        average_error: -1.0,
        max_error: -1.0,
        poly_count: 0,
    }
}

fn names(list: &[&str]) -> Vec<Vec<u8>> {
    list.iter().map(|s| s.as_bytes().to_vec()).collect()
}

#[test]
fn every_detail_whose_name_contains_collision_is_a_collision_detail_in_detail_order() {
    // Case does not matter, the number does not, nor does text around it; "Detail-1" is not one.
    let n = names(&["Detail-1", "COLLISION-1", "collision-3", "myCollision-99"]);
    let d = [detail(0), detail(1), detail(2), detail(3)];
    let slots = collision_slots(&n, &d);
    assert_eq!(slots.collision, [1, 2, 3]);
    assert_eq!(slots.hulls(), 3);
}

#[test]
fn there_is_no_limit_of_eight_collision_details() {
    let list: Vec<String> = (1..=12).map(|i| format!("Collision-{i}")).collect();
    let n: Vec<Vec<u8>> = list.iter().map(|s| s.as_bytes().to_vec()).collect();
    let d: Vec<Detail> = (0..12).map(detail).collect();
    let slots = collision_slots(&n, &d);
    assert_eq!(slots.collision, (0..12).collect::<Vec<i32>>());
    assert_eq!(slots.los, (0..12).collect::<Vec<i32>>());
}

#[test]
fn a_collision_detail_takes_the_los_detail_named_after_its_own_index_plus_nine_else_itself() {
    // Detail 0 (Collision-1) finds LOS-9; detail 1 (Collision-2) looks for LOS-10, absent, and borrows itself.
    let n = names(&["Collision-1", "Collision-2", "LOS-9", "LOS-1"]);
    let d = [detail(0), detail(1), detail(2), detail(3)];
    let slots = collision_slots(&n, &d);
    assert_eq!(slots.collision, [0, 1]);
    // Then the LOS details no entry claims, in detail order: LOS-1 (detail 3); LOS-9 is claimed already.
    assert_eq!(slots.los, [2, 1, 3]);
    assert_eq!(slots.own_los(), 2);
}

#[test]
fn the_los_name_follows_the_detail_index_not_the_collision_count() {
    // Collision-1 is detail 2: it looks for LOS-11, not LOS-9.
    let n = names(&["Detail-1", "Detail-2", "Collision-1", "LOS-9", "LOS-11"]);
    let d = [detail(0), detail(1), detail(2), detail(3), detail(4)];
    let slots = collision_slots(&n, &d);
    assert_eq!(slots.collision, [2]);
    assert_eq!(slots.los, [4, 3]);
}

#[test]
fn the_first_detail_that_names_the_entry_wins_and_a_name_with_no_detail_is_missing() {
    // Both details name "Collision-1": both are collision details (the name is tested per detail).
    let n = names(&["Collision-1", "Collision-2"]);
    let d = [detail(0), detail(0)];
    let slots = collision_slots(&n, &d);
    assert_eq!(slots.collision, [0, 1]);
    // A lookup by name takes the first detail that names the entry; a name no detail uses finds none.
    assert_eq!(find_detail(&n, &d, "collision-1"), 0);
    assert_eq!(find_detail(&n, &d, "Collision-2"), -1);
    assert_eq!(collision_slots(&[], &[]).hulls(), 0);
    // A name index outside the table names nothing.
    assert_eq!(
        collision_slots(&n, &[detail(7), detail(-1)]),
        Slots::default()
    );
}

/// A host's rule with two entries: the detail named `Hull` in the second, the first left empty.
fn second_only(names: &[Vec<u8>], details: &[Detail]) -> Slots {
    Slots {
        collision: vec![-1, find_detail(names, details, "Hull")],
        los: vec![-1, -1],
    }
}

#[test]
fn a_host_rule_may_leave_holes_and_the_facts_follow_its_entries() {
    let model = crate::synth::hull_shape(
        [-10.0, -10.0, -10.0, 10.0, 10.0, 10.0],
        &[("Hull", [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0])],
        &[],
        &[],
    );
    let f = facts_with(&model.shape, second_only, &mut |_| true).unwrap();
    assert_eq!(f.slots.collision, [-1, 0]);
    assert_eq!(f.findings.len(), 2);
    assert_eq!(f.findings[0], None);
    assert_eq!(f.findings[1], Some(Finding::Inside));
    // TGE's rule over the same shape: no detail named `collision-`.
    let f = facts(&model.shape, &mut |_| true).unwrap();
    assert!(f.slots.collision.is_empty());
    assert!(f.findings.is_empty());
}
