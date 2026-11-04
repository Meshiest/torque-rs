use super::*;

fn tiny() -> Vec<u8> {
    let mut out = Vec::from(SIGNATURE);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&2u32.to_be_bytes());
    ihdr.extend_from_slice(&3u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &[1, 2, 3]);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[test]
fn a_png_ends_after_its_iend_and_the_next_field_follows() {
    let mut bytes = tiny();
    let len = bytes.len();
    bytes.extend_from_slice(&[7, 0, 0, 0]);
    let mut r = Reader::new(&bytes);
    let png = read(&mut r, "png").unwrap();
    assert_eq!((png.width, png.height, png.bytes.len()), (2, 3, len));
    assert_eq!(r.u32("next").unwrap(), 7);
}

#[test]
fn a_bad_crc_or_a_cut_png_is_refused() {
    let mut bytes = tiny();
    bytes[30] ^= 1;
    assert!(read(&mut Reader::new(&bytes), "png").is_err());
    let bytes = tiny();
    assert!(read(&mut Reader::new(&bytes[..bytes.len() - 1]), "png").is_err());
    // The CRC of "IEND" with no data, as every PNG ends.
    assert_eq!(crc32(&[b"IEND"]), 0xae42_6082);
}
