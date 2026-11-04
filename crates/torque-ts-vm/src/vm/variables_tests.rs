use super::*;

#[test]
fn atob_is_true_for_true_and_nonzero_numbers() {
    assert!(atob(b"TRUE") && atob(b"1") && atob(b"0.5") && atob(b"nan"));
    assert!(!atob(b"false") && !atob(b"0") && !atob(b"") && !atob(b"yes"));
}

#[test]
fn atob_converts_the_number_through_a_float() {
    // `export`'s append flag is `(F32)atof(s) != 0`, as a bool field's.
    for (text, want) in [
        (&b"1e-50"[..], false),
        (b"-1e-50", false),
        (b"7e-46", false),
        (b"7.1e-46", true),
        (b"1e-45", true),
        (b"0x1p-200", false),
        (b"1e-300", false),
        (b"1", true),
        (b"0", false),
        (b"true", true),
    ] {
        assert_eq!(atob(text), want, "{}", String::from_utf8_lossy(text));
    }
}

#[test]
fn names_compare_ignoring_ascii_case() {
    use std::cmp::Ordering::{Equal, Greater, Less};
    assert_eq!(cmp_ignoring_case(b"$A", b"$a"), Equal);
    assert_eq!(cmp_ignoring_case(b"$a", b"$B"), Less);
    assert_eq!(cmp_ignoring_case(b"$ab", b"$A"), Greater);
}
