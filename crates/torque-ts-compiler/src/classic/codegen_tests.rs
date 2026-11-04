use super::*;

use torque_ts_text::atof;

fn number(text: &[u8]) -> (f64, bool) {
    string_to_number(text, atof)
}

#[test]
fn strings_used_as_numbers_go_through_a_float() {
    assert_eq!(number(b"0.1").0, f64::from(0.1f32));
    assert_eq!(number(b"TRUE"), (1.0, false));
    assert_eq!(number(b"False"), (0.0, false));
    assert_eq!(number(b"abc"), (0.0, true));
    assert_eq!(number(b"0"), (0.0, true));
}

/// A node and its first field can share an address; their keys still differ (an `ObjectDecl` and its
/// inline `class` expression, whatever layout rustc picks).
#[test]
fn a_node_and_its_first_field_have_different_keys() {
    #[repr(C)]
    struct Outer {
        first: u32,
        second: u32,
    }
    let outer = Outer {
        first: 1,
        second: 2,
    };
    assert_eq!(
        &outer as *const Outer as usize,
        &outer.first as *const u32 as usize
    );
    assert_ne!(key(&outer), key(&outer.first));
    assert_eq!(key(&outer), key(&outer));
}

/// `"-0"` is zero: the warning and a positive zero.
#[test]
fn minus_zero_text_is_a_warned_positive_zero() {
    for text in [&b"-0"[..], b"-0.0", b"+0", b"-0e5"] {
        let (value, warn) = number(text);
        assert_eq!(value.to_bits(), 0, "{text:?}");
        assert!(warn, "{text:?}");
    }
}
