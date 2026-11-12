//! `manifest.toml`: what a bundle is and every file in it with its size and SHA-256, sorted by path. It holds no times and
//! no local paths, so the same inputs give the same bytes; [`verify`] checks a bundle folder against it.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use torque_bytes::hex;
use torque_resource::plain::{PlainError, is_link, read_plain};

/// The manifest's path in a bundle.
pub const FILE: &str = "manifest.toml";

/// The format this tool writes and reads.
pub const FORMAT: u32 = 1;

/// What the bundle is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub version: String,
    pub target: String,
    /// `compiled`: the game's plugins compiled into its binaries.
    pub mode: String,
    pub features: Vec<String>,
    pub rustc: String,
}

/// A file of the bundle.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

impl Entry {
    pub fn of(path: &str, bytes: &[u8]) -> Entry {
        Entry {
            path: path.to_string(),
            size: bytes.len() as u64,
            sha256: hex(&Sha256::digest(bytes)),
        }
    }
}

/// A TOML basic string.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The manifest's text: the header, then `entries` sorted by path.
pub fn write(header: &Header, entries: &[Entry]) -> String {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let features: Vec<String> = header.features.iter().map(|f| quote(f)).collect();
    let mut out = format!(
        "# The files of this bundle, each with its size and SHA-256 (`torque-bundle --verify <folder>` checks them).\n\
         format = {FORMAT}\nname = {}\nversion = {}\ntarget = {}\nmode = {}\nfeatures = [{}]\nrustc = {}\n",
        quote(&header.name),
        quote(&header.version),
        quote(&header.target),
        quote(&header.mode),
        features.join(", "),
        quote(&header.rustc),
    );
    for entry in sorted {
        out.push_str(&format!(
            "\n[[file]]\npath = {}\nsize = {}\nsha256 = {}\n",
            quote(&entry.path),
            entry.size,
            quote(&entry.sha256)
        ));
    }
    out
}

#[derive(Deserialize)]
struct Raw {
    format: u32,
    #[serde(default, rename = "file")]
    files: Vec<Entry>,
}

/// The entries of a manifest's text.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let raw: Raw = toml::from_str(text).map_err(|e| format!("{FILE}: {}", e.message()))?;
    if raw.format != FORMAT {
        return Err(format!(
            "{FILE}: format {} (this tool reads {FORMAT})",
            raw.format
        ));
    }
    Ok(raw.files)
}

/// The differences between the bundle folder `dir` and its manifest: files changed, missing or not listed. Empty when they
/// agree.
pub fn verify(dir: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(dir.join(FILE))
        .map_err(|e| format!("{}: {e}", dir.join(FILE).display()))?;
    let listed: BTreeMap<String, Entry> = parse(&text)?
        .into_iter()
        .map(|e| (e.path.clone(), e))
        .collect();
    let mut on_disk = Vec::new();
    list(dir, "", &mut on_disk)?;
    let mut problems = Vec::new();
    for path in &on_disk {
        if path == FILE {
            continue;
        }
        match listed.get(path) {
            None => problems.push(format!("{path}: not in the manifest")),
            // A link is never followed, in a folder on the way or as the file: a bundle from elsewhere could point one out of its
            // folder, or swap one in after the listing.
            Some(entry) => match read_below(dir, path, entry.size) {
                Ok(bytes) if Entry::of(path, &bytes) == *entry => {}
                Ok(_) | Err(PlainError::TooLarge { .. }) => {
                    problems.push(format!("{path}: changed"));
                }
                Err(PlainError::NotPlain) => problems.push(format!("{path}: is not a plain file")),
                Err(PlainError::Io(e)) => return Err(format!("{path}: {e}")),
            },
        }
    }
    let on_disk: HashSet<&String> = on_disk.iter().collect();
    for path in listed.keys() {
        if !on_disk.contains(path) {
            problems.push(format!("{path}: missing"));
        }
    }
    Ok(problems)
}

/// The file at `path` (`/` between its parts) below `dir`, at most `max` bytes: every part is looked at without following it, and
/// the file is opened without following a link ([`torque_resource::plain`]).
pub fn read_below(dir: &Path, path: &str, max: u64) -> Result<Vec<u8>, PlainError> {
    let mut at = dir.to_path_buf();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(PlainError::NotPlain);
        }
        at.push(part);
        if is_link(&std::fs::symlink_metadata(&at)?) {
            return Err(PlainError::NotPlain);
        }
    }
    read_plain(&at, max, None)
}

/// Every file under `dir`, `/` between folders.
pub fn list(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        let kind = entry.file_type().map_err(|e| format!("{path}: {e}"))?;
        if kind.is_dir() {
            list(&entry.path(), &format!("{path}/"), out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            name: "demo".into(),
            version: "0.1.0".into(),
            target: "x86_64-pc-windows-msvc".into(),
            mode: "compiled".into(),
            features: vec!["bundled".into()],
            rustc: "rustc 1.99.1 (58cf8f9c3 2026-10-13)".into(),
        }
    }

    #[test]
    fn the_manifest_is_sorted_and_reads_back() {
        let entries = [Entry::of("game/b.cs", b"b"), Entry::of("a.exe", b"")];
        let text = write(&header(), &entries);
        assert!(text.find("a.exe").unwrap() < text.find("game/b.cs").unwrap());
        assert_eq!(
            entries[1].sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back, [entries[1].clone(), entries[0].clone()]);
        assert_eq!(write(&header(), &back), text);
    }

    #[test]
    fn odd_paths_are_quoted() {
        let text = write(&header(), &[Entry::of("game/a \"b\"\\c.cs", b"x")]);
        assert_eq!(parse(&text).unwrap()[0].path, "game/a \"b\"\\c.cs");
        assert!(parse(&text.replace("format = 1", "format = 2")).is_err());
    }
}
