//! Byte patches: the delta of a channel whose state is one long byte string. [`diff`] describes how `new` differs from `old` as
//! the new length and the runs of bytes that changed; [`apply`] makes the old bytes the new ones.
//!
//! The format is the new length (4 bytes, little-endian) and then, for each run, its offset and length (4 bytes each) and its
//! bytes. A byte string that grew has its added bytes as a run; one that shrank only says the shorter length.
//! [`apply`] reads a patch from a peer: the length is checked against the cap the caller gives before anything grows, and every
//! run is checked against the length and the bytes the patch has.

/// A run header is 8 bytes: two runs closer than this are one run.
const MERGE_GAP: usize = 8;

/// Why a patch was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchError {
    /// The patch ended inside a field.
    Truncated,
    /// The new length is above the cap.
    TooLarge { len: usize, cap: usize },
    /// A run reaches past the new length.
    Range,
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::Truncated => f.write_str("the patch ends inside a field"),
            PatchError::TooLarge { len, cap } => {
                write!(f, "a patch to {len} bytes is above the cap of {cap}")
            }
            PatchError::Range => f.write_str("a run reaches past the patch's length"),
        }
    }
}

impl std::error::Error for PatchError {}

fn put(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&u32::try_from(value).unwrap_or(u32::MAX).to_le_bytes());
}

/// The patch that turns `old` into `new`, or `None` when they are the same.
pub fn diff(old: &[u8], new: &[u8]) -> Option<Vec<u8>> {
    if old == new {
        return None;
    }
    let mut out = Vec::new();
    put(&mut out, new.len());
    let common = old.len().min(new.len());
    let mut i = 0;
    while i < common {
        if old[i] == new[i] {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 1;
        let mut quiet = 0;
        // The run goes on through gaps shorter than a run header.
        while end + quiet < common {
            if old[end + quiet] == new[end + quiet] {
                quiet += 1;
                if quiet >= MERGE_GAP {
                    break;
                }
            } else {
                end += quiet + 1;
                quiet = 0;
            }
        }
        put(&mut out, start);
        put(&mut out, end - start);
        out.extend_from_slice(&new[start..end]);
        i = end;
    }
    if new.len() > old.len() {
        put(&mut out, old.len());
        put(&mut out, new.len() - old.len());
        out.extend_from_slice(&new[old.len()..]);
    }
    Some(out)
}

/// Makes `base` the bytes `patch` describes. Nothing changes on an error. `max_len` caps the new length.
pub fn apply(base: &mut Vec<u8>, patch: &[u8], max_len: usize) -> Result<(), PatchError> {
    let word = |at: usize| -> Result<usize, PatchError> {
        let bytes = patch.get(at..at + 4).ok_or(PatchError::Truncated)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    };
    let new_len = word(0)?;
    if new_len > max_len {
        return Err(PatchError::TooLarge {
            len: new_len,
            cap: max_len,
        });
    }
    // Checked in full before the first byte is touched.
    let mut at = 4;
    while at < patch.len() {
        let offset = word(at)?;
        let len = word(at + 4)?;
        at += 8;
        if offset.checked_add(len).is_none_or(|end| end > new_len) {
            return Err(PatchError::Range);
        }
        if patch.len() - at < len {
            return Err(PatchError::Truncated);
        }
        at += len;
    }
    base.resize(new_len, 0);
    let mut at = 4;
    while at < patch.len() {
        let (offset, len) = (word(at)?, word(at + 4)?);
        at += 8;
        base[offset..offset + len].copy_from_slice(&patch[at..at + len]);
        at += len;
    }
    Ok(())
}

#[cfg(test)]
#[path = "patch_tests.rs"]
mod tests;
