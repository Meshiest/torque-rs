use super::*;

/// A cache folder for the tests.
const CACHE_DIR: &str = "cache";

/// A grey PNG of `w` x `h` (pixel value `f(x, y)`).
pub(crate) fn grey_png(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let raw: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| f(x, y))
        .collect();
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Grayscale);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().expect("header");
        wr.write_image_data(&raw).expect("data");
    }
    out
}

/// A `.uft` as `GFont::write` writes it: the remap's range from its lowest to its highest code unit (none: `S32_MAX`, 0).
pub(crate) fn uft(
    height: u32,
    baseline: u32,
    glyphs: &[Glyph],
    sheets: &[Vec<u8>],
    remap: &[(u32, i32)],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&uft::VERSION.to_le_bytes());
    out.push(5);
    out.extend_from_slice(b"Arial");
    for v in [14, 0, height, baseline, baseline, height - baseline] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&(glyphs.len() as u32).to_le_bytes());
    for g in glyphs {
        out.extend_from_slice(&g.bitmap_index.to_le_bytes());
        out.extend_from_slice(&[
            g.x_offset,
            g.y_offset,
            g.width,
            g.height,
            g.x_origin as u8,
            g.y_origin as u8,
            g.x_increment as u8,
        ]);
    }
    out.extend_from_slice(&(sheets.len() as u32).to_le_bytes());
    for s in sheets {
        out.extend_from_slice(s);
    }
    for v in [3i32, 4, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let table = Remap::from_pairs(remap);
    match table.range() {
        Some((lo, hi)) => {
            out.extend_from_slice(&lo.to_le_bytes());
            out.extend_from_slice(&hi.to_le_bytes());
            let raw: Vec<u8> = (lo..=hi).flat_map(|u| table.get(u).to_be_bytes()).collect();
            let packed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9);
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&packed);
        }
        None => {
            out.extend_from_slice(&i32::MAX.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }
    }
    out
}

/// A small font: space (advance 3, no bitmap), 'A' (4 x 5 at 0,0, origin 0,5, advance 5), 'B' (3 x 4 at 4,0 on sheet 1, origin
/// 1,4, advance 4), U+00E9 and U+20AC sharing A's glyph, a tab glyph (advance 7), and a remap entry past the table for 'Z'.
pub(crate) fn sample() -> Vec<u8> {
    let glyphs = [
        Glyph {
            bitmap_index: 0,
            x_increment: 3,
            ..Glyph::default()
        },
        Glyph {
            bitmap_index: 0,
            x_offset: 0,
            y_offset: 0,
            width: 4,
            height: 5,
            x_origin: 0,
            y_origin: 5,
            x_increment: 5,
        },
        Glyph {
            bitmap_index: 1,
            x_offset: 4,
            y_offset: 0,
            width: 3,
            height: 4,
            x_origin: 1,
            y_origin: 4,
            x_increment: 4,
        },
        Glyph {
            bitmap_index: 0,
            x_increment: 7,
            ..Glyph::default()
        },
    ];
    let remap = [
        (u32::from(b' '), 0),
        (u32::from(b'A'), 1),
        (u32::from(b'B'), 2),
        (u32::from(b'\t'), 3),
        (u32::from(b'Z'), 40),
        (0xe9, 1),
        (0x20ac, 1),
    ];
    let sheets = [
        grey_png(16, 8, |x, y| (x * 16 + y) as u8),
        grey_png(8, 5, |_, _| 200),
    ];
    uft(9, 7, &glyphs, &sheets, &remap)
}

#[test]
fn a_written_font_reads_back() {
    let file = uft::UftFile::parse(&sample()).unwrap();
    assert_eq!(
        (&file.face[..], file.size, file.charset, file.pen),
        (&b"Arial"[..], 14, 0, [3, 4, 0])
    );
    let f = file.font;
    assert_eq!((f.height, f.baseline, f.ascent, f.descent), (9, 7, 7, 2));
    assert_eq!(
        (f.glyphs.len(), f.sheets.len(), f.units),
        (4, 2, Units::Utf16)
    );
    assert_eq!(f.sheets[0].format, torque_images::Format::Alpha);
    assert_eq!(f.sheet_texture_size(1), Some((8, 8)));
    assert_eq!(f.glyph(b'A').unwrap().x_increment, 5);
    assert_eq!(f.glyph(0x20acu32), f.glyph(b'A'));
    assert!(f.glyph(b'Z').is_none() && f.is_valid_char(b'Z'));
    assert!(!f.is_valid_char(b'q'));
    assert_eq!(f.remap.range(), Some((9, 0x20ac)));
    // Bytes after the remap are never read.
    let mut longer = sample();
    longer.extend_from_slice(b"trailing");
    assert_eq!(Font::parse(&longer).unwrap(), f);
}

#[test]
fn a_font_with_no_glyphs_has_no_remap() {
    let f = Font::parse(&uft(10, 8, &[], &[grey_png(1, 1, |_, _| 0)], &[])).unwrap();
    assert_eq!(f.remap.range(), None);
    assert!(!f.is_valid_char(b'A'));
    assert_eq!(f.str_width(b"AB"), 0);
}

#[test]
fn short_files_other_versions_and_bad_remaps_are_refused() {
    let bytes = sample();
    for cut in [0, 3, 5, 20, 40, bytes.len() - 1] {
        assert!(Font::parse(&bytes[..cut]).is_err(), "cut at {cut}");
    }
    // Only version 3 is read.
    let mut v1 = bytes.clone();
    v1[0] = 1;
    assert_eq!(Font::parse(&v1), Err(FontError::Version(1)));
    // The glyph count right after the fixed header (version, name, six fields).
    let count_at = 4 + 1 + 5 + 24;
    let mut many = bytes.clone();
    many[count_at..count_at + 4].copy_from_slice(&(MAX_GLYPHS + 1).to_le_bytes());
    assert!(matches!(Font::parse(&many), Err(FontError::Cap { .. })));
    // The remap's tail: a damaged zlib stream, then a range past the code units.
    let mut damaged = bytes.clone();
    let n = damaged.len();
    damaged[n - 3] ^= 0xff;
    assert!(matches!(Font::parse(&damaged), Err(FontError::Remap(_))));
    let font = Font::parse(&bytes).unwrap();
    let mut wide = uft(
        9,
        7,
        &font.glyphs[..1],
        &[grey_png(1, 1, |_, _| 0)],
        &[(0x41, 0)],
    );
    // The range's max sits before the length and the zlib data of the one entry: set it to 0x10000.
    let packed_len = miniz_oxide::deflate::compress_to_vec_zlib(&0i32.to_be_bytes(), 9).len();
    let max_field = wide.len() - packed_len - 4 - 4;
    wide[max_field..max_field + 4].copy_from_slice(&0x1_0000u32.to_le_bytes());
    assert!(matches!(
        Font::parse(&wide),
        Err(FontError::Cap {
            what: "remap code unit",
            ..
        })
    ));
}

#[test]
fn text_is_utf8_converted_to_utf16_as_tge_converts_it() {
    assert_eq!(
        utf16_units("A\u{e9}\u{20ac}".as_bytes()),
        [0x41, 0xe9, 0x20ac]
    );
    // Above the Basic Multilingual Plane, a stray continuation, a cut sequence: U+FFFD, one byte on for the bad ones.
    assert_eq!(utf16_units("\u{1f600}".as_bytes()), [0xfffd]);
    assert_eq!(utf16_units(b"\x80A"), [0xfffd, 0x41]);
    assert_eq!(utf16_units(b"\xc3A"), [0xfffd, 0x41]);
    assert_eq!(utf16_units(b"\xe2\x82"), [0xfffd, 0xfffd]);
    // An overlong form is decoded as written; a NUL ends the text.
    assert_eq!(utf16_units(b"\xc1\x81"), [0x41]);
    assert_eq!(utf16_units(b"AB\0C"), [0x41, 0x42]);
    // TGE's surrogate test leaves out both ends of the range.
    assert_eq!(utf16_units(b"\xed\xa0\x80"), [0xd800]);
    assert_eq!(utf16_units(b"\xed\xa0\x81"), [0xfffd]);
}

#[test]
fn widths_count_advances_and_tabs_as_three_spaces() {
    let f = Font::parse(&sample()).unwrap();
    assert_eq!(f.str_width(b"AB"), 9);
    // The tab has a glyph: its own advance.
    assert_eq!(f.str_width(b"A\tB"), 5 + 7 + 4);
    assert_eq!(f.str_width("\u{e9}\u{20ac}".as_bytes()), 10);
    // 'Z' names a glyph past the table: nothing; 'q' has none; the text ends at a NUL.
    assert_eq!(f.str_width(b"Zq"), 0);
    assert_eq!(f.str_width(b"A\0B"), 5);
}

#[test]
fn the_precise_width_adds_the_last_glyphs_overhang_only_when_every_byte_is_a_unit() {
    let f = Font::parse(&sample()).unwrap();
    // B: width 3, advance 4: no overhang. A space then 'A': none either. A glyph wider than its advance:
    let mut wide = f.clone();
    wide.glyphs[1].x_increment = 2;
    assert_eq!(wide.str_width_precise(b"BA"), 4 + 2 + 2);
    // A two-byte character before the last: the last unit TGE looks at is the end of the text.
    assert_eq!(wide.str_width_precise("\u{e9}A".as_bytes()), 2 + 2);
    assert_eq!(wide.str_width_precise(b""), 0);
}

#[test]
fn the_break_position_counts_utf16_units() {
    let f = Font::parse(&sample()).unwrap();
    assert_eq!(f.break_pos(b"AAAA", 12, false), 2);
    assert_eq!(f.break_pos(b"A AA", 12, true), 2);
    // Two two-byte characters fit in 10 pixels: two units (four bytes).
    assert_eq!(f.break_pos("\u{e9}\u{e9}A".as_bytes(), 10, false), 2);
    assert_eq!(f.break_pos(b"AB", 100, false), 2);
}

#[test]
fn the_layout_places_glyphs_at_the_baseline_and_advances_by_x_increment() {
    let f = Font::parse(&sample()).unwrap();
    let mut colors = TextColors::new([10, 20, 30, 255], [0; 4]);
    let l = f.layout((100, 50), b"A B", &mut colors, None, 9);
    assert_eq!(l.quads.len(), 2);
    let a = l.quads[0];
    assert_eq!(a.rect, [100, 50 + 7 - 5, 104, 52 + 5]);
    assert_eq!(a.uv, [0.0, 0.0, 4.0 / 16.0, 5.0 / 8.0]);
    let b = l.quads[1];
    // A advances 5, the space 3: B's pen at 108, drawn at 108 + 1.
    assert_eq!(b.rect, [109, 53, 112, 57]);
    assert_eq!(b.sheet, 1);
    assert_eq!(b.uv, [4.0 / 8.0, 0.0, 7.0 / 8.0, 4.0 / 8.0]);
    assert_eq!(l.advance, 12);
    assert_eq!(a.color, [10, 20, 30, 255]);
    // A character past the bytes draws its glyph.
    let l = f.layout((0, 0), "\u{20ac}".as_bytes(), &mut colors, None, 9);
    assert_eq!((l.quads.len(), l.advance), (1, 5));
}

#[test]
fn a_tab_is_three_spaces_whatever_glyph_it_has_and_a_nul_ends_the_line() {
    let f = Font::parse(&sample()).unwrap();
    let mut colors = TextColors::new([0; 4], [0; 4]);
    assert_eq!(
        f.layout((0, 0), b"\tA", &mut colors, None, 9).advance,
        9 + 5
    );
    assert_eq!(
        f.layout((0, 0), b"A\0A", &mut colors, None, 9).quads.len(),
        1
    );
}

#[test]
fn colour_codes_pick_from_the_table_and_15_16_17_restore() {
    let f = Font::parse(&sample()).unwrap();
    let table: Vec<[u8; 4]> = (0..10).map(|i| [i as u8, 0, 0, 255]).collect();
    let mut colors = TextColors::new([9, 9, 9, 255], [0; 4]);
    let l = f.layout(
        (0, 0),
        b"\x03A\x10\x0eA\x0fA\x11A",
        &mut colors,
        Some(&table),
        9,
    );
    let c: Vec<[u8; 4]> = l.quads.iter().map(|q| q.color).collect();
    assert_eq!(
        c,
        vec![
            [2, 0, 0, 255],
            [9, 0, 0, 255],
            [9, 9, 9, 255],
            [2, 0, 0, 255]
        ]
    );
    // Above the caller's highest index, or with no table: unchanged.
    let mut colors = TextColors::new([9, 9, 9, 255], [0; 4]);
    let l = f.layout((0, 0), b"\x0eA", &mut colors, Some(&table), 8);
    assert_eq!(l.quads[0].color, [9, 9, 9, 255]);
}

#[test]
fn a_byte_font_takes_each_byte_as_a_unit() {
    let mut f = Font::parse(&sample()).unwrap();
    f.units = Units::Bytes;
    // 0xe9 is one unit (the glyph U+00E9 has); the UTF-8 of U+00E9 is two units with no glyph.
    assert_eq!(f.str_width(b"\xe9"), 5);
    assert_eq!(f.str_width("\u{e9}".as_bytes()), 0);
    // A NUL is a unit without a glyph; a tab with a glyph is that glyph when measured.
    assert_eq!(f.str_width(b"A\0A"), 10);
    assert_eq!(f.str_width(b"\tA"), 7 + 5);
}

#[test]
fn a_tab_draws_as_three_spaces_or_as_measured() {
    let mut f = Font::parse(&sample()).unwrap();
    f.units = Units::Bytes;
    let spaces = 3 * f.str_width(b" ");
    let mut colors = TextColors::new([0; 4], [0; 4]);
    assert_eq!(
        f.layout((0, 0), b"\tA", &mut colors, None, 9).advance,
        spaces + 5
    );
    f.tab = Tab::GlyphOrSpaces;
    assert_eq!(
        f.layout((0, 0), b"\tA", &mut colors, None, 9).advance,
        7 + 5
    );
}

#[test]
fn a_png_ends_at_iend() {
    let p = grey_png(3, 2, |_, _| 1);
    let mut longer = p.clone();
    longer.extend_from_slice(&[1, 2, 3]);
    assert_eq!(png_extent(&longer), Some(p.len()));
    assert_eq!(png_extent(&p[..p.len() - 1]), None);
}

#[test]
fn cache_files_are_named_by_face_size_and_charset() {
    assert_eq!(cache_file_name("Arial", 14), "Arial 14 (ansi).uft");
    assert_eq!(
        uft::file_name("Lucida Console", 12, 16).as_deref(),
        Some("Lucida Console 12 (russian).uft")
    );
    assert_eq!(uft::file_name("Arial", 14, 19), None);
    assert!(uft::Uft::new(19).is_none());
}

#[test]
fn the_cache_loads_once_and_counts_what_it_could_not_find() {
    let bytes = sample();
    let reads = std::cell::Cell::new(0);
    let read = |p: &str| {
        reads.set(reads.get() + 1);
        (p == format!("{CACHE_DIR}/Arial 14 (ansi).uft")).then(|| bytes.clone())
    };
    let mut cache = FontCache::new(CACHE_DIR, Box::new(NoFallback));
    assert!(cache.get("Arial", 14, &read).is_some());
    assert!(cache.get("arial", 14, &read).is_some());
    assert!(cache.get("Impact", 18, &read).is_none());
    assert!(cache.get("Impact", 18, &read).is_none());
    assert_eq!((reads.get(), cache.missing, cache.fallbacks), (2, 1, 0));
}

/// Another cache format: every file is the sample, named by size alone.
struct BySize;

impl CacheFormat for BySize {
    fn file_name(&self, _face: &str, size: i32) -> String {
        format!("{size}.font")
    }

    fn parse(&self, bytes: &[u8]) -> Result<Font, FontError> {
        Font::parse(bytes)
    }
}

#[test]
fn a_cache_reads_the_format_it_is_given() {
    let bytes = sample();
    let read = |p: &str| (p == "cache/9.font").then(|| bytes.clone());
    let mut cache = FontCache::with_format(CACHE_DIR, Box::new(NoFallback), Box::new(BySize));
    assert!(cache.get("Any", 9, &read).is_some());
    assert!(cache.get("Any", 10, &read).is_none());
}

/// A fallback that makes a font of one `side` x `side` sheet at any size (as a system fallback does at large sizes).
struct BigFallback(u32);

impl Fallback for BigFallback {
    fn make(&self, _face: &str, _size: i32) -> Option<Font> {
        Some(Font {
            height: 10,
            baseline: 8,
            ascent: 8,
            descent: 2,
            glyphs: Vec::new(),
            sheets: vec![Bitmap {
                width: self.0,
                height: self.0,
                format: torque_images::Format::Alpha,
                bits: vec![0; (self.0 * self.0) as usize],
            }],
            remap: Remap::default(),
            units: Units::Utf16,
            tab: Tab::Spaces,
        })
    }

    fn name(&self) -> &'static str {
        "big"
    }
}

#[test]
fn fallback_and_cache_file_sheets_count_toward_one_pixel_budget() {
    // 1,024 fallback fonts at large sizes were several GiB.
    let mut cache = FontCache::new(CACHE_DIR, Box::new(BigFallback(1024)));
    cache.pixel_cap = 3 * 1024 * 1024;
    let none = |_: &str| None;
    let kept = (1..=10)
        .filter(|size| cache.get("Arial", *size, &none).is_some())
        .count();
    assert_eq!(kept, 3);
    assert_eq!(cache.over_budget, 7);
    assert!(cache.sheet_pixels() <= cache.pixel_cap);
    // A refused font is remembered as missing, not made again on every draw.
    assert!(cache.get("Arial", 9, &none).is_none());
    assert_eq!(cache.over_budget, 7);
}

#[test]
fn removing_a_face_drops_its_fonts_and_gives_their_pixels_back() {
    let mut cache = FontCache::new(CACHE_DIR, Box::new(BigFallback(64)));
    let none = |_: &str| None;
    assert!(cache.get("Arial", 10, &none).is_some());
    assert!(cache.get("arial", 12, &none).is_some());
    assert!(cache.get("Other", 10, &none).is_some());
    assert_eq!(cache.sheet_pixels(), 3 * 64 * 64);
    let dropped = cache.remove_face("ARIAL");
    assert_eq!(dropped.len(), 2);
    assert_eq!(cache.sheet_pixels(), 64 * 64);
    assert_eq!(cache.found().len(), 1);
    // The next lookup makes the font again.
    assert!(cache.get("Arial", 10, &none).is_some());
    assert_eq!(cache.fallbacks, 4);
}
