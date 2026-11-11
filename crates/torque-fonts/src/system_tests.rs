//! The folder lookup (Windows' way): a face is its own file in the folder's listing, checked, never a path; real Windows fonts
//! come out as they did before the checks; a face read once serves every size.

use std::path::{Path, PathBuf};

use super::SystemFonts;
use crate::Fallback;
use crate::ttf_fuzz::{self, Synth};

/// An empty folder of this test's own.
fn folder(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("torque-fonts-system-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn put(dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn a_folders_face_is_its_own_file_in_any_case_and_nothing_else() {
    let dir = folder("own");
    let fonts_dir = dir.join("Fonts");
    let arial = put(&fonts_dir, "ARIAL.TTF", &Synth::basic().build());
    let lucon = put(&fonts_dir, "lucon.ttf", &Synth::basic().build());
    put(
        &fonts_dir,
        "LiberationSans-Regular.ttf",
        &Synth::basic().build(),
    );
    put(&fonts_dir, "sub/Tahoma.ttf", &Synth::basic().build());
    let fonts = SystemFonts::folder(&fonts_dir)
        .unwrap()
        .with_file("Lucida Console", "LUCON.ttf");
    assert_eq!(fonts.resolve("Arial"), Some(arial));
    assert_eq!(fonts.resolve("lucida console"), Some(lucon));
    // No stand-ins and no subfolders in a folder.
    assert_eq!(fonts.resolve("Times New Roman"), None);
    assert_eq!(fonts.resolve("Tahoma"), None);
    assert!(SystemFonts::folder(&dir.join("missing")).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_face_name_never_reaches_a_path() {
    let dir = folder("paths");
    let fonts_dir = dir.join("Fonts");
    std::fs::create_dir_all(&fonts_dir).unwrap();
    let evil = put(&dir, "Evil.ttf", &Synth::basic().build());
    put(&fonts_dir, "Plain.ttf", &Synth::basic().build());
    let absolute = evil.with_extension("").display().to_string();
    let names = [
        "../Evil".to_string(),
        "..\\Evil".to_string(),
        "..".to_string(),
        absolute,
        "C:Evil".to_string(),
        "Pla\0in".to_string(),
        "Plain/".to_string(),
        String::new(),
    ];
    for fonts in [
        SystemFonts::folder(&fonts_dir).unwrap(),
        SystemFonts::scan(std::slice::from_ref(&fonts_dir)),
    ] {
        for name in &names {
            assert_eq!(fonts.resolve(name), None, "{name:?}");
            assert!(fonts.make(name, 14).is_none(), "{name:?}");
        }
        assert!(fonts.make("Plain", 14).is_some());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folders_files_are_checked_and_capped() {
    let dir = folder("checked");
    let (_, bomb) = ttf_fuzz::hostile_shapes()
        .into_iter()
        .find(|(n, _)| *n == "composite bomb")
        .unwrap();
    put(&dir, "Arial.ttf", &bomb);
    put(&dir, "Big.ttf", &vec![0u8; crate::ttf::MAX_TTF_BYTES + 1]);
    let fonts = SystemFonts::folder(&dir).unwrap();
    assert!(fonts.make("Arial", 14).is_none());
    assert!(fonts.make("Big", 14).is_none());
    assert_eq!(fonts.indexed(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_face_is_read_once_for_every_size() {
    let dir = folder("reuse");
    put(&dir, "Arial.ttf", &Synth::basic().build());
    let fonts = SystemFonts::scan(std::slice::from_ref(&dir));
    for size in [10, 14, 24] {
        assert!(fonts.make("Arial", size).is_some());
        assert!(fonts.make("Arial Bold", size).is_some());
    }
    assert_eq!(fonts.faces.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// TGE's common faces in Windows' fonts folder make the same fonts through the checks as fontdue made of the raw file.
#[cfg(windows)]
#[test]
fn windows_fonts_come_out_as_before_the_checks() {
    use super::rasterize;

    let Some(windir) = std::env::var_os("WINDIR") else {
        return;
    };
    let dir = Path::new(&windir).join("Fonts");
    let Some(fonts) = SystemFonts::folder(&dir) else {
        return;
    };
    let mut compared = 0;
    for face in [
        "arial", "arialbd", "times", "cour", "lucon", "tahoma", "verdana", "georgia", "segoeui",
        "consola",
    ] {
        let Ok(bytes) = std::fs::read(dir.join(format!("{face}.ttf"))) else {
            continue;
        };
        let raw = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).unwrap();
        for size in [10, 14, 24] {
            let checked = fonts.make(face, size).unwrap_or_else(|| panic!("{face}"));
            assert!(rasterize(&raw, size) == Some(checked), "{face} {size}");
        }
        compared += 1;
    }
    assert!(compared > 0, "no common face in {}", dir.display());
}

#[test]
fn a_face_the_folder_has_resolves_and_any_other_does_not() {
    let dir = folder("resolves");
    let fonts_dir = dir.join("Fonts");
    put(&fonts_dir, "ARIAL.TTF", &Synth::basic().build());
    let fonts = SystemFonts::folder(&fonts_dir).unwrap();
    assert!(fonts.resolves("arial"));
    assert!(!fonts.resolves("Times New Roman"));
    assert!(!fonts.resolves("../ARIAL"));
    // A face a cache of the host's fonts asks about is answered through the registry's wrapper.
    let cache = crate::faces::FaceCache::new(
        "cache",
        Box::new(fonts),
        Box::new(crate::uft::Uft::default()),
        |f| f,
    );
    assert!(cache.fallback_resolves("Arial"));
    assert!(!cache.fallback_resolves("Nope"));
    let _ = std::fs::remove_dir_all(&dir);
}
