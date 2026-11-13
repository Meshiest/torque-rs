use super::*;

#[test]
fn the_global_is_read_as_a_number() {
    let mut vm = Vm::new();
    assert!(!engine_classes_global(&vm));
    for (text, on) in [("0", false), ("1", true), ("2", true), ("0.0", false)] {
        vm.set_global(ENGINE_CLASSES_GLOBAL, text);
        assert_eq!(engine_classes_global(&vm), on, "{text}");
    }
}
