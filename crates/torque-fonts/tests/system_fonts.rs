//! The system fonts' scan of font folders (Linux's and macOS's way) over temporary folders of synthetic faces: a face found by
//! file name, TGE's common faces on their metric-compatible stand-ins, the last resort, refused and oversized files passed over,
//! the depth limit; and the system's own fonts drawing `A` when it has any.
#![cfg(feature = "system-fonts")]

use std::path::{Path, PathBuf};

use torque_fonts::Fallback;
use torque_fonts::system::{self, SystemFonts};
use torque_fonts::ttf_fuzz::{self, Synth};

/// An empty folder of this test's own.
fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("system-fonts-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A well-formed face at `dir/rel`.
fn face(dir: &Path, rel: &str) -> PathBuf {
    put(dir, rel, &Synth::basic().build())
}

fn put(dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn face_keys_drop_case_separators_and_regular() {
    for name in [
        "Liberation Sans",
        "LiberationSans-Regular",
        "liberation_sans",
    ] {
        assert_eq!(system::face_key(name), "liberationsans", "{name}");
    }
    assert_eq!(system::face_key("Times_New_Roman"), "timesnewroman");
    assert_eq!(system::face_key("Regular"), "regular");
}

#[test]
fn the_common_faces_map_to_metric_compatible_free_faces() {
    assert_eq!(
        system::candidates("Arial"),
        ["arial", "liberationsans", "dejavusans"]
    );
    assert_eq!(
        system::candidates("Times New Roman"),
        ["timesnewroman", "liberationserif", "dejavuserif"]
    );
    assert_eq!(
        system::candidates("Courier New"),
        ["couriernew", "liberationmono", "dejavusansmono"]
    );
    assert_eq!(
        system::candidates("Arial Bold"),
        [
            "arialbold",
            "liberationsansbold",
            "dejavusansbold",
            "arial",
            "liberationsans",
            "dejavusans"
        ]
    );
    assert_eq!(
        system::candidates("Arial Italic")[..4],
        [
            "arialitalic",
            "liberationsansitalic",
            "liberationsansoblique",
            "dejavusansitalic"
        ]
    );
    assert_eq!(system::candidates("Some Face"), ["someface"]);
}

#[test]
fn a_named_face_is_found_before_its_stand_ins_in_nested_folders() {
    let dir = folder("named");
    face(&dir, "truetype/liberation/LiberationSans-Regular.ttf");
    let arial = face(&dir, "truetype/msttcorefonts/Arial.ttf");
    let times = face(&dir, "truetype/msttcorefonts/Times_New_Roman.ttf");
    let fonts = SystemFonts::scan(std::slice::from_ref(&dir));
    assert_eq!(fonts.indexed(), 3);
    assert_eq!(fonts.resolve("Arial"), Some(arial));
    assert_eq!(fonts.resolve("times new roman"), Some(times));
    let f = fonts.make("Arial", 14).expect("made");
    assert!(f.glyph(b'A').is_some_and(|a| a.width > 0 && a.height > 0));
}

#[test]
fn arial_falls_back_on_liberation_sans_then_dejavu_sans() {
    let user = folder("subs-user");
    let shared = folder("subs-shared");
    let dejavu = face(&shared, "truetype/dejavu/DejaVuSans.ttf");
    let liberation = face(&shared, "truetype/liberation/LiberationSans-Regular.ttf");
    let bold = face(&shared, "truetype/liberation/LiberationSans-Bold.ttf");
    let mono = face(&shared, "truetype/liberation/LiberationMono-Regular.ttf");
    face(&user, "Zed.ttf");
    let fonts = SystemFonts::scan(&[user.clone(), shared.clone()]);
    assert_eq!(fonts.resolve("Arial"), Some(liberation.clone()));
    assert_eq!(fonts.resolve("ARIAL BOLD"), Some(bold));
    assert_eq!(fonts.resolve("Courier New"), Some(mono));
    // No bold italic: the regular stand-in.
    assert_eq!(fonts.resolve("Arial Bold Italic"), Some(liberation.clone()));
    std::fs::remove_file(&liberation).unwrap();
    let fonts = SystemFonts::scan(&[user, shared]);
    assert_eq!(fonts.resolve("Arial"), Some(dejavu));
    assert!(fonts.make("Arial", 16).is_some());
}

#[test]
fn a_face_with_nothing_like_it_falls_back_on_any_sans_face_then_any_face() {
    let dir = folder("last-resort");
    face(&dir, "a/AnotherSerif.ttf");
    face(&dir, "b/SomeSansMono.ttf");
    let sans = face(&dir, "c/SomeSans-Regular.ttf");
    let fonts = SystemFonts::scan(std::slice::from_ref(&dir));
    assert_eq!(fonts.resolve("Unheard Of"), Some(sans.clone()));
    assert_eq!(fonts.resolve("Arial"), Some(sans));
    assert!(fonts.make("Unheard Of", 12).is_some());

    let dir = folder("any-face");
    let only = face(&dir, "Plain.ttf");
    let fonts = SystemFonts::scan(&[dir]);
    assert_eq!(fonts.resolve("Arial"), Some(only));
}

#[test]
fn refused_oversized_and_other_files_are_passed_over() {
    let dir = folder("refused");
    let (_, bomb) = ttf_fuzz::hostile_shapes()
        .into_iter()
        .find(|(n, _)| *n == "composite bomb")
        .unwrap();
    put(&dir, "Arial.ttf", &bomb);
    put(&dir, "LiberationSans-Regular.ttf", b"not a font");
    put(
        &dir,
        "DejaVuSans.ttf",
        &vec![0u8; torque_fonts::ttf::MAX_TTF_BYTES + 1],
    );
    put(&dir, "Readme.txt", b"text");
    let good = face(&dir, "fallback/FreeSans.ttf");
    let fonts = SystemFonts::scan(std::slice::from_ref(&dir));
    // The oversized file and the text file are not indexed.
    assert_eq!(fonts.indexed(), 3);
    assert_eq!(fonts.resolve("Arial"), Some(good));
    assert!(fonts.make("Arial", 14).is_some());

    let empty = SystemFonts::scan(&[folder("empty"), dir.join("missing")]);
    assert_eq!(empty.indexed(), 0);
    assert!(empty.make("Arial", 14).is_none());
}

#[test]
fn the_scan_stops_at_its_depth() {
    let dir = folder("depth");
    let mut deep = String::new();
    for i in 0..system::MAX_SCAN_DEPTH {
        deep.push_str(&format!("d{i}/"));
    }
    let kept = face(&dir, &format!("{deep}Kept.ttf"));
    face(&dir, &format!("{deep}past/Arial.ttf"));
    let fonts = SystemFonts::scan(&[dir]);
    assert_eq!(fonts.indexed(), 1);
    assert_eq!(fonts.resolve("Arial"), Some(kept));
}

#[test]
fn a_file_named_for_a_face_is_found_by_its_name() {
    let dir = folder("with-file");
    face(&dir, "Arial.ttf");
    let named = face(&dir, "x/Custom-Face-File.ttf");
    let fonts = SystemFonts::scan(&[dir]).with_file("Arial", "custom-face-file.TTF");
    assert_eq!(fonts.resolve("arial"), Some(named));
}

/// The system's own fonts (Windows' folder, the XDG folders or macOS's) draw `A` for Arial, the face TGE's profiles name most:
/// the face itself or a stand-in. Skipped where the system has no font files.
#[test]
fn the_systems_fonts_draw_arial() {
    let Some(fonts) = SystemFonts::new() else {
        eprintln!("this system has no font files: skipped");
        return;
    };
    eprintln!(
        "{} scanned files; Arial from {:?}",
        fonts.indexed(),
        fonts.resolve("Arial")
    );
    for size in [12, 14, 24] {
        let f = fonts.make("Arial", size).expect("Arial or a stand-in");
        assert!(f.glyph(b'A').is_some_and(|a| a.width > 0 && a.height > 0));
    }
}
