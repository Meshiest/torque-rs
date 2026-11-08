use crate::param::{ArgKind, ParamKind, native_shape_error};

#[test]
fn the_shape_rule_is_the_engines() {
    use ArgKind::{Optional, Required, Rest};
    let script = |kinds: &[ArgKind]| {
        kinds
            .iter()
            .map(|&k| ParamKind::Script(k))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        native_shape_error(&script(&[Required, Optional, Rest])),
        None
    );
    assert_eq!(
        native_shape_error(&[ParamKind::Injected, ParamKind::Script(Required)]),
        None
    );
    assert!(native_shape_error(&script(&[Optional, Required])).is_some());
    assert!(native_shape_error(&script(&[Rest, Required])).is_some());
    assert!(native_shape_error(&script(&[Rest, Rest])).is_some());
}

/// greets
#[crate::function(name = "greet")]
fn hello(_call: crate::Call, who: &str, loud: Option<bool>) -> String {
    let text = format!("hi {who}");
    if loud.unwrap_or(false) {
        text.to_uppercase()
    } else {
        text
    }
}

#[test]
fn the_attribute_declares_through_the_sdk() {
    let natives = crate::natives![hello];
    assert_eq!(natives.len(), 1);
    let decl = natives[0];
    assert_eq!(
        (decl.namespace, decl.name, decl.method),
        ("", "greet", false)
    );
    assert_eq!(decl.params, ["call", "who", "loud"]);
    assert_eq!(decl.description, "greets");
    let native = (decl.native)();
    assert_eq!(native.args(), [ArgKind::Required, ArgKind::Optional]);
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
