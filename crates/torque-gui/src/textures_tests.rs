use super::*;

#[test]
fn padding_repeats_the_last_column_and_row() {
    let b = Bitmap {
        width: 3,
        height: 2,
        format: Format::Rgb,
        bits: (0..18).collect(),
    };
    let (w, h, t) = padded_texels(&b).unwrap();
    assert_eq!((w, h), (4, 2));
    let px = |x: usize, y: usize| &t[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
    assert_eq!(px(2, 0), &[6, 7, 8, 255]);
    assert_eq!(px(3, 0), &[6, 7, 8, 255]);
    assert_eq!(px(3, 1), &[15, 16, 17, 255]);
    let a = Bitmap {
        width: 1,
        height: 3,
        format: Format::Alpha,
        bits: vec![10, 20, 30],
    };
    let (w, h, t) = padded_texels(&a).unwrap();
    assert_eq!((w, h), (1, 4));
    // One-byte rows read four bytes apart (GL_UNPACK_ALIGNMENT): row 1 starts at byte 4, past the 4-byte buffer.
    assert_eq!(
        t.as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .collect::<Vec<_>>(),
        vec![10, 0, 0, 0]
    );
    assert_eq!(&t[..3], &[255, 255, 255]);
}

#[test]
fn names_get_extensions_then_parent_folders() {
    let c = candidates("art/ui/menu/panel");
    assert_eq!(
        &c[..3],
        &[
            "art/ui/menu/panel",
            "art/ui/menu/panel.jpg",
            "art/ui/menu/panel.png"
        ]
    );
    assert!(c.contains(&"art/panel.png".to_string()));
    assert!(!c.contains(&"panel.png".to_string()));
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = ::png::Encoder::new(&mut out, w, h);
        e.set_color(::png::ColorType::Rgb);
        e.set_depth(::png::BitDepth::Eight);
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(&vec![7; (w * h * 3) as usize]).unwrap();
    }
    out
}

#[test]
fn a_full_cache_drops_its_least_recently_used_and_still_loads_new_bitmaps() {
    // Past 4,096 names nothing new loaded for the rest of the session, and past the byte cap
    // every new bitmap was refused; names of one file are one texture.
    let files: std::collections::HashMap<String, Vec<u8>> = (0..(MAX_TEXTURES + 50))
        .map(|i| (format!("ui/b{i}.png"), png(2, 2)))
        .chain([("ui/big.png".to_string(), png(64, 64))])
        .collect();
    let reads = std::cell::Cell::new(0usize);
    let read = |p: &str| {
        reads.set(reads.get() + 1);
        // The file host matches names ignoring case.
        files.get(&p.to_ascii_lowercase()).cloned()
    };
    let mut t = GuiTextures::default();
    for i in 0..(MAX_TEXTURES + 50) {
        assert!(t.get(&format!("ui/b{i}"), true, &read).is_some(), "{i}");
    }
    assert!(t.len() <= MAX_TEXTURES && t.held().0 <= MAX_TEXTURES);
    // Spellings of one file: one texture, one decode.
    let decodes = t.decodes;
    let a = t.get("ui/BIG", true, &read).unwrap();
    for name in ["UI/big", "ui/Big.png", "Ui/bIg"] {
        assert!(Arc::ptr_eq(&a, &t.get(name, true, &read).unwrap()));
    }
    assert_eq!(t.decodes, decodes + 1);
    // The byte cap: the least recently used go, new ones load.
    let mut small = GuiTextures::default();
    let one = 2 * 2 * 4;
    small.set_caps(MAX_TEXTURES, 3 * one);
    for i in 0..10 {
        assert!(small.get(&format!("ui/b{i}"), true, &read).is_some(), "{i}");
        assert!(small.held().1 <= 3 * one);
    }
    // A frame that draws more than fits: what it drew stays, the rest is not drawn and not decoded again within the frame.
    small.begin_frame();
    let drawn: Vec<bool> = (20..25)
        .map(|i| small.get(&format!("ui/b{i}"), true, &read).is_some())
        .collect();
    assert_eq!(drawn, [true, true, true, false, false]);
    let decodes = small.decodes;
    for i in 20..25 {
        let _ = small.get(&format!("ui/b{i}"), true, &read);
    }
    assert_eq!(small.decodes, decodes, "nothing decoded again in the frame");
    // The next frame draws others: they load.
    small.begin_frame();
    assert!(small.get("ui/b30", true, &read).is_some());
    // A bitmap above the whole cap is not kept, with a note; the notes stay bounded.
    let mut tiny = GuiTextures::default();
    tiny.set_caps(MAX_TEXTURES, 100);
    assert!(tiny.get("ui/big", true, &read).is_none());
    assert!(tiny.notes.iter().any(|n| n.contains("too large")));
    for i in 0..(MAX_NOTES * 4) {
        let _ = tiny.get(&format!("missing/{i}"), true, &read);
    }
    assert!(tiny.notes.len() <= MAX_NOTES);
}
