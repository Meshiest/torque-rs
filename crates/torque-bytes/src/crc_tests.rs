use super::*;

/// The definition, a bit at a time.
fn bitwise(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn the_standard_check_values() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"IEND"), 0xae42_6082);
    assert_eq!(crc32_parts(&[b"IE", b"", b"ND"]), 0xae42_6082);
    let mut c = Crc32::new();
    c.update(b"1234");
    c.update(b"56789");
    assert_eq!(c.finish(), 0xCBF4_3926);
    assert_eq!(c.raw(), !0xCBF4_3926);
    assert_eq!(Crc32::new().raw(), 0xFFFF_FFFF);
    assert_eq!(Crc32::default(), Crc32::new());
}

#[test]
fn the_wide_step_is_the_definition() {
    let bytes: Vec<u8> = (0u32..300)
        .map(|i| (i.wrapping_mul(0x9e37_79b1) >> 13) as u8)
        .collect();
    for len in 0..bytes.len() {
        let all = &bytes[..len];
        assert_eq!(crc32(all), bitwise(all), "{len} bytes");
        let (a, b) = all.split_at(len / 3);
        assert_eq!(
            crc32_parts(&[a, b]),
            bitwise(all),
            "{len} bytes in two parts"
        );
    }
}
