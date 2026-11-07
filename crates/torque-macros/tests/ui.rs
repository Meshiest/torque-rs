//! The macros as a user meets them: small programs compiled by `trybuild`.

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/passthrough.rs");
}
