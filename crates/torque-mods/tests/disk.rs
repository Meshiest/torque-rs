//! Mods on the player's disk: folders and zips of a folder outside the game's files, with links never followed.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::fs;
use std::path::PathBuf;

use common::{eval, files, server, zip_of};
use torque_engine::EngineBuilder;
use torque_mods::ModsPlugin;

/// A fresh folder below the test build folder (never the system's temp folder).
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("mods-disk")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch folder");
    dir
}

#[test]
fn folders_and_zips_on_the_disk_load_and_links_are_not_followed() {
    let root = scratch("root");
    let outside = scratch("outside");
    fs::write(outside.join("secret.txt"), b"outside the mod").expect("write");
    fs::create_dir_all(root.join("folk/art")).expect("mkdir");
    fs::write(
        root.join("folk/mod.toml"),
        b"id = \"folk\"\nversion = \"1\"\n",
    )
    .expect("write");
    fs::write(
        root.join("folk/server.cs"),
        b"function folk() { return 1; }",
    )
    .expect("write");
    fs::write(root.join("folk/art/a.txt"), b"inside").expect("write");
    // A link out of the mod, when the system lets this user make one.
    let linked = {
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(
            outside.join("secret.txt"),
            root.join("folk/art/link.txt"),
        );
        #[cfg(unix)]
        let made =
            std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("folk/art/link.txt"));
        made.is_ok()
    };
    let zip = zip_of(&[
        ("mod.toml", b"id = \"zipped\"\nversion = \"1\"\n"),
        ("server.cs", b"function zipped() { return 2; }"),
    ]);
    fs::write(root.join("zipped.zip"), zip).expect("write");

    let engine = EngineBuilder::new()
        .add_plugins(ModsPlugin::new().disk(&root))
        .build()
        .expect("builds");
    let mut vm = server(&engine, files(&[("server.cs", b"")]), "server.cs");
    assert_eq!(eval(&mut vm, "return getModList();"), "folk zipped");
    assert_eq!(eval(&mut vm, "return folk();"), "1");
    assert_eq!(eval(&mut vm, "return zipped();"), "2");
    assert_eq!(eval(&mut vm, "return isFile(\"art/a.txt\");"), "1");
    if linked {
        assert_eq!(
            eval(&mut vm, "return isFile(\"art/link.txt\");"),
            "0",
            "a link out of a mod is not followed"
        );
    }
}

#[test]
fn a_disk_folder_over_the_limits_is_refused() {
    let root = scratch("big");
    fs::create_dir_all(root.join("fat")).expect("mkdir");
    fs::write(
        root.join("fat/mod.toml"),
        b"id = \"fat\"\nversion = \"1\"\n",
    )
    .expect("write");
    for n in 0..5 {
        fs::write(root.join(format!("fat/f{n}")), b"x").expect("write");
    }
    let limits = torque_mods::ModLimits {
        files: 3,
        ..torque_mods::ModLimits::default()
    };
    let engine = EngineBuilder::new()
        .add_plugins(ModsPlugin::new().limits(limits).disk(&root))
        .build()
        .expect("builds");
    let mut vm = server(&engine, files(&[("server.cs", b"")]), "server.cs");
    assert_eq!(eval(&mut vm, "return getModList();"), "");
    assert!(
        eval(&mut vm, "return getModField(fat, reason);")
            .contains("files; a mod may have at most 3")
    );
}
