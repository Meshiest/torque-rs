//! Snapshot tests of a script engine's native surface: [`Vm::native_listing`](crate::vm::Vm::native_listing) compared with a
//! checked-in text file.
//!
//! A host's test builds its engine the way the host does, passes the snapshot and the file to [`check_listing`], and fails
//! with the lines that differ when the surface changed. With `TORQUE_BLESS=1` in the environment the file is rewritten
//! instead, so a deliberate change is one run and one reviewed diff.

use std::collections::BTreeSet;
use std::path::Path;

/// The variable that makes [`check_listing`] rewrite the file.
pub const BLESS_VAR: &str = "TORQUE_BLESS";

/// The lines of `expected` and `actual` that the other lacks: `- line` for one only in `expected` (a native that is gone),
/// `+ line` for one only in `actual` (a new one or a changed one), the missing ones first. Line endings do not matter. Empty
/// when the snapshot files hold the same lines.
pub fn diff_listing(expected: &str, actual: &str) -> Vec<String> {
    let lines = |text: &'_ str| -> BTreeSet<String> {
        text.lines()
            .map(|line| line.trim_end_matches('\r').to_string())
            .filter(|line| !line.is_empty())
            .collect()
    };
    let (expected, actual) = (lines(expected), lines(actual));
    let missing = expected.difference(&actual).map(|line| format!("- {line}"));
    let added = actual.difference(&expected).map(|line| format!("+ {line}"));
    missing.chain(added).collect()
}

/// Compares `actual` with the snapshot in `file`.
///
/// # Panics
///
/// When the file cannot be read, or its lines differ from `actual`'s: the message names the file and every differing line.
/// With `TORQUE_BLESS=1` the file is written (its folder made) and nothing panics unless the write fails.
pub fn check_listing(file: &Path, actual: &str) {
    if std::env::var_os(BLESS_VAR).is_some_and(|v| v == "1") {
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder)
                .unwrap_or_else(|e| panic!("cannot make {}: {e}", folder.display()));
        }
        std::fs::write(file, actual)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", file.display()));
        return;
    }
    let expected = std::fs::read_to_string(file).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e} (run with {BLESS_VAR}=1 to write it)",
            file.display()
        )
    });
    if let Err(message) = compare_listing(&expected, actual, file) {
        panic!("{message}");
    }
}

/// The comparison [`check_listing`] makes: an error naming `file` and every differing line when `expected` and `actual`
/// differ.
pub fn compare_listing(expected: &str, actual: &str, file: &Path) -> Result<(), String> {
    let diff = diff_listing(expected, actual);
    if diff.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the natives differ from {} (run with {BLESS_VAR}=1 to rewrite it):
{}",
        file.display(),
        diff.join(
            "
"
        )
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{Native, Vm};

    fn vm() -> Vm {
        let mut vm = Vm::new();
        vm.register("zeta", 1, 1, "zeta();", |_, _| Native::Bool(true));
        vm.register("alpha", 2, 0, "alpha(a, b...);\nsecond \\ line", |_, _| {
            Native::Bool(true)
        });
        vm.register_in("Thing", "poke", 1, 3, "", |_, _| Native::Bool(true));
        vm
    }

    #[test]
    fn the_listing_has_one_sorted_line_per_native_with_counts_and_usage() {
        let listing = vm().native_listing();
        let lines: Vec<&str> = listing.lines().collect();
        assert!(lines.windows(2).all(|w| w[0] < w[1]), "sorted and unique");
        assert!(lines.contains(&r"alpha 2..* alpha(a, b...);\nsecond \\ line"));
        assert!(lines.contains(&"zeta 1..1 zeta();"));
        assert!(
            lines.contains(&"Thing::poke 1..3"),
            "no usage, no trailing space"
        );
        assert!(listing.ends_with('\n'));
    }

    #[test]
    fn a_dropped_or_changed_line_is_named() {
        let listing = vm().native_listing();
        let without: String = listing
            .lines()
            .filter(|line| !line.starts_with("zeta "))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(diff_listing(&listing, &listing), Vec::<String>::new());
        assert_eq!(
            diff_listing(&without, &listing),
            vec!["+ zeta 1..1 zeta();".to_string()]
        );
        assert_eq!(
            diff_listing(&listing, &without),
            vec!["- zeta 1..1 zeta();".to_string()]
        );
        let changed = listing.replace("zeta 1..1", "zeta 1..2");
        assert_eq!(diff_listing(&listing, &changed).len(), 2);
    }

    #[test]
    fn line_endings_do_not_matter() {
        let listing = vm().native_listing();
        assert!(diff_listing(&listing.replace('\n', "\r\n"), &listing).is_empty());
    }

    #[test]
    fn a_checked_in_file_missing_a_native_fails_naming_it() {
        let listing = vm().native_listing();
        let engine = listing.replace(
            "zeta 1..1 zeta();
",
            "",
        );
        let file = Path::new("tests/natives/engine.txt");
        let message = compare_listing(&listing, &engine, file).unwrap_err();
        assert!(message.contains("tests/natives/engine.txt"), "{message}");
        assert!(message.contains("- zeta 1..1 zeta();"), "{message}");
        assert!(compare_listing(&listing, &listing, file).is_ok());
    }
}
