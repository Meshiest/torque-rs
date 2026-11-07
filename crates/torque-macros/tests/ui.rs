//! The macros as a user meets them: small programs compiled by `trybuild`.

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/passthrough.rs");
    cases.pass("tests/ui/plugin_id.rs");
    cases.compile_fail("tests/ui/plugin_id_bad.rs");
}
