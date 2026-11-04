use super::*;

#[test]
fn the_breakdowns_function_sets_the_pref_and_answers_the_mode() {
    let mut vm = Vm::new();
    register(&mut vm, "perfMode", "$Pref::PerfMode");
    assert_eq!(vm.eval(b"return perfMode();").unwrap(), b"-1");
    assert_eq!(vm.eval(b"return perfMode(1);").unwrap(), b"1");
    assert_eq!(Mode::of_vm(&vm, "$Pref::PerfMode"), Mode::On);
    assert_eq!(vm.eval(b"return perfMode(0);").unwrap(), b"0");
    assert_eq!(Mode::of_vm(&vm, "$Pref::PerfMode"), Mode::Auto);
    assert_eq!(vm.eval(b"return perfMode(\"x\");").unwrap(), b"-1");
    assert_eq!(Mode::of_pref(b""), Mode::Off);
    assert_eq!(Mode::of_pref(b" 7 "), Mode::On);
}
