use torque_fonts::ttf_fuzz::Synth;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use super::*;
use crate::draw::{DrawCache, draw_canvas};
use crate::{canvas, register as register_gui};

/// A GUI over files holding a TrueType face (`mods/fonts/a.ttf`, `b.ttf`, `c.ttf`), one that is not one and one too large.
fn rig() -> (Vm, Gui, MemFiles) {
    let mut vm = Vm::new();
    let gui = register_gui(&mut vm);
    gui.force_canvas_size(Some((200, 100)));
    let files = MemFiles::new();
    let ttf = Synth::basic().build();
    for name in ["a", "b", "c", "d"] {
        files.insert(&format!("mods/fonts/{name}.ttf"), &ttf);
    }
    files.insert("mods/fonts/broken.ttf", b"not a font");
    files.insert("mods/fonts/huge.ttf", &vec![0; MAX_TTF_BYTES + 1]);
    vm.set_file_host(files.clone());
    (vm, gui, files)
}

fn register_font(vm: &mut Vm, face: &str, path: &str) -> bool {
    let call = format!("return registerFont(\"{face}\", \"{path}\");");
    vm.eval(call.as_bytes()).unwrap() == b"1"
}

fn console(vm: &mut Vm) -> String {
    vm.drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_face_from_a_file_below_the_game_folder_is_registered() {
    let (mut vm, gui, _) = rig();
    assert!(register_font(&mut vm, "Mod Face", "mods/fonts/a.ttf"));
    assert_eq!(gui.0.borrow().faces.len(), 1);
    // The layout's fonts took it.
    let fonts = gui.0.borrow().fonts.0.clone();
    let fonts = fonts.borrow();
    let cache = fonts
        .as_any()
        .downcast_ref::<torque_fonts::faces::FaceCache>()
        .expect("the default fonts take faces");
    assert!(cache.is_registered("mod face"));
}

#[test]
fn what_is_not_a_face_a_path_or_a_file_the_host_gives_is_refused_with_a_line() {
    let (mut vm, gui, _) = rig();
    let refused = [
        // The names.
        ("", "mods/fonts/a.ttf"),
        ("Bad/Name", "mods/fonts/a.ttf"),
        ("Bad:Name", "mods/fonts/a.ttf"),
        (" Lead", "mods/fonts/a.ttf"),
        ("Trail ", "mods/fonts/a.ttf"),
        (&"x".repeat(MAX_FACE_BYTES + 1), "mods/fonts/a.ttf"),
        // The paths.
        ("F", ""),
        ("F", "/mods/fonts/a.ttf"),
        ("F", "mods/../mods/fonts/a.ttf"),
        ("F", "../a.ttf"),
        ("F", "C:/fonts/a.ttf"),
        ("F", "mods\\fonts\\a.ttf"),
        ("F", "mods//fonts/a.ttf"),
        ("F", "./mods/fonts/a.ttf"),
        ("F", "//host/share/x.ttf"),
        ("F", "con.ttf"),
        ("F", "mods/fonts/NUL.ttf"),
        ("F", "mods/fonts/com1.ttf"),
        ("F", "mods/lpt9.ttf"),
        ("F", "aux/a.ttf"),
        ("F", "mods/fonts/a.otf"),
        ("F", "mods/fonts/a"),
        // The files.
        ("F", "mods/fonts/missing.ttf"),
        ("F", "mods/fonts/broken.ttf"),
        ("F", "mods/fonts/huge.ttf"),
    ];
    for (face, path) in refused {
        assert!(!register_font(&mut vm, face, path), "{face:?} {path:?}");
    }
    assert!(gui.0.borrow().faces.is_empty());
    let lines = console(&mut vm);
    assert_eq!(
        lines.matches("registerFont: ").count(),
        refused.len(),
        "{lines}"
    );
    assert!(lines.contains("no such file"), "{lines}");
    assert!(lines.contains("is 4194305 bytes"), "{lines}");
}

#[test]
fn a_face_is_registered_once_whatever_the_case_and_only_so_many_are() {
    let (mut vm, gui, files) = rig();
    assert!(register_font(&mut vm, "Once", "mods/fonts/a.ttf"));
    assert!(!register_font(&mut vm, "ONCE", "mods/fonts/b.ttf"));
    assert!(console(&mut vm).contains("in use already"));
    let ttf = Synth::basic().build();
    let mut made = 1;
    for i in 0..torque_fonts::faces::MAX_REGISTERED + 2 {
        files.insert(&format!("mods/fonts/n{i}.ttf"), &ttf);
        made += usize::from(register_font(
            &mut vm,
            &format!("Face {i}"),
            &format!("mods/fonts/n{i}.ttf"),
        ));
    }
    assert_eq!(made, torque_fonts::faces::MAX_REGISTERED);
    assert_eq!(gui.0.borrow().faces.len(), made);
    assert!(console(&mut vm).contains("faces are registered already"));
}

#[test]
fn a_face_name_the_games_fonts_have_is_refused_in_any_case() {
    let (mut vm, gui, files) = rig();
    // A default face of the client script layer, and the faces cache files of the font folder name.
    files.insert("common/ui/cache/Menu Face 12 (ANSI).uft", b"");
    files.insert("common/ui/cache/Menu Face 16 (ANSI).uft", b"");
    files.insert("common/ui/cache/Other-Face 9 (ANSI).uft", b"");
    for face in [
        "Arial",
        "ARIAL",
        "lucida console",
        "Menu Face",
        "MENU FACE",
        "other-face",
    ] {
        assert!(!register_font(&mut vm, face, "mods/fonts/a.ttf"), "{face}");
    }
    let lines = console(&mut vm);
    assert_eq!(lines.matches("in use already").count(), 6, "{lines}");
    assert!(gui.0.borrow().faces.is_empty());
    // A name that only starts like one is free.
    assert!(register_font(&mut vm, "Menu", "mods/fonts/a.ttf"));
    assert!(register_font(&mut vm, "Menu Face 2", "mods/fonts/b.ttf"));
    assert!(!register_font(&mut vm, "menu face 2", "mods/fonts/c.ttf"));
}

#[test]
fn the_bytes_are_paid_for_before_they_are_read() {
    let (mut vm, gui, _) = rig();
    let saved = vm.begin_host_budget(16);
    let result = register_face(&vm, &gui, b"Budgeted", b"mods/fonts/a.ttf");
    vm.end_host_budget(saved);
    assert_eq!(result, Err(Refused::Budget));
    assert!(gui.0.borrow().faces.is_empty());
}

#[test]
fn only_so_many_calls_are_answered() {
    let (mut vm, _gui, _) = rig();
    for _ in 0..MAX_ATTEMPTS {
        assert!(!register_font(&mut vm, "F", "mods/fonts/missing.ttf"));
    }
    // Past the cap even a good file is not looked at.
    assert!(!register_font(&mut vm, "Good", "mods/fonts/a.ttf"));
    assert!(console(&mut vm).contains("more than 64 calls"));
}

#[test]
fn every_draw_cache_draws_text_in_the_registered_face() {
    let (mut vm, gui, _) = rig();
    assert!(canvas::create_canvas(&mut vm, &gui).unwrap());
    vm.eval(b"new GuiControl(Top) { extent = \"200 100\"; }; Canvas.setContent(Top);")
        .unwrap();
    let mut first = DrawCache::default();
    draw_canvas(&vm, &gui, &mut first);
    let glyphs = |draws: &crate::draw::GuiDraws| {
        draws.batches.iter().filter(|b| b.texture.is_some()).count()
    };
    assert!(register_font(&mut vm, "Mod Face", "mods/fonts/a.ttf"));
    // A profile made after the registration (mods load after the game's own scripts) can use the face.
    vm.eval(
        b"new GuiControlProfile(ModProfile) { fontType = \"Mod Face\"; fontSize = 16; fontColors[0] = \"255 255 255 255\"; };           Top.add(new GuiTextCtrl(Label) { profile = ModProfile; position = \"4 4\"; extent = \"100 20\"; text = \"ABC\"; });",
    )
    .unwrap();
    // The cache that drew before and one made after both draw the text from the registered face.
    for cache in [&mut first, &mut DrawCache::default()] {
        let after = draw_canvas(&vm, &gui, cache).draws;
        assert!(
            after.report.missing_fonts.is_empty(),
            "{:?}",
            after.report.missing_fonts
        );
        assert!(glyphs(&after) > 0);
    }
}

#[test]
fn the_names_and_paths_the_rules_take() {
    assert!(face_ok("Arial Bold-2_x"));
    assert!(!face_ok("Ünï"));
    assert!(path_ok(b"mods/a/b.TTF"));
    // The paths that reach another machine or a device, as the rule sees them (not through a script's string escapes).
    assert!(!path_ok(b"//host/share/x.ttf"));
    assert!(!path_ok(br"\\host\x.ttf"));
    assert!(!path_ok(br"\\.\pipe\x.ttf"));
    for device in [
        "con", "CON", "nul", "prn", "aux", "com1", "LPT9", "nul.txt", "con.x",
    ] {
        assert!(!path_ok(format!("{device}.ttf").as_bytes()), "{device}");
        assert!(
            !path_ok(format!("mods/{device}/a.ttf").as_bytes()),
            "{device}"
        );
    }
    // Names that only look like them are files.
    for plain in ["console", "com10", "com0", "lpt", "nullable", "auxiliary"] {
        assert!(path_ok(format!("mods/{plain}.ttf").as_bytes()), "{plain}");
    }
    assert!(!path_ok(b"mods/a/b.ttf\0"));
    assert!(!path_ok(&[b'a'; MAX_PATH_BYTES + 1]));
}

#[test]
fn a_face_a_profile_names_is_refused_though_it_was_never_drawn_and_has_no_cache_file() {
    let (mut vm, gui, _) = rig();
    vm.eval(b"new GuiControlProfile(MenuProfile) { fontType = \"Ghost Face\"; fontSize = 14; };")
        .unwrap();
    // Nothing drew it and no font cache file names it: the profile does.
    assert!(!register_font(&mut vm, "ghost face", "mods/fonts/a.ttf"));
    assert!(!register_font(&mut vm, "GHOST FACE", "mods/fonts/a.ttf"));
    assert!(console(&mut vm).contains("in use already"));
    assert!(gui.0.borrow().faces.is_empty());
    assert!(register_font(&mut vm, "Ghost Face 2", "mods/fonts/a.ttf"));
}

#[test]
fn a_face_the_fonts_fallback_would_make_is_refused() {
    use torque_fonts::faces::FaceCache;
    use torque_fonts::{Fallback, Font};

    struct System;
    impl Fallback for System {
        fn make(&self, _face: &str, _size: i32) -> Option<Font> {
            None
        }
        fn resolves(&self, face: &str) -> bool {
            face.eq_ignore_ascii_case("Sys Face")
        }
        fn name(&self) -> &'static str {
            "system"
        }
    }

    let (mut vm, gui, _) = rig();
    let cache = FaceCache::new(
        "fonts",
        Box::new(System),
        Box::new(torque_fonts::uft::Uft::default()),
        std::convert::identity,
    );
    gui.0.borrow_mut().fonts = crate::SharedFonts::new(Box::new(cache));
    assert!(!register_font(&mut vm, "sys face", "mods/fonts/a.ttf"));
    assert!(console(&mut vm).contains("in use already"));
    assert!(register_font(&mut vm, "Free Face", "mods/fonts/a.ttf"));
}

#[test]
fn a_draw_cache_whose_own_fallback_makes_a_face_is_not_given_the_registered_one() {
    use torque_fonts::faces::FaceCache;
    use torque_fonts::{Fallback, Font};

    /// A system's fonts that have `Sys Face`, which the layout's fonts (no fallback) do not.
    struct System;
    impl Fallback for System {
        fn make(&self, _face: &str, _size: i32) -> Option<Font> {
            None
        }
        fn resolves(&self, face: &str) -> bool {
            face.eq_ignore_ascii_case("Sys Face")
        }
        fn name(&self) -> &'static str {
            "system"
        }
    }

    let (mut vm, gui, _) = rig();
    assert!(canvas::create_canvas(&mut vm, &gui).unwrap());
    vm.eval(b"new GuiControl(Top) { extent = \"200 100\"; }; Canvas.setContent(Top);")
        .unwrap();
    // The layout's fonts do not have it, so the registration is accepted.
    assert!(register_font(&mut vm, "Sys Face", "mods/fonts/a.ttf"));
    let own = || {
        let cache = FaceCache::new(
            "fonts",
            Box::new(System),
            Box::new(torque_fonts::uft::Uft::default()),
            std::convert::identity,
        );
        DrawCache::new(Box::new(cache))
    };
    let has = |cache: &DrawCache| {
        cache
            .fonts
            .as_any()
            .downcast_ref::<FaceCache>()
            .expect("a face cache")
            .is_registered("sys face")
    };
    let (mut withheld, mut plain) = (own(), DrawCache::default());
    for _ in 0..2 {
        gui.frame(&mut vm);
        draw_canvas(&vm, &gui, &mut withheld);
        draw_canvas(&vm, &gui, &mut plain);
    }
    // The draw cache that has the face of its own keeps it; the other is given the registered one.
    assert!(!has(&withheld));
    assert!(has(&plain));
    assert_eq!(
        console(&mut vm)
            .matches("a font with this name already exists")
            .count(),
        1
    );
}
