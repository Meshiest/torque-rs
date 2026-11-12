//! The `rustc` wrapper the bundle's builds run under: it gives every crate a `-C metadata` that does not depend on the folder the
//! checkout is in.
//!
//! Cargo derives that value from the crate's package id, and for a path dependency outside the build's workspace root (the
//! extensions are their own workspaces and depend on the engine's crates by path) the id holds the absolute path. rustc hashes
//! it into the crate's identity, so every symbol name and the order of the code differs between two checkouts: the library is
//! the same program in another order. The wrapper replaces the value with a hash of the rest of the command line, with the
//! folders `--remap-path-prefix` names written as what they are remapped to and the hashes cargo put in file names removed.
//!
//! It is this program run by cargo (`RUSTC_WRAPPER`), told so by [`ENV`]: cargo runs `<wrapper> <rustc> <arguments>`.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use sha2::{Digest, Sha256};

/// Set (to anything) in the environment of the builds the tool runs, so that `main` acts as the wrapper.
pub const ENV: &str = "TORQUE_BUNDLE_RUSTC_WRAPPER";

/// The version of the rewrite below: part of the build folder's key (`Build::build_dir`), so a change to it never reuses crates
/// built under the old one. Raise it with any change to what [`rewrite`] produces.
pub const VERSION: u32 = 1;

/// The program to run the builds under: this one when it is `torque-bundle`, else (a test binary of this crate) the
/// `torque-bundle` built beside it or one folder up.
///
/// # Errors
/// When there is none.
pub fn program() -> Result<PathBuf, String> {
    let this = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let name = format!("torque-bundle{}", std::env::consts::EXE_SUFFIX);
    if this.file_name().is_some_and(|n| *n == *name) {
        return Ok(this);
    }
    this.ancestors()
        .skip(1)
        .take(2)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("cannot find {name} to run the builds under"))
}

/// Runs `rustc` (the first argument) with the rest, their `-C metadata` rewritten ([`rewrite`]).
pub fn run() -> ExitCode {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        eprintln!("torque-bundle: the rustc wrapper needs the compiler to run");
        return ExitCode::FAILURE;
    }
    let rustc = args.remove(0);
    let args = match expand(args) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("torque-bundle: {e}");
            return ExitCode::FAILURE;
        }
    };
    match Command::new(&rustc).args(rewrite(&args)).status() {
        // An exit code over 255 (Windows has them) does not fit `ExitCode`, and a death by signal has none: both are 1.
        Ok(status) => status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        Err(e) => {
            eprintln!("torque-bundle: cannot run {}: {e}", rustc.to_string_lossy());
            ExitCode::FAILURE
        }
    }
}

/// `args` with each `@file` argument replaced by the file's lines, as rustc reads it: one argument per line (a `\r\n` ends
/// one), no quoting. rustc expands only the command line's own arguments, so a line that starts with `@` would mean something
/// else once expanded here; that, a file that cannot be read as UTF-8 and an argument that is not Unicode after `@` are errors,
/// since rewriting a command line this does not read the way rustc does could leave a `-C metadata` in it unchanged.
///
/// # Errors
/// As above.
pub fn expand(args: Vec<OsString>) -> Result<Vec<OsString>, String> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        let Some(name) = arg.to_str().and_then(|a| a.strip_prefix('@')) else {
            if arg.as_encoded_bytes().first() == Some(&b'@') {
                return Err(format!(
                    "the argument file name {} is not Unicode",
                    arg.to_string_lossy()
                ));
            }
            out.push(arg);
            continue;
        };
        let text = std::fs::read_to_string(name)
            .map_err(|e| format!("cannot read the argument file {name}: {e}"))?;
        for line in text.lines() {
            if line.starts_with('@') {
                return Err(format!(
                    "{name}: a line that starts with @ is not expanded here"
                ));
            }
            out.push(OsString::from(line));
        }
    }
    Ok(out)
}

/// `args` with the value of `-C metadata=` replaced by a hash of the others, or as they are when there is none (a probe such as
/// `-vV`).
///
/// The arguments are read as text with anything that is not Unicode replaced (two arguments that differ only in such bytes hash
/// the same, which only a pair of units of one crate could notice), and the others are passed on as they were.
pub fn rewrite(original: &[OsString]) -> Vec<OsString> {
    let args: Vec<std::borrow::Cow<str>> =
        original.iter().map(|a| OsStr::to_string_lossy(a)).collect();
    let is_metadata = |at: usize| match &*args[at] {
        "-C" => args.get(at + 1).is_some_and(|v| v.starts_with("metadata=")),
        arg => arg.starts_with("-Cmetadata="),
    };
    let Some(at) = (0..args.len()).find(|&at| is_metadata(at)) else {
        return original.to_vec();
    };
    let split = &*args[at] == "-C";
    let value_at = if split { at + 1 } else { at };

    let mut remaps: Vec<(String, &str)> = args
        .iter()
        .filter_map(|arg| arg.strip_prefix("--remap-path-prefix="))
        .filter_map(|pair| pair.rsplit_once('='))
        .map(|(from, to)| (from.replace('\\', "/"), to))
        .collect();
    remaps.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));

    let mut hash = Sha256::new();
    for (i, arg) in args.iter().enumerate() {
        // The value being replaced, and the file name suffix cargo built from the same hash.
        let derived = i == value_at
            || arg.starts_with("extra-filename=")
            || arg.starts_with("-Cextra-filename=");
        if !derived {
            // The hash cargo put in a dependency's file name (`--extern name=<folder>/libname-<hash>.rlib`) is left out: it comes
            // from the dependency's own `-C metadata`, which for a path dependency outside the workspace holds the checkout's
            // folder, so keeping it would keep the difference. A dependency is still told apart by its name here, and a unit
            // by its package's source path, features and flags, all of which are in the arguments.
            hash.update(without_hashes(&remapped(arg, &remaps)));
            hash.update([0]);
        }
    }
    let digest = hash.finalize();
    let mut key = String::with_capacity(16);
    for byte in &digest[..8] {
        let _ = write!(key, "{byte:02x}");
    }
    let mut out = original.to_vec();
    out[value_at] = OsString::from(if split {
        format!("metadata={key}")
    } else {
        format!("-Cmetadata={key}")
    });
    out
}

/// `arg` with `\` as `/` and each folder of `remaps` (longest first) written as its neutral name.
fn remapped(arg: &str, remaps: &[(String, &str)]) -> String {
    let mut text = arg.replace('\\', "/");
    for (from, to) in remaps {
        if !from.is_empty() {
            text = text.replace(from.as_str(), to);
        }
    }
    text
}

/// `text` without the `-` and 16 lower-case hex digits cargo puts in the names of a crate's files and folders.
fn without_hashes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let hashed = bytes[i] == b'-'
            && bytes.get(i + 1..i + 17).is_some_and(|run| {
                run.iter()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
            })
            && bytes.get(i + 17).is_none_or(|b| !b.is_ascii_alphanumeric());
        if hashed {
            i += 17;
        } else {
            let len = text[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&text[i..i + len]);
            i += len;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn os(args: Vec<String>) -> Vec<OsString> {
        args.into_iter().map(OsString::from).collect()
    }

    fn args(root: &str, hash: &str, features: &str) -> Vec<OsString> {
        os(vec![
            "--crate-name".to_string(),
            "sdk".to_string(),
            format!("{root}\\crates\\sdk\\src\\lib.rs"),
            format!("--cfg=feature=\"{features}\""),
            "-C".to_string(),
            format!("metadata={hash}"),
            "-C".to_string(),
            format!("extra-filename=-{hash}"),
            format!("--remap-path-prefix={root}=/workspace"),
            "-L".to_string(),
            format!("dependency={root}\\target\\release\\deps"),
            "--extern".to_string(),
            format!("dep={root}\\target\\release\\deps\\libdep-{hash}.rlib"),
        ])
    }

    fn metadata(args: &[OsString]) -> String {
        strings(args)
            .iter()
            .find_map(|a| a.strip_prefix("metadata=").map(String::from))
            .unwrap()
    }

    #[test]
    fn two_checkouts_give_the_same_metadata() {
        let one = rewrite(&args("C:\\one", "d86e49337ae79186", "std"));
        let two = rewrite(&args("D:\\work\\two", "5ec62b49c0fd4289", "std"));
        assert_eq!(metadata(&one), metadata(&two));
        assert_ne!(metadata(&one), "d86e49337ae79186");
        assert_eq!(metadata(&one).len(), 16);
    }

    #[test]
    fn other_crates_and_other_features_give_other_metadata() {
        let base = rewrite(&args("C:\\one", "d86e49337ae79186", "std"));
        let features = rewrite(&args("C:\\one", "d86e49337ae79186", "alloc"));
        assert_ne!(metadata(&base), metadata(&features));
        let mut other = args("C:\\one", "d86e49337ae79186", "std");
        other[1] = OsString::from("other");
        assert_ne!(metadata(&base), metadata(&rewrite(&other)));
    }

    /// The hash in a dependency's file name is left out of the key, so the dependency's name is what tells units apart.
    #[test]
    fn units_that_differ_only_in_the_crates_they_name_get_other_metadata() {
        let with = |name: &str| {
            let mut list = args("C:\\one", "d86e49337ae79186", "std");
            list[12] = OsString::from(format!(
                "{name}=C:\\one\\target\\release\\deps\\lib{name}-d86e49337ae79186.rlib"
            ));
            metadata(&rewrite(&list))
        };
        assert_ne!(with("dep"), with("other"));
        assert_eq!(with("dep"), with("dep"));
    }

    #[test]
    fn only_the_metadata_value_changes() {
        let before = args("C:\\one", "d86e49337ae79186", "std");
        let after = rewrite(&before);
        let changed: Vec<usize> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        assert_eq!(changed, [5]);
        let joined = os(vec!["-Cmetadata=abc".to_string(), "x.rs".to_string()]);
        assert!(strings(&rewrite(&joined))[0].starts_with("-Cmetadata="));
        assert_ne!(rewrite(&joined)[0], joined[0]);
    }

    #[test]
    fn a_probe_has_no_metadata_and_is_left_alone() {
        let probe = os(vec!["-vV".to_string()]);
        assert_eq!(rewrite(&probe), probe);
    }

    #[test]
    fn file_name_hashes_are_removed_and_other_dashes_are_not() {
        assert_eq!(
            without_hashes("libdep-d86e49337ae79186.rlib"),
            "libdep.rlib"
        );
        assert_eq!(
            without_hashes("a-b-c-0123456789abcdef0"),
            "a-b-c-0123456789abcdef0"
        );
        assert_eq!(without_hashes("é-d86e49337ae79186"), "é");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "torque-bundle-wrapper-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A `-C metadata` inside an argument file is rewritten like one on the command line, and what is passed on holds no `@`.
    #[test]
    fn an_argument_file_is_expanded_before_the_metadata_is_rewritten() {
        let dir = scratch("expand");
        let listed = args("C:\\one", "d86e49337ae79186", "std");
        let file = dir.join("args.rsp");
        let mut text = strings(&listed).join("\r\n");
        text.push('\n');
        std::fs::write(&file, text).unwrap();

        let given = vec![
            OsString::from("--edition=2024"),
            OsString::from(format!("@{}", file.display())),
        ];
        let expanded = expand(given).unwrap();
        assert_eq!(expanded.len(), listed.len() + 1);
        assert!(
            expanded
                .iter()
                .all(|a| !a.to_string_lossy().starts_with('@'))
        );
        let rewritten = rewrite(&expanded);
        assert_ne!(metadata(&rewritten), "d86e49337ae79186");
        let mut inline = vec![OsString::from("--edition=2024")];
        inline.extend(listed);
        assert_eq!(metadata(&rewritten), metadata(&rewrite(&inline)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_argument_file_that_cannot_be_read_as_rustc_does_is_an_error() {
        let dir = scratch("refuse");
        let nested = dir.join("nested.rsp");
        std::fs::write(&nested, "-Cfoo\n@other\n").unwrap();
        let error = expand(vec![OsString::from(format!("@{}", nested.display()))]).unwrap_err();
        assert!(error.contains("starts with @"), "{error}");
        let missing = dir.join("missing.rsp");
        let error = expand(vec![OsString::from(format!("@{}", missing.display()))]).unwrap_err();
        assert!(error.contains("cannot read"), "{error}");
        let bytes = dir.join("bytes.rsp");
        std::fs::write(&bytes, [0xff, 0xfe, b'\n']).unwrap();
        assert!(expand(vec![OsString::from(format!("@{}", bytes.display()))]).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// An argument that is not Unicode is passed on as it was, and does not stop the rewrite.
    #[cfg(any(unix, windows))]
    #[test]
    fn an_argument_that_is_not_unicode_is_passed_on_unchanged() {
        #[cfg(unix)]
        let odd = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![b'x', 0xff, b'y'])
        };
        #[cfg(windows)]
        let odd = {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(&[u16::from(b'x'), 0xd800, u16::from(b'y')])
        };
        let mut list = args("C:\\one", "d86e49337ae79186", "std");
        list.push(odd.clone());
        let after = rewrite(&list);
        assert_eq!(after.last(), Some(&odd));
        assert_ne!(metadata(&after), "d86e49337ae79186");
        assert!(expand(vec![odd]).is_ok());
    }
}
