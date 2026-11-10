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

/// An object made from Rust is the one the script's declaration makes: its fields written through their types (never run as
/// script).
#[test]
fn an_object_made_from_rust_is_the_declarations_object() {
    let mut vm = Vm::new();
    vm.register_class("Crate", "SimObject", &["weight", "label"])
        .expect("registers");
    let id = vm
        .new_object(
            b"Crate",
            &[(b"weight", b"3"), (b"label", b"\"); quit(); (\"")],
        )
        .unwrap()
        .expect("an object");
    assert_eq!(
        vm.eval(format!("return {id}.weight;").as_bytes()).unwrap(),
        b"3"
    );
    let label = vm.eval(format!("return {id}.label;").as_bytes()).unwrap();
    assert_eq!(label, b"\"); quit(); (\"");
    // An unknown class makes nothing.
    assert_eq!(vm.new_object(b"NoSuchClass", &[]).unwrap(), None);
}

/// A class that takes constructor arguments sees them in its behaviour's `process_arguments`, which may refuse them; one that
/// does not take them fails any declaration with arguments, as before.
#[test]
fn constructor_arguments_reach_the_behaviour_that_may_refuse_them() {
    struct Count;
    impl crate::classes::EngineClass for Count {
        fn process_arguments(
            &self,
            _: &crate::sim::Objects,
            object: &mut crate::sim::SimObject,
            args: &[Vec<u8>],
        ) -> bool {
            object.ext = Some(Box::new(args.len()));
            args.len() == 1
        }
        fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
            let seen = vm
                .objects_mut()
                .get_mut(id)
                .and_then(|o| o.ext.take())
                .and_then(|ext| ext.downcast::<usize>().ok())
                .map_or(0, |n| *n);
            vm.set_global_text(b"$seen", seen.to_string().as_bytes());
            Ok(true)
        }
    }
    let mut vm = Vm::new();
    let class = vm
        .register_class("Takes", "SimObject", &[])
        .expect("registers");
    vm.objects_mut()
        .classes
        .get_mut(class)
        .expect("the class")
        .takes_arguments = true;
    vm.set_class_behaviour("Takes", std::rc::Rc::new(Count));
    vm.register_class("Plain", "SimObject", &[])
        .expect("registers");
    let made = vm
        .eval(b"$a = new Takes(A, 7); $b = new Takes(B, 1, 2); $c = new Plain(C, 1); return isObject($a) SPC isObject($b) SPC isObject($c) SPC $seen;")
        .expect("runs");
    // One argument is taken; two are refused (after the behaviour saw them), and a class without arguments refuses any.
    assert_eq!(made, b"1 0 0 1");
}
