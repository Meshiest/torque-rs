//! The `tscc` binary: a source file in, a DSO (or its text form) out, the exit code 1 after a syntax error.

use std::path::PathBuf;
use std::process::Command;

fn temp(name: &str) -> PathBuf {
    scratch_root().join(format!("tscc-test-{}-{name}", std::process::id()))
}

fn tscc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tscc"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn tscc_writes_the_dso_the_library_compiles() {
    let src = temp("a.cs");
    let dso = temp("a.dso");
    let source = b"function f(%a) { return %a + 1; }\n$x = f(2);\n";
    std::fs::write(&src, source).unwrap();
    let out = tscc(&[src.to_str().unwrap(), dso.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read(&dso).unwrap();
    let name = src.to_str().unwrap().as_bytes();
    let dialect = torque_ts_bytecode::Dialect::torque3d();
    let expected = torque_ts_compiler::compile(
        source,
        Some(name),
        torque_ts_compiler::Purpose::Run,
        &dialect,
    )
    .dso
    .unwrap();
    assert_eq!(written, expected.to_bytes(&dialect));
    // The file reads back as a DSO.
    assert_eq!(
        torque_ts_bytecode::DsoFile::parse(&written, &dialect).unwrap(),
        expected
    );

    // `--text` prints the text form and `--name` sets the name of the messages.
    let out = tscc(&["--text", "--name", "my.cs", src.to_str().unwrap()]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        text,
        torque_ts_bytecode::dsotext::render(&expected, &dialect)
    );
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dso);
}

#[test]
fn tscc_reports_messages_and_fails_on_a_syntax_error_or_no_code() {
    let src = temp("b.cs");
    // A statement before the error is still compiled (`Purpose::Run`), and the exit code says it failed.
    std::fs::write(&src, b"$a = 1;\n$b = = 2;\n").unwrap();
    let out = tscc(&["--text", "--name", "b.cs", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("Error: b.cs Line: 2 - Syntax error."), "{err}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("OP_SETCURVAR_CREATE \"$a\""));
    // Nothing to compile, a missing file, and no arguments.
    std::fs::write(&src, b"// nothing\n").unwrap();
    assert_eq!(
        tscc(&["--text", src.to_str().unwrap()]).status.code(),
        Some(1)
    );
    let _ = std::fs::remove_file(&src);
    assert_eq!(
        tscc(&["--text", src.to_str().unwrap()]).status.code(),
        Some(1)
    );
    assert_eq!(tscc(&[]).status.code(), Some(2));
}

/// A folder below the repository's git-ignored `.scratch/` (never the system Temp), made if it is missing.
fn scratch_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap_or(manifest);
    let dir = root.join(".scratch").join("test-tmp");
    let _ = std::fs::create_dir_all(&dir);
    dir
}
