//! `readPNG`'s rules on files made here (the `png` crate's encoder writes the image; chunks are spliced in by hand where a test needs
//! exact `tRNS` bytes or a broken chunk). The expected bits follow libpng 1.2.8's transforms; the cases marked as such were also
//! checked against libpng itself (built locally and driven as `readPNG` drives it). The crate's encoder cannot
//! write interlaced files; Adam7 files were checked against that local libpng only.

use super::*;

fn encode(
    w: u32,
    h: u32,
    color: ::png::ColorType,
    depth: ::png::BitDepth,
    raw: &[u8],
    plte: Option<&[u8]>,
) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = ::png::Encoder::new(&mut out, w, h);
        e.set_color(color);
        e.set_depth(depth);
        if let Some(p) = plte {
            e.set_palette(p.to_vec());
        }
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(raw).unwrap();
    }
    out
}

fn chunk(name: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(name);
    c.extend_from_slice(data);
    let crc = crc32(&c[4..]);
    c.extend_from_slice(&crc.to_be_bytes());
    c
}

/// `file` with `extra` inserted before the chunk named `before` (the first one).
fn splice(file: &[u8], before: &[u8; 4], extra: &[u8]) -> Vec<u8> {
    let mut at = 8;
    while at + 8 <= file.len() {
        let len = be32(file, at).unwrap() as usize;
        if &file[at + 4..at + 8] == before {
            let mut out = file[..at].to_vec();
            out.extend_from_slice(extra);
            out.extend_from_slice(&file[at..]);
            return out;
        }
        at += 12 + len;
    }
    panic!("no {before:?} chunk");
}

use ::png::{BitDepth as D, ColorType as C};

#[test]
fn grey_is_alpha_with_low_depths_scaled_and_sixteen_bits_stripped() {
    // 1 bit: 0 and 1 -> 0, 255; 2 bits x 0x55; 4 bits x 0x11.
    let one = decode(&encode(8, 1, C::Grayscale, D::One, &[0b1010_0001], None)).unwrap();
    assert_eq!(one.format, Format::Alpha);
    assert_eq!(one.bits, [255, 0, 255, 0, 0, 0, 0, 255]);
    let two = decode(&encode(4, 1, C::Grayscale, D::Two, &[0b00_01_10_11], None)).unwrap();
    assert_eq!(two.bits, [0, 0x55, 0xaa, 0xff]);
    let four = decode(&encode(2, 1, C::Grayscale, D::Four, &[0x3c], None)).unwrap();
    assert_eq!(four.bits, [0x33, 0xcc]);
    let sixteen = decode(&encode(
        2,
        1,
        C::Grayscale,
        D::Sixteen,
        &[0x12, 0xff, 0xab, 0x00],
        None,
    ))
    .unwrap();
    assert_eq!(sixteen.bits, [0x12, 0xab]);
}

#[test]
fn grey_with_trns_keeps_the_first_bytes_of_the_grey_alpha_stream() {
    // 4 x 2 grey 8-bit, key 7: libpng writes (g, a) pairs; the bitmap holds the first 8 bytes of the 16.
    let raw = [7, 1, 2, 7, 3, 4, 5, 6];
    let f = splice(
        &encode(4, 2, C::Grayscale, D::Eight, &raw, None),
        b"IDAT",
        &chunk(b"tRNS", &[0, 7]),
    );
    let b = decode(&f).unwrap();
    assert_eq!(b.format, Format::Alpha);
    assert_eq!(b.bits, [7, 0, 1, 255, 2, 255, 7, 0]);
    // An 8-bit image compares the byte with the whole 16-bit key: 0x0107 never matches.
    let f = splice(
        &encode(4, 2, C::Grayscale, D::Eight, &raw, None),
        b"IDAT",
        &chunk(b"tRNS", &[1, 7]),
    );
    assert_eq!(decode(&f).unwrap().bits, [7, 255, 1, 255, 2, 255, 7, 255]);
    // Below 8 bits libpng 1.2.8 scales the key unmasked: 3 at 4 bits matches 0x33, 0x13 (0x143 scaled) matches nothing.
    let f = splice(
        &encode(2, 1, C::Grayscale, D::Four, &[0x31], None),
        b"IDAT",
        &chunk(b"tRNS", &[0, 3]),
    );
    assert_eq!(decode(&f).unwrap().bits, [0x33, 0]);
    let f = splice(
        &encode(2, 1, C::Grayscale, D::Four, &[0x31], None),
        b"IDAT",
        &chunk(b"tRNS", &[0, 0x13]),
    );
    assert_eq!(decode(&f).unwrap().bits, [0x33, 255]);
}

#[test]
fn rgb_with_trns_is_rgba_compared_at_the_file_depth() {
    let raw = [1, 2, 3, 4, 5, 6];
    let f = splice(
        &encode(2, 1, C::Rgb, D::Eight, &raw, None),
        b"IDAT",
        &chunk(b"tRNS", &[0, 4, 0, 5, 0, 6]),
    );
    let b = decode(&f).unwrap();
    assert_eq!(b.format, Format::Rgba);
    assert_eq!(b.bits, [1, 2, 3, 255, 4, 5, 6, 0]);
    // 16 bits: equal high bytes are not enough.
    let raw16 = [1, 0, 2, 0, 3, 0, 1, 1, 2, 0, 3, 0];
    let f = splice(
        &encode(2, 1, C::Rgb, D::Sixteen, &raw16, None),
        b"IDAT",
        &chunk(b"tRNS", &[1, 0, 2, 0, 3, 0]),
    );
    assert_eq!(decode(&f).unwrap().bits, [1, 2, 3, 0, 1, 2, 3, 255]);
    // Without a tRNS: plain RGB.
    assert_eq!(
        decode(&encode(2, 1, C::Rgb, D::Eight, &raw, None))
            .unwrap()
            .format,
        Format::Rgb
    );
}

#[test]
fn palette_expands_with_its_alphas_and_a_zeroed_tail() {
    let plte = [10, 11, 12, 20, 21, 22, 30, 31, 32];
    // 2-bit indices 0, 1, 2, 3 (3 is past the palette: black).
    let raw = [0b00_01_10_11];
    let plain = decode(&encode(4, 1, C::Indexed, D::Two, &raw, Some(&plte))).unwrap();
    assert_eq!(plain.format, Format::Rgb);
    assert_eq!(plain.bits, [10, 11, 12, 20, 21, 22, 30, 31, 32, 0, 0, 0]);
    let f = splice(
        &encode(4, 1, C::Indexed, D::Two, &raw, Some(&plte)),
        b"IDAT",
        &chunk(b"tRNS", &[0, 128]),
    );
    let b = decode(&f).unwrap();
    assert_eq!(b.format, Format::Rgba);
    assert_eq!(
        b.bits,
        [
            10, 11, 12, 0, 20, 21, 22, 128, 30, 31, 32, 255, 0, 0, 0, 255
        ]
    );
    // A tRNS longer than the palette is dropped (libpng: "Incorrect tRNS chunk length").
    let f = splice(
        &encode(4, 1, C::Indexed, D::Two, &raw, Some(&plte)),
        b"IDAT",
        &chunk(b"tRNS", &[0, 1, 2, 3]),
    );
    assert_eq!(decode(&f).unwrap(), plain);
}

#[test]
fn grey_alpha_and_rgba_are_rgba() {
    let ga = decode(&encode(
        2,
        1,
        C::GrayscaleAlpha,
        D::Sixteen,
        &[9, 1, 8, 2, 7, 3, 6, 4],
        None,
    ))
    .unwrap();
    assert_eq!(ga.format, Format::Rgba);
    assert_eq!(ga.bits, [9, 9, 9, 8, 7, 7, 7, 6]);
    let rgba = decode(&encode(1, 1, C::Rgba, D::Eight, &[1, 2, 3, 4], None)).unwrap();
    assert_eq!(rgba.bits, [1, 2, 3, 4]);
}

#[test]
fn bad_ancillary_chunks_are_dropped_and_bad_critical_ones_refused() {
    let raw = [1, 2, 3, 4, 5, 6];
    let good = encode(2, 1, C::Rgb, D::Eight, &raw, None);
    let want = decode(&good).unwrap();
    // A text chunk with a broken CRC, a tRNS in an RGBA file's place (wrong length here), a tRNS after IDAT, a duplicate.
    let mut bad_text = chunk(b"tEXt", b"k\0v");
    let n = bad_text.len();
    bad_text[n - 1] ^= 1;
    assert_eq!(decode(&splice(&good, b"IDAT", &bad_text)).unwrap(), want);
    assert_eq!(
        decode(&splice(&good, b"IDAT", &chunk(b"tRNS", &[0, 4]))).unwrap(),
        want
    );
    assert_eq!(
        decode(&splice(
            &good,
            b"IEND",
            &chunk(b"tRNS", &[0, 4, 0, 5, 0, 6])
        ))
        .unwrap(),
        want
    );
    let first = chunk(b"tRNS", &[0, 1, 0, 2, 0, 3]);
    let second = chunk(b"tRNS", &[0, 4, 0, 5, 0, 6]);
    let dup = splice(&splice(&good, b"IDAT", &first), b"IDAT", &second);
    assert_eq!(decode(&dup).unwrap().bits, [1, 2, 3, 0, 4, 5, 6, 255]);
    let rgba = encode(1, 1, C::Rgba, D::Eight, &[1, 2, 3, 4], None);
    assert_eq!(
        decode(&splice(&rgba, b"IDAT", &chunk(b"tRNS", &[0, 1])))
            .unwrap()
            .bits,
        [1, 2, 3, 4]
    );
    // A critical chunk with a bad CRC, a cut file, a bad signature.
    let idat = good.windows(4).position(|w| w == b"IDAT").unwrap();
    let mut broken = good.clone();
    broken[idat + 6] ^= 0x40;
    assert!(decode(&broken).is_err());
    assert!(decode(&good[..good.len() - 5]).is_err());
    assert!(decode(b"\x89PNG\r\n\x1a\x0b").is_err());
    assert!(decode(&[]).is_err());
}

#[test]
fn a_trns_whose_crc_fails_still_widens_libpngs_rows() {
    let raw = [1, 2, 3, 4, 5, 6];
    let mut t = chunk(b"tRNS", &[0, 4, 0, 5, 0, 6]);
    let n = t.len();
    t[n - 1] ^= 1;
    // RGB: libpng expands to RGBA, readPNG keeps RGB: the first 6 bytes of (1,2,3,255, 4,5,6,0).
    let b = decode(&splice(
        &encode(2, 1, C::Rgb, D::Eight, &raw, None),
        b"IDAT",
        &t,
    ))
    .unwrap();
    assert_eq!((b.format, b.bits), (Format::Rgb, vec![1, 2, 3, 255, 4, 5]));
    // Palette: RGB rows at the RGBA stride over the 0xFF fill (2 x 2, row 1 starts at byte 8 of 12).
    let plte = [10, 11, 12, 20, 21, 22];
    let mut t = chunk(b"tRNS", &[0]);
    let n = t.len();
    t[n - 1] ^= 1;
    let f = splice(
        &encode(2, 2, C::Indexed, D::Eight, &[0, 1, 1, 0], Some(&plte)),
        b"IDAT",
        &t,
    );
    let b = decode(&f).unwrap();
    assert_eq!(b.format, Format::Rgb);
    assert_eq!(b.bits, [10, 11, 12, 20, 21, 22, 255, 255, 20, 21, 22, 10]);
}

#[test]
fn the_height_cap_is_tges() {
    let raw = vec![0u8; 2049];
    assert!(matches!(
        decode(&encode(1, 2049, C::Grayscale, D::Eight, &raw, None)),
        Err(ImageError::Cap {
            what: "png height",
            ..
        })
    ));
    assert!(decode(&encode(1, 2048, C::Grayscale, D::Eight, &raw[..2048], None)).is_ok());
}

#[test]
fn the_header_size_is_the_decoded_size_and_refuses_what_decode_refuses() {
    let file = encode(3, 2, C::Rgb, D::Eight, &[0; 18], None);
    let b = decode(&file).unwrap();
    assert_eq!(header_size(&file), Some((b.width, b.height)));
    assert_eq!(crate::size_of(crate::Kind::Png, &file), Some((3, 2)));
    // A height past readPNG's row table, a zero width, a first chunk that is not IHDR, a short file.
    let mut tall = file.clone();
    tall[20..24].copy_from_slice(&(MAX_PNG_HEIGHT + 1).to_be_bytes());
    assert_eq!(header_size(&tall), None);
    let mut zero = file.clone();
    zero[16..20].copy_from_slice(&0u32.to_be_bytes());
    assert_eq!(header_size(&zero), None);
    let mut other = file.clone();
    other[12..16].copy_from_slice(b"IDAT");
    assert_eq!(header_size(&other), None);
    assert_eq!(header_size(&file[..20]), None);
}
