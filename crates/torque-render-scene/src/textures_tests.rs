use super::*;

#[test]
fn a_missing_texture_is_looked_for_in_each_parent_folder() {
    let c = candidates("demo/data/art/props/crate.png");
    assert_eq!(c[0], "demo/data/art/props/crate.png");
    assert!(c.contains(&"demo/data/art/crate.png".to_string()));
    assert!(c.contains(&"demo/data/crate.png".to_string()));
    assert!(c.contains(&"demo/crate.png".to_string()));
    assert!(!c.contains(&"crate.png".to_string()), "never the root");
    assert_eq!(
        candidates("demo/data/skies/dawn/1")[1],
        "demo/data/skies/dawn/1.jpg"
    );
}

/// A gray PNG of `w` x `h` with stored (uncompressed) deflate blocks.
pub fn png(w: u32, h: u32, v: u8) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = !0u32;
        for b in bytes {
            c ^= u32::from(*b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xedb8_8320
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    let mut raw = Vec::new();
    for _ in 0..h {
        raw.push(0);
        raw.extend(std::iter::repeat_n(v, w as usize));
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (k, b) in blocks.iter().enumerate() {
        z.push(u8::from(k + 1 == blocks.len()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for x in &raw {
        a = (a + u32::from(*x)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

struct Files(std::collections::HashMap<String, Vec<u8>>);

impl crate::Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(path).cloned()
    }
}

#[test]
fn names_of_one_file_share_one_texture_and_a_frame_keeps_its_byte_cap() {
    // The parent-folder search found packs/X/t.png for every packs/X/<n>/t, and each name was a copy of its own.
    let mut files = std::collections::HashMap::new();
    files.insert("packs/X/t.png".to_string(), png(256, 256, 7));
    for i in 0..4 {
        files.insert(format!("packs/Y/d{i}.png"), png(128, 128, i));
    }
    let files = Files(files);
    let mut set = TextureSet::new(&files);
    let first = set.get("packs/X/n0/t").unwrap();
    for i in 1..2_000 {
        assert_eq!(set.get(&format!("packs/X/n{i}/t")), Some(first));
    }
    // Spellings the (case-blind) reader takes for one file share it too.
    let files_ci = Files(
        [(
            "packs/x/t.png".to_string(),
            files.0["packs/X/t.png"].clone(),
        )]
        .into(),
    );
    struct Blind<'a>(&'a Files);
    impl crate::Assets for Blind<'_> {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.read(&path.to_ascii_lowercase())
        }
    }
    let blind = Blind(&files_ci);
    let mut ci = TextureSet::new(&blind);
    let a = ci.get("packs/X/t").unwrap();
    assert_eq!(ci.get("PACKS/x/T"), Some(a));
    assert_eq!(ci.len(), 1);
    assert_eq!(set.len(), 1);
    let one = set.bytes();
    assert!((256 * 256 * 4..256 * 256 * 4 * 2).contains(&one), "{one}");
    // Distinct files past the frame's cap are not loaded, with a note.
    let mut capped = TextureSet::new(&files);
    // Two textures and their entry costs (ENTRY_COST and the path).
    capped.cap = 2 * (128 * 128 * 4 * 4 / 3 + ENTRY_COST + 20) + 64;
    let loaded = (0..4)
        .filter(|i| capped.get(&format!("packs/Y/d{i}")).is_some())
        .count();
    assert_eq!(loaded, 2);
    assert!(capped.bytes() <= capped.cap);
    assert!(
        capped.notes.iter().any(|n| n.contains("over the frame")),
        "{:?}",
        capped.notes
    );
}

#[test]
fn a_flood_of_names_or_of_tiny_files_stays_under_the_entry_caps() {
    // 1 x 1 textures and missing names reached no byte cap.
    let mut files = std::collections::HashMap::new();
    for i in 0..(MAX_SET_TEXTURES + 100) {
        files.insert(format!("t/{i}.png"), png(1, 1, 3));
    }
    let files = Files(files);
    let mut set = TextureSet::new(&files);
    let loaded = (0..(MAX_SET_TEXTURES + 100))
        .filter(|i| set.get(&format!("t/{i}")).is_some())
        .count();
    assert_eq!(loaded, MAX_SET_TEXTURES);
    assert_eq!(set.len(), MAX_SET_TEXTURES);
    let mut names = TextureSet::new(&files);
    for i in 0..(MAX_SET_NAMES + 500) {
        let _ = names.get(&format!("missing/{i}"));
    }
    assert_eq!(names.by_name.len(), MAX_SET_NAMES);
}

#[test]
fn a_missing_texture_is_searched_once_until_the_files_change() {
    // a server's shapes named textures it never sent, and every frame searched each in every parent folder again.
    clear_resolved();
    struct Counting {
        files: std::cell::RefCell<std::collections::HashMap<String, Vec<u8>>>,
        reads: std::cell::Cell<usize>,
    }
    impl crate::Assets for Counting {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            self.files.borrow().get(&path.to_ascii_lowercase()).cloned()
        }
    }
    let assets = Counting {
        files: Default::default(),
        reads: std::cell::Cell::new(0),
    };
    let name = "packs/rover_paint/skin";
    let frame = || {
        let mut set = TextureSet::new(&assets);
        let got = set.get(name);
        (got, set.notes.len())
    };
    // Other tests bump the process-wide files generation, and each bump rightly searches again: a run counts only when
    // nothing bumped it meanwhile.
    let (searched, later, remembered_at) = (0..20)
        .find_map(|_| {
            files_changed();
            let generation = files_generation();
            let before = assets.reads.get();
            assert_eq!(frame(), (None, 1));
            let searched = assets.reads.get() - before;
            for _ in 0..10 {
                assert_eq!(frame(), (None, 1), "a note each frame, as before");
            }
            let later = assets.reads.get() - before - searched;
            (files_generation() == generation).then_some((searched, later, generation))
        })
        .expect("a run with the files unchanged");
    assert!(searched >= 2, "{searched}");
    assert_eq!(later, 0, "the later frames read nothing");
    // The file arrives (a download): the name is searched again and found.
    assets
        .files
        .borrow_mut()
        .insert("packs/rover_paint/skin.png".into(), png(8, 8, 3));
    // (Only while nothing bumped the generation since the miss was remembered.)
    let got = frame().0;
    if files_generation() == remembered_at {
        assert_eq!(got, None, "still remembered as missing");
    }
    files_changed();
    assert!(frame().0.is_some(), "found after the files changed");
    let found_at = assets.reads.get();
    let _ = frame();
    assert_eq!(assets.reads.get(), found_at, "and remembered as found");
    clear_resolved();
}

#[test]
fn five_hundred_spellings_of_one_texture_cost_one_read_a_session() {
    // A frame's set read every candidate of each name and hashed the file before asking the decoded
    // cache; the particles make a set every frame, so 512 emitters naming one PNG in 512 spellings were 512 reads a frame.
    clear_resolved();
    struct Counting {
        files: std::collections::HashMap<String, Vec<u8>>,
        reads: std::cell::Cell<usize>,
    }
    impl crate::Assets for Counting {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            self.files.get(&path.to_ascii_lowercase()).cloned()
        }
    }
    let dir = "demo/data/shapes/trees/pine";
    let assets = Counting {
        files: [(format!("{dir}/scrubbrush.png"), png(64, 64, 9))].into(),
        reads: std::cell::Cell::new(0),
    };
    let spell = |i: usize| spelling(&format!("{dir}/ScrubBrush"), i);
    let frame = || {
        let t = std::time::Instant::now();
        let mut set = TextureSet::new(&assets);
        let first = set.get_modulated(&spell(0)).expect("found");
        for i in 1..512 {
            assert_eq!(set.get_modulated(&spell(i)), Some(first));
        }
        assert_eq!(set.len(), 1);
        t.elapsed()
    };
    let first = frame();
    // The name with `.jpg`, then with `.png` (the name as given has no image extension and is not read): two reads for the first
    // spelling's search, then none.
    let after_first = assets.reads.get();
    assert_eq!(after_first, 2);
    let mut later = std::time::Duration::ZERO;
    for _ in 0..5 {
        later = later.max(frame());
    }
    assert_eq!(
        assets.reads.get(),
        after_first,
        "the later frames read nothing"
    );
    eprintln!("512 spellings, first frame {first:?}, later frames at most {later:?}");
    // A new session searches again.
    clear_resolved();
    let _ = frame();
    assert_eq!(assets.reads.get(), 2 * after_first);
    clear_resolved();
}

/// `path` with its first 20 letters in upper case where `i`'s bits are set (a spelling the case-blind reader takes as one file).
fn spelling(path: &str, i: usize) -> String {
    path.chars()
        .enumerate()
        .map(|(k, c)| {
            if k < 20 && (i >> k) & 1 == 1 {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}
