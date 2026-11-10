//! What the demo's end-to-end tests share: the built Lua extension and a folder of each test's own.

#![allow(dead_code, reason = "each test file uses its own share of these")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use torque_example_mods_demo::{Lua, library_file_name, lua_library};

/// The build folder: the one `cargo test` was given, else the workspace's.
pub fn target_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("a build folder")
        .to_path_buf()
}

/// The extension's library: the file `TORQUE_LUA_LIBRARY` names, else built by cargo once (a build of its own workspace, as the
/// extension's own tests build it).
pub fn library() -> &'static Path {
    static LIBRARY: OnceLock<PathBuf> = OnceLock::new();
    LIBRARY.get_or_init(|| {
        if let Some(library) = lua_library() {
            return library;
        }
        let manifest =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/torque-plugin-lua/Cargo.toml");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--quiet", "--lib", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(target_dir())
            .status()
            .expect("cargo runs");
        assert!(status.success(), "the Lua extension builds");
        let library = target_dir()
            .join("debug")
            .join(library_file_name("torque_lua"));
        assert!(library.is_file(), "{} is built", library.display());
        library
    })
}

/// A fresh folder of the test `name`'s own.
pub fn work(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("mods-demo-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The Lua extension installed in `dir`.
pub fn lua(dir: &Path) -> Lua {
    Lua::install(library(), &dir.join("lua")).expect("the extension installs")
}
