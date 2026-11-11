use super::*;

#[test]
fn the_first_spelling_wins_ignoring_case() {
    let mut table = StringTable::default();
    assert_eq!(&*table.intern(b"Abc"), b"Abc");
    assert_eq!(&*table.intern(b"aBC"), b"Abc");
    assert_eq!(&*table.intern(b"ABC"), b"Abc");
    assert_eq!(&*table.intern(b"abd"), b"abd");
    assert_eq!(&*table.intern(b""), b"");
}

#[test]
fn past_the_cap_a_text_is_returned_as_written_and_not_remembered() {
    // Room for one 3-byte text (3 bytes of key and 3 of value) and nothing more.
    let mut table = StringTable::with_cap(6);
    assert_eq!(&*table.intern(b"Abc"), b"Abc");
    assert_eq!(&*table.intern(b"Xyz"), b"Xyz");
    assert_eq!(&*table.intern(b"xYZ"), b"xYZ");
    assert_eq!(&*table.intern(b"xyz"), b"xyz");
    // What was remembered before the cap still answers.
    assert_eq!(&*table.intern(b"aBC"), b"Abc");
    assert_eq!(table.bytes, 6);
}

#[test]
fn every_class_name_is_in_the_vms_table_before_any_script_runs() {
    let mut vm = crate::vm::Vm::new();
    assert_eq!(&*vm.strings.intern(b"SCRIPTOBJECT"), b"ScriptObject");
    assert_eq!(&*vm.strings.intern(b"simgroup"), b"SimGroup");
    assert!(vm.register_class("P1Class", "SimObject", &[]).is_some());
    assert_eq!(&*vm.strings.intern(b"p1class"), b"P1Class");
    // A refused registration (the name is taken) leaves the first spelling.
    assert!(vm.register_class("P1CLASS", "SimObject", &[]).is_none());
    assert_eq!(&*vm.strings.intern(b"P1CLASS"), b"P1Class");
}

fn typed(name: &str, ty: crate::fields::FieldType, count: u16) -> crate::sim::StaticField {
    crate::sim::StaticField {
        name: name.as_bytes().to_vec(),
        ty,
        count,
        offset: None,
        validator: None,
        doc: None,
    }
}

fn read(vm: &crate::vm::Vm, class: crate::sim::ClassId, field: &str, index: &str) -> Vec<u8> {
    let object = crate::sim::SimObject::new(class, &vm.objects().classes);
    object.data_field(&vm.objects().classes, field.as_bytes(), index.as_bytes())
}

#[test]
fn defaults_are_parsed_by_the_fields_type_and_run_no_validator() {
    use crate::fields::FieldType;
    let mut vm = crate::vm::Vm::new();
    let mut ranged = typed("n", FieldType::S32, 1);
    ranged.validator = Some(Validator::ScaledIntRange {
        factor: 32,
        min: 0,
        max: 10,
    });
    let class = vm
        .register_class_typed(
            "DefaultsClass",
            "SimObject",
            vec![
                ranged,
                typed("arr", FieldType::S32, 3),
                typed("p", FieldType::Point3F, 1),
                typed("s", FieldType::String, 1),
            ],
            true,
        )
        .unwrap();
    vm.apply_defaults(
        class,
        &[
            ("n", "625"),
            ("arr[2]", "7"),
            ("p", "1 2 3"),
            ("s", "Mixed"),
        ],
        false,
    );
    // 625 is stored as written: the validator (625 / 32 = 19, then clamped to 10) did not run.
    assert_eq!(read(&vm, class, "n", ""), b"625");
    assert_eq!(read(&vm, class, "arr", "2"), b"7");
    assert_eq!(read(&vm, class, "arr", "0"), b"0");
    assert_eq!(read(&vm, class, "p", ""), b"1 2 3");
    assert_eq!(read(&vm, class, "s", ""), b"Mixed");
    // The string went through the runtime's string table, as a write would.
    assert_eq!(&*vm.strings.intern(b"MIXED"), b"Mixed");
}

#[test]
fn defaults_naming_what_the_class_lacks_or_that_are_malformed_are_skipped() {
    use crate::fields::FieldType;
    let mut vm = crate::vm::Vm::new();
    let class = vm
        .register_class_typed(
            "SkipClass",
            "SimObject",
            vec![typed("a", FieldType::S32, 2)],
            true,
        )
        .unwrap();
    vm.apply_defaults(
        class,
        &[
            ("nosuch", "1"),
            ("a[", "1"),
            ("a[]", "2"),
            ("a[2]", "3"),
            ("a[-1]", "4"),
            ("a[99999999999999999999]", "5"),
            ("[0]", "6"),
            ("", "7"),
            ("a[1]]]", "8"),
        ],
        false,
    );
    // `a[` and `a[]` are element 0 (atoi of nothing); `a[1]]]` is element 1; the rest write nothing.
    assert_eq!(read(&vm, class, "a", "0"), b"2");
    assert_eq!(read(&vm, class, "a", "1"), b"8");
    // An unknown class id is ignored.
    vm.apply_defaults(9999, &[("a", "1")], false);
}

#[test]
fn a_child_class_does_not_keep_the_parents_default_the_dump_did_not_list() {
    use crate::fields::FieldType;
    let mut vm = crate::vm::Vm::new();
    let parent = vm
        .register_class_typed(
            "DefaultsParent",
            "SimObject",
            vec![typed("a", FieldType::S32, 1)],
            true,
        )
        .unwrap();
    vm.apply_defaults(parent, &[("a", "5")], false);
    let child = vm
        .register_class_typed(
            "DefaultsChild",
            "DefaultsParent",
            vec![typed("b", FieldType::S32, 1)],
            true,
        )
        .unwrap();
    // The child's layout started from the parent's defaults ...
    assert_eq!(read(&vm, child, "a", ""), b"5");
    // ... and a dump that lists only `b` means `a` reads empty (0) on a fresh child.
    vm.apply_defaults(child, &[("b", "2")], false);
    assert_eq!(read(&vm, child, "a", ""), b"0");
    assert_eq!(read(&vm, child, "b", ""), b"2");
    assert_eq!(read(&vm, parent, "a", ""), b"5");
}

#[test]
fn an_inheriting_child_keeps_the_parents_defaults_it_does_not_list() {
    use crate::fields::FieldType;
    let mut vm = crate::vm::Vm::new();
    let parent = vm
        .register_class_typed(
            "InheritParent",
            "SimObject",
            vec![typed("a", FieldType::S32, 1)],
            true,
        )
        .unwrap();
    vm.apply_defaults(parent, &[("a", "5")], false);
    let child = vm
        .register_class_typed(
            "InheritChild",
            "InheritParent",
            vec![typed("b", FieldType::S32, 1)],
            true,
        )
        .unwrap();
    vm.apply_defaults(child, &[("b", "2")], true);
    assert_eq!(read(&vm, child, "a", ""), b"5");
    assert_eq!(read(&vm, child, "b", ""), b"2");
}

#[test]
fn a_text_of_any_bytes_is_kept_whole() {
    let mut table = StringTable::default();
    let odd: &[u8] = b"a\0B\xff";
    assert_eq!(&*table.intern(odd), odd);
    assert_eq!(&*table.intern(b"A\0b\xff"), odd);
}
#[test]
fn a_write_to_the_frames_object_reaches_a_registered_one_as_it_reaches_a_pending_one() {
    use crate::fields::FieldType;
    use crate::vm::objops::Creating;
    let mut vm = crate::vm::Vm::new();
    let field = typed("n", FieldType::S32, 1);
    vm.register_class_typed("LiveClass", "SimObject", vec![field], true)
        .unwrap();
    let id = vm.add_engine_object(b"LiveClass", &[]).unwrap();
    // The frame's object is a registered one (an existing datablock being declared again).
    vm.creating.push(Creating {
        object: Some(NewObject::Live(id)),
        fail_jump: 0,
    });
    vm.write_field(None, b"n", b"", b"7");
    vm.write_field(None, b"dyn", b"", b"text");
    assert_eq!(vm.objects().field(id, b"n", b""), Some(b"7".to_vec()));
    assert_eq!(vm.objects().field(id, b"dyn", b""), Some(b"text".to_vec()));
    // No object being built: nothing happens, and nothing panics.
    vm.creating.clear();
    vm.write_field(None, b"n", b"", b"9");
    assert_eq!(vm.objects().field(id, b"n", b""), Some(b"7".to_vec()));
    // The same through a pending object.
    let pending = crate::sim::SimObject::new(
        vm.objects().classes.find(b"LiveClass").unwrap(),
        &vm.objects().classes,
    );
    vm.creating.push(Creating {
        object: Some(NewObject::Pending(Box::new(pending))),
        fail_jump: 0,
    });
    vm.write_field(None, b"n", b"", b"5");
    let Some(NewObject::Pending(object)) = vm.creating.last().and_then(|c| c.object.as_ref())
    else {
        panic!("the pending object is gone");
    };
    assert_eq!(
        object.data_field(&vm.objects().classes, b"n", b""),
        b"5".to_vec()
    );
}

/// A retiring datablock (a host's reference-counted removal) resolves for no datablock-typed field write, by name or id, and
/// its name declares a new datablock; the set lets the id go with the object.
#[test]
fn a_retiring_datablock_is_found_by_no_field_write_and_its_name_declares_a_new_one() {
    use crate::fields::FieldType;
    let mut vm = crate::vm::Vm::new();
    vm.register_class_typed(
        "Holder",
        "SimObject",
        vec![typed("data", FieldType::DataBlock("SimDataBlock"), 1)],
        true,
    )
    .unwrap();
    vm.eval(b"datablock SimDataBlock(D) {}; new Holder(H) { data = D; };")
        .unwrap();
    let (d, h) = (
        vm.objects().find(b"D").unwrap(),
        vm.objects().find(b"H").unwrap(),
    );
    let data = |vm: &crate::vm::Vm| {
        let object = vm.objects().get(h).unwrap();
        let (_, slots) = vm
            .objects()
            .classes
            .static_field(object.class(), b"data")
            .unwrap();
        match object.slot(slots[0]) {
            Some(crate::fields::Value::Object(id)) => *id,
            other => panic!("a pointer slot: {other:?}"),
        }
    };
    assert_eq!(data(&vm), Some(d));

    assert!(vm.objects_mut().retire_datablock(d));
    assert!(!vm.objects_mut().retire_datablock(d), "retired once");
    assert!(!vm.objects_mut().retire_datablock(h), "not a datablock");
    assert!(vm.objects().is_retiring(d));
    assert_eq!(vm.objects().retiring_datablocks(), [d]);
    // Neither the name nor the id finds it for a write; the field is cleared, as for a name that is no datablock.
    vm.eval(b"H.data = D;").unwrap();
    assert_eq!(data(&vm), None);
    vm.eval(format!("H.data = {d};").as_bytes()).unwrap();
    assert_eq!(data(&vm), None);
    assert_eq!(
        vm.log_texts()
            .iter()
            .filter(|l| l.contains("is not a datablock of class SimDataBlock"))
            .count(),
        2
    );
    // The name declares a new datablock, with a new id; the retiring one is still registered and still named.
    vm.eval(b"datablock SimDataBlock(D) {};").unwrap();
    let d2 = vm.objects().find(b"D").unwrap();
    assert_ne!(d2, d);
    assert!(vm.objects().contains(d) && !vm.objects().is_retiring(d2));
    vm.eval(b"H.data = D;").unwrap();
    assert_eq!(data(&vm), Some(d2));
    vm.delete_object(d).unwrap();
    assert!(vm.objects().retiring_datablocks().is_empty());
    assert_eq!(
        vm.objects().find(b"D"),
        Some(d2),
        "the new one keeps the name"
    );
}
