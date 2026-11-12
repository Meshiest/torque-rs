//! Which files of a game folder ship: all of them but the [`DEFAULT_EXCLUDES`] and the game's own `exclude` patterns.
//!
//! A pattern without `/` matches a file or folder name anywhere (`*.bak`, `tests`); one with `/` matches the path from the game
//! folder (`dev/**`, `scripts/debug.cs`). `*` and `?` stay within a name, `**` crosses folders. Names compare without regard to
//! ASCII case, as Windows and macOS file systems do. Links are never followed: a link in a game folder is an error, since it
//! could pull in a file from anywhere. Every message that names an entry is made printable, since the names are the folder's.

use std::path::{Path, PathBuf};

use crate::precompile::printable;

/// Left out of every bundle: hidden files and folders, build and test folders, backups and editor files, logs, debug symbols,
/// source maps and the operating systems' folder files.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    ".*",
    "target",
    "tests",
    "*.bak",
    "*~",
    "*.tmp",
    "*.swp",
    "*.orig",
    "*.log",
    "*.pdb",
    "*.dSYM",
    "*.js.map",
    "*.wasm.map",
    "Thumbs.db",
    "desktop.ini",
];

/// A file to ship: its path in the game folder (`/` between folders) and on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    pub path: String,
    pub source: PathBuf,
}

/// How far and how wide the walk of a game folder goes: a hostile folder (a deep tree, a million files) is an error where it
/// passes these, not a walk without end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// The deepest folder walked (the game folder is 0).
    pub depth: usize,
    /// The most entries met, files and left-out files and folders together.
    pub entries: usize,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps {
            depth: 64,
            entries: torque_resource::zips::MAX_ENTRIES,
        }
    }
}

/// The files of `dir` to ship, sorted by path, and the paths left out. The walk stops with an error past `caps`.
pub fn select(
    dir: &Path,
    exclude: &[String],
    caps: Caps,
) -> Result<(Vec<Selected>, Vec<String>), String> {
    let patterns: Vec<&str> = DEFAULT_EXCLUDES
        .iter()
        .copied()
        .chain(exclude.iter().map(String::as_str))
        .collect();
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut walk = Walk {
        caps,
        met: 0,
        patterns: &patterns,
        files: &mut files,
        skipped: &mut skipped,
    };
    walk.dir(dir, "", 0)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    skipped.sort();
    Ok((files, skipped))
}

/// The state of one walk.
struct Walk<'a> {
    caps: Caps,
    met: usize,
    patterns: &'a [&'a str],
    files: &'a mut Vec<Selected>,
    skipped: &'a mut Vec<String>,
}

impl Walk<'_> {
    fn dir(&mut self, dir: &Path, prefix: &str, depth: usize) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| io_error(dir, &e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_error(dir, &e))?;
            self.met += 1;
            if self.met > self.caps.entries {
                return Err(format!(
                    "the game folder holds more than {} files and folders",
                    self.caps.entries
                ));
            }
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| {
                printable(&format!("{}: a file name is not UTF-8", dir.display()))
            })?;
            let path = format!("{prefix}{name}");
            let kind = entry
                .file_type()
                .map_err(|e| printable(&format!("{path}: {e}")))?;
            if is_excluded(&path, self.patterns) {
                self.skipped.push(path);
                continue;
            }
            if kind.is_symlink() {
                return Err(link_error(&path));
            }
            if kind.is_dir() {
                if depth >= self.caps.depth {
                    return Err(depth_error(&path, self.caps.depth));
                }
                self.dir(&entry.path(), &format!("{path}/"), depth + 1)?;
            } else if kind.is_file() {
                self.files.push(Selected {
                    path,
                    source: entry.path(),
                });
            }
        }
        Ok(())
    }
}

/// The error for a read of the folder `dir` that failed with `error`.
fn io_error(dir: &Path, error: &std::io::Error) -> String {
    printable(&format!("{}: {error}", dir.display()))
}

/// The error for a link at `path`.
fn link_error(path: &str) -> String {
    printable(&format!(
        "{path}: a link; a game folder ships only plain files"
    ))
}

/// The error for a folder at `path` nested past `depth`.
fn depth_error(path: &str, depth: usize) -> String {
    printable(&format!("{path}: folders nested more than {depth} deep"))
}

/// Whether `path` (from the game folder, `/` between folders) is left out by one of `patterns`.
pub fn is_excluded(path: &str, patterns: &[&str]) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    patterns.iter().any(|pattern| {
        if pattern.contains('/') {
            glob(pattern.as_bytes(), path.as_bytes())
        } else {
            glob(pattern.as_bytes(), name.as_bytes())
        }
    })
}

/// `*` (within a name), `?` (one character but `/`) and `**` (anything, across folders), ASCII case-insensitive.
pub fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern {
        [] => text.is_empty(),
        [b'*', b'*', rest @ ..] => match rest.strip_prefix(b"/") {
            // `**/`: none or whole folders.
            Some(rest) => (0..=text.len())
                .filter(|&at| at == 0 || text[at - 1] == b'/')
                .any(|at| glob(rest, &text[at..])),
            None => (0..=text.len()).any(|at| glob(rest, &text[at..])),
        },
        [b'*', rest @ ..] => {
            let end = text.iter().position(|&c| c == b'/').unwrap_or(text.len());
            (0..=end).any(|at| glob(rest, &text[at..]))
        }
        [b'?', rest @ ..] => matches!(text, [c, tail @ ..] if *c != b'/' && glob(rest, tail)),
        [p, rest @ ..] => {
            matches!(text, [c, tail @ ..] if p.eq_ignore_ascii_case(c) && glob(rest, tail))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name with an escape in it, as a hostile folder could hold on a file system that allows one.
    const HOSTILE: &str = "a\u{1b}]0;title\u{7}/b\u{9b}.cs";

    #[test]
    fn the_messages_that_name_an_entry_hold_no_control_character() {
        let messages = [
            link_error(HOSTILE),
            depth_error(HOSTILE, 64),
            io_error(
                Path::new(HOSTILE),
                &std::io::Error::other(format!("cannot read {HOSTILE}")),
            ),
        ];
        for message in messages {
            assert!(message.contains("a?]0;title?/b?.cs"), "{message:?}");
            assert!(!message.chars().any(char::is_control), "{message:?}");
        }
    }

    /// The same through the walk, where the file system takes the name (not Windows, whose names hold no control character).
    #[cfg(unix)]
    #[test]
    fn a_link_or_a_deep_folder_named_with_an_escape_is_reported_without_it() {
        let dir = crate::test_dir("select-escape");
        std::fs::create_dir_all(dir.join("x\u{1b}[2J/y")).unwrap();
        let caps = Caps {
            depth: 0,
            ..Caps::default()
        };
        let error = select(&dir, &[], caps).unwrap_err();
        assert!(error.contains("x?[2J"), "{error:?}");
        assert!(!error.chars().any(char::is_control), "{error:?}");

        let linked = dir.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(&dir, linked.join("l\u{1b}[31m")).unwrap();
        let error = select(&linked, &[], Caps::default()).unwrap_err();
        assert!(error.contains("l?[31m"), "{error:?}");
        assert!(!error.chars().any(char::is_control), "{error:?}");
    }

    #[test]
    fn globs_stay_within_names_unless_doubled() {
        assert!(glob(b"*.bak", b"server.cs.bak"));
        assert!(!glob(b"*.cs", b"mods/a.cs"));
        assert!(glob(b"dev/**", b"dev/a/b.cs"));
        assert!(glob(b"**/x.cs", b"x.cs"));
        assert!(glob(b"**/x.cs", b"a/b/x.cs"));
        assert!(!glob(b"**/x.cs", b"a/bx.cs"));
        assert!(glob(b"a?c", b"abc"));
        assert!(!glob(b"a?c", b"a/c"));
        assert!(glob(b"THUMBS.DB", b"thumbs.db"));
    }

    #[test]
    fn the_defaults_leave_out_dev_files_and_keep_game_files() {
        let defaults: Vec<&str> = DEFAULT_EXCLUDES.to_vec();
        for out in [
            ".git",
            "mods/.DS_Store",
            "tests",
            "a/target",
            "server.cs.bak",
            "notes.txt~",
            "console.log",
            "game.pdb",
            "web/page.js.map",
            "Thumbs.db",
        ] {
            assert!(is_excluded(out, &defaults), "kept {out}");
        }
        for kept in [
            "server.cs",
            "game.toml",
            "prefs.cs",
            "maps/arena.map",
            "mods/soccer/lib/rules.lua",
            "shapes/test_cube.dts",
        ] {
            assert!(!is_excluded(kept, &defaults), "left out {kept}");
        }
    }

    #[test]
    fn a_games_own_patterns_match_names_or_paths() {
        let patterns = ["debug.cs", "dev/**"];
        assert!(is_excluded("scripts/debug.cs", &patterns));
        assert!(is_excluded("dev/tool.cs", &patterns));
        assert!(!is_excluded("scripts/dev/tool.cs", &patterns));
    }
}
