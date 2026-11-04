//! The JPEG decoder's tests, over files a small encoder here writes from chosen quantised coefficients (no forward DCT: the test
//! controls every coefficient). The encoder writes baseline (single- or multi-scan, with or without restart intervals) and
//! progressive files (any scan script: spectral selection, successive approximation, EOB runs) of the same coefficients, so the
//! sequential and progressive decoders must give the same image. The digests of [`GOLDEN`] were taken from libjpeg 6b itself (built
//! locally and driven as `readJPEG` drives it) on the files [`cases`] writes.

use super::*;
use crate::testenc::*;

// The cases and their libjpeg digests

/// Named files of the encoder: every sampling layout libjpeg's upsampler distinguishes, multi-scan and restarts, progressive
/// (complete and coarse, the latter smoothed), a truncated progressive file, 16-bit quantisation tables, an Adobe RGB file.
fn cases() -> Vec<(&'static str, Vec<u8>)> {
    let mut v = Vec::new();
    let one = |w, h, samp: &[(u8, u8)], seed| Enc::random(w, h, samp, seed);
    let ycc420 = one(37, 29, &[(2, 2), (1, 1), (1, 1)], 11);
    v.push(("ycc420", ycc420.baseline(&[vec![0, 1, 2]])));
    v.push((
        "ycc422",
        one(33, 17, &[(2, 1), (1, 1), (1, 1)], 12).baseline(&[vec![0, 1, 2]]),
    ));
    v.push((
        "ycc444",
        one(19, 23, &[(1, 1), (1, 1), (1, 1)], 13).baseline(&[vec![0, 1, 2]]),
    ));
    v.push((
        "ycc440",
        one(21, 26, &[(1, 2), (1, 1), (1, 1)], 14).baseline(&[vec![0, 1, 2]]),
    ));
    v.push((
        "ycc411",
        one(45, 9, &[(4, 1), (1, 1), (1, 1)], 15).baseline(&[vec![0, 1, 2]]),
    ));
    v.push((
        "ycc31",
        one(30, 12, &[(3, 1), (1, 1), (1, 1)], 16).baseline(&[vec![0, 1, 2]]),
    ));
    v.push((
        "ycc22_21",
        one(27, 31, &[(2, 2), (2, 1), (1, 1)], 17).baseline(&[vec![0, 1, 2]]),
    ));
    v.push((
        "narrow420",
        one(3, 18, &[(2, 2), (1, 1), (1, 1)], 18).baseline(&[vec![0, 1, 2]]),
    ));
    v.push(("gray", one(26, 15, &[(1, 1)], 19).baseline(&[vec![0]])));
    v.push(("gray22", one(17, 13, &[(2, 2)], 20).baseline(&[vec![0]])));
    v.push(("multiscan", ycc420.baseline(&[vec![0], vec![1, 2]])));
    let mut rst = ycc420.clone();
    rst.restart = 2;
    v.push(("restart", rst.baseline(&[vec![0, 1, 2]])));
    v.push(("prog_full", ycc420.progressive(&full_script(3))));
    v.push(("prog_coarse", ycc420.progressive(&coarse_script(3))));
    v.push(("prog_restart", rst.progressive(&full_script(3))));
    // Cut in the middle of the third scan's data (luma AC 6-63): every later band stays incomplete and the decoder smooths.
    let full = ycc420.progressive(&full_script(3));
    let cut = scan_data(&full)
        .get(2)
        .map_or(full.len() / 2, |&(a, b)| (a + b) / 2);
    v.push(("prog_cut", full[..cut].to_vec()));
    let mut q16 = one(16, 16, &[(1, 1), (1, 1), (1, 1)], 21);
    q16.quant[0][0] = 300;
    q16.quant[1][5] = 1000;
    v.push(("quant16", q16.baseline(&[vec![0, 1, 2]])));
    let mut rgb = one(12, 10, &[(1, 1), (1, 1), (1, 1)], 22);
    rgb.jfif = false;
    for (i, c) in rgb.comps.iter_mut().enumerate() {
        c.id = [b'R', b'G', b'B'][i];
    }
    v.push(("rgb_ids", rgb.baseline(&[vec![0, 1, 2]])));
    let base = ycc420.baseline(&[vec![0, 1, 2]]);
    v.push(("cut_baseline", base[..base.len() - 40].to_vec()));
    v
}

/// Where each scan's entropy-coded data starts (after its SOS header) and ends, by walking the marker segments.
fn scan_data(f: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 2;
    while i + 4 <= f.len() {
        if f[i] != 0xff {
            i += 1;
            continue;
        }
        let m = f[i + 1];
        if m == 0xd9 {
            break;
        }
        let len = usize::from(u16::from_be_bytes([f[i + 2], f[i + 3]]));
        i += 2 + len;
        if m == 0xda {
            let start = i;
            // Skip the data to the next marker that is not a stuffed byte or a restart.
            while i + 1 < f.len()
                && !(f[i] == 0xff && f[i + 1] != 0 && !(0xd0..=0xd7).contains(&f[i + 1]))
            {
                i += 1;
            }
            out.push((start, i));
        }
    }
    out
}

/// libjpeg 6b's decodes of [`cases`]: `(name, width, height, components, FNV-1a 64 of the bits)`.
const GOLDEN: &[(&str, u32, u32, usize, u64)] = &[
    ("cut_baseline", 37, 29, 3, 0x909a3ae581c72cfd),
    ("gray", 26, 15, 1, 0xeb22996266d3c699),
    ("gray22", 17, 13, 1, 0x80042ad6ffdef297),
    ("multiscan", 37, 29, 3, 0x006e7bdcdbb9ee55),
    ("narrow420", 3, 18, 3, 0x5f871974dbc90d7c),
    ("prog_coarse", 37, 29, 3, 0x389c51aa3701d287),
    ("prog_cut", 37, 29, 3, 0x24fcc87e7912aa2c),
    ("prog_full", 37, 29, 3, 0x006e7bdcdbb9ee55),
    ("prog_restart", 37, 29, 3, 0x006e7bdcdbb9ee55),
    ("quant16", 16, 16, 3, 0x6b3eda1109d8574b),
    ("restart", 37, 29, 3, 0x006e7bdcdbb9ee55),
    ("rgb_ids", 12, 10, 3, 0x636bb3c7b254aa18),
    ("ycc22_21", 27, 31, 3, 0x39c579e23189c9f9),
    ("ycc31", 30, 12, 3, 0x82c0d51261ffe40b),
    ("ycc411", 45, 9, 3, 0x45918062b2f8b231),
    ("ycc420", 37, 29, 3, 0x006e7bdcdbb9ee55),
    ("ycc422", 33, 17, 3, 0xcb5e52c3b974f82e),
    ("ycc440", 21, 26, 3, 0xf57f48a19db7748a),
    ("ycc444", 19, 23, 3, 0xff4646a6b50ac5b4),
];

#[test]
fn cases_match_libjpeg_digests() {
    let cases = cases();
    assert_eq!(cases.len(), GOLDEN.len());
    let mut bad = Vec::new();
    for (name, w, h, comps, digest) in GOLDEN {
        let Some((_, bytes)) = cases.iter().find(|(n, _)| n == name) else {
            bad.push(format!("{name}: no such case"));
            continue;
        };
        match dec(bytes) {
            Ok((b, info)) => {
                // The two files whose bands stay incomplete are the smoothed ones.
                assert_eq!(info.smoothed, name.starts_with("prog_c"), "{name}");
                let got = (b.width, b.height, b.format.bytes_per_pixel(), fnv(&b.bits));
                if got != (*w, *h, *comps, *digest) {
                    bad.push(format!(
                        "{name}: libjpeg {w}x{h}x{comps} {digest:#x}, got {got:?}"
                    ));
                }
            }
            Err(e) => bad.push(format!("{name}: {e}")),
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Writes [`cases`] to `IMAGES_WRITE_CASES` (for taking the digests with a local libjpeg build).
#[test]
#[ignore]
fn write_cases() {
    let Some(dir) = std::env::var_os("IMAGES_WRITE_CASES") else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    let mut list = String::new();
    for (name, bytes) in cases() {
        let p = dir.join(format!("{name}.jpg"));
        std::fs::write(&p, bytes).unwrap();
        list.push_str(&format!("{} {}.out\n", p.display(), p.display()));
    }
    std::fs::write(dir.join("cases.list"), list).unwrap();
}

/// Prints this decoder's digests in [`GOLDEN`]'s form (to compare with libjpeg's by hand).
#[test]
#[ignore]
fn print_digests() {
    for (name, bytes) in cases() {
        match dec(&bytes) {
            Ok((b, _)) => println!(
                "    (\"{name}\", {}, {}, {}, {:#018x}),",
                b.width,
                b.height,
                b.format.bytes_per_pixel(),
                fnv(&b.bits)
            ),
            Err(e) => println!("    // {name}: {e}"),
        }
    }
}

// Self-consistency

#[test]
fn progressive_decodes_the_same_coefficients_as_baseline() {
    for (samp, w, h, seed) in [
        (vec![(2u8, 2u8), (1, 1), (1, 1)], 37usize, 29usize, 1u64),
        (vec![(1, 1)], 20, 9, 2),
        (vec![(2, 1), (1, 1), (1, 1)], 50, 24, 3),
        (vec![(1, 1), (1, 1), (1, 1)], 8, 8, 4),
    ] {
        let e = Enc::random(w, h, &samp, seed);
        let n = samp.len();
        let base = dec(&e.baseline(&[(0..n).collect()])).unwrap().0;
        let prog = dec(&e.progressive(&full_script(n))).unwrap();
        assert!(prog.1.progressive && !prog.1.smoothed);
        assert_eq!(base, prog.0, "{samp:?}");
        // Non-interleaved sequential scans, and restart intervals, change nothing either.
        let groups: Vec<Vec<usize>> = (0..n).map(|c| vec![c]).collect();
        assert_eq!(base, dec(&e.baseline(&groups)).unwrap().0);
        let mut r = e.clone();
        for interval in [1u16, 3, 7] {
            r.restart = interval;
            let (b, info) = dec(&r.baseline(&[(0..n).collect()])).unwrap();
            assert!(info.restarts);
            assert_eq!(base, b, "restart {interval}");
            assert_eq!(
                base,
                dec(&r.progressive(&full_script(n))).unwrap().0,
                "progressive restart {interval}"
            );
        }
    }
}

#[test]
fn a_coarse_progressive_file_is_smoothed() {
    let e = Enc::random(40, 32, &[(2, 2), (1, 1), (1, 1)], 9);
    let (smooth, info) = dec(&e.progressive(&coarse_script(3))).unwrap();
    assert!(info.smoothed);
    // Without smoothing it would be exactly the baseline decode of the truncated coefficients; smoothing changes it.
    let plain = dec(&truncated_to(&e, 1).baseline(&[vec![0, 1, 2]]))
        .unwrap()
        .0;
    assert_eq!((smooth.width, smooth.height), (plain.width, plain.height));
    assert_ne!(smooth.bits, plain.bits);
}

#[test]
fn a_dc_only_block_is_flat() {
    // One 8x8 grey block with DC 10 and multiplier 8: every sample is 128 + 10 (the IDCT's DC gain is 1/8).
    let mut e = Enc::random(8, 8, &[(1, 1)], 5);
    e.comps[0].blocks[0] = [0; 64];
    e.comps[0].blocks[0][0] = 10;
    e.quant[0] = [8; 64];
    let (b, info) = dec(&e.baseline(&[vec![0]])).unwrap();
    assert_eq!(b.format, Format::Alpha);
    assert_eq!(info.color_space, "gray");
    assert!(b.bits.iter().all(|&v| v == 138), "{:?}", &b.bits[..8]);
}

#[test]
fn a_truncated_file_ends_in_grey() {
    // The fake EOI ends the data: the rest of the image is the IDCT of zero coefficients (128), chroma neutral: grey.
    let e = Enc::random(64, 64, &[(1, 1), (1, 1), (1, 1)], 6);
    let full = e.baseline(&[vec![0, 1, 2]]);
    let (b, info) = dec(&full[..full.len() / 3]).unwrap();
    assert!(info.warnings > 0);
    let last = &b.bits[b.bits.len() - 3..];
    assert_eq!(last, &[128, 128, 128]);
}

#[test]
fn the_idct_of_zero_is_mid_grey_and_the_range_limit_wraps() {
    let mut out = [[0u8; 8]; 8];
    idct::islow(&[0; 64], &[1; 64], &mut out);
    assert!(out.iter().flatten().all(|&v| v == 128));
    assert_eq!(idct::idct_limit(0), 128);
    assert_eq!(idct::idct_limit(127), 255);
    assert_eq!(idct::idct_limit(300), 255);
    assert_eq!(idct::idct_limit(-1), 127);
    assert_eq!(idct::idct_limit(-128), 0);
    assert_eq!(idct::idct_limit(-300), 0);
    // A hostile coefficient overflows as libjpeg's 32-bit arithmetic does, without a panic.
    let mut c = [0i16; 64];
    c[0] = i16::MAX;
    c[1] = i16::MIN;
    c[63] = i16::MAX;
    idct::islow(&c, &[u16::MAX; 64], &mut out);
}

// Markers and refusals

fn small() -> Vec<u8> {
    Enc::random(16, 8, &[(1, 1), (1, 1), (1, 1)], 7).baseline(&[vec![0, 1, 2]])
}

/// The bytes of `f` with the marker segment `code` (the first one) replaced by `with`.
fn replace_segment(f: &[u8], code: u8, with: &[u8]) -> Vec<u8> {
    let mut i = 2;
    while i + 4 <= f.len() {
        let len = usize::from(u16::from_be_bytes([f[i + 2], f[i + 3]]));
        if f[i + 1] == code {
            let mut out = f[..i].to_vec();
            out.extend_from_slice(with);
            out.extend_from_slice(&f[i + 2 + len..]);
            return out;
        }
        i += 2 + len;
    }
    f.to_vec()
}

#[test]
fn refusals_name_the_libjpeg_error() {
    assert_eq!(dec(&[]).unwrap_err(), JpegError::Fatal("JERR_INPUT_EMPTY"));
    assert_eq!(dec(b"GIF89a").unwrap_err(), JpegError::Fatal("JERR_NO_SOI"));
    assert_eq!(
        dec(&[0xff, 0xd8, 0xff, 0xd9]).unwrap_err(),
        JpegError::Fatal("JERR_NO_IMAGE")
    );
    let f = small();
    // An unsupported SOF (lossless) and an arithmetic one.
    let mut lossless = f.clone();
    let sof = lossless.windows(2).position(|w| w == [0xff, 0xc0]).unwrap();
    lossless[sof + 1] = 0xc3;
    assert_eq!(
        dec(&lossless).unwrap_err(),
        JpegError::Fatal("JERR_SOF_UNSUPPORTED")
    );
    lossless[sof + 1] = 0xc9;
    assert_eq!(
        dec(&lossless).unwrap_err(),
        JpegError::Fatal("JERR_ARITH_NOTIMPL")
    );
    // 12-bit precision.
    let mut p12 = f.clone();
    p12[sof + 4] = 12;
    assert_eq!(
        dec(&p12).unwrap_err(),
        JpegError::Fatal("JERR_BAD_PRECISION")
    );
    // No Huffman tables at all.
    let no_dht = replace_segment(&replace_segment(&f, 0xc4, &[]), 0xc4, &[]);
    assert_eq!(
        dec(&no_dht).unwrap_err(),
        JpegError::Fatal("JERR_NO_HUFF_TABLE")
    );
    // A reserved marker before the frame.
    let mut reserved = f.clone();
    reserved.splice(2..2, [0xff, 0x05, 0, 2]);
    assert_eq!(
        dec(&reserved).unwrap_err(),
        JpegError::Fatal("JERR_UNKNOWN_MARKER")
    );
    // A second SOS in a single-scan file is read by jpeg_finish_decompress.
    let mut twice = f.clone();
    twice.truncate(twice.len() - 2);
    let sos = f.windows(2).rposition(|w| w == [0xff, 0xda]).unwrap();
    twice.extend_from_slice(&f[sos..]);
    assert_eq!(
        dec(&twice).unwrap_err(),
        JpegError::Fatal("JERR_EOI_EXPECTED")
    );
    // Four components: CMYK, which readJPEG refuses.
    let cmyk =
        Enc::random(8, 8, &[(1, 1), (1, 1), (1, 1), (1, 1)], 8).baseline(&[vec![0, 1, 2, 3]]);
    assert_eq!(dec(&cmyk).unwrap_err(), JpegError::ColorSpace("CMYK"));
    // A fractional sampling ratio (3 against 2).
    let frac = Enc::random(24, 8, &[(3, 1), (2, 1), (1, 1)], 9).baseline(&[vec![0, 1, 2]]);
    assert_eq!(
        dec(&frac).unwrap_err(),
        JpegError::Fatal("JERR_FRACT_SAMPLE_NOTIMPL")
    );
}

#[test]
fn caps_are_enforced_before_allocating() {
    let mut f = small();
    let sof = f.windows(2).position(|w| w == [0xff, 0xc0]).unwrap();
    f[sof + 5..sof + 9].copy_from_slice(&[0x7f, 0xff, 0x7f, 0xff]);
    assert!(matches!(
        dec(&f).unwrap_err(),
        JpegError::Cap {
            what: "jpeg width",
            ..
        }
    ));
    let tight = Limits {
        max_pixels: 100,
        ..Limits::default()
    };
    assert!(matches!(
        decode_with(&small(), &tight).unwrap_err(),
        JpegError::Cap {
            what: "jpeg pixels",
            ..
        }
    ));
}

#[test]
fn garbage_and_padding_around_markers_are_tolerated() {
    let f = small();
    let good = dec(&f).unwrap();
    // Fill bytes (FF FF) before a marker, a COM and an APP1, and garbage after EOI.
    let mut g = f.clone();
    g.splice(
        2..2,
        [
            0xff, 0xff, 0xff, 0xfe, 0, 5, b'h', b'i', b'!', 0xff, 0xe1, 0, 3, 0,
        ],
    );
    g.extend_from_slice(b"trailing junk");
    assert_eq!(good.0, dec(&g).unwrap().0);
    // An Adobe APP14 with transform 0 makes the components RGB: the colours change.
    let mut adobe = f.clone();
    adobe.splice(
        2..2,
        [
            0xff, 0xee, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 0,
        ],
    );
    // (The JFIF APP0 comes first, and JFIF wins: still YCbCr.)
    assert_eq!(good.0, dec(&adobe).unwrap().0);
    let no_jfif = replace_segment(&adobe, 0xe0, &[]);
    let (b, info) = dec(&no_jfif).unwrap();
    assert_eq!(info.color_space, "rgb");
    assert_ne!(b, good.0);
}

#[test]
fn hostile_bytes_never_panic() {
    let mut rng = Rng(0x5eed);
    let bases = [
        small(),
        Enc::random(9, 9, &[(2, 2), (1, 1), (1, 1)], 3).progressive(&full_script(3)),
    ];
    for i in 0..3000 {
        let mut f = bases[i % 2].clone();
        for _ in 0..1 + rng.range(6) {
            let at = rng.range(f.len() as u64) as usize;
            f[at] = rng.next() as u8;
        }
        if rng.range(4) == 0 {
            f.truncate(rng.range(f.len() as u64) as usize);
        }
        let _ = decode_with(
            &f,
            &Limits {
                max_pixels: 1 << 14,
                ..Limits::default()
            },
        );
    }
}

#[test]
fn the_header_size_is_the_decoded_size() {
    for (name, file) in cases() {
        let (w, h) = match decode(&file) {
            Ok(b) => (b.width, b.height),
            Err(_) => continue,
        };
        assert_eq!(header_size(&file), Some((w, h)), "{name}");
    }
    let f = small();
    // Garbage before a marker and fill bytes are skipped; no frame header, or a frame type the decoder refuses, gives none.
    let b = decode(&f).unwrap();
    assert_eq!(
        crate::size_of(crate::Kind::Jpeg, &f),
        Some((b.width, b.height))
    );
    assert_eq!(header_size(&f[..2]), None);
    assert_eq!(header_size(b"\xff\xd8\xff\xd9"), None);
    assert_eq!(
        header_size(b"\xff\xd8\xff\xc3\x00\x0b\x08\x00\x01\x00\x01\x01\x01\x11\x00"),
        None
    );
    assert_eq!(
        header_size(b"\xff\xd8\x00\x00\xff\xff\xc0\x00\x0b\x08\x00\x02\x00\x03\x01\x01\x11\x00"),
        Some((3, 2))
    );
    assert_eq!(
        header_size(b"\xff\xd8\xff\xc0\x00\x0b\x08\x00\x00\x00\x03\x01\x01\x11\x00"),
        None
    );
}
