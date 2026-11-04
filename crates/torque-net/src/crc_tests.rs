use super::*;

#[test]
fn the_standard_check_values() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
    let mut c = Crc32::new();
    c.update(b"1234");
    c.update(b"56789");
    assert_eq!(c.finish(), 0xCBF4_3926);
    assert_eq!(c.raw(), !0xCBF4_3926);
    assert_eq!(Crc32::new().raw(), 0xFFFF_FFFF);
}
