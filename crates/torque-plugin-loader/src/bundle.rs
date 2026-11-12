//! A packaged game's own manifest at run time: the files it lists are checked at start and the ones that changed or are
//! missing are reported. **It approves nothing**: the manifest sits in the game's folder, which whoever can replace a file
//! there can also write, so a clean report proves nothing and the gate for an extension library is the set of hashes compiled
//! into the executable ([`NativeExtensionsPlugin::shipped`](crate::NativeExtensionsPlugin::shipped)).
//!
//! The manifest is `manifest.toml` in the bundle's folder, as `torque-bundle` writes it (`format = 1`, then one
//! `[[file]]` of `path`, `size` and `sha256` for each file). It is read as untrusted input: it is read within a cap,
//! every listed path must be a name the engine would serve (no `..`, no drive, no absolute path, no device name) and
//! stay below the folder, and a path listed twice, a bad hash or a path that breaks the rule is refused on its own and
//! never read ([`BundleManifest::refused`]).
//!
//! [`BundleManifest::verify`] reads each listed file without following a link (not in a folder on the way, not as the
//! file) and says which changed or are missing. Files in the folder that the manifest does not list are not reported: a
//! player's own mods and saves live beside the game. It reads every listed file once, bounded by [`MAX_BUNDLE_BYTES`].

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use torque_resource::plain::{PlainError, is_link, open_plain, read_plain};
use torque_resource::zips::entry_name_ok;

/// The manifest's file name in a bundle's folder.
pub const BUNDLE_MANIFEST_FILE: &str = "manifest.toml";

/// The format version this reads.
const FORMAT: i64 = 1;

/// The largest manifest read (bytes).
pub const MAX_BUNDLE_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// The most files a manifest may list; a longer one is refused whole.
pub const MAX_BUNDLE_FILES: usize = 100_000;

/// The most bytes the listed files may add up to; a manifest that lists more is refused whole (checking them reads them all).
pub const MAX_BUNDLE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

/// The longest path a manifest may list (bytes).
const MAX_PATH_BYTES: usize = 512;

/// One file the manifest lists.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Listed {
    size: u64,
    sha256: String,
}

/// What a bundle's manifest lists, read from its folder.
#[derive(Clone, Debug)]
pub struct BundleManifest {
    dir: PathBuf,
    files: BTreeMap<String, Listed>,
    refused: Vec<String>,
}

impl BundleManifest {
    /// Reads `<dir>/manifest.toml`: Err with the reason when it is missing, is not a plain file, is over
    /// [`MAX_BUNDLE_MANIFEST_BYTES`], does not parse, has another format, or lists more than [`MAX_BUNDLE_FILES`]
    /// files or [`MAX_BUNDLE_BYTES`] bytes.
    pub fn read(dir: &Path) -> Result<BundleManifest, String> {
        let path = dir.join(BUNDLE_MANIFEST_FILE);
        let bytes = read_plain(&path, MAX_BUNDLE_MANIFEST_BYTES, None)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let text =
            String::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8", path.display()))?;
        BundleManifest::parse(dir, &text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The manifest in `text` for the bundle at `dir`.
    pub fn parse(dir: &Path, text: &str) -> Result<BundleManifest, String> {
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| e.message().to_string())?;
        match table.get("format") {
            Some(toml::Value::Integer(n)) if *n == FORMAT => {}
            Some(toml::Value::Integer(n)) => {
                return Err(format!("format {n} (this reads {FORMAT})"));
            }
            _ => return Err("it has no `format`".to_string()),
        }
        let entries: &[toml::Value] = match table.get("file") {
            None => &[],
            Some(toml::Value::Array(files)) => files,
            Some(_) => return Err("`file` is not a list of tables".to_string()),
        };
        if entries.len() > MAX_BUNDLE_FILES {
            return Err(format!("it lists more than {MAX_BUNDLE_FILES} files"));
        }
        let mut files: BTreeMap<String, Listed> = BTreeMap::new();
        let mut refused = Vec::new();
        let mut doubled: Vec<String> = Vec::new();
        let mut total = 0u64;
        for value in entries {
            let (path, listed) = match parse_entry(value) {
                Ok(parsed) => parsed,
                Err(reason) => {
                    refused.push(reason);
                    continue;
                }
            };
            total = total.saturating_add(listed.size);
            if files.insert(path.clone(), listed).is_some() {
                doubled.push(path);
            }
        }
        if total > MAX_BUNDLE_BYTES {
            return Err(format!("it lists more than {MAX_BUNDLE_BYTES} bytes"));
        }
        // A path listed twice may carry two hashes: neither is trusted.
        for path in doubled {
            files.remove(&path);
            refused.push(format!("{path}: listed more than once"));
        }
        Ok(BundleManifest {
            dir: dir.to_path_buf(),
            files,
            refused,
        })
    }

    /// How many files it lists (not counting the refused).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.files.len()
    }

    /// The entries that were refused, each as a message: a path that is not a name below the folder, a bad size or hash,
    /// a path listed twice. They are never trusted and never checked.
    pub fn refused(&self) -> &[String] {
        &self.refused
    }

    /// The listed files that changed, are missing or cannot be read, each as a message naming the file; empty when they
    /// all match. Every file is read without following a link, at most its listed size, so a file that grew or was
    /// swapped for a link is a change, not a long read.
    pub fn verify(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (path, listed) in &self.files {
            match self.check(path, listed) {
                Ok(true) => {}
                Ok(false) => problems.push(format!(
                    "{path}: changed since the game was packaged (it does not match {BUNDLE_MANIFEST_FILE})"
                )),
                Err(PlainError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    problems.push(format!(
                        "{path}: missing (it is listed in {BUNDLE_MANIFEST_FILE})"
                    ));
                }
                Err(PlainError::NotPlain) => problems.push(format!(
                    "{path}: is a link or not a plain file (it is listed in {BUNDLE_MANIFEST_FILE})"
                )),
                Err(e) => problems.push(format!("{path}: cannot be checked: {e}")),
            }
        }
        problems
    }

    /// Whether the file at `path` below the folder has the listed size and hash.
    fn check(&self, path: &str, listed: &Listed) -> Result<bool, PlainError> {
        let mut at = self.dir.clone();
        for part in path.split('/') {
            at.push(part);
            if is_link(&std::fs::symlink_metadata(&at)?) {
                return Err(PlainError::NotPlain);
            }
        }
        let (file, len) = open_plain(&at)?;
        if len != listed.size {
            return Ok(false);
        }
        let mut hasher = Sha256::new();
        let read = std::io::copy(&mut file.take(listed.size.saturating_add(1)), &mut hasher)?;
        Ok(read == listed.size && crate::stage::hex(&hasher.finalize()) == listed.sha256)
    }
}

/// One `[[file]]` table: its path and what is listed for it, or why it is refused.
fn parse_entry(value: &toml::Value) -> Result<(String, Listed), String> {
    let table = value.as_table().ok_or("a `file` entry is not a table")?;
    let path = table
        .get("path")
        .and_then(toml::Value::as_str)
        .ok_or("a `file` entry has no `path`")?;
    let shown: String = path.chars().filter(|c| !c.is_control()).take(80).collect();
    if path.len() > MAX_PATH_BYTES || !entry_name_ok(path.as_bytes()) {
        return Err(format!("{shown}: not a name below the bundle's folder"));
    }
    let size = table
        .get("size")
        .and_then(toml::Value::as_integer)
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| format!("{shown}: no valid size"))?;
    let sha256 = table
        .get("sha256")
        .and_then(toml::Value::as_str)
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| format!("{shown}: no valid sha256"))?;
    Ok((
        path.to_string(),
        Listed {
            size,
            sha256: sha256.to_ascii_lowercase(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// A TOML basic string.
    fn quote(text: &str) -> String {
        let mut out = String::from('"');
        for c in text.chars() {
            match c {
                '"' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn file(path: &str, size: i64, sha: &str) -> String {
        format!(
            "\n[[file]]\npath = {}\nsize = {size}\nsha256 = {}\n",
            quote(path),
            quote(sha)
        )
    }

    fn parse(text: &str) -> Result<BundleManifest, String> {
        BundleManifest::parse(Path::new("bundle"), text)
    }

    #[test]
    fn only_a_name_below_the_folder_is_listed() {
        let mut text = String::from("format = 1\n");
        for ok in ["a.exe", "game/boot.cs", "plugins/lib.dll"] {
            text.push_str(&file(ok, 0, SHA));
        }
        for bad in [
            "../x",
            "a/../x",
            "./x",
            "/x",
            "C:/x",
            r"a\b",
            "a//b",
            "a/",
            "",
            "con",
            "a/nul.txt",
            "a\u{1b}b",
        ] {
            text.push_str(&file(bad, 0, SHA));
        }
        text.push_str(&file(&"a/".repeat(300), 0, SHA));
        let m = parse(&text).expect("the manifest parses");
        assert_eq!(m.len(), 3);
        assert_eq!(m.refused().len(), 13, "{:?}", m.refused());
        assert!(m.refused().iter().all(|r| !r.chars().any(char::is_control)));
    }

    #[test]
    fn a_bad_size_or_hash_is_refused_and_a_path_listed_twice_is_trusted_for_neither() {
        let mut text = String::from("format = 1\n");
        text.push_str(&file("short", 1, "abc"));
        text.push_str(&file("negative", -1, SHA));
        text.push_str(&file("twice", 1, SHA));
        text.push_str(&file("twice", 1, &"1".repeat(64)));
        text.push_str(&file("fine", 1, &SHA.to_ascii_uppercase()));
        text.push_str("\n[[file]]\npath = \"nosize\"\nsha256 = \"abc\"\n");
        let m = parse(&text).expect("the manifest parses");
        assert_eq!(m.len(), 1);
        assert_eq!(m.refused().len(), 4, "{:?}", m.refused());
        assert_eq!(
            m.files.get("fine").map(|l| l.sha256.as_str()),
            Some(SHA),
            "hashes are kept in lower case"
        );
        assert!(!m.files.contains_key("twice"));
    }

    #[test]
    fn a_manifest_of_another_format_or_past_a_cap_is_refused_whole() {
        assert!(parse("").expect_err("no format").contains("format"));
        assert!(
            parse("format = 2")
                .expect_err("another format")
                .contains("format 2")
        );
        assert!(parse("format = 1\nfile = 3").is_err());
        assert!(parse("not toml [").is_err());
        let mut text = String::from("format = 1\n");
        for n in 0..3 {
            text.push_str(&file(&format!("h{n}"), i64::MAX, SHA));
        }
        assert!(
            parse(&text)
                .expect_err("past the byte cap")
                .contains("bytes")
        );
        let mut many = String::from("format = 1\n");
        for n in 0..=MAX_BUNDLE_FILES {
            many.push_str(&format!(
                "[[file]]\npath = \"f{n}\"\nsize = 0\nsha256 = \"{SHA}\"\n"
            ));
        }
        assert!(
            parse(&many)
                .expect_err("past the file cap")
                .contains("files")
        );
    }
}
