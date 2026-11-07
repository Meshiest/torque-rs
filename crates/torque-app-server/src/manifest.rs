//! The list of a web build's files (`files.json`, beside its page): each file's path, size and SHA-256. A game built for the
//! web without its files inside it fetches them by this list, and takes a file only when its size and hash are the list's.
//!
//! **The format.** One JSON object; its `files` array holds an object per file, other keys are ignored:
//!
//! ```json
//! { "files": [ { "path": "game.pack", "size": 2203456, "sha256": "<64 lowercase hex digits>" } ] }
//! ```
//!
//! `path` is relative to the page: ASCII letters, digits, `.`, `_` and `-`, with `/` between folders (no empty, `.` or `..`
//! part). A path ending in `.pack` is a pack of files ([`crate::files::unpack`]); any other is one file of the game folder.
//!
//! **Caps.** The list is fetched from a server, so a page reads it as hostile: at most [`MAX_BYTES`] of text, [`MAX_FILES`]
//! files, [`MAX_TOTAL`] bytes in all and [`MAX_PATH`] bytes a path; a hash is exactly 64 lowercase hex digits; two paths may
//! not differ only in case.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The list's file name, beside the page.
pub const FILE_NAME: &str = "files.json";

/// The most bytes of text a list may have.
pub const MAX_BYTES: usize = 512 * 1024;
/// The most files a list may name.
pub const MAX_FILES: usize = 1024;
/// The most bytes the files of a list may have in all.
pub const MAX_TOTAL: u64 = 256 * 1024 * 1024;
/// The longest path a list may name, in bytes.
pub const MAX_PATH: usize = 200;

/// A web build's files (module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub files: Vec<ManifestFile>,
}

/// One file of a [`Manifest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    /// Its path relative to the page.
    pub path: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

impl Manifest {
    /// The list of `files` (paths and bytes), in their order (Err: one breaks a cap).
    pub fn of<'a>(
        files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<Manifest, String> {
        let manifest = Manifest {
            files: files
                .into_iter()
                .map(|(path, bytes)| ManifestFile {
                    path: path.to_string(),
                    size: bytes.len() as u64,
                    sha256: Sha256::digest(bytes).into(),
                })
                .collect(),
        };
        manifest.check()?;
        Ok(manifest)
    }

    /// The list in `text` (Err: not one, or it breaks a cap).
    pub fn parse(text: &str) -> Result<Manifest, String> {
        if text.len() > MAX_BYTES {
            return Err(format!("the list of files is over {MAX_BYTES} bytes"));
        }
        let value: Value = serde_json::from_str(text)
            .map_err(|e| format!("the list of files is not JSON: {e}"))?;
        let files = value
            .get("files")
            .and_then(Value::as_array)
            .ok_or("the list of files has no \"files\" array")?;
        if files.len() > MAX_FILES {
            return Err(format!("the list names over {MAX_FILES} files"));
        }
        let manifest = Manifest {
            files: files.iter().map(entry).collect::<Result<_, _>>()?,
        };
        manifest.check()?;
        Ok(manifest)
    }

    /// The list as text (module docs), one file a line.
    pub fn to_json(&self) -> String {
        let lines: Vec<String> = self
            .files
            .iter()
            .map(|f| {
                format!(
                    "    {{ \"path\": {}, \"size\": {}, \"sha256\": \"{}\" }}",
                    Value::from(f.path.as_str()),
                    f.size,
                    hex(&f.sha256)
                )
            })
            .collect();
        format!("{{\n  \"files\": [\n{}\n  ]\n}}\n", lines.join(",\n"))
    }

    /// The file `path`, in any case.
    pub fn file(&self, path: &str) -> Option<&ManifestFile> {
        self.files
            .iter()
            .find(|f| f.path.eq_ignore_ascii_case(path))
    }

    /// The bytes of every file.
    pub fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// The caps (module docs) on the whole list.
    fn check(&self) -> Result<(), String> {
        if self.files.len() > MAX_FILES {
            return Err(format!("the list names over {MAX_FILES} files"));
        }
        let mut total = 0u64;
        let mut seen = std::collections::HashSet::new();
        for f in &self.files {
            check_path(&f.path)?;
            if !seen.insert(f.path.to_ascii_lowercase()) {
                return Err(format!("the list names {} twice", f.path));
            }
            total = total.saturating_add(f.size);
            if total > MAX_TOTAL {
                return Err(format!("the files of the list are over {MAX_TOTAL} bytes"));
            }
        }
        Ok(())
    }
}

impl ManifestFile {
    /// Whether `bytes` are this file: its size and its hash (Err: the message to show).
    pub fn check(&self, bytes: &[u8]) -> Result<(), String> {
        self.check_size(bytes.len())?;
        self.check_hash(&Sha256::digest(bytes).into())
    }

    /// Whether a file of `len` bytes whose SHA-256 the caller computed, `digest`, is this file (Err: the message to show,
    /// also when `digest` is not 32 bytes). The caller vouches for the digest: it must come from hashing the very bytes it
    /// hands over, never from a server or a cache.
    pub fn check_digest(&self, len: usize, digest: &[u8]) -> Result<(), String> {
        self.check_size(len)?;
        let digest: &[u8; 32] = digest.try_into().map_err(|_| {
            format!(
                "{}: a SHA-256 digest has 32 bytes, not {}",
                self.path,
                digest.len()
            )
        })?;
        self.check_hash(digest)
    }

    fn check_size(&self, len: usize) -> Result<(), String> {
        if len as u64 != self.size {
            return Err(format!(
                "{} has {} bytes where the list of files says {}",
                self.path, len, self.size
            ));
        }
        Ok(())
    }

    fn check_hash(&self, got: &[u8; 32]) -> Result<(), String> {
        if *got != self.sha256 {
            return Err(format!(
                "{} is not the file the list names: its SHA-256 is {} where the list says {}",
                self.path,
                hex(got),
                hex(&self.sha256)
            ));
        }
        Ok(())
    }

    /// Its hash as 64 lowercase hex digits.
    pub fn sha256_hex(&self) -> String {
        hex(&self.sha256)
    }
}

/// One file of the list's `files` array.
fn entry(value: &Value) -> Result<ManifestFile, String> {
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .ok_or("a file of the list has no \"path\" string")?;
    check_path(path)?;
    let size = value
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{path}: no \"size\" (a whole number of bytes)"))?;
    if size > MAX_TOTAL {
        return Err(format!("{path}: over {MAX_TOTAL} bytes"));
    }
    let hash = value
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path}: no \"sha256\" string"))?;
    let sha256 = unhex(hash)
        .ok_or_else(|| format!("{path}: its \"sha256\" is not 64 lowercase hex digits"))?;
    Ok(ManifestFile {
        path: path.to_string(),
        size,
        sha256,
    })
}

/// A path the list may name (module docs).
fn check_path(path: &str) -> Result<(), String> {
    let shown: String = path.chars().take(MAX_PATH).collect();
    if path.is_empty() || path.len() > MAX_PATH {
        return Err(format!(
            "the list names a path of {} bytes: {shown:?}",
            path.len()
        ));
    }
    let fine = path.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    });
    if fine {
        Ok(())
    } else {
        Err(format!(
            "the list names the path {shown:?}, which a page may not fetch"
        ))
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 64 lowercase hex digits as 32 bytes.
fn unhex(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let digit = |d: u8| match d {
        b'0'..=b'9' => Some(d - b'0'),
        b'a'..=b'f' => Some(d - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (i, pair) in digits.as_chunks::<2>().0.iter().enumerate() {
        out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn list(files: &str) -> String {
        format!("{{\"files\": [{files}]}}")
    }

    #[test]
    fn a_list_gives_back_its_files_and_checks_them() {
        let made = Manifest::of([("game.pack", &b"abc"[..]), ("kit/a.png", &[][..])]).unwrap();
        assert_eq!(
            made.files[0].sha256_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(made.files[1].sha256_hex(), EMPTY_SHA256);
        let back = Manifest::parse(&made.to_json()).unwrap();
        assert_eq!(back, made);
        assert_eq!(back.total(), 3);
        let pack = back.file("GAME.pack").unwrap();
        assert!(pack.check(b"abc").is_ok());
        let wrong = pack.check(b"abd").unwrap_err();
        assert!(wrong.contains("not the file the list names"), "{wrong}");
        assert!(pack.check(b"abcd").unwrap_err().contains("4 bytes"));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let text = format!(
            "{{\"made\": 1, \"files\": [{{\"path\": \"a\", \"size\": 0, \"sha256\": \"{EMPTY_SHA256}\", \"x\": []}}]}}"
        );
        assert_eq!(Manifest::parse(&text).unwrap().files.len(), 1);
    }

    #[test]
    fn hostile_lists_are_refused() {
        let file = |path: &str, size: &str, hash: &str| {
            format!("{{\"path\": {path}, \"size\": {size}, \"sha256\": {hash}}}")
        };
        let good = format!("\"{EMPTY_SHA256}\"");
        let refused = [
            "not json".to_string(),
            "[]".to_string(),
            "{\"files\": {}}".to_string(),
            list(&file("\"../secret\"", "0", &good)),
            list(&file("\"/abs\"", "0", &good)),
            list(&file("\"a//b\"", "0", &good)),
            list(&file("\"a/./b\"", "0", &good)),
            list(&file("\"c:\\\\x\"", "0", &good)),
            list(&file("\"a b\"", "0", &good)),
            list(&file("\"\"", "0", &good)),
            list(&file(
                &format!("\"{}\"", "a".repeat(MAX_PATH + 1)),
                "0",
                &good,
            )),
            list(&file("1", "0", &good)),
            list(&file("\"a\"", "-1", &good)),
            list(&file("\"a\"", "1.5", &good)),
            list(&file("\"a\"", &(MAX_TOTAL + 1).to_string(), &good)),
            list(&file(
                "\"a\"",
                "0",
                &format!("\"{}\"", EMPTY_SHA256.to_uppercase()),
            )),
            list(&file("\"a\"", "0", &format!("\"{}\"", &EMPTY_SHA256[1..]))),
            list(&file("\"a\"", "0", &format!("\"{EMPTY_SHA256}0\""))),
            list(&file("\"a\"", "0", "\"zz\"")),
            list(&file("\"a\"", "0", "null")),
            list(&[file("\"a\"", "0", &good), file("\"A\"", "0", &good)].join(",")),
            list(
                &[
                    file("\"a\"", &MAX_TOTAL.to_string(), &good),
                    file("\"b\"", "1", &good),
                ]
                .join(","),
            ),
            list(&vec![file("\"a\"", "0", &good); MAX_FILES + 1].join(",")),
            format!("{}{}", list(""), " ".repeat(MAX_BYTES)),
        ];
        for text in &refused {
            let shown: String = text.chars().take(120).collect();
            assert!(Manifest::parse(text).is_err(), "took {shown}");
        }
        let many: Vec<String> = (0..MAX_FILES)
            .map(|i| file(&format!("\"f{i}\""), "0", &good))
            .collect();
        assert_eq!(
            Manifest::parse(&list(&many.join(","))).unwrap().files.len(),
            MAX_FILES
        );
    }

    #[test]
    fn a_list_made_of_hostile_files_is_refused() {
        assert!(Manifest::of([("../a", &b""[..])]).is_err());
        assert!(Manifest::of([("a", &b""[..]), ("A", &b""[..])]).is_err());
    }
}
