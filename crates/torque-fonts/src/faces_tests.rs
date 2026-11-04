use std::sync::Arc;

use crate::gft::byte_font;
use crate::ttf::{self, TtfFace};
use crate::{NoFallback, Units, ttf_fuzz, uft};

use super::*;

fn synthetic_face() -> SharedFace {
    Arc::new(TtfFace::parse(&ttf_fuzz::Synth::basic().build()).expect("synthetic"))
}

/// A cache over the `.uft` files of `cache/` whose made fonts are kept as made.
fn cache(fallback: Box<dyn Fallback>) -> FaceCache {
    FaceCache::new("cache", fallback, Box::new(uft::Uft::default()), |f| f)
}

#[test]
fn a_registered_face_is_found_before_the_cache_files_and_counted() {
    let mut cache = cache(Box::new(NoFallback));
    let read = |_: &str| -> Option<Vec<u8>> { None };
    assert!(cache.get("Inter", 14, &read).is_none());
    assert_eq!(cache.missing, 1);
    let face = synthetic_face();
    assert!(cache.register("Inter", Arc::clone(&face)));
    let (f, src) = cache.get("inter", 14, &read).expect("registered");
    assert_eq!(src, Source::Registered);
    assert_eq!(f.height, 14);
    assert_eq!(cache.made(), 1);
    assert_eq!(
        cache.found(),
        [("inter".to_string(), 14, Source::Registered)]
    );
    // Outside the sizes: none, counted missing.
    assert!(cache.get("Inter", 200, &read).is_none());
    // An empty name is not registered; nor a ninth face.
    assert!(!cache.register("", Arc::clone(&face)));
    for i in 1..MAX_REGISTERED {
        assert!(cache.register(&format!("Face {i}"), Arc::clone(&face)));
    }
    assert!(!cache.register("One More", face));
    assert_eq!(cache.registered(), MAX_REGISTERED);
}

#[test]
fn a_registered_face_never_reads_its_cache_file() {
    let reads = std::cell::Cell::new(0);
    let read = |_: &str| {
        reads.set(reads.get() + 1);
        Some(b"not a font".to_vec())
    };
    let mut cache = cache(Box::new(NoFallback));
    cache.register("Inter", synthetic_face());
    assert!(cache.get("Inter", 14, &read).is_some());
    assert_eq!(reads.get(), 0);
    assert!(cache.get("Other", 14, &read).is_none());
    assert_eq!((reads.get(), cache.bad_files.len()), (1, 1));
}

#[test]
fn registering_a_face_drops_what_the_cache_had_for_it() {
    let mut cache = cache(Box::new(NoFallback));
    let read = |_: &str| -> Option<Vec<u8>> { None };
    assert!(cache.get("Inter", 14, &read).is_none());
    assert_eq!(cache.misses(), [("inter".to_string(), 14)]);
    assert!(cache.register("Inter", synthetic_face()));
    assert!(cache.misses().is_empty());
    assert!(cache.get("Inter", 14, &read).is_some());
    let pixels = cache.sheet_pixels();
    assert!(pixels > 0);
    // Registering it again forgets the font made from the first and gives its pixels back.
    assert!(cache.register("INTER", synthetic_face()));
    assert_eq!(cache.sheet_pixels(), 0);
    assert!(cache.get("Inter", 14, &read).is_some());
    assert_eq!((cache.made(), cache.sheet_pixels()), (2, pixels));
}

#[test]
fn the_other_faces_go_to_the_hosts_fallback() {
    struct Own(Arc<TtfFace>);
    impl Fallback for Own {
        fn make(&self, _face: &str, size: i32) -> Option<Font> {
            self.0.make(size)
        }
        fn name(&self) -> &'static str {
            "own"
        }
    }
    let mut cache = cache(Box::new(Own(synthetic_face())));
    let read = |_: &str| -> Option<Vec<u8>> { None };
    assert_eq!(
        cache.get("Arial", 14, &read).map(|f| f.1),
        Some(Source::Fallback)
    );
    assert_eq!(
        (cache.fallbacks, cache.made(), cache.fallback_name()),
        (1, 0, "own")
    );
    cache.register("Inter", synthetic_face());
    assert_eq!(
        cache.get("Inter", 14, &read).map(|f| f.1),
        Some(Source::Registered)
    );
    assert_eq!(
        cache.get("Arial", 14, &read).map(|f| f.1),
        Some(Source::Fallback)
    );
}

#[test]
fn a_byte_font_converter_makes_byte_fonts_of_the_windows_1252_characters() {
    let face = synthetic_face();
    let unicode = face.make(14).expect("made");
    assert_eq!(unicode.units, Units::Utf16);
    let bytes = byte_font(unicode.clone());
    assert_eq!(bytes.units, Units::Bytes);
    assert_eq!(
        (&bytes.glyphs, &bytes.sheets),
        (&unicode.glyphs, &unicode.sheets)
    );
    for b in 0..=255u8 {
        let c = u32::from(ttf::cp1252(b));
        assert_eq!(
            bytes.glyph(b),
            if b < 32 { None } else { unicode.glyph(c) },
            "{b}"
        );
    }
    let mut cache = FaceCache::new(
        "cache",
        Box::new(NoFallback),
        Box::new(uft::Uft::default()),
        byte_font,
    );
    assert!(cache.register("Inter", face));
    let (f, _) = cache.get("Inter", 14, &|_| None).expect("registered");
    assert_eq!(*f, bytes);
}
