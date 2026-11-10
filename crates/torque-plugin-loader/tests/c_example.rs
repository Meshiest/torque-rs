//! The C example (examples/native-extension/c/sample.c) against the checked-in header: compiled when a C compiler is
//! found (`CC`, else `cc`, `clang` or `gcc`), built as a library, loaded from a plugins folder and called. With no
//! compiler, or one that cannot link a shared library here, the test says so and passes.

mod common;

use std::path::Path;
use std::process::Command;

use common::{library_file_name, loaded_engine, scratch, server_vm, workspace};

/// The first C compiler that runs.
fn compiler() -> Option<String> {
    let mut candidates: Vec<String> = std::env::var("CC").ok().into_iter().collect();
    candidates.extend(["cc", "clang", "gcc"].map(String::from));
    candidates.into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

fn run(command: &mut Command) -> Result<(), String> {
    let out = command.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[test]
fn the_c_example_loads_and_runs() {
    let Some(cc) = compiler() else {
        eprintln!("skipped: no C compiler (set CC, or put cc, clang or gcc on the PATH)");
        return;
    };
    let source = workspace().join("examples/native-extension/c/sample.c");
    let include = workspace().join("crates/torque-plugin-abi/include");
    let folder = scratch("c-example");

    // The source must compile cleanly against the header: a failure here is the example's or the header's.
    let object = folder.join("sample.o");
    run(Command::new(&cc)
        .args(["-std=c99", "-Wall", "-Wextra", "-Werror", "-pedantic", "-c"])
        .arg("-I")
        .arg(&include)
        .arg("-o")
        .arg(&object)
        .arg(&source))
    .unwrap_or_else(|e| panic!("{cc} does not compile the C example cleanly:\n{e}"));

    // Linking a shared library needs the platform's linker as well, which a bare compiler may lack.
    let library = folder.join(library_file_name("c_sample"));
    let mut link = Command::new(&cc);
    link.arg("-shared");
    if !cfg!(target_os = "windows") {
        link.arg("-fPIC");
    }
    link.arg("-I")
        .arg(&include)
        .arg("-o")
        .arg(&library)
        .arg(&source);
    if let Err(e) = run(&mut link) {
        eprintln!("skipped: {cc} cannot link a shared library here:\n{e}");
        return;
    }
    std::fs::write(
        folder.join("c_sample.toml"),
        "id = \"example.c-sample\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let engine = loaded_engine(Path::new(&folder));
    assert!(
        engine.is_plugin_added_id("example.c-sample"),
        "{:?}",
        engine.warnings()
    );
    let mut vm = server_vm(&engine);
    assert!(
        vm.native_listing()
            .contains("C::twice 2..2 C::twice(text) - the text twice\n")
    );
    assert_eq!(vm.eval(b"return C::twice(\"ab\");").unwrap(), b"abab");
}
