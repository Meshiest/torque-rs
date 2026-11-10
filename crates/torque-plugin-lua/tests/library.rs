//! The Lua extension built as its own library and loaded from a `plugins/` folder, as a game loads it: the same scripts
//! run as with it linked in.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{console, files, global, run, script, server};
use torque_engine::EngineBuilder;
use torque_mods::ModsPlugin;
use torque_plugin_loader::{
    ApprovalRequest, LoadReport, NativeExtensions, NativeExtensionsPlugin, Outcome,
};

/// The build folder: CARGO_TARGET_DIR's, else the crate's own.
fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
}

/// The platform's file name of the library `name`.
fn library_file_name(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{name}.dylib")
    } else {
        format!("lib{name}.so")
    }
}

/// The extension's library, built by cargo.
fn library() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--quiet", "--lib", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(target_dir())
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the extension builds");
    let library = target_dir()
        .join("debug")
        .join(library_file_name("torque_lua"));
    assert!(library.is_file(), "{} is built", library.display());
    library
}

#[test]
fn the_library_loads_from_a_plugins_folder_and_runs_lua() {
    let dir = target_dir()
        .join("lua-ext-tests")
        .join(format!("plugins-{}", std::process::id()));
    let user = dir.with_file_name(format!("user-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&user);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(library(), dir.join(library_file_name("torque_lua"))).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("torque_lua.toml"),
        dir.join("torque_lua.toml"),
    )
    .unwrap();
    let engine = EngineBuilder::new()
        .add_plugins((
            ModsPlugin::new(),
            NativeExtensions(
                NativeExtensionsPlugin::new(&dir)
                    .user_folder(&user)
                    .approver(|_: &ApprovalRequest| true),
            ),
        ))
        .enable_id("torque.native-extensions")
        .build()
        .expect("the engine builds");
    let report = engine.resource::<LoadReport>().expect("the loader ran");
    assert_eq!(
        report.get("torque.lua").map(|e| &e.outcome),
        Some(&Outcome::Loaded)
    );
    drop(report);
    let files = files(&[]);
    let mut vm = server(&engine, &files);
    torque_ts_vm::vm::Vm::eval(
        &mut vm,
        b"new ScriptObject(Thing) { health = 10; }; function tsDouble(%x) { return %x * 2; }",
    )
    .unwrap();
    for name in ["basics.lua", "sandbox.lua"] {
        vm.set_global("$Test::Result", "");
        assert_eq!(run(&mut vm, &files, name, &script(name)), Ok(true));
        let log = console(&mut vm);
        assert_eq!(global(&vm, "$Test::Result"), "ok", "{name}: {log}");
    }
    vm.set_step_limit(Some(100_000));
    assert_eq!(
        run(&mut vm, &files, "loop.lua", "while true do end"),
        Err(torque_ts_vm::vm::VmError::StepLimit)
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&user);
}
