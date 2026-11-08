use crate::manifest::{Manifest, library_file_name};

#[test]
fn a_manifest_names_its_id_version_and_library() {
    let m = Manifest::parse("id = \"example.sample\"\nversion = \"1.2.0\"\n", "sample").unwrap();
    assert_eq!(
        (m.id.as_str(), m.version.as_str(), m.library.as_str()),
        ("example.sample", "1.2.0", "sample")
    );
    let m = Manifest::parse(
        "id = \"a\"\nversion = \"1\"\nlibrary = \"other_lib\"\n",
        "x",
    )
    .unwrap();
    assert_eq!(m.library, "other_lib");
    for bad in [
        "version = \"1\"",
        "id = \"a b\"\nversion = \"1\"",
        "id = \"a\"",
        "id = \"a\"\nversion = \"1 2\"",
        "id = \"a\"\nversion = \"1\"\nlibrary = \"../escape\"",
        "id = \"a\"\nversion = \"1\"\nlibrary = \"dir/lib\"",
        "id = \"a\"\nversion = \"1\"\nextra = 1",
        "id = 3\nversion = \"1\"",
        "not toml",
    ] {
        assert!(Manifest::parse(bad, "x").is_err(), "{bad:?} is refused");
    }
    assert!(library_file_name("sample").contains("sample"));
}

/// The `[lints]` tables, by line, below the headers that start with `prefix`.
fn lint_lines(manifest: &str, prefix: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut table = None;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            table = line
                .strip_prefix(prefix)
                .map(|t| t.trim_end_matches(']').to_string());
        } else if let Some(table) = &table
            && !line.is_empty()
            && !line.starts_with('#')
        {
            out.push((table.clone(), line.to_string()));
        }
    }
    out
}

#[test]
fn the_lints_are_the_workspaces() {
    let workspace = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"));
    let ours = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let mut expected = lint_lines(workspace, "[workspace.lints.");
    assert!(expected.len() > 10, "the workspace's lints were not found");
    for (_, line) in &mut expected {
        if line == "unsafe_code = \"forbid\"" {
            *line = "unsafe_code = \"deny\"".into();
        }
    }
    assert_eq!(lint_lines(ours, "[lints."), expected);
}

/// `unsafe` is allowed in one module, `ffi`, and every `unsafe` block there has a SAFETY comment.
#[test]
fn unsafe_code_is_only_in_ffi() {
    let allow = concat!("allow(", "unsafe_code)");
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let allows = text.matches(allow).count();
        assert_eq!(allows, usize::from(name == "ffi.rs"), "{name}");
        if name == "ffi.rs" {
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if line.contains(concat!("unsafe", " {")) && !line.trim_start().starts_with("//") {
                    let before = lines[..i]
                        .iter()
                        .rev()
                        .take(4)
                        .any(|l| l.contains("SAFETY:"));
                    assert!(
                        before,
                        "ffi.rs:{}: an unsafe block without a SAFETY comment",
                        i + 1
                    );
                }
            }
        }
    }
}
