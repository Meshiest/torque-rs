//! The macros as a user meets them: small programs compiled by `trybuild`.

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/passthrough.rs");
    cases.pass("tests/ui/plugin_id.rs");
    cases.compile_fail("tests/ui/plugin_id_bad.rs");
    cases.pass("tests/ui/natives.rs");
    cases.pass("tests/ui/prefs_classes.rs");
    // A wrong default or range type, a default outside its range and a field type that is not a pref or console field
    // type are `compile_fail` doctests in torque-engine (`Prefs`, `EngineClass`): that text is rustc's own.
    cases.compile_fail("tests/ui/prefs_bad_attribute.rs");
    cases.compile_fail("tests/ui/class_bad_attribute.rs");
    // An unknown parameter type and a name taken twice in one `natives!` list are `compile_fail` doctests in
    // torque-engine (`function`, `natives!`): their compiler output changes between releases.
    for case in [
        "native_option_first",
        "native_two_vm_mut",
        "native_vm_mut_and_ref",
        "native_methods_collision",
        "native_self_in_function",
        "native_on_trait",
        "native_bad_options",
    ] {
        cases.compile_fail(format!("tests/ui/{case}.rs"));
    }
}
