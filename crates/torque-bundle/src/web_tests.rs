use std::path::PathBuf;

use super::*;
use crate::validate::{Limits, Mode};

fn options(validate: bool) -> Options {
    Options {
        crate_dir: PathBuf::new(),
        out: PathBuf::new(),
        target_dir: PathBuf::new(),
        check: false,
        server: false,
        archive: false,
        max_inflated_bytes: crate::scan::MAX_INFLATED_BYTES,
        mode: Mode {
            validate,
            ..Mode::default()
        },
        limits: Limits::default(),
        web: true,
        scripts: None,
        lua: None,
        allow: Vec::new(),
    }
}

fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
    torque_app_server::files::pack(files.iter().copied())
}

fn listed(path: &str, bytes: &[u8]) -> Listed {
    let entry = Entry::of(path, bytes);
    Listed {
        path: entry.path,
        size: entry.size,
        sha256: entry.sha256,
    }
}

#[test]
fn the_pack_is_the_engines() {
    assert_eq!(PACK_MAGIC, torque_app_server::files::PACK_MAGIC);
    let files: [(&str, &[u8]); 3] = [
        ("game.toml", b"[mods]\nfolder = \"mods\"\n"),
        ("scripts/a.cs", b"function f() {}\n"),
        ("empty.txt", b""),
    ];
    let bytes = pack(&files);
    assert_eq!(unpack(&bytes, &Limits::default()).unwrap(), files);
    assert_eq!(
        torque_app_server::files::unpack(&bytes).unwrap(),
        unpack(&bytes, &Limits::default()).unwrap()
    );
}

#[test]
fn a_hostile_pack_is_refused() {
    let limits = Limits::default();
    let good = pack(&[("a.txt", b"hello")]);
    assert!(unpack(&good, &limits).is_ok());
    assert!(unpack(b"ZIP!", &limits).is_err());
    assert!(unpack(&good[..good.len() - 1], &limits).is_err());
    let mut trailing = good.clone();
    trailing.push(0);
    assert!(unpack(&trailing, &limits).unwrap_err().contains("after"));
    // A count that no bytes could hold, and one past the caps.
    let mut many = PACK_MAGIC.to_vec();
    many.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(unpack(&many, &limits).is_err());
    let few = Limits { files: 1, ..limits };
    assert!(
        unpack(&pack(&[("a", b""), ("b", b"")]), &few)
            .unwrap_err()
            .contains("limit")
    );
    for name in ["../x", "/x", "a//b", "c:/x", "a\\b", "con", "x\u{1b}[2J"] {
        let bad = pack(&[(name, b"x")]);
        assert!(unpack(&bad, &limits).is_err(), "{name:?} was taken");
    }
}

#[test]
fn a_file_the_list_does_not_match_is_named() {
    let page = b"<html></html>".as_slice();
    let module = b"\0asm".as_slice();
    let list = [listed("index.html", page), listed("game_bg.wasm", module)];
    let entries = [
        Entry::of("index.html", page),
        Entry::of("game_bg.wasm", module),
    ];
    assert_eq!(list_matches(&list, &entries).unwrap(), 2);

    let changed = [Entry::of("index.html", b"changed"), entries[1].clone()];
    let error = list_matches(&list, &changed).unwrap_err();
    assert!(
        error.contains("index.html") && !error.contains("game_bg"),
        "{error}"
    );
    let missing = list_matches(&list, &entries[..1]).unwrap_err();
    assert!(
        missing.contains("game_bg.wasm: listed, but not in the bundle"),
        "{missing}"
    );
    let mut resized = listed("index.html", page);
    resized.size += 1;
    assert!(
        list_matches(&[resized], &entries)
            .unwrap_err()
            .contains("bytes")
    );
}

/// The lists the engine's page and this tool must read alike.
fn lists() -> Vec<Vec<u8>> {
    let hash = "ab".repeat(32);
    let file = |path: &str, size: &str, sha: &str| {
        format!("{{\"path\":\"{path}\",\"size\":{size},\"sha256\":\"{sha}\"}}")
    };
    let list = |files: &[String]| format!("{{\"files\":[{}]}}", files.join(","));
    let mut texts = vec![
        list(&[file("game.pack", "3", &hash)]),
        list(&[file("a/b/c.png", "0", &hash), file("x-y_z.0", "7", &hash)]),
        list(&[]),
        // other keys are ignored
        format!(
            "{{\"extra\":1,\"files\":[{{\"path\":\"a\",\"size\":1,\"sha256\":\"{hash}\",\"more\":[]}}]}}"
        ),
        list(&[file("A.pack", "1", &hash), file("a.PACK", "1", &hash)]),
        list(&[file("a", "1", &hash), file("A", "1", &hash)]),
        list(&[file("a", "1", &"AB".repeat(32))]),
        list(&[file("a", "1", "abc")]),
        list(&[file("a", "1.5", &hash)]),
        list(&[file("a", "-1", &hash)]),
        list(&[file("a", "268435457", &hash)]),
        list(&[file("a", "268435456", &hash)]),
        list(&[file("a", "268435456", &hash), file("b", "1", &hash)]),
        list(&[file(&"a".repeat(200), "1", &hash)]),
        list(&[file(&"a".repeat(201), "1", &hash)]),
        list(&[file("", "1", &hash)]),
        "{\"files\":[".to_string(),
        "[]".to_string(),
        "{\"files\":3}".to_string(),
        "{\"files\":[{\"size\":1}]}".to_string(),
        " ".repeat(MAX_LIST_BYTES + 1),
    ];
    for path in [
        "../x", "/x", "a//b", "a/", "./a", "a/./b", "a b", "a\\b", "c:x", "é", ".hidden",
        "a\u{1b}b",
    ] {
        texts.push(list(&[file(path, "1", &hash)]));
    }
    let many = |n: usize| {
        let files: Vec<String> = (0..n).map(|i| file(&format!("f{i}"), "1", &hash)).collect();
        list(&files)
    };
    texts.push(many(MAX_LISTED_FILES));
    texts.push(many(MAX_LISTED_FILES + 1));
    texts.into_iter().map(String::into_bytes).collect()
}

#[test]
fn the_list_is_the_engines() {
    use torque_app_server::manifest as engine;

    assert_eq!(MAX_LIST_BYTES, engine::MAX_BYTES);
    assert_eq!(MAX_LISTED_FILES, engine::MAX_FILES);
    assert_eq!(MAX_LISTED_TOTAL, engine::MAX_TOTAL);
    assert_eq!(MAX_LISTED_PATH, engine::MAX_PATH);
    assert_eq!(LIST, engine::FILE_NAME);
    for text in lists() {
        let ours = parse_list_text(&text);
        let theirs = std::str::from_utf8(&text)
            .map_err(|e| e.to_string())
            .and_then(engine::Manifest::parse);
        match (&ours, &theirs) {
            (Ok(ours), Ok(theirs)) => {
                let theirs: Vec<Listed> = theirs
                    .files
                    .iter()
                    .map(|f| Listed {
                        path: f.path.clone(),
                        size: f.size,
                        sha256: f.sha256_hex(),
                    })
                    .collect();
                assert_eq!(
                    *ours,
                    theirs,
                    "{}",
                    String::from_utf8_lossy(&text[..text.len().min(200)])
                );
            }
            (Err(_), Err(_)) => {}
            _ => panic!(
                "the engine says {theirs:?}, this tool {ours:?} of {}",
                String::from_utf8_lossy(&text[..text.len().min(200)])
            ),
        }
    }
}

#[test]
fn a_list_is_needed_and_holds_what_a_page_may_fetch() {
    assert!(parse_list(&[]).unwrap_err().contains("no files.json"));
    let hash = "ab".repeat(32);
    let good =
        format!("{{\"files\":[{{\"path\":\"game.pack\",\"size\":3,\"sha256\":\"{hash}\"}}]}}");
    let read = |text: &str| parse_list(&[(LIST.to_string(), text.as_bytes().to_vec())]);
    assert_eq!(read(&good).unwrap().len(), 1);
    assert!(read(&good.replace("game.pack", "../game.pack")).is_err());
    assert!(read(&good.replace(&hash, &hash.to_uppercase())).is_err());
}

fn game_pack(script: &[u8], png: &[u8]) -> Vec<u8> {
    pack(&[
        ("game.toml", b""),
        ("scripts/a.cs", script),
        ("art/x.png", png),
    ])
}

#[test]
fn the_files_of_a_pack_are_opened_by_their_loaders() {
    let script = b"function f() { return 1; }\n".as_slice();
    let good = pack(&[("game.toml", b""), ("scripts/a.cs", script)]);
    let written = [("game.pack".to_string(), good.clone())];
    let list = [listed("game.pack", &good)];
    let opened = open_listed(&list, &written, &options(true)).unwrap();
    assert_eq!((opened.pack_files, opened.other_files), (2, 0));

    let bad = game_pack(script, b"not a png");
    let written = [("game.pack".to_string(), bad.clone())];
    let list = [listed("game.pack", &bad)];
    let error = open_listed(&list, &written, &options(true)).unwrap_err();
    assert!(
        error.contains("art/x.png") && error.contains("game.pack"),
        "{error:?}"
    );
    // With validation off the pack is only unpacked.
    assert_eq!(
        open_listed(&list, &written, &options(false))
            .unwrap()
            .pack_files,
        3
    );

    // A script with a syntax error is refused like a game folder's.
    let broken = pack(&[("scripts/a.cs", b"function (")]);
    let written = [("game.pack".to_string(), broken.clone())];
    let list = [listed("game.pack", &broken)];
    assert!(open_listed(&list, &written, &options(true)).is_err());
}

#[test]
fn a_listed_pack_that_does_not_unpack_fails_in_any_case_of_its_name() {
    for name in ["game.pack", "game.PACK", "Game.Pack", "kit/x.pAcK"] {
        for validate in [true, false] {
            let written = [(name.to_string(), b"not a pack, not even TQPK".to_vec())];
            let list = [listed(name, &written[0].1)];
            let error = open_listed(&list, &written, &options(validate)).unwrap_err();
            assert!(error.contains("not a pack"), "{name}: {error}");
        }
    }
    // Magic without the rest is cut short, and a pack with a hostile name inside is refused.
    let written = [("a.pack".to_string(), PACK_MAGIC.to_vec())];
    assert!(open_listed(&[listed("a.pack", PACK_MAGIC)], &written, &options(false)).is_err());
    assert!(is_pack("a.PaCk") && !is_pack("a.pack.txt") && !is_pack("pack") && !is_pack(".pac"));
}

#[test]
fn a_listed_file_that_is_not_a_pack_is_opened_by_its_loader_too() {
    let script = b"function f() { return 1; }\n".as_slice();
    let written = [
        ("kit/a.cs".to_string(), script.to_vec()),
        ("kit/b.png".to_string(), b"not a png".to_vec()),
        ("notes.txt".to_string(), b"anything".to_vec()),
    ];
    let good = [listed("kit/a.cs", script), listed("notes.txt", b"anything")];
    let opened = open_listed(&good, &written, &options(true)).unwrap();
    assert_eq!((opened.pack_files, opened.other_files), (0, 2));

    let bad = [listed("kit/b.png", b"not a png")];
    let error = open_listed(&bad, &written, &options(true)).unwrap_err();
    assert!(error.contains("kit/b.png"), "{error:?}");
    assert!(open_listed(&bad, &written, &options(false)).is_ok());

    let missing = [listed("kit/c.png", b"")];
    let error = open_listed(&missing, &written, &options(true)).unwrap_err();
    assert!(error.contains("listed, but not in the bundle"), "{error}");
}

#[test]
fn two_files_of_one_path_or_name_or_the_manifest_are_refused() {
    let one = |p: &str| (p.to_string(), Vec::new());
    assert!(check_unique(&[one("a"), one("b")]).is_ok());
    assert!(check_unique(&[one("licenses/x"), one("Licenses/X")]).is_err());
    assert!(check_unique(&[one("manifest.toml")]).is_err());
    assert!(check_unique(&[one("Manifest.TOML")]).is_err());
}

#[test]
fn only_the_workspaces_web_folder_is_run_unless_scripts_are_given() {
    let dir = crate::test_dir("web-scripts");
    let root = dir.join("root");
    let krate = root.join("crates/game");
    for folder in ["web", "web/sub", "elsewhere", "crates/game"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    let web = |scripts: &str| Web {
        name: None,
        scripts: scripts.to_string(),
        example: "demo".to_string(),
        package: None,
        flags: Vec::new(),
        lua: None,
        check: None,
    };
    let find = |scripts: &str, given: Option<&std::path::Path>| {
        web_scripts(given, &krate, &web(scripts), &root)
    };
    assert!(find("../../web", None).is_ok());
    assert!(find("../../web/sub", None).is_ok());
    for outside in [
        "../../elsewhere",
        "..",
        "../../web/../elsewhere",
        "../../missing",
    ] {
        let error = find(outside, None).unwrap_err();
        assert!(error.contains("--scripts"), "{outside}: {error}");
    }
    // An explicit folder on the command line is taken as it is.
    let elsewhere = root.join("elsewhere");
    assert_eq!(
        find("../../elsewhere", Some(&elsewhere)).unwrap(),
        elsewhere
    );
}

#[test]
fn the_lua_extension_must_lie_in_the_workspace_unless_given_or_allowed() {
    let dir = crate::test_dir("web-lua");
    let root = dir.join("root");
    for folder in ["root/crates/game", "root/ext", "elsewhere"] {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
    }
    std::fs::write(root.join("ext/Cargo.toml"), "").unwrap();
    std::fs::write(dir.join("elsewhere/Cargo.toml"), "").unwrap();
    let krate = root.join("crates/game");
    let web = |lua: Option<&str>| Web {
        name: None,
        scripts: "../../web".to_string(),
        example: "demo".to_string(),
        package: None,
        flags: Vec::new(),
        lua: lua.map(String::from),
        check: None,
    };
    let find = |lua: Option<&str>, given: Option<&std::path::Path>, allow: &[PathBuf]| {
        web_lua(given, &krate, &web(lua), &root, allow)
    };
    assert_eq!(find(None, None, &[]).unwrap(), None);
    assert!(
        find(Some("../../ext/Cargo.toml"), None, &[])
            .unwrap()
            .is_some()
    );
    let outside = "../../../elsewhere/Cargo.toml";
    let error = find(Some(outside), None, &[]).unwrap_err();
    assert!(error.contains("--lua"), "{error}");
    assert!(find(Some("../../ext/missing.toml"), None, &[]).is_err());
    let elsewhere = dir.join("elsewhere");
    assert!(find(Some(outside), None, std::slice::from_ref(&elsewhere)).is_ok());
    let given = elsewhere.join("Cargo.toml");
    assert_eq!(find(Some(outside), Some(&given), &[]).unwrap(), Some(given));
}
