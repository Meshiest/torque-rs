use super::*;

/// What `point` and `flag` read before they read the stored numbers: the field as text, read back.
fn read_as_text(vm: &Vm, id: ObjId, field: &[u8]) -> (i32, i32) {
    let text = vm.objects().field(id, field, b"").unwrap_or_default();
    let mut words = text
        .split(|b| *b == b' ')
        .filter(|w| !w.is_empty())
        .map(atoi);
    (words.next().unwrap_or(0), words.next().unwrap_or(0))
}

#[test]
fn a_position_and_a_flag_read_as_stored_are_what_their_text_reads_as() {
    let mut vm = Vm::new();
    crate::register(&mut vm);
    vm.eval(
        b"new GuiControl(Odd) { position = \"-5 70000\"; extent = \"3\"; visible = 0; };\
          new GuiControl(Plain) { position = \"10 20\"; extent = \"30 40\"; };\
          new GuiControl(Huge) { position = \"2147483647 -2147483648\"; };",
    )
    .expect("the controls");
    for name in [&b"Odd"[..], b"Plain", b"Huge"] {
        let id = vm.objects().find(name).expect("the control");
        for field in [&b"position"[..], b"extent"] {
            assert_eq!(
                point(&vm, id, field),
                read_as_text(&vm, id, field),
                "{} {}",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(field)
            );
        }
    }
    let odd = vm.objects().find(b"Odd").expect("Odd");
    let plain = vm.objects().find(b"Plain").expect("Plain");
    assert_eq!(point(&vm, odd, b"position"), (-5, 70_000));
    assert!(!flag(&vm, odd, b"visible"));
    assert!(flag(&vm, plain, b"visible"));
    // A field the class does not declare is a dynamic field, read as text.
    vm.eval(b"Plain.spot = \"7 8\"; Plain.on = 1;")
        .expect("fields");
    assert_eq!(point(&vm, plain, b"spot"), (7, 8));
    assert!(flag(&vm, plain, b"on"));
    assert_eq!(point(&vm, plain, b"missing"), (0, 0));
}
