mod common;

use std::process::Command;
use torque_ts_bytecode::DsoFile;
use torque_ts_bytecode::opcodes::Op;

/// The code word of `op` in Torque3D's numbering.
fn c(op: Op) -> u32 {
    common::t3d().opcodes.code(op)
}

/// The path of the built `dsotool` binary.
fn dsotool() -> &'static str {
    env!("CARGO_BIN_EXE_dsotool")
}

/// Writes a DSO whose code is `code` (no tables, no identifiers) to a temp file named after `label`.
fn write_dso(label: &str, code: Vec<u32>) -> std::path::PathBuf {
    let mut s = common::empty();
    s.code = code;
    let path = scratch_root().join(format!("dsotool-{label}-{}.dso", std::process::id()));
    std::fs::write(&path, common::encode(&s)).unwrap();
    path
}

#[test]
fn dsotool_info_shows_a_plus_when_the_problem_list_hit_its_cap() {
    use torque_ts_bytecode::decode::MAX_PROBLEMS;

    // Each `LoadImmedStr 999` is one problem (no such string offset); the last word is a Return.
    let many = |count: usize| {
        let mut code = Vec::new();
        for _ in 0..count {
            code.extend([c(Op::LoadImmedStr), 999]);
        }
        code.push(c(Op::Return));
        code
    };
    let tool = dsotool();
    let cases = [
        (0, "consistency problems: 0\n"),
        (1, "consistency problems: 1\n"),
        (MAX_PROBLEMS - 1, "consistency problems: 49\n"),
        (MAX_PROBLEMS, "consistency problems: 50+\n"),
        (MAX_PROBLEMS + 20, "consistency problems: 50+\n"),
    ];
    for (count, expected) in cases {
        let path = write_dso(&format!("cap-{count}"), many(count));
        let info = Command::new(tool).arg("info").arg(&path).output().unwrap();
        let _ = std::fs::remove_file(&path);
        let text = String::from_utf8_lossy(&info.stdout);
        assert!(text.contains(expected), "{count} problems:\n{text}");
        // Exactly the cap many problems are listed, never more.
        assert_eq!(
            text.lines().filter(|l| l.starts_with("  ")).count(),
            count.min(MAX_PROBLEMS),
            "{count} problems:\n{text}"
        );
        assert_eq!(info.status.success(), count == 0, "{count} problems");
    }
}

#[test]
fn dsotool_info_and_decode_work_on_a_file() {
    let mut s = common::empty();
    s.global_strings = b"x\0".to_vec();
    s.code = vec![c(Op::LoadImmedUint), 5, c(Op::Return)];
    let bytes = DsoFile::parse(&common::encode(&s), &common::t3d())
        .unwrap()
        .to_bytes(&common::t3d());
    let path = scratch_root().join(format!("dsotool-test-{}.dso", std::process::id()));
    std::fs::write(&path, bytes).unwrap();

    let tool = dsotool();
    let info = Command::new(tool).arg("info").arg(&path).output().unwrap();
    assert!(
        info.status.success(),
        "{}",
        String::from_utf8_lossy(&info.stderr)
    );
    let info = String::from_utf8_lossy(&info.stdout);
    assert!(info.contains("version: 50"), "{info}");
    assert!(info.contains("instructions: 2"), "{info}");

    let decoded = Command::new(tool)
        .arg("decode")
        .arg(&path)
        .output()
        .unwrap();
    assert!(decoded.status.success());
    let decoded = String::from_utf8_lossy(&decoded.stdout);
    assert!(decoded.contains("OP_LOADIMMED_UINT 5"), "{decoded}");
    assert!(decoded.contains("OP_RETURN"), "{decoded}");

    let usage = Command::new(tool).output().unwrap();
    assert_eq!(usage.status.code(), Some(2));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dsotool_text_prints_the_text_form_that_reads_back_to_the_file() {
    let mut s = common::empty();
    s.global_strings = b"x\0tg\0\0\0\0".to_vec();
    s.global_floats = vec![1.5];
    s.local_variables = vec![(b"(null)::f".to_vec(), vec![b"%a".to_vec()])];
    s.code = vec![c(Op::LoadImmedUint), 5, c(Op::Return)];
    s.line_breaks = vec![3, 0];
    let bytes = DsoFile::parse(&common::encode(&s), &common::t3d())
        .unwrap()
        .to_bytes(&common::t3d());
    let path = scratch_root().join(format!("dsotool-text-{}.dso", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    let out = Command::new(dsotool())
        .arg("text")
        .arg(&path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("dso 50\nglobal-strings 8\n"), "{text}");
    assert!(
        text.contains("local-variables 1\n  \"(null)::f\" \"%a\"\n"),
        "{text}"
    );
    assert!(text.contains("OP_LOADIMMED_UINT 5"), "{text}");
    assert_eq!(
        torque_ts_bytecode::dsotext::parse(&text)
            .unwrap()
            .to_bytes(&common::t3d()),
        bytes
    );
    // Not a DSO: a failure, not a panic.
    let junk = scratch_root().join(format!("dsotool-junk-{}.dso", std::process::id()));
    std::fs::write(&junk, b"not a dso").unwrap();
    let out = Command::new(dsotool())
        .arg("text")
        .arg(&junk)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&junk);
    assert_eq!(out.status.code(), Some(1));
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
