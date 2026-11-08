use super::*;

fn field_type(vm: &Vm, class: &str, field: &str) -> Option<FieldType> {
    let classes = &vm.objects().classes;
    let def = classes.get(classes.find(class.as_bytes())?)?;
    def.layout.field(field.as_bytes()).map(|(f, _)| f.ty)
}

#[test]
fn the_authored_classes_alone_make_the_gui_tree_with_its_typed_fields() {
    let mut vm = Vm::new();
    crate::register(&mut vm);
    let classes = &vm.objects().classes;
    let control = classes.find(b"GuiControl").expect("GuiControl");
    for spec in CLASSES {
        let class = classes.find(spec.name.as_bytes()).expect(spec.name);
        let parent = classes.find(spec.parent.unwrap().as_bytes()).unwrap();
        assert!(
            classes.is_a(class, parent),
            "{} under {:?}",
            spec.name,
            spec.parent
        );
        let profile_like = matches!(
            spec.name,
            "GuiControlProfile" | "GuiCursor" | "MessageVector"
        );
        assert_eq!(classes.is_a(class, control), !profile_like, "{}", spec.name);
    }
    // A subclass reads its ancestors' fields (any case), each with its type.
    assert_eq!(
        field_type(&vm, "GuiWindowCtrl", "horizsizing"),
        Some(FieldType::Enum(&HORIZ_SIZING))
    );
    assert_eq!(
        field_type(&vm, "GuiWindowCtrl", "text"),
        Some(FieldType::CaseString)
    );
    assert_eq!(
        field_type(&vm, "GuiRadioCtrl", "buttonType"),
        Some(FieldType::Enum(&BUTTON_TYPE))
    );
    assert_eq!(
        field_type(&vm, "GuiScrollCtrl", "profile"),
        Some(FieldType::GuiProfile)
    );
    assert_eq!(
        field_type(&vm, "GuiControlProfile", "fontColors"),
        Some(FieldType::ColorI)
    );
    assert_eq!(field_type(&vm, "GuiControl", "fontColors"), None);
    // A second install adds nothing.
    assert_eq!(install(&mut vm), 0);
}

#[test]
fn a_class_the_host_registered_first_keeps_its_own_fields() {
    let mut vm = Vm::new();
    let own = vec![torque_ts_vm::sim::StaticField {
        name: b"ownField".to_vec(),
        ty: FieldType::S32,
        count: 1,
        offset: None,
        validator: None,
        doc: None,
    }];
    vm.register_class_typed("GuiControl", "SimGroup", own, true)
        .unwrap();
    assert_eq!(install(&mut vm), CLASSES.len() - 1);
    assert_eq!(
        field_type(&vm, "GuiControl", "ownField"),
        Some(FieldType::S32)
    );
    assert_eq!(field_type(&vm, "GuiControl", "horizSizing"), None);
    // The classes under it still read their own.
    assert_eq!(
        field_type(&vm, "GuiTextCtrl", "maxLength"),
        Some(FieldType::S32)
    );
}
