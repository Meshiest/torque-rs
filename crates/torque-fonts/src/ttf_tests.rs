//! `ttf`'s tests: real faces (from the folder named by `TORQUE_TEST_FONTS`, skipped when it is unset) made at every size with the
//! cell height, a face behind a cache's fallback, and a short hostile sweep.

use std::sync::Arc;

use crate::ttf::{self, TtfFace};
use crate::ttf_fuzz;
use crate::{Fallback, Font, FontCache, Source};

/// A real face from the folder named by `TORQUE_TEST_FONTS`, or None (the test then has nothing to check).
fn bundled(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("TORQUE_TEST_FONTS")?;
    let p = std::path::Path::new(&dir).join(name);
    Some(std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
}

#[test]
fn bundled_faces_make_every_size_with_the_cell_height() {
    for name in [
        "Inter-Regular.ttf",
        "Inter-SemiBold.ttf",
        "JetBrainsMono-Regular.ttf",
    ] {
        let t = std::time::Instant::now();
        let Some(bytes) = bundled(name) else {
            eprintln!("{name}: TORQUE_TEST_FONTS is not set, skipped");
            continue;
        };
        let face = TtfFace::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        eprintln!("{name}: {:?}, parsed in {:?}", face.metrics, t.elapsed());
        for size in ttf::MIN_SIZE..=ttf::MAX_SIZE {
            let f = face.make(size).expect("made");
            assert_eq!(f.height, size as u32, "{name} {size}");
            assert!(f.baseline > 0 && f.baseline <= f.height, "{name} {size}");
            let a = f.glyph(b'A').expect("A");
            assert!(a.width > 0 && a.height > 0, "{name} {size}");
            assert!(f.glyph(b' ').is_some_and(|g| g.x_increment > 0));
            // Every glyph's bitmap inside its sheet.
            for g in &f.glyphs {
                if g.width == 0 {
                    continue;
                }
                let s = &f.sheets[g.bitmap_index as usize];
                assert!(u32::from(g.x_offset) + u32::from(g.width) <= s.width);
                assert!(u32::from(g.y_offset) + u32::from(g.height) <= s.height);
            }
        }
        // Deterministic.
        assert_eq!(face.make(14), face.make(14));
    }
}

/// A fallback that makes fonts from one parsed face.
struct FaceFallback(Arc<TtfFace>);

impl Fallback for FaceFallback {
    fn make(&self, _face: &str, size: i32) -> Option<Font> {
        self.0.make(size)
    }

    fn name(&self) -> &'static str {
        "face"
    }
}

#[test]
fn a_face_behind_a_fallback_is_found_after_the_cache_files_and_counted() {
    let face = Arc::new(TtfFace::parse(&ttf_fuzz::Synth::basic().build()).expect("synthetic"));
    let mut cache = FontCache::new("fonts", Box::new(FaceFallback(face)));
    let read = |_: &str| -> Option<Vec<u8>> { None };
    let (f, src) = cache.get("Inter", 14, &read).expect("made");
    assert_eq!(src, Source::Fallback);
    assert_eq!(f.height, 14);
    assert_eq!(cache.fallbacks, 1);
    // Outside the sizes: none, counted missing.
    assert!(cache.get("Inter", 200, &read).is_none());
    assert_eq!(cache.missing, 1);
}

#[test]
fn the_hostile_shapes_are_refused_or_bounded() {
    for (name, bytes) in ttf_fuzz::hostile_shapes() {
        let t = std::time::Instant::now();
        let r = TtfFace::parse(&bytes);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        eprintln!(
            "{name}: {:?} in {ms:.2} ms",
            r.as_ref().map(|f| f.metrics).map_err(|e| e.to_string())
        );
        assert!(ms < 2_000.0, "{name} took {ms} ms");
        match name {
            "empty components" => assert!(r.is_ok(), "{name} refused"),
            "composite bomb"
            | "composite cycle"
            | "composite fan of empty components"
            | "curve bomb"
            | "cmap past the glyphs"
            | "cmap full range"
            | "kern loop"
            | "maxp 65535"
            | "cmap format 12 x1000 full ranges"
            | "cff"
            | "collection"
            | "unsorted directory"
            | "glyf past the end"
            | "empty"
            | "12 bytes" => assert!(r.is_err(), "{name} accepted"),
            _ => {}
        }
        if let Ok(f) = r {
            for size in [6, 50, 96] {
                let _ = f.make(size);
            }
        }
    }
}

/// The hostile sweep. `TORQUE_FUZZ_TTF` sets the case count (600 by default); under Miri (the scheduled sanitizer job, which
/// checks the rasterizer's unchecked writes) the real face seeds are left out and the count is small.
#[test]
fn a_short_hostile_sweep_has_no_panics() {
    let cases: u64 = std::env::var("TORQUE_FUZZ_TTF")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(miri) { 60 } else { 600 });
    let seeds: Vec<Vec<u8>> = if cfg!(miri) {
        Vec::new()
    } else {
        bundled("JetBrainsMono-Regular.ttf").into_iter().collect()
    };
    let s = ttf_fuzz::sweep(cases, 7, &seeds, 10);
    eprintln!("{s:?}");
    assert_eq!(s.cases, cases);
    assert_eq!(s.panics, 0, "{s:?}");
}
