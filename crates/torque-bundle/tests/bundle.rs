//! Bundles the fixture game crate (`tests/fixture`: no dependencies, built in a second) from a copy outside the source tree:
//! what ships and what does not, the derived features, the check from the bundle folder, the same manifest twice from clean
//! builds, a manifest that catches changes, and a binary holding its build folder refused by the leak scan.

use std::path::{Path, PathBuf};

use torque_bundle::{Options, bundle, manifest};

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bundle-{name}"));
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

/// A copy of the fixture in `dir/crate`, with `edit` applied to its bundle.toml.
fn fixture(dir: &Path, edit: impl Fn(String) -> String) -> PathBuf {
    let crate_dir = dir.join("crate");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture"),
        &crate_dir,
    );
    let spec = crate_dir.join("bundle.toml");
    let text = std::fs::read_to_string(&spec).unwrap();
    std::fs::write(&spec, edit(text)).unwrap();
    crate_dir
}

fn options(dir: &Path, crate_dir: PathBuf, out: &str) -> Options {
    Options {
        crate_dir,
        out: dir.join(out),
        target_dir: dir.join("target"),
        check: true,
        server: false,
        archive: false,
    }
}

fn files_of(folder: &Path) -> Vec<String> {
    let mut files = Vec::new();
    manifest::list(folder, "", &mut files).unwrap();
    files
}

#[test]
fn the_fixture_bundles_runs_from_its_folder_and_bundles_the_same_twice() {
    let dir = tmp("twice");
    let crate_dir = fixture(&dir, |s| s);

    let first = bundle(&options(&dir, crate_dir.clone(), "one")).unwrap();
    let out = first.check.as_deref().unwrap();
    assert!(out.contains("game says hello") && out.contains("gizmos on"));
    assert!(!out.contains("dev build"), "the default features were on");
    assert!(
        out.contains("shipped plugins: []"),
        "the game's build is told what it ships in plugins/: {out}"
    );
    assert_eq!(first.features, ["gizmos"]);
    assert!(
        first.missing_licenses.is_empty(),
        "{:?}",
        first.missing_licenses
    );

    let files = files_of(&first.folder);
    let exe = format!("bundle-fixture{}", std::env::consts::EXE_SUFFIX);
    for shipped in [
        exe.as_str(),
        "game/game.toml",
        "game/hello.txt",
        "licenses/index.txt",
        "licenses/bundle-fixture-0.1.0/LICENSE",
        "manifest.toml",
    ] {
        assert!(
            files.iter().any(|f| f == shipped),
            "{shipped} not in {files:?}"
        );
    }
    assert!(
        files
            .iter()
            .all(|f| !f.contains("secret") && !f.ends_with(".bak"))
    );
    assert!(files.iter().any(|f| f.starts_with("licenses/rust-std/")));
    assert!(first.skipped.contains(&"hello.txt.bak".to_string()));

    // From a clean build folder: the same bytes.
    std::fs::remove_dir_all(dir.join("target")).unwrap();
    let second = bundle(&options(&dir, crate_dir, "two")).unwrap();
    let read = |f: &Path| std::fs::read_to_string(f.join(manifest::FILE)).unwrap();
    assert_eq!(read(&first.folder), read(&second.folder));
    assert!(manifest::verify(&second.folder).unwrap().is_empty());
}

#[test]
fn the_manifest_reports_a_changed_a_missing_and_an_extra_file() {
    let dir = tmp("verify");
    let crate_dir = fixture(&dir, |s| s);
    let report = bundle(&Options {
        check: false,
        ..options(&dir, crate_dir, "out")
    })
    .unwrap();
    let folder = &report.folder;
    std::fs::write(folder.join("game/hello.txt"), "changed\n").unwrap();
    std::fs::remove_file(folder.join("game/game.toml")).unwrap();
    std::fs::write(folder.join("game/extra.cs"), "").unwrap();
    let mut problems = manifest::verify(folder).unwrap();
    problems.sort();
    assert_eq!(
        problems,
        [
            "game/extra.cs: not in the manifest",
            "game/game.toml: missing",
            "game/hello.txt: changed",
        ]
    );
}

#[test]
fn a_binary_that_holds_its_build_folder_is_refused() {
    let dir = tmp("leak");
    let crate_dir = fixture(&dir, |s| {
        s.replace("exclude =", "features = [\"leak\"]\nexclude =")
    });
    let error = bundle(&options(&dir, crate_dir, "out")).unwrap_err();
    assert!(
        error.contains("bundle-fixture") && error.contains("holds the"),
        "{error}"
    );
    // Nothing reached the disk: no bundle folder (which --verify would pass) and no half-written one.
    let out = dir.join("out");
    let left: Vec<_> = std::fs::read_dir(&out)
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_dotted_bundle_name_keeps_its_work_folder_apart_from_a_sibling() {
    let dir = tmp("dotted");
    let crate_dir = fixture(&dir, |s| {
        s.replace("name = \"fixture\"", "name = \"fixture-1.2\"")
    });
    // What `with_extension("work")` would have made of `fixture-1.2-<os>-<arch>`: someone else's folder.
    let decoy = dir.join("out").join("fixture-1.work");
    std::fs::create_dir_all(&decoy).unwrap();
    std::fs::write(decoy.join("keep.txt"), "keep me").unwrap();

    let report = bundle(&options(&dir, crate_dir.clone(), "out")).unwrap();
    assert!(report.check.is_some());
    let name = report.folder.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("fixture-1.2-"), "{name}");
    let work = dir.join("out").join(format!("{name}.work"));
    assert!(work.join(torque_bundle::MARKER).is_file());
    assert_eq!(
        std::fs::read_to_string(decoy.join("keep.txt")).unwrap(),
        "keep me"
    );

    // A work folder this tool did not make is never emptied.
    std::fs::remove_file(work.join(torque_bundle::MARKER)).unwrap();
    std::fs::write(work.join("theirs.txt"), "theirs").unwrap();
    let error = bundle(&options(&dir, crate_dir, "out")).unwrap_err();
    assert!(error.contains("not a folder this tool made"), "{error}");
    assert!(work.join("theirs.txt").is_file());
    assert!(torque_bundle::own_folder(&decoy).is_err());
    assert!(decoy.join("keep.txt").is_file());
}

#[test]
fn a_folder_that_is_not_a_bundle_is_never_replaced() {
    let dir = tmp("keep");
    let crate_dir = fixture(&dir, |s| s);
    let folder = dir.join("out").join(torque_bundle::folder_name(
        "fixture",
        &torque_bundle::cargo::Toolchain::find().unwrap().host,
    ));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("mine.txt"), "keep me").unwrap();
    let error = bundle(&options(&dir, crate_dir, "out")).unwrap_err();
    assert!(error.contains("not a bundle folder"), "{error}");
    assert_eq!(
        std::fs::read_to_string(folder.join("mine.txt")).unwrap(),
        "keep me"
    );
}

#[test]
fn a_crate_with_no_text_gets_the_standard_texts_of_its_licenses() {
    let sysroot = torque_bundle::cargo::Toolchain::find().unwrap().sysroot;
    let texts =
        torque_bundle::licenses::generic_texts("(MIT OR Apache-2.0) AND Unicode-3.0", &sysroot);
    let names: Vec<&str> = texts.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "LICENSE-Apache-2.0.txt",
            "LICENSE-MIT.txt",
            "LICENSE-Unicode-3.0.txt"
        ]
    );
    assert!(torque_bundle::licenses::generic_texts("LicenseRef-Proprietary", &sysroot).is_empty());
}

fn archive_options(dir: &Path, crate_dir: PathBuf, out: &str, server: bool) -> Options {
    Options {
        check: false,
        server,
        archive: true,
        ..options(dir, crate_dir, out)
    }
}

#[test]
fn archives_from_two_clean_builds_have_the_same_sha256_and_unpack_to_the_bundle() {
    let dir = tmp("archive");
    let crate_dir = fixture(&dir, |s| s);

    let first = bundle(&archive_options(&dir, crate_dir.clone(), "one", false)).unwrap();
    std::fs::remove_dir_all(dir.join("target")).unwrap();
    let second = bundle(&archive_options(&dir, crate_dir, "two", false)).unwrap();
    let (a, b) = (first.archive.unwrap(), second.archive.unwrap());
    println!("archive sha256 {} and {}", a.sha256, b.sha256);
    assert_eq!(a.sha256, b.sha256);
    assert_eq!(
        std::fs::read(&a.path).unwrap(),
        std::fs::read(&b.path).unwrap()
    );
    assert_eq!(a.bytes, std::fs::metadata(&a.path).unwrap().len());

    // Every file of the bundle folder is in the archive, with the same bytes.
    let name = first.folder.file_name().unwrap().to_str().unwrap();
    let windows = cfg!(windows);
    assert_eq!(
        a.path.file_name().unwrap().to_str().unwrap(),
        format!("{name}.{}", if windows { "zip" } else { "tar.gz" })
    );
    if windows {
        let file = std::fs::File::open(&a.path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        for path in files_of(&first.folder) {
            let mut entry = zip.by_name(&format!("{name}/{path}")).unwrap();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            assert_eq!(bytes, std::fs::read(first.folder.join(&path)).unwrap());
        }
    }
}

#[test]
fn the_server_variant_is_its_own_bundle_with_its_own_features_and_check() {
    let dir = tmp("server");
    let crate_dir = fixture(&dir, |s| {
        format!(
            "{s}\n[server]\npackage = \"bundle-fixture\"\nbins = [\"bundle-fixture\"]\n\
             [server.check]\nbin = \"bundle-fixture\"\nargs = [\"{{work}}\"]\nexpect = [\"game says hello\"]\n"
        )
    });
    let report = bundle(&Options {
        check: true,
        ..archive_options(&dir, crate_dir.clone(), "out", true)
    })
    .unwrap();
    let name = report.folder.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("fixture-server-"), "{name}");
    // The game.toml of the fixture enables gizmos, which the server variant does not derive.
    assert!(report.features.is_empty(), "{:?}", report.features);
    assert!(!report.check.unwrap().contains("gizmos on"));
    assert_eq!(
        report.server.map(|s| (s.drawing, s.system_libraries)),
        Some((0, 0))
    );
    // `[server]` has its own exclusions (none here), not the bundle's.
    assert!(
        files_of(&report.folder)
            .iter()
            .any(|f| f == "game/secret/key.txt")
    );
    assert!(report.archive.is_some());

    // A crate without a `[server]` table has no server variant.
    let plain = fixture(&tmp("server-none"), |s| s);
    let error = bundle(&archive_options(&dir, plain, "none", true)).unwrap_err();
    assert!(error.contains("no [server] table"), "{error}");
}

/// What the dedicated server of the minimal example and the window game of the mods demo link, read from `cargo tree`: the
/// server none of the renderer, window and audio crates, the game some (so the check that refuses them is not blind).
#[test]
fn the_minimal_server_links_no_drawing_crates_and_the_mods_demo_does() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let host = torque_bundle::cargo::Toolchain::find().unwrap().host;
    let linked = |example: &str, package: &str| {
        torque_bundle::cargo::Build {
            manifest: workspace.join("examples").join(example).join("Cargo.toml"),
            package: Some(package.to_string()),
            bins: Vec::new(),
            features: Some(vec!["bundled".to_string()]),
            target: host.clone(),
            target_dir: PathBuf::from("unused"),
            rustflags: Vec::new(),
            cflags: Vec::new(),
            env: Vec::new(),
        }
        .linked()
        .unwrap()
    };
    let server = linked("minimal", "torque-example-minimal");
    let drawing = torque_bundle::server::drawing_crates(&server);
    println!(
        "minimal: {} crates linked, {} drawing",
        server.len(),
        drawing.len()
    );
    assert!(server.len() > 20 && drawing.is_empty(), "{drawing:?}");
    let game = linked("mods-demo", "torque-example-mods-demo");
    let drawing = torque_bundle::server::drawing_crates(&game);
    println!(
        "mods-demo: {} crates linked, {} drawing",
        game.len(),
        drawing.len()
    );
    assert!(
        drawing.iter().any(|c| c.starts_with("wgpu ")),
        "{drawing:?}"
    );
}
