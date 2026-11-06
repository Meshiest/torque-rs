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

#[test]
fn an_encoded_png_reads_back_and_decodes_to_its_pixels() {
    for (w, h, runs) in [
        (1, 1, false),
        (2, 2, true),
        (5, 3, false),
        (300, 260, false),
        (256, 256, true),
        (97, 13, true),
    ] {
        let rgb: Vec<u8> = (0..w * h * 3)
            .map(|i| {
                if runs {
                    (i / 700 % 3) as u8 * 40
                } else {
                    (i * 7 % 251) as u8
                }
            })
            .collect();
        let bytes = encode_rgb(w, h, &rgb);
        let png = read(&mut Reader::new(&bytes), "png").unwrap();
        assert_eq!(
            (png.width, png.height, png.bytes.len()),
            (w, h, bytes.len())
        );
        let b = torque_images::png::decode(&bytes).unwrap();
        assert_eq!(
            (b.width, b.height, b.format),
            (w, h, torque_images::Format::Rgb)
        );
        assert_eq!(b.bits, rgb, "{w}x{h}");
    }
    // A flat page is small.
    assert!(encode_rgb(256, 256, &[]).len() < 1_500);
}

#[test]
fn the_crc_eight_bytes_a_step_is_the_crc_one_byte_a_step() {
    let bytewise = |parts: &[&[u8]]| {
        let mut c = 0xffff_ffffu32;
        for part in parts {
            for &b in *part {
                c = CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
            }
        }
        !c
    };
    let bytes: Vec<u8> = (0u32..300)
        .map(|i| (i.wrapping_mul(0x9e37_79b1) >> 13) as u8)
        .collect();
    for len in 0..bytes.len() {
        let all = &bytes[..len];
        assert_eq!(crc32(&[all]), bytewise(&[all]), "{len} bytes");
        let (a, b) = all.split_at(len / 3);
        assert_eq!(crc32(&[a, b]), bytewise(&[all]), "{len} bytes in two parts");
    }
}
