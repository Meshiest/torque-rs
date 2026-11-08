use super::*;

#[test]
fn sorting_is_charged_by_its_comparisons() {
    // n elements take about n * log2(n) comparisons: n times the bits of n, rounded up.
    assert_eq!(sort_work(0), 0);
    assert_eq!(sort_work(1), 1);
    assert_eq!(sort_work(2), 4);
    assert_eq!(sort_work(1000), 10_000);
    assert_eq!(sort_work(1 << 20), (1 << 20) * 21);
}

#[test]
fn a_dump_line_carries_the_first_4096_bytes_of_its_name_and_value() {
    let name = vec![b'n'; DUMP_PART_BYTES + 10];
    let value = vec![b'v'; DUMP_PART_BYTES + 10];
    let line = field_line(&name, &value);
    assert_eq!(line.len(), 2 + DUMP_PART_BYTES + 4 + DUMP_PART_BYTES + 1);
    // The value is expanded (a newline is `\n`, two characters); the name is not.
    assert_eq!(field_line(b"a", b"b\n"), br#"  a = "b\n""#);
}

/// A console class's dump, line for line: the classes that document no fields print no `Field Docs:` section, so
/// their dumps are what they were before field docs existed.
#[test]
fn the_dump_of_a_console_class_without_field_docs_is_unchanged() {
    let mut vm = Vm::new();
    vm.register_class("Crate", "SimObject", &["weight", "label"])
        .expect("registers");
    vm.eval(b"new Crate(Box) { weight = 3; label = \"a b\"; extra = 7; }; Box.dump();")
        .expect("runs");
    assert_eq!(vm.log_texts(), DUMP_BEFORE_FIELD_DOCS);
}

/// What that dump printed before field docs existed.
const DUMP_BEFORE_FIELD_DOCS: &[&str] = &[
    "Member Fields:",
    "  label = \"a b\"",
    "  weight = \"3\"",
    "Tagged Fields:",
    "  extra = \"7\"",
    "Methods:",
    "  delete() - object.delete() - deletes the object",
    "  dump() - object.dump() - prints the object's fields and methods",
    "  getClassName() - object.getClassName() - the object's class",
    "  getGroup() - object.getGroup() - the group that holds the object",
    "  getId() - object.getId() - the object's id",
    "  getInternalName() - obj.getInternalName()",
    "  getName() - object.getName() - the object's name",
    "  getType() - object.getType() - the object's type mask",
    "  schedule() - object.schedule(delay, method, args...) - calls a method of the object later",
    "  setInternalName() - obj.setInternalName(newInternalName)",
    "  setName() - object.setName(name) - renames the object",
];
