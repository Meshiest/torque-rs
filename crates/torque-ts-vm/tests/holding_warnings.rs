//! `Vm::holding_warnings`: warnings kept back inside the call, handed to the caller, and never held once it is over.

use std::panic::{AssertUnwindSafe, catch_unwind};

use torque_ts_vm::vm::{MAX_HELD_WARNINGS, Vm};

#[test]
fn a_hold_keeps_its_warnings_and_logs_the_other_levels() {
    let mut vm = Vm::new();
    let ((), held) = vm.holding_warnings(|vm| {
        vm.warn("one".into());
        vm.error("an error".into());
        vm.print(b"a line");
        vm.warn("two\tthree".into());
    });
    assert_eq!(held, ["one", "two^three"]);
    assert_eq!(vm.log_texts(), ["an error", "a line"]);
    vm.warn("after".into());
    assert_eq!(vm.log_texts().last().map(String::as_str), Some("after"));
}

#[test]
fn a_nested_hold_keeps_its_own_lines_and_the_outer_one_resumes() {
    let mut vm = Vm::new();
    let ((inner, ()), outer) = vm.holding_warnings(|vm| {
        vm.warn("outer 1".into());
        let inner = vm.holding_warnings(|vm| vm.warn("inner".into())).1;
        vm.warn("outer 2".into());
        (inner, ())
    });
    assert_eq!(inner, ["inner"]);
    assert_eq!(outer, ["outer 1", "outer 2"]);
    assert!(vm.log_texts().is_empty());
}

#[test]
fn a_hold_ends_when_the_call_panics() {
    let mut vm = Vm::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        vm.holding_warnings(|vm| {
            vm.warn("held".into());
            panic!("the call fails");
        })
    }));
    assert!(result.is_err());
    vm.warn("kept".into());
    assert_eq!(vm.log_texts(), ["kept"]);
}

#[test]
fn a_hold_is_capped_and_cuts_long_lines() {
    let mut vm = Vm::new();
    let ((), held) = vm.holding_warnings(|vm| {
        vm.warn("x".repeat(5000));
        for i in 0..MAX_HELD_WARNINGS + 3 {
            vm.warn(format!("w{i}"));
        }
    });
    assert_eq!(held.len(), MAX_HELD_WARNINGS);
    assert_eq!(held[0].len(), torque_ts_vm::vm::MAX_LOG_LINE);
    // The warnings past the cap were logged as usual.
    assert_eq!(vm.log_texts().len(), 4);
}
