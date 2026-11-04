use super::*;

fn bitmap(w: u32, h: u32, format: Format, f: impl Fn(usize) -> u8) -> Bitmap {
    let n = w as usize * h as usize * format.bytes_per_pixel();
    Bitmap {
        width: w,
        height: h,
        format,
        bits: (0..n).map(f).collect(),
    }
}

#[test]
fn levels_are_laid_end_to_end_down_to_one_pixel() {
    let c = extrude(
        &bitmap(4, 2, Format::Rgb, |i| i as u8),
        RgbExtrude::C,
        false,
    )
    .unwrap();
    let dims: Vec<_> = c
        .levels
        .iter()
        .map(|l| (l.width, l.height, l.offset))
        .collect();
    assert_eq!(dims, [(4, 2, 0), (2, 1, 24), (1, 1, 30)]);
    assert_eq!(c.bits.len(), 33);
    assert_eq!(
        c.level(0).unwrap(),
        &(0..24).map(|i| i as u8).collect::<Vec<_>>()[..]
    );
}

#[test]
fn the_c_box_filter_rounds_and_the_mmx_one_truncates() {
    // 4 x 2 RGB, every channel of the left 2 x 2 square 0, 0, 0, 3 and of the right one 1, 1, 1, 2: C gives (3 + 2) >> 2 = 1 and
    // (5 + 2) >> 2 = 1; MMX gives 3 >> 2 = 0 and 5 >> 2 = 1.
    let px = |x: usize, y: usize| -> u8 {
        match (x, y) {
            (1, 1) => 3,
            (2..=3, _) if (x, y) == (3, 1) => 2,
            (2..=3, _) => 1,
            _ => 0,
        }
    };
    let b = bitmap(4, 2, Format::Rgb, |i| px((i / 3) % 4, i / 12));
    let c = extrude(&b, RgbExtrude::C, false).unwrap();
    let m = extrude(&b, RgbExtrude::Mmx, false).unwrap();
    assert_eq!(c.level(1).unwrap(), [1, 1, 1, 1, 1, 1]);
    assert_eq!(m.level(1).unwrap(), [0, 0, 0, 1, 1, 1]);
    // RGBA always takes the C version.
    let b = bitmap(2, 2, Format::Rgba, |i| [0, 0, 0, 3][i / 4]);
    assert_eq!(
        extrude(&b, RgbExtrude::Mmx, false)
            .unwrap()
            .level(1)
            .unwrap(),
        [1, 1, 1, 1]
    );
}

#[test]
fn alpha_levels_keep_the_fill() {
    let c = extrude(
        &bitmap(4, 4, Format::Alpha, |_| 7),
        RgbExtrude::default(),
        false,
    )
    .unwrap();
    assert_eq!(c.level(0).unwrap(), [7; 16]);
    assert!(
        c.level(1)
            .unwrap()
            .iter()
            .chain(c.level(2).unwrap())
            .all(|&v| v == 0xff)
    );
}

#[test]
fn the_width_one_path_strides_too_far() {
    // 1 x 4 RGB: the C version averages rows 0 and 1, then advances 6 + 3 bytes and averages "rows" 3 and 4, where row 4 is the
    // first bytes of level 1 (just written).
    let b = bitmap(1, 4, Format::Rgb, |i| (i * 10) as u8);
    let c = extrude(&b, RgbExtrude::Mmx, false).unwrap();
    let l0: Vec<u32> = (0..12).map(|i| (i * 10) as u32).collect();
    let first: Vec<u32> = (0..3).map(|k| (l0[k] + l0[3 + k] + 1) >> 1).collect();
    let second: Vec<u32> = (0..3).map(|k| (l0[9 + k] + first[k] + 1) >> 1).collect();
    let want: Vec<u8> = first.iter().chain(&second).map(|&v| v as u8).collect();
    assert_eq!(c.level(1).unwrap(), &want[..]);
}

#[test]
fn zero_borders_clear_every_ring_above_level_0() {
    let c = extrude(&bitmap(8, 8, Format::Rgba, |_| 200), RgbExtrude::C, true).unwrap();
    assert!(c.level(0).unwrap().iter().all(|&v| v == 200));
    let l1 = c.level(1).unwrap();
    for y in 0..4 {
        for x in 0..4 {
            let inner = (1..3).contains(&x) && (1..3).contains(&y);
            let px = &l1[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
            assert_eq!(px, if inner { [200; 4] } else { [0; 4] }, "({x}, {y})");
        }
    }
    assert!(c.level(2).unwrap().iter().all(|&v| v == 0));
}

#[test]
fn odd_sizes_and_the_level_cap() {
    // A 3 x 2 image is extruded by TGE's arithmetic as it stands (no panic, every level present).
    let c = extrude(
        &bitmap(3, 2, Format::Rgb, |i| i as u8),
        RgbExtrude::Mmx,
        false,
    )
    .unwrap();
    assert_eq!(c.levels.len(), 2);
    assert!(matches!(
        extrude(&bitmap(4096, 1, Format::Alpha, |_| 0), RgbExtrude::C, false),
        Err(ImageError::Cap {
            what: "mip levels",
            ..
        })
    ));
    assert!(extrude(&bitmap(2048, 1, Format::Alpha, |_| 0), RgbExtrude::C, false).is_ok());
    let bad = Bitmap {
        width: 2,
        height: 2,
        format: Format::Rgb,
        bits: vec![0; 3],
    };
    assert!(extrude(&bad, RgbExtrude::C, false).is_err());
}
