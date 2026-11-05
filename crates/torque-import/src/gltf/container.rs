//! The parts around the JSON: the `.glb` container, URIs (`data:` and relative paths), base64, and image headers.

use crate::{ImportError, caps};

const GLB_MAGIC: &[u8; 4] = b"glTF";
const CHUNK_JSON: u32 = u32::from_le_bytes(*b"JSON");
const CHUNK_BIN: u32 = u32::from_le_bytes(*b"BIN\0");

fn malformed<T>(what: &str) -> Result<T, ImportError> {
    Err(ImportError::Malformed(what.to_string()))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Whether `bytes` look like a glTF: the `.glb` magic, or JSON (a `{` after any white space).
pub fn is_gltf(bytes: &[u8]) -> bool {
    bytes.starts_with(GLB_MAGIC)
        || bytes
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|&b| b == b'{')
}

/// The JSON and the binary chunk of a `.glb`, or the whole of a `.gltf` as JSON.
pub fn split(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), ImportError> {
    if bytes.len() > caps::MAX_FILE_BYTES {
        return Err(ImportError::TooLarge {
            what: "the file",
            bytes: bytes.len() as u64,
            cap: caps::MAX_FILE_BYTES as u64,
        });
    }
    if !bytes.starts_with(GLB_MAGIC) {
        return check_json(bytes).map(|j| (j, None));
    }
    let (Some(version), Some(length)) = (le32(bytes, 4), le32(bytes, 8)) else {
        return malformed("a .glb header shorter than 12 bytes");
    };
    if version != 2 {
        return Err(ImportError::Unsupported(format!(".glb version {version}")));
    }
    let Some(file) = bytes.get(..length as usize) else {
        return malformed("a .glb shorter than its header says");
    };
    let mut at = 12usize;
    let mut json = None;
    let mut bin = None;
    while at < file.len() {
        let (Some(len), Some(kind)) = (le32(file, at), le32(file, at + 4)) else {
            return malformed("a .glb chunk header cut short");
        };
        let start = at + 8;
        let Some(end) = start.checked_add(len as usize).filter(|&e| e <= file.len()) else {
            return malformed("a .glb chunk longer than the file");
        };
        let data = &file[start..end];
        match kind {
            CHUNK_JSON if json.is_none() && at == 12 => json = Some(data),
            CHUNK_BIN if bin.is_none() && json.is_some() => bin = Some(data),
            CHUNK_JSON | CHUNK_BIN => return malformed("a .glb chunk out of order"),
            _ => {}
        }
        // Chunks are aligned to four bytes.
        at = end.div_ceil(4) * 4;
    }
    let Some(json) = json else {
        return malformed("a .glb without its JSON chunk");
    };
    Ok((check_json(json)?, bin))
}

fn check_json(json: &[u8]) -> Result<&[u8], ImportError> {
    if json.len() > caps::MAX_JSON_BYTES {
        return Err(ImportError::TooLarge {
            what: "the JSON",
            bytes: json.len() as u64,
            cap: caps::MAX_JSON_BYTES as u64,
        });
    }
    Ok(json)
}

/// Where a URI's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UriSource {
    /// The bytes of a `data:` URI (decoded), and its media type.
    Data(Vec<u8>, String),
    /// A side file: a relative path, `/`-separated, with no `..`.
    File(String),
}

/// What a URI names: a `data:` URI decoded (at most `max` bytes), or a side file's relative path. Anything else (a scheme
/// such as `http:` or `file:`, a drive letter, an absolute path, a `..` part, a backslash, a control character) is refused.
pub fn resolve_uri(uri: &str, max: usize) -> Result<UriSource, ImportError> {
    if let Some(rest) = uri.strip_prefix("data:") {
        if uri.len() > caps::MAX_DATA_URI_BYTES {
            return Err(ImportError::TooLarge {
                what: "a data: URI",
                bytes: uri.len() as u64,
                cap: caps::MAX_DATA_URI_BYTES as u64,
            });
        }
        let Some((head, body)) = rest.split_once(',') else {
            return malformed("a data: URI without a comma");
        };
        let Some(mime) = head.strip_suffix(";base64") else {
            return Err(ImportError::Unsupported(
                "a data: URI that is not base64".into(),
            ));
        };
        return Ok(UriSource::Data(
            base64(body.as_bytes(), max)?,
            mime.to_string(),
        ));
    }
    if uri.len() > caps::MAX_URI_BYTES {
        return Err(ImportError::Uri(format!("{} bytes long", uri.len())));
    }
    let refuse = |why: &str| Err(ImportError::Uri(format!("{why}: {}", uri.escape_debug())));
    // A scheme (`http:`, `file:`) or a drive letter: a colon before the first slash.
    if uri
        .split('/')
        .next()
        .is_some_and(|first| first.contains(':'))
    {
        return refuse("not a relative path");
    }
    let decoded =
        percent_decode(uri).ok_or_else(|| ImportError::Uri("bad percent escape".into()))?;
    if decoded.starts_with('/') || decoded.contains('\\') || decoded.chars().any(char::is_control) {
        return refuse("not a relative path");
    }
    let mut parts = Vec::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => return refuse("a parent folder"),
            p if p.contains(':') => return refuse("not a relative path"),
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return refuse("empty");
    }
    Ok(UriSource::File(parts.join("/")))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Standard base64 (padding optional), at most `max` bytes out; the length is checked before anything is decoded.
pub fn base64(text: &[u8], max: usize) -> Result<Vec<u8>, ImportError> {
    let text = text
        .strip_suffix(b"==")
        .or_else(|| text.strip_suffix(b"="))
        .unwrap_or(text);
    let out_len = text.len() / 4 * 3 + (text.len() % 4).saturating_sub(1);
    if out_len > max {
        return Err(ImportError::TooLarge {
            what: "a data: URI",
            bytes: out_len as u64,
            cap: max as u64,
        });
    }
    if text.len() % 4 == 1 {
        return malformed("a base64 length");
    }
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(out_len);
    for chunk in text.chunks(4) {
        let mut word = 0u32;
        for (k, &c) in chunk.iter().enumerate() {
            let Some(v) = value(c) else {
                return malformed("a base64 character");
            };
            word |= v << (18 - 6 * k);
        }
        let bytes = word.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Ok(out)
}

/// The width and height a PNG's or a JPEG's header gives.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) && bytes.get(12..16)? == b"IHDR" {
        return Some((be32(16)?, be32(20)?));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut at = 2usize;
        while at + 9 < bytes.len() {
            if bytes[at] != 0xff {
                return None;
            }
            let marker = bytes[at + 1];
            let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                let h = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]);
                let w = u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]);
                return Some((u32::from(w), u32::from(h)));
            }
            at += 2 + len;
        }
    }
    None
}
