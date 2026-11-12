use std::rc::Rc;

use torque_ts_vm::exec::classic::{self, ClassicExecutor};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use super::*;

const SCRIPT: &[u8] = b"function f(%a)\n{\n   return %a * 2 + 1;\n}\n$Done = f(20);\n";

const TORQUE3D: ScriptFormat = ScriptFormat::Torque3d;
const CLASSIC: ScriptFormat = ScriptFormat::Classic;

/// Runs `x.cs` of `files` in `vm` and gives what `f(3)` and `$Done` say, and the console's lines.
fn run(mut vm: Vm, files: MemFiles) -> (String, String) {
    vm.set_file_host(files);
    vm.exec_file(b"x.cs", false).unwrap();
    let value = vm.eval(b"return f(3) @ \"/\" @ $Done;").unwrap();
    let log: Vec<String> = vm
        .drain_log()
        .into_iter()
        .map(|line| String::from_utf8_lossy(&line.text).into_owned())
        .collect();
    (String::from_utf8(value).unwrap(), log.join("\n"))
}

fn classic_vm() -> Vm {
    let mut vm = Vm::with_executor(Rc::new(ClassicExecutor::new()));
    classic::install(&mut vm);
    vm
}

/// `x.cs` compiled as `format` and run beside the same script as source: the same answers, the compiled one loaded.
fn runs_as_its_source_does(format: ScriptFormat, vm: impl Fn() -> Vm) {
    let dso = format.compile("x.cs", SCRIPT).unwrap().unwrap();

    let from_source = MemFiles::new();
    from_source.insert("x.cs", SCRIPT);
    let (value, log) = run(vm(), from_source);
    assert_eq!(value, "7/41");
    assert!(log.contains("Executing x.cs."), "{log}");

    // The source is not shipped: the engine finds `x.cs.dso` for `exec("x.cs")` and loads it without compiling.
    let from_dso = MemFiles::new();
    from_dso.insert(&format!("x.cs{EXTENSION}"), &dso);
    let (value, log) = run(vm(), from_dso);
    assert_eq!(value, "7/41");
    assert!(log.contains("Loading compiled script x.cs."), "{log}");
    assert!(!log.contains("Executing x.cs."), "{log}");
}

#[test]
fn a_compiled_script_runs_in_place_of_its_source_and_does_the_same() {
    runs_as_its_source_does(TORQUE3D, Vm::new);
}

#[test]
fn a_game_on_the_classic_executor_gets_its_own_format() {
    runs_as_its_source_does(CLASSIC, classic_vm);
    // The two formats are not one another's: the engine refuses the other's file, and so does the check.
    let torque3d = TORQUE3D.compile("x.cs", SCRIPT).unwrap().unwrap();
    let classic = CLASSIC.compile("x.cs", SCRIPT).unwrap().unwrap();
    assert!(CLASSIC.check_compiled("x.cs", &torque3d).is_err());
    assert!(TORQUE3D.check_compiled("x.cs", &classic).is_err());
}

#[test]
fn a_compile_gives_the_same_bytes_every_time() {
    for format in [TORQUE3D, CLASSIC] {
        let first = format.compile("x.cs", SCRIPT).unwrap().unwrap();
        for _ in 0..3 {
            assert_eq!(format.compile("x.cs", SCRIPT).unwrap().unwrap(), first);
        }
    }
}

#[test]
fn a_script_with_a_syntax_error_is_not_compiled_and_says_where() {
    let error = TORQUE3D
        .compile("bad.cs", b"function f( { return 1; }\n")
        .unwrap_err();
    assert!(error.starts_with("does not compile: "), "{error}");
    assert!(error.contains("bad.cs"), "{error}");
    assert!(TORQUE3D.check_source("bad.cs", b"function f( {").is_err());
}

#[test]
fn a_script_of_only_comments_has_nothing_to_ship_compiled() {
    assert_eq!(
        TORQUE3D.compile("c.cs", b"// nothing here\n").unwrap(),
        None
    );
    assert_eq!(TORQUE3D.compile("empty.cs", b"").unwrap(), None);
}

#[test]
fn a_compiled_file_the_loader_does_not_take_is_refused_with_a_reason() {
    let dso = TORQUE3D.compile("x.cs", SCRIPT).unwrap().unwrap();
    assert!(TORQUE3D.check_compiled("x.cs", &dso).is_ok());
    for bad in [&b""[..], b"not a compiled file", &dso[..dso.len() / 2]] {
        assert!(TORQUE3D.check_compiled("x.cs", bad).is_err());
    }
    let mut other_version = dso.clone();
    other_version[..4].copy_from_slice(&1u32.to_le_bytes());
    let error = TORQUE3D.check_compiled("x.cs", &other_version).unwrap_err();
    assert!(error.contains("version"), "{error}");
}

#[test]
fn a_hostile_script_cannot_make_a_long_message() {
    let source = format!("function f( {{ {} }}\n", "(".repeat(100_000));
    let error = TORQUE3D.compile("deep.cs", source.as_bytes()).unwrap_err();
    assert!(
        error.len() < QUOTED_LINES * (QUOTED_CHARS + 4) + 64,
        "{} bytes",
        error.len()
    );
}

#[test]
fn a_path_whose_compiled_name_passes_what_the_engine_builds_is_kept_as_source() {
    let longest = "a".repeat(MAX_DSO_PATH - EXTENSION.len() - ".cs".len()) + ".cs";
    assert_eq!(longest.len() + EXTENSION.len(), MAX_DSO_PATH);
    assert!(TORQUE3D.compile(&longest, SCRIPT).unwrap().is_some());
    let too_long = format!("a{longest}");
    let error = TORQUE3D.compile(&too_long, SCRIPT).unwrap_err();
    assert!(error.contains("compiled path"), "{error}");
}

#[test]
fn a_mods_scripts_are_those_below_its_folder_or_a_folder_with_a_manifest() {
    let paths = ["packs/arena/mod.toml", "Other/Pack/MOD.TOML", "mod.toml"];
    let dirs = mod_dirs("mods", paths);
    for yes in [
        "mods/soccer/server.cs",
        "MODS/x.cs",
        "packs/arena/lib/a.cs",
        "Packs/Arena/a.cs",
        "other/pack/a.cs",
    ] {
        assert!(in_mod(yes, &dirs), "{yes}");
    }
    for no in [
        "server.cs",
        "modsy/a.cs",
        "mods.cs",
        "packs/arenas/a.cs",
        "packs/a.cs",
    ] {
        assert!(!in_mod(no, &dirs), "{no}");
    }
    // A manifest at the root of the game folder does not make the whole game a mod.
    assert!(!in_mod("main.cs", &dirs));
}

#[test]
fn the_mods_folder_is_the_one_game_toml_names() {
    assert_eq!(mods_folder(None), "mods");
    assert_eq!(mods_folder(Some(b"[game]\nname = \"x\"\n")), "mods");
    assert_eq!(mods_folder(Some(b"not toml at all [")), "mods");
    assert_eq!(
        mods_folder(Some(b"[mods]\nfolder = \"addons\"\n")),
        "addons"
    );
    let dirs = mod_dirs(
        &mods_folder(Some(b"[mods]\nfolder = \"Add\\\\Ons/\"\n")),
        [],
    );
    assert!(in_mod("add/ons/a/server.cs", &dirs), "{dirs:?}");
    assert!(!in_mod("mods/a/server.cs", &dirs));
}

#[test]
fn only_cs_and_gui_files_are_compiled() {
    for yes in ["a.cs", "mods/A.CS", "ui/menu.gui", "x/y.Gui"] {
        assert!(is_source(yes), "{yes}");
    }
    for no in ["a.mis", "a.dso", "a.lua", "cs", "a.cs.bak", "a.txt"] {
        assert!(!is_source(no), "{no}");
    }
}

#[test]
fn printable_replaces_every_control_character() {
    assert_eq!(printable("a\u{1b}[2Jb\u{7}\r\n\u{9b}c"), "a?[2Jb????c");
    assert_eq!(printable("plain ünïcode"), "plain ünïcode");
}
