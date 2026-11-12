use super::*;

#[test]
fn hex_text_is_exact_lower_case() {
    assert_eq!(hex(&[]), "");
    assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    assert_eq!(from_hex::<4>("000fa5ff"), Some([0x00, 0x0f, 0xa5, 0xff]));
    assert_eq!(from_hex::<0>(""), Some([]));
    for bad in [
        "",
        "000fa5f",
        "000fa5ff00",
        "000FA5FF",
        "000fa5fg",
        "000fa5 f",
        "000fa5+f",
    ] {
        assert_eq!(from_hex::<4>(bad), None, "{bad:?}");
    }
    // Two bytes of text, one multi-byte character: the length in bytes decides, and no digit matches.
    assert_eq!(from_hex::<1>("\u{e9}"), None);
}

#[test]
fn every_byte_round_trips() {
    let all: Vec<u8> = (0..=255).collect();
    let text = hex(&all);
    assert_eq!(text.len(), 512);
    let back = from_hex::<256>(&text).expect("its own text");
    assert_eq!(back.as_slice(), all.as_slice());
}
