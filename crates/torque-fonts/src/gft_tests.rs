use super::*;
use crate::{FontCache, Glyph, NoFallback, TextColors};

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

/// A `.gft` as `GOldFont::write` writes it.
pub(crate) fn gft(
    height: u32,
    baseline: u32,
    glyphs: &[Glyph],
    sheets: &[Vec<u8>],
    remap: &[i16; 256],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&GFT_VERSION.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&baseline.to_le_bytes());
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
    for r in remap {
        out.extend_from_slice(&r.to_le_bytes());
    }
    out
}

/// A small font: space (advance 3, no bitmap), 'A' (4 x 5 at 0,0, origin 0,5, advance 5), 'B' (3 x 4 at 4,0 on sheet 1, origin
/// 1,4, advance 4), and a remap entry past the table for 'Z'.
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
    ];
    let mut remap = [-1i16; 256];
    remap[b' ' as usize] = 0;
    remap[b'A' as usize] = 1;
    remap[b'B' as usize] = 2;
    remap[b'Z' as usize] = 40;
    let sheets = [
        grey_png(16, 8, |x, y| (x * 16 + y) as u8),
        grey_png(8, 5, |_, _| 200),
    ];
    gft(9, 7, &glyphs, &sheets, &remap)
}

#[test]
fn a_written_font_reads_back() {
    let f = parse_gft(&sample()).unwrap();
    assert_eq!(
        (f.height, f.baseline, f.glyphs.len(), f.sheets.len()),
        (9, 7, 3, 2)
    );
    assert_eq!(f.sheets[0].format.name(), "alpha");
    assert_eq!((f.sheets[1].width, f.sheets[1].height), (8, 5));
    assert_eq!(f.sheet_texture_size(1), Some((8, 8)));
    assert_eq!(f.glyph(b'A').unwrap().x_increment, 5);
    assert!(f.glyph(b'Z').is_none() && f.is_valid_char(b'Z'));
    // Bytes after the remap are never read.
    let mut longer = sample();
    longer.extend_from_slice(b"trailing");
    assert_eq!(parse_gft(&longer).unwrap(), f);
}

#[test]
fn short_files_and_other_versions_are_refused() {
    let bytes = sample();
    for cut in [0, 3, 15, 20, bytes.len() - 513, bytes.len() - 1] {
        assert!(parse_gft(&bytes[..cut]).is_err(), "cut at {cut}");
    }
    let mut v2 = bytes.clone();
    v2[0] = 2;
    assert_eq!(parse_gft(&v2), Err(FontError::Version(2)));
    let mut many = bytes;
    many[12..16].copy_from_slice(&(MAX_GFT_GLYPHS + 1).to_le_bytes());
    assert!(matches!(parse_gft(&many), Err(FontError::Cap { .. })));
}

#[test]
fn widths_count_advances_and_tabs_as_three_spaces() {
    let f = parse_gft(&sample()).unwrap();
    assert_eq!(f.str_width(b"AB"), 9);
    assert_eq!(f.str_width(b"A\tB"), 5 + 9 + 4);
    // 'Z' names a glyph past the table: nothing (TGE reads past the table); 'q' has none.
    assert_eq!(f.str_width(b"Zq"), 0);
}

#[test]
fn the_layout_places_glyphs_at_the_baseline_and_advances_by_x_increment() {
    let f = parse_gft(&sample()).unwrap();
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
}

#[test]
fn colour_codes_pick_from_the_table_and_15_16_17_restore() {
    let f = parse_gft(&sample()).unwrap();
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
fn the_cache_loads_once_and_counts_what_it_could_not_find() {
    let bytes = sample();
    let reads = std::cell::Cell::new(0);
    let read = |p: &str| {
        reads.set(reads.get() + 1);
        (p == format!("{CACHE_DIR}/Arial_14.gft")).then(|| bytes.clone())
    };
    let mut cache = FontCache::with_format(CACHE_DIR, Box::new(NoFallback), Box::new(Gft));
    assert!(cache.get("Arial", 14, &read).is_some());
    assert!(cache.get("arial", 14, &read).is_some());
    assert!(cache.get("Impact", 18, &read).is_none());
    assert!(cache.get("Impact", 18, &read).is_none());
    assert_eq!((reads.get(), cache.missing, cache.fallbacks), (2, 1, 0));
}
