//! The crate-wide rule: no crate depends on the testkit outside `[dev-dependencies]`.

use std::path::{Path, PathBuf};

/// Every manifest of the workspace's members (`crates/*`, `examples/*`, `templates/*`, `tests/*`, `tests/web/*`), plus the
/// root's.
fn manifests(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![root.join("Cargo.toml")];
    for group in ["crates", "examples", "templates", "tests", "tests/web"] {
        let Ok(entries) = std::fs::read_dir(root.join(group)) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest = entry.path().join("Cargo.toml");
            if manifest.is_file() {
                found.push(manifest);
            }
        }
    }
    found
}

/// The lines of `text` that name the testkit, each with the section header above it.
fn testkit_lines(text: &str) -> Vec<(String, String)> {
    let mut section = String::new();
    let mut found = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.to_string();
            // The table form names the crate in the header: `[dependencies.torque-testkit]`.
            if line.contains("torque-testkit") || line.contains("torque_testkit") {
                found.push((section.clone(), line.to_string()));
            }
        } else if line.starts_with("torque-testkit")
            || line.starts_with("torque_testkit")
            || line.contains("package = \"torque-testkit\"")
        {
            found.push((section.clone(), line.to_string()));
        }
    }
    found
}

#[test]
fn no_crate_ships_the_testkit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifests = manifests(&root);
    assert!(manifests.len() > 10, "the workspace's manifests are found");
    for manifest in manifests {
        let text = std::fs::read_to_string(&manifest).expect("read a manifest");
        for (section, line) in testkit_lines(&text) {
            assert!(
                section.contains("dev-dependencies"),
                "{} lists the testkit under {section}: {line}",
                manifest.display()
            );
        }
    }
}

#[test]
fn the_checker_flags_a_normal_dependency() {
    let text = "[dependencies]\ntorque-testkit = { path = \"x\" }\n[dev-dependencies]\ntorque-testkit = { path = \"x\" }\n";
    let found = testkit_lines(text);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].0, "[dependencies]");
    assert_eq!(found[1].0, "[dev-dependencies]");
}

#[test]
fn the_checker_flags_the_table_form_and_a_renamed_dependency() {
    let text = "[dependencies.torque-testkit]
path = \"x\"
[dependencies]
kit = { package = \"torque-testkit\", path = \"x\" }
[dev-dependencies.torque-testkit]
path = \"x\"
";
    let found = testkit_lines(text);
    let normal: Vec<_> = found
        .iter()
        .filter(|(section, _)| !section.contains("dev-dependencies"))
        .collect();
    assert_eq!(normal.len(), 2, "{found:?}");
    assert_eq!(found.len(), 3);
}
