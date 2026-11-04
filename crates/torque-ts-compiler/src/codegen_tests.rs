use super::*;

#[test]
fn strings_used_as_numbers_go_through_a_float() {
    assert_eq!(string_to_number(b"0.1").0, f64::from(0.1f32));
    assert_eq!(string_to_number(b"TRUE"), (1.0, false));
    assert_eq!(string_to_number(b"False"), (0.0, false));
    assert_eq!(string_to_number(b"abc"), (0.0, true));
    assert_eq!(string_to_number(b"0"), (0.0, true));
}

/// `"-0"` is zero: the warning and a positive zero (`"-0" + 1`).
#[test]
fn minus_zero_text_is_a_warned_positive_zero() {
    for text in [&b"-0"[..], b"-0.0", b"+0", b"-0e5"] {
        let (value, warn) = string_to_number(text);
        assert_eq!(value.to_bits(), 0, "{text:?}");
        assert!(warn, "{text:?}");
    }
}

/// A NaN made from text at compile time is a quiet NaN with the text's sign, widened from a float.
#[test]
fn nan_texts_are_quiet_nans_with_their_sign() {
    let bits = |t: &[u8]| string_to_number(t).0.to_bits();
    for t in [
        "nan", "NaN", "NAN", "+nan", "NAN(abc)", "nan(1)", "nan()", "nan(", " nan",
    ] {
        assert_eq!(bits(t.as_bytes()), 0x7FF8_0000_0000_0000, "{t}");
    }
    for t in ["-nan", "-NaN", "-nan(abc)"] {
        assert_eq!(bits(t.as_bytes()), 0xFFF8_0000_0000_0000, "{t}");
    }
}
