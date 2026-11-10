//! A mod's content as servers and clients share it: which files count, their hash, and the messages that carry them.
//!
//! **What counts.** A mod's content is its files that could never run as script: no `.cs`, `.dso`, `.gui`, `.mis` or `.ed` in any
//! spelling ([`is_script_path`]), not its manifest and not the entry scripts its layout named. A server hashes and sends the
//! content of a mod; a client compares its own copy by the same rule, so a mod whose scripts differ but whose content is the same
//! counts as the same mod.
//!
//! **The hash** is SHA-256 over the content in a canonical order: the label `torque-mod-content-v1`, then for each file, ordered by
//! its path in ASCII lower case, the path's length (4 bytes, little endian), the path in lower case, the file's length (8 bytes,
//! little endian) and its bytes. Paths are compared without regard to case everywhere in a package, so two spellings of one path
//! hash the same.
//!
//! **The messages** are the offer a server sends (see [`crate::offer`]) and the content message that is also the cache's entry
//! (the mod's files and the hash they must have). Both are read from hostile bytes: every length is checked against the caps and
//! the bytes left before anything is allocated, a path must pass the package's rules, and a path that could be script is refused.

use sha2::{Digest, Sha256};

use crate::files::is_script_path;
use crate::package::{MAX_PATH_LEN, ModLimits, ModPackage};

/// The label the hash starts with.
const LABEL: &[u8] = b"torque-mod-content-v1";

/// The magic that starts every message.
pub(crate) const MAGIC: [u8; 4] = *b"TQM1";

const KIND_CONTENT: u8 = 2;

/// The size of a content hash.
pub type Hash = [u8; 32];

/// A mod's content: its hash, files and bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContentSummary {
    /// The hash ([module docs](self)).
    pub hash: Hash,
    /// How many files the content has.
    pub files: u32,
    /// The bytes of those files.
    pub bytes: u64,
}

/// The lower-case hexadecimal text of a hash.
pub fn hex(hash: &Hash) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(64);
    for byte in hash {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// The hash from its hexadecimal text (lower case, 64 digits).
pub fn from_hex(text: &str) -> Option<Hash> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut hash = [0u8; 32];
    let (pairs, _) = bytes.as_chunks::<2>();
    for (slot, pair) in hash.iter_mut().zip(pairs) {
        *slot = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(hash)
}

/// The paths of `package` that are content, spelled as the package spells them, in the order the hash takes them. `hidden` are
/// the files a layout keeps out of the game's files (its manifest and entry scripts).
pub(crate) fn content_names(package: &ModPackage, hidden: &[String]) -> Vec<String> {
    let mut names: Vec<String> = package
        .names()
        .iter()
        .filter(|name| !is_script_path(name))
        .filter(|name| !hidden.iter().any(|h| h.eq_ignore_ascii_case(name)))
        .cloned()
        .collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// The hasher of a content, fed the files in order ([`ContentHasher::file`]).
pub struct ContentHasher {
    hasher: Sha256,
    files: u32,
    bytes: u64,
}

impl Default for ContentHasher {
    fn default() -> ContentHasher {
        let mut hasher = Sha256::new();
        hasher.update(LABEL);
        ContentHasher {
            hasher,
            files: 0,
            bytes: 0,
        }
    }
}

impl ContentHasher {
    /// Adds a file. The files must come ordered by their paths in ASCII lower case.
    pub fn file(&mut self, path: &str, data: &[u8]) {
        let lower = path.to_ascii_lowercase();
        self.hasher.update((lower.len() as u32).to_le_bytes());
        self.hasher.update(lower.as_bytes());
        self.hasher.update((data.len() as u64).to_le_bytes());
        self.hasher.update(data);
        self.files = self.files.saturating_add(1);
        self.bytes = self.bytes.saturating_add(data.len() as u64);
    }

    /// The content's hash and totals.
    pub fn finish(self) -> ContentSummary {
        ContentSummary {
            hash: self.hasher.finalize().into(),
            files: self.files,
            bytes: self.bytes,
        }
    }
}

/// The content of `package` (`None` when a file cannot be read: a zip entry that no longer unpacks).
pub(crate) fn summarize(package: &ModPackage, hidden: &[String]) -> Option<ContentSummary> {
    let mut hasher = ContentHasher::default();
    for name in content_names(package, hidden) {
        hasher.file(&name, &package.read(&name)?);
    }
    Some(hasher.finish())
}

/// The content of `files` (any order).
pub fn summarize_files(files: &[(String, Vec<u8>)]) -> ContentSummary {
    let mut order: Vec<&(String, Vec<u8>)> = files.iter().collect();
    order.sort_by_key(|(name, _)| name.to_ascii_lowercase());
    let mut hasher = ContentHasher::default();
    for (name, data) in order {
        hasher.file(name, data);
    }
    hasher.finish()
}

// ---- messages

/// Why a message was refused.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MessageError {
    /// Not a message of this protocol, or cut short.
    Malformed(&'static str),
    /// A count or a size over what the receiver allows.
    TooLarge(&'static str),
    /// A path the receiver does not take.
    BadPath(String),
    /// The message names a file that could be script.
    Script(String),
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageError::Malformed(what) => write!(f, "the server sent a bad message ({what})"),
            MessageError::TooLarge(what) => write!(f, "the server sent too much ({what})"),
            MessageError::BadPath(path) => {
                write!(f, "the server sent a file path that is refused (`{path}`)")
            }
            MessageError::Script(path) => write!(
                f,
                "the server sent a script file (`{path}`); a server sends content, never scripts"
            ),
        }
    }
}

impl std::error::Error for MessageError {}

fn shown(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect()
}

pub(crate) fn put_text(out: &mut Vec<u8>, text: &str) {
    out.push(text.len().min(255) as u8);
    out.extend_from_slice(&text.as_bytes()[..text.len().min(255)]);
}

/// The content message of one mod: `files` are `(path, bytes)`, in any order.
pub fn encode_content(
    id: &str,
    version: &str,
    hash: &Hash,
    files: &[(String, Vec<u8>)],
) -> Vec<u8> {
    let size: usize = files.iter().map(|(p, d)| p.len() + d.len() + 6).sum();
    let mut out = Vec::with_capacity(size + 64);
    out.extend_from_slice(&MAGIC);
    out.push(KIND_CONTENT);
    put_text(&mut out, id);
    put_text(&mut out, version);
    out.extend_from_slice(hash);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (path, data) in files {
        out.extend_from_slice(&(path.len().min(MAX_PATH_LEN) as u16).to_le_bytes());
        out.extend_from_slice(&path.as_bytes()[..path.len().min(MAX_PATH_LEN)]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

/// A cursor over hostile bytes: each read checks what is left.
pub(crate) struct Cursor<'a>(pub(crate) &'a [u8]);

impl<'a> Cursor<'a> {
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], MessageError> {
        if self.0.len() < n {
            return Err(MessageError::Malformed("cut short"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, MessageError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, MessageError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, MessageError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn hash(&mut self) -> Result<Hash, MessageError> {
        let mut h = [0u8; 32];
        h.copy_from_slice(self.take(32)?);
        Ok(h)
    }

    pub(crate) fn text(&mut self) -> Result<String, MessageError> {
        let len = usize::from(self.u8()?);
        let bytes = self.take(len)?;
        let text = std::str::from_utf8(bytes).map_err(|_| MessageError::Malformed("text"))?;
        if text.chars().any(char::is_control) {
            return Err(MessageError::Malformed("text"));
        }
        Ok(text.to_string())
    }
}

/// The id, version and hash a content message starts with, read from its first bytes alone (a cache lists what it holds this
/// way, without reading the files).
pub fn peek_head(bytes: &[u8]) -> Option<(String, String, Hash)> {
    let mut c = Cursor(bytes);
    if c.take(4).ok()? != MAGIC || c.u8().ok()? != KIND_CONTENT {
        return None;
    }
    let id = c.text().ok()?;
    let version = c.text().ok()?;
    let hash = c.hash().ok()?;
    (!id.is_empty()).then_some((id, version, hash))
}

/// How many bytes the start of a content message that [`peek_head`] reads takes at most.
pub const HEAD_BYTES: usize = 4 + 1 + 256 + 256 + 32;

/// What a content message may hold.
#[derive(Clone, Copy, Debug)]
pub struct ContentCaps {
    /// The most files.
    pub files: usize,
    /// The largest file.
    pub file_bytes: u64,
    /// The most bytes in all.
    pub total_bytes: u64,
}

impl ContentCaps {
    /// The caps of `limits`' files and bytes, held to `total_bytes`.
    pub fn of(limits: &ModLimits, files: usize, file_bytes: u64, total_bytes: u64) -> ContentCaps {
        ContentCaps {
            files: files.min(limits.files),
            file_bytes: file_bytes.min(limits.file_bytes),
            total_bytes: total_bytes.min(limits.total_bytes),
        }
    }
}

/// A content message read: the mod, the hash the sender said, and the files.
#[derive(Debug)]
pub struct Content {
    /// The mod's id.
    pub id: String,
    /// The mod's version.
    pub version: String,
    /// The hash the sender says the files have (checked by the receiver against their real hash).
    pub hash: Hash,
    /// The files.
    pub files: Vec<(String, Vec<u8>)>,
}

/// Reads a content message under `caps`. A file that could be script is refused, not skipped.
pub fn decode_content(bytes: &[u8], caps: &ContentCaps) -> Result<Content, MessageError> {
    let mut c = Cursor(bytes);
    if c.take(4)? != MAGIC {
        return Err(MessageError::Malformed("magic"));
    }
    if c.u8()? != KIND_CONTENT {
        return Err(MessageError::Malformed("kind"));
    }
    let id = c.text()?;
    let version = c.text()?;
    let hash = c.hash()?;
    let count = c.u32()? as usize;
    // Each file takes at least a path length, one path byte and a size.
    if count > caps.files || count.saturating_mul(2 + 1 + 4) > c.0.len() {
        return Err(MessageError::TooLarge("files"));
    }
    let mut files = Vec::with_capacity(count);
    let mut total: u64 = 0;
    for _ in 0..count {
        let path_len = usize::from(c.u16()?);
        if path_len == 0 || path_len > MAX_PATH_LEN {
            return Err(MessageError::BadPath(String::new()));
        }
        let raw = c.take(path_len)?;
        let path = std::str::from_utf8(raw)
            .map_err(|_| MessageError::BadPath(shown(raw)))?
            .to_string();
        if crate::package::check_path(&path, &ModLimits::default()).is_err() {
            return Err(MessageError::BadPath(shown(path.as_bytes())));
        }
        let len = u64::from(c.u32()?);
        if len > caps.file_bytes {
            return Err(MessageError::TooLarge("file size"));
        }
        total = total.saturating_add(len);
        if total > caps.total_bytes {
            return Err(MessageError::TooLarge("total size"));
        }
        let data = c.take(len as usize)?.to_vec();
        if is_script_path(&path) {
            return Err(MessageError::Script(shown(path.as_bytes())));
        }
        files.push((path, data));
    }
    if !c.0.is_empty() {
        return Err(MessageError::Malformed("trailing bytes"));
    }
    Ok(Content {
        id,
        version,
        hash,
        files,
    })
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
