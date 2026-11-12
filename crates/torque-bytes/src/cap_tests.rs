use super::*;

#[test]
fn a_value_over_its_cap_is_an_error_that_says_both() {
    assert_eq!(CapExceeded::check("glyphs", 10, 10), Ok(()));
    let err = CapExceeded::check("glyphs", 11, 10).unwrap_err();
    assert_eq!(
        err,
        CapExceeded {
            what: "glyphs",
            value: 11,
            cap: 10
        }
    );
    assert_eq!(err.to_string(), "glyphs: 11 is above the limit of 10");
}
