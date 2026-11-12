//! The game folder's assets are opened by the engine's loaders when a game is bundled: the mods demo opens with nothing
//! refused, a planted bad asset is named with its reason, and the opt-out and the limits.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use torque_bundle::select::Caps;
use torque_bundle::validate::{GameInfo, Limits, Mode, Refusal, Report};
use torque_bundle::{Options, bundle, manifest, precompile, select, validate, validate_game};

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("content-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A copy of the fixture game crate in `dir/crate`.
fn fixture(dir: &Path) -> PathBuf {
    let crate_dir = dir.join("crate");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture"),
        &crate_dir,
    );
    crate_dir
}

fn put(crate_dir: &Path, path: &str, bytes: &[u8]) {
    let file = crate_dir.join("game").join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
}

fn options(dir: &Path, crate_dir: PathBuf) -> Options {
    Options {
        crate_dir,
        out: dir.join("out"),
        target_dir: dir.join("target"),
        check: false,
        server: false,
        archive: false,
        max_inflated_bytes: torque_bundle::scan::MAX_INFLATED_BYTES,
        mode: validate_only(),
        limits: Limits::default(),
        web: false,
        scripts: None,
        lua: None,
        allow: Vec::new(),
    }
}

fn validate_only() -> Mode {
    Mode {
        validate: true,
        ..Mode::default()
    }
}

fn precompiling() -> Mode {
    Mode {
        precompile: true,
        ..validate_only()
    }
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        w.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn mods_demo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods-demo")
}

fn refused(report: &Report) -> Vec<(&str, &str)> {
    report
        .refused
        .iter()
        .map(|r| (r.path.as_str(), r.reason.as_str()))
        .collect()
}

#[test]
fn the_mods_demo_game_folder_opens_with_nothing_refused() {
    let report = validate_game(&mods_demo(), &Limits::default(), validate_only()).unwrap();
    assert_eq!(refused(&report), []);
    assert_eq!(report.opened.get("script"), Some(&5), "{report:?}");
    assert_eq!(report.opened.get("settings"), Some(&4), "{report:?}");
    // The Lua game mode belongs to the Lua extension; the engine's loaders do not own it.
    assert_eq!(report.unowned, 5);

    // Its scripts compile to files the engine loads.
    let compiled = validate_game(&mods_demo(), &Limits::default(), precompiling()).unwrap();
    assert_eq!(refused(&compiled), []);
    // Its entry scripts compile; the two scripts of its mods stay source.
    assert_eq!(
        (compiled.precompiled, compiled.not_precompiled.len()),
        (3, 2)
    );
}

#[test]
fn a_planted_bad_asset_is_reported_with_its_path_and_the_reason() {
    let dir = tmp("planted");
    let crate_dir = fixture(&dir);
    put(&crate_dir, "shapes/ok.cs", b"function ok() { return 1; }\n");
    put(&crate_dir, "shapes/bad.dts", b"this is not a shape");
    put(&crate_dir, "textures/bad.png", b"\x89PNG\r\n\x1a\nnope");
    put(&crate_dir, "scripts/bad.cs", b"function f( { return 1; }\n");
    put(
        &crate_dir,
        "packs/art.zip",
        &zip_of(&[("good.cs", b"$a = 1;\n"), ("bad.ter", b"short")]),
    );
    put(&crate_dir, "packs/broken.zip", b"PK\x03\x04 not really");

    let report = validate_game(&crate_dir, &Limits::default(), validate_only()).unwrap();
    let mut paths: Vec<&str> = report.refused.iter().map(|r| r.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "packs/art.zip/bad.ter",
            "packs/broken.zip",
            "scripts/bad.cs",
            "shapes/bad.dts",
            "textures/bad.png",
        ]
    );
    assert!(report.refused.iter().all(|r| !r.reason.is_empty()));
    assert!(report.opened.get("script").copied().unwrap_or(0) >= 2);

    // The command line names them and fails.
    let output = Command::new(env!("CARGO_BIN_EXE_torque-bundle"))
        .arg("--validate")
        .arg(&crate_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("shapes/bad.dts") && error.contains("5 files refused"),
        "{error}"
    );
}

#[test]
fn a_bundle_with_a_refused_asset_fails_before_anything_is_built_unless_validation_is_off() {
    let dir = tmp("refuse");
    let crate_dir = fixture(&dir);
    put(&crate_dir, "shapes/bad.dts", b"this is not a shape");

    let error = bundle(&options(&dir, crate_dir.clone())).unwrap_err();
    assert!(
        error.contains("shapes/bad.dts") && error.contains("nothing was built"),
        "{error}"
    );
    assert!(!dir.join("target").exists(), "a build ran");
    assert!(!dir.join("out").exists(), "something was written");

    // The opt-out ships the file as it is.
    let report = bundle(&Options {
        mode: Mode::default(),
        ..options(&dir, crate_dir)
    })
    .unwrap();
    assert_eq!(report.content, Report::default());
    let shipped = std::fs::read(report.folder.join("game/shapes/bad.dts")).unwrap();
    assert_eq!(shipped, b"this is not a shape");
}

#[test]
fn a_precompiled_bundle_ships_the_compiled_script_in_place_of_its_source() {
    let dir = tmp("precompile");
    let crate_dir = fixture(&dir);
    put(
        &crate_dir,
        "boot.cs",
        b"function boot() { return 1; }\n$Boot = boot();\n",
    );
    put(
        &crate_dir,
        "maps/arena.mis",
        b"new SimGroup(MissionGroup) { };\n",
    );
    put(&crate_dir, "notes.cs", b"// only a comment\n");

    let report = bundle(&Options {
        mode: precompiling(),
        ..options(&dir, crate_dir)
    })
    .unwrap();
    assert_eq!(report.content.precompiled, 1, "{:?}", report.content);
    let mut files = Vec::new();
    manifest::list(&report.folder, "", &mut files).unwrap();
    for shipped in [
        "game/boot.cs.dso",
        "game/maps/arena.mis",
        "game/notes.cs",
        "game/hello.txt",
    ] {
        assert!(files.iter().any(|f| f == shipped), "{shipped} in {files:?}");
    }
    assert!(!files.iter().any(|f| f == "game/boot.cs"), "{files:?}");
    assert!(manifest::verify(&report.folder).unwrap().is_empty());

    // What was shipped is a file the engine's loader takes.
    let dso = std::fs::read(report.folder.join("game/boot.cs.dso")).unwrap();
    precompile::ScriptFormat::Torque3d
        .check_compiled("boot.cs", &dso)
        .unwrap();
}

#[test]
fn a_compiled_file_already_beside_its_source_is_not_overwritten() {
    let dir = tmp("beside");
    let crate_dir = fixture(&dir);
    put(&crate_dir, "boot.cs", b"$a = 1;\n");
    put(&crate_dir, "boot.cs.dso", b"theirs");
    let selected = select::select(&crate_dir.join("game"), &[], Caps::default())
        .unwrap()
        .0;
    let mode = Mode {
        validate: false,
        ..precompiling()
    };
    let (files, report) =
        validate::process(&selected, &Limits::default(), mode, &GameInfo::default());
    assert_eq!(report.precompiled, 0);
    assert_eq!(report.not_precompiled.len(), 1, "{report:?}");
    assert!(report.not_precompiled[0].reason.contains("already"));
    let boot: Vec<_> = files
        .iter()
        .filter(|f| f.path.starts_with("boot"))
        .collect();
    assert_eq!(boot.len(), 2);
    assert_eq!(
        boot.iter().find(|f| f.path == "boot.cs.dso").unwrap().bytes,
        b"theirs"
    );
}

#[test]
fn a_mods_scripts_stay_source() {
    let dir = tmp("mods");
    let crate_dir = fixture(&dir);
    put(
        &crate_dir,
        "mods/soccer/server.cs",
        b"$a = 1;
",
    );
    put(
        &crate_dir,
        "extra/pack/mod.toml",
        b"name = \"pack\"
",
    );
    put(
        &crate_dir,
        "extra/pack/boot.cs",
        b"$b = 1;
",
    );
    put(
        &crate_dir,
        "main.cs",
        b"$c = 1;
",
    );
    let selected = select::select(&crate_dir.join("game"), &[], Caps::default())
        .unwrap()
        .0;
    let mode = Mode {
        validate: false,
        ..precompiling()
    };
    let (files, report) =
        validate::process(&selected, &Limits::default(), mode, &GameInfo::default());
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    for shipped in ["main.cs.dso", "mods/soccer/server.cs", "extra/pack/boot.cs"] {
        assert!(paths.contains(&shipped), "{shipped} in {paths:?}");
    }
    assert_eq!(report.precompiled, 1);
    assert_eq!(report.not_precompiled.len(), 2, "{report:?}");
}

#[test]
fn the_limits_hold_a_hostile_game_folder() {
    let dir = tmp("limits");
    let crate_dir = fixture(&dir);
    put(&crate_dir, "big.png", &[0u8; 4096]);
    put(&crate_dir, "a.cs", b"$a = 1;\n");
    put(&crate_dir, "b.cs", b"$b = 1;\n");

    let small = Limits {
        file_bytes: 1024,
        ..Limits::default()
    };
    let report = validate_game(&crate_dir, &small, validate_only()).unwrap();
    assert_eq!(refused(&report).len(), 1);
    assert!(
        report.refused[0].path == "big.png" && report.refused[0].reason.contains("over the limit"),
        "{report:?}"
    );

    let few = Limits {
        files: 2,
        ..Limits::default()
    };
    // The walk itself stops at the cap.
    let error = validate_game(&crate_dir, &few, validate_only()).unwrap_err();
    assert!(error.contains("more than 2 files and folders"), "{error}");

    let total = Limits {
        total_bytes: 4000,
        ..Limits::default()
    };
    let report = validate_game(&crate_dir, &total, validate_only()).unwrap();
    assert!(
        report.refused.iter().any(|r| r.reason.contains("in all")),
        "{report:?}"
    );

    // A zip whose entries inflate past the cap is refused where it passes it.
    put(
        &crate_dir,
        "packs/p.zip",
        &zip_of(&[("one.cs", &[b' '; 600]), ("two.cs", &[b' '; 600])]),
    );
    let inflated = Limits {
        inflated_bytes: 1000,
        ..Limits::default()
    };
    let report = validate_game(&crate_dir, &inflated, validate_only()).unwrap();
    assert!(
        report
            .refused
            .iter()
            .any(|r| r.path == "packs/p.zip/two.cs" && r.reason.contains("inflate")),
        "{report:?}"
    );
}

#[test]
fn a_zip_entry_named_to_escape_the_folder_is_never_opened() {
    let dir = tmp("escape");
    let crate_dir = fixture(&dir);
    put(
        &crate_dir,
        "packs/evil.zip",
        &zip_of(&[
            ("../outside.cs", b"function f( {"),
            ("..\\outside2.cs", b"function f( {"),
            ("C:/abs.cs", b"function f( {"),
            ("con.cs", b"function f( {"),
            ("fine.cs", b"$a = 1;\n"),
        ]),
    );
    let report = validate_game(&crate_dir, &Limits::default(), validate_only()).unwrap();
    // The engine does not serve those names, so they are not opened and not a refusal; the fine one is.
    assert_eq!(refused(&report), []);
    assert_eq!(report.opened.get("script"), Some(&1), "{report:?}");
    assert!(!dir.join("outside.cs").exists());
}

/// The bytes of `path` of the game folder, shipped by `process` under `mode`.
fn processed(crate_dir: &Path, mode: Mode, game: &GameInfo) -> (Vec<(String, Vec<u8>)>, Report) {
    let selected = select::select(&crate_dir.join("game"), &[], Caps::default())
        .unwrap()
        .0;
    let (files, report) = validate::process(&selected, &Limits::default(), mode, game);
    (
        files.into_iter().map(|f| (f.path, f.bytes)).collect(),
        report,
    )
}

#[test]
fn a_mods_folder_game_toml_names_keeps_its_scripts_as_source() {
    let dir = tmp("mods-folder");
    let crate_dir = fixture(&dir);
    put(&crate_dir, "game.toml", b"[mods]\nfolder = \"addons\"\n");
    put(&crate_dir, "addons/soccer/server.cs", b"$a = 1;\n");
    put(&crate_dir, "mods/other/server.cs", b"$b = 1;\n");
    let game = GameInfo::read(&crate_dir.join("game"));
    assert_eq!(game.mods_folder, "addons");
    let (files, _) = processed(&crate_dir, precompiling(), &game);
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert!(paths.contains(&"addons/soccer/server.cs"), "{paths:?}");
    assert!(paths.contains(&"mods/other/server.cs.dso"), "{paths:?}");
}

#[test]
fn a_manifest_in_any_case_makes_a_folder_a_mod() {
    let dir = tmp("manifest-case");
    let crate_dir = fixture(&dir);
    put(&crate_dir, "pack/Mod.Toml", b"name = \"pack\"\n");
    put(&crate_dir, "pack/boot.cs", b"$a = 1;\n");
    let (files, report) = processed(&crate_dir, precompiling(), &GameInfo::default());
    assert!(files.iter().any(|(p, _)| p == "pack/boot.cs"));
    assert_eq!(report.precompiled, 0);
}

#[test]
fn a_script_whose_compiled_path_is_too_long_stays_source_with_a_note() {
    let dir = tmp("long-path");
    let crate_dir = fixture(&dir);
    // Folders of 40 characters: the path is far past 511 bytes but each name is a name the engine serves.
    let long = format!("{}/x.cs", vec!["d".repeat(40); 13].join("/"));
    let short = "ok.cs";
    put(&crate_dir, &long, b"$a = 1;\n");
    put(&crate_dir, short, b"$a = 1;\n");
    let (files, report) = processed(&crate_dir, precompiling(), &GameInfo::default());
    assert!(files.iter().any(|(p, _)| p == &long), "kept as source");
    assert!(files.iter().any(|(p, _)| p == "ok.cs.dso"));
    assert_eq!(report.not_precompiled.len(), 1, "{report:?}");
    assert!(report.not_precompiled[0].reason.contains("compiled path"));
}

#[test]
fn paths_that_differ_only_in_case_are_refused_and_so_is_a_dso_name_that_collides() {
    let dir = tmp("folded");
    let crate_dir = fixture(&dir);
    put(&crate_dir, "one.cs", b"$a = 1;\n");
    let one = crate_dir.join("game/one.cs");
    // Two names of one file stand for two files that a case-sensitive disk would hold.
    let selected = vec![
        select::Selected {
            path: "Scripts/Main.cs".into(),
            source: one.clone(),
        },
        select::Selected {
            path: "scripts/main.cs".into(),
            source: one.clone(),
        },
    ];
    let (files, report) = validate::process(
        &selected,
        &Limits::default(),
        validate_only(),
        &GameInfo::default(),
    );
    assert_eq!(files.len(), 1);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(report.refused[0].path, "scripts/main.cs");
    assert!(report.refused[0].reason.contains("when case is ignored"));

    // A `.dso` of another case already beside the source: the compiled name would be the same file.
    let selected = vec![
        select::Selected {
            path: "a.cs".into(),
            source: one.clone(),
        },
        select::Selected {
            path: "A.CS.DSO".into(),
            source: one,
        },
    ];
    let (_, report) = validate::process(
        &selected,
        &Limits::default(),
        precompiling(),
        &GameInfo::default(),
    );
    assert_eq!(report.precompiled, 0);
    assert_eq!(report.not_precompiled.len(), 1, "{report:?}");
}

#[test]
fn the_walk_of_a_deep_or_wide_folder_stops_where_it_passes_its_caps() {
    let dir = tmp("walk");
    let root = dir.join("game");
    let mut deep = root.clone();
    for _ in 0..6 {
        deep = deep.join("d");
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("x.cs"), "$a = 1;\n").unwrap();
    let caps = Caps {
        depth: 4,
        entries: 100,
    };
    let error = select::select(&root, &[], caps).unwrap_err();
    assert!(error.contains("nested more than 4 deep"), "{error}");
    let roomy = Caps { depth: 6, ..caps };
    assert_eq!(select::select(&root, &[], roomy).unwrap().0.len(), 1);

    // Many files: the walk stops at the cap, not after listing them all.
    let wide = dir.join("wide");
    std::fs::create_dir_all(&wide).unwrap();
    for i in 0..20 {
        std::fs::write(wide.join(format!("f{i}.txt")), "x").unwrap();
    }
    let caps = Caps {
        depth: 4,
        entries: 10,
    };
    let error = select::select(&wide, &[], caps).unwrap_err();
    assert!(error.contains("more than 10 files and folders"), "{error}");
    // The limits carry the same caps to a bundle.
    let limits = Limits {
        files: 10,
        depth: 4,
        ..Limits::default()
    };
    assert_eq!(limits.walk(), caps);
}

#[test]
fn a_refusal_made_of_a_files_text_holds_no_control_character() {
    let dir = tmp("escapes");
    let crate_dir = fixture(&dir);
    put(&crate_dir, "bad.cs", b"function f( { \x1b]0;pwned\x07 }\n");
    let report = validate_game(&crate_dir, &Limits::default(), validate_only()).unwrap();
    assert_eq!(report.refused.len(), 1);
    let Refusal { path, reason } = &report.refused[0];
    assert!(!path.chars().chain(reason.chars()).any(char::is_control));
}
