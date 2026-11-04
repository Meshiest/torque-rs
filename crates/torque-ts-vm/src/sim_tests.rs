//! The work counters: every loop whose length a script controls must charge its length.
use super::*;

fn add(objects: &mut Objects, class: ClassId, name: &str) -> ObjId {
    let mut object = SimObject::new(class, &objects.classes);
    object.set_initial_name(name.as_bytes());
    objects.register(object).map(|r| r.id).unwrap_or(0)
}

fn filled(class: ClassId, count: usize) -> (Objects, ObjId, Vec<ObjId>) {
    let mut objects = Objects::new();
    let set = add(&mut objects, class, "S");
    let kids: Vec<ObjId> = (0..count)
        .map(|_| add(&mut objects, SCRIPT_OBJECT, ""))
        .collect();
    for kid in &kids {
        assert!(objects.add(set, *kid));
    }
    objects.take_work();
    (objects, set, kids)
}

#[test]
fn unregistering_a_set_charges_its_members() {
    let (mut objects, set, _) = filled(SIM_SET, 7);
    objects.unregister(set);
    assert_eq!(objects.take_work(), 7);
}

#[test]
fn moving_a_member_charges_the_list() {
    let (mut objects, set, kids) = filled(SIM_SET, 5);
    objects.move_member(set, kids[2], true);
    assert_eq!(objects.take_work(), 5);
    objects.move_member(set, kids[2], false);
    assert_eq!(objects.take_work(), 5);
}

#[test]
fn clearing_charges_the_members() {
    let (mut objects, set, _) = filled(SIM_SET, 6);
    assert_eq!(objects.clear(set), 6);
    assert_eq!(objects.take_work(), 6);
}

#[test]
fn skipping_ids_in_use_charges_each_skip() {
    let mut objects = Objects::new();
    objects.take_work();
    let first = SimConfig::default().last_dynamic_id;
    objects.set_last_dynamic_id(first + 4);
    // The 12 start-up objects come first: 8 skips past the fifth.
    assert_eq!(add(&mut objects, SCRIPT_OBJECT, ""), first + 13);
    assert_eq!(objects.take_work(), 8);
}

#[test]
fn a_path_through_a_plain_set_charges_its_members() {
    let (objects, _, _) = filled(SIM_SET, 9);
    assert_eq!(objects.find(b"S/nobody"), None);
    assert_eq!(objects.take_work(), 9);
    // A group has its own name table: no scan.
    let (objects, _, _) = filled(SIM_GROUP, 9);
    assert_eq!(objects.find(b"S/nobody"), None);
    assert_eq!(objects.take_work(), 0);
}

#[test]
fn the_cascade_snapshot_charges_the_members() {
    let (objects, set, _) = filled(SIM_GROUP, 4);
    assert_eq!(objects.live_members_by_id(set).len(), 4);
    assert_eq!(objects.take_work(), 4);
}

#[test]
fn the_cascade_snapshot_charges_dead_ids_it_scans() {
    // A cascaded member leaves its group's list only when the group goes: the scan still passes it.
    let (mut objects, set, kids) = filled(SIM_GROUP, 4);
    objects.unlink_from_group(kids[0]);
    objects.unregister(kids[0]);
    assert_eq!(objects.live_members_by_id(set).len(), 3);
    assert_eq!(objects.take_work(), 4);
}

#[test]
fn sorting_a_member_list_charges_it_and_orders_it_by_id() {
    let (mut objects, set, kids) = filled(SIM_GROUP, 5);
    objects.move_member(set, kids[0], false);
    objects.take_work();
    objects.sort_members(set);
    assert_eq!(objects.take_work(), 5);
    assert_eq!(objects.members(set).unwrap().as_slice(), &kids[..]);
    // The index follows: removal by id still swaps the right member.
    assert!(objects.remove(set, kids[1]));
    assert_eq!(objects.members(set).unwrap().get(1), Some(kids[4]));
}

#[test]
fn registering_again_starts_unheld() {
    let mut objects = Objects::new();
    let id = add(&mut objects, SCRIPT_OBJECT, "H");
    objects.lock(id);
    objects.lock(id);
    assert!(objects.defer_delete(id));
    let object = objects.unregister(id).unwrap();
    assert_eq!((object.locks(), object.delete_pending()), (2, true));
    assert_eq!(objects.register(object).map(|r| r.id), Some(id));
    let again = objects.get(id).unwrap();
    assert_eq!((again.locks(), again.delete_pending()), (0, false));
}

#[test]
fn a_copy_of_dynamic_fields_costs_a_unit_per_field_and_per_byte() {
    let mut fields = Fields::default();
    assert_eq!(fields.copy_cost(), 0);
    // The folded name (3), the spelling (3) and the value (4), and the field itself.
    fields.set(b"AbC", b"wxyz");
    assert_eq!(fields.copy_cost(), 1 + 3 + 3 + 4);
    fields.set(b"d", b"12");
    assert_eq!(fields.copy_cost(), (1 + 3 + 3 + 4) + (1 + 1 + 1 + 2));
    // A longer value costs its bytes; a deleted field nothing.
    fields.set(b"abc", b"wxyz0000");
    assert_eq!(fields.copy_cost(), (1 + 3 + 3 + 8) + (1 + 1 + 1 + 2));
    fields.set(b"d", b"");
    assert_eq!(fields.copy_cost(), 1 + 3 + 3 + 8);
}

#[test]
fn a_dynamic_read_is_charged_by_its_length_and_refused_without_a_copy() {
    let classes = ClassRegistry::new();
    let mut object = SimObject::new(SCRIPT_OBJECT, &classes);
    object.set_data_field(&classes, b"dyn", b"", b"twelve bytes");
    let mut charged = 0;
    let read = object.data_field_charged(&classes, b"dyn", b"", &|_| None, &mut |units| {
        charged += units;
        true
    });
    assert_eq!((read.as_slice(), charged), (&b"twelve bytes"[..], 12));
    // A field that is not there costs nothing; a refused read copies nothing.
    let read = object.data_field_charged(&classes, b"none", b"", &|_| None, &mut |_| {
        panic!("charged")
    });
    assert!(read.is_empty());
    let read = object.data_field_charged(&classes, b"dyn", b"", &|_| None, &mut |_| false);
    assert!(read.is_empty());
}
