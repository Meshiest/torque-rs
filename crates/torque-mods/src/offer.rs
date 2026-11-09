//! What a server sends a client that can take content, and what a client asks for: the offer, a request and the data that answers it.
//!
//! **The offer** lists the mods the server requires. Per mod: its id, version and package hash ([`crate::content`]), whether the
//! server serves its content, and, when it does, per file the path, size and SHA-256, and per block of [`BLOCK_BYTES`] of the
//! file a strong hash ([`block_hash`]: SHA-256 cut to [`BLOCK_HASH_LEN`] bytes). A client matches what it has against it, by
//! package hash, then by file hash, then by block hash within a file of the same path of an older copy of the mod, and asks for
//! only the blocks it still lacks. The offer is one message, built once and shared by every connection.
//!
//! **A request** is a run of blocks of one file: `(mod, file, first block, count)`, at most [`MAX_REQUEST_BLOCKS`] blocks. It rides
//! as the remote command `TorqueModsGet`. **The data** that answers it is a message of the mods channel with the run's bytes.
//! A server serves only what its offer lists, to anyone who asks; a request outside the offer is refused.
//!
//! Everything here is read from hostile bytes: every count is checked against the bytes left and a cap before anything is
//! allocated, a path passes the package's rules and is never a script, and a malformed message is an `Err`.

use sha2::{Digest, Sha256};

use crate::content::{Cursor, Hash, MAGIC, MessageError};
use crate::files::is_script_path;
use crate::package::{MAX_PATH_LEN, ModLimits, check_path};

pub use torque_transfer::{
    BLOCK_BYTES, BLOCK_HASH_LEN, BlockHash, block_count, block_hash, run_bytes,
};

/// The most blocks one request asks for.
pub const MAX_REQUEST_BLOCKS: u32 = torque_transfer::MAX_RUN_BLOCKS;

/// The most bytes of the offer message.
pub const MAX_OFFER_BYTES: usize = 1 << 20;

/// The most mods an offer lists.
pub const MAX_OFFER_MODS: usize = torque_net::modsync::MAX_REQUIRED;

const KIND_OFFER: u8 = 1;
const KIND_DATA: u8 = 3;

/// The SHA-256 of a file.
pub fn file_hash(data: &[u8]) -> Hash {
    Sha256::digest(data).into()
}

/// One file of an offered mod.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileOffer {
    /// The file's path.
    pub path: String,
    /// Its size.
    pub size: u32,
    /// Its SHA-256.
    pub hash: Hash,
    /// The hash of each block.
    pub blocks: Vec<BlockHash>,
}

impl FileOffer {
    /// The offer of a file with these bytes (`None` over 4 GiB).
    pub fn of(path: &str, data: &[u8]) -> Option<FileOffer> {
        Some(FileOffer {
            path: path.to_string(),
            size: u32::try_from(data.len()).ok()?,
            hash: file_hash(data),
            blocks: data.chunks(BLOCK_BYTES).map(block_hash).collect(),
        })
    }
}

/// One mod of the offer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModOffer {
    /// The mod's id.
    pub id: String,
    /// Its version.
    pub version: String,
    /// The package hash of its content.
    pub hash: Hash,
    /// The server serves its content (`files` is listed); else a client must have the mod already.
    pub download: bool,
    /// The files, when the server serves them (in the order the package hash takes them).
    pub files: Vec<FileOffer>,
}

impl ModOffer {
    /// The bytes of the content listed (0 when it is not served).
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| u64::from(f.size)).sum()
    }
}

/// What an offer may hold.
#[derive(Clone, Copy, Debug)]
pub struct OfferCaps {
    /// The most files in all.
    pub files: usize,
    /// The largest file.
    pub file_bytes: u64,
    /// The most content bytes listed in all.
    pub total_bytes: u64,
}

impl OfferCaps {
    /// The caps of a client with these mod limits.
    pub fn of(limits: &ModLimits) -> OfferCaps {
        OfferCaps {
            files: limits.files,
            file_bytes: limits.file_bytes,
            total_bytes: limits.total_bytes,
        }
    }
}

fn put_text(out: &mut Vec<u8>, text: &str) {
    crate::content::put_text(out, text);
}

/// The offer message of `mods` (at most [`MAX_OFFER_MODS`]).
pub fn encode_offer(mods: &[ModOffer]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.push(KIND_OFFER);
    out.extend_from_slice(&(mods.len().min(MAX_OFFER_MODS) as u16).to_le_bytes());
    for m in mods.iter().take(MAX_OFFER_MODS) {
        put_text(&mut out, &m.id);
        put_text(&mut out, &m.version);
        out.extend_from_slice(&m.hash);
        out.push(u8::from(m.download));
        let files: &[FileOffer] = if m.download { &m.files } else { &[] };
        out.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for f in files {
            out.extend_from_slice(&(f.path.len().min(MAX_PATH_LEN) as u16).to_le_bytes());
            out.extend_from_slice(&f.path.as_bytes()[..f.path.len().min(MAX_PATH_LEN)]);
            out.extend_from_slice(&f.size.to_le_bytes());
            out.extend_from_slice(&f.hash);
            for b in &f.blocks {
                out.extend_from_slice(b);
            }
        }
    }
    out
}

fn shown(path: &str) -> String {
    path.chars().filter(|c| !c.is_control()).take(60).collect()
}

/// Reads an offer under `caps`.
pub fn decode_offer(bytes: &[u8], caps: &OfferCaps) -> Result<Vec<ModOffer>, MessageError> {
    if bytes.len() > MAX_OFFER_BYTES {
        return Err(MessageError::TooLarge("offer"));
    }
    let mut c = Cursor(bytes);
    if c.take(4)? != MAGIC {
        return Err(MessageError::Malformed("magic"));
    }
    if c.u8()? != KIND_OFFER {
        return Err(MessageError::Malformed("kind"));
    }
    let count = usize::from(c.u16()?);
    // Each mod takes at least two text lengths, a hash, a flag and a file count.
    if count > MAX_OFFER_MODS || count.saturating_mul(2 + 32 + 1 + 4) > c.0.len() {
        return Err(MessageError::TooLarge("mods in the offer"));
    }
    let (mut files_total, mut bytes_total) = (0usize, 0u64);
    let mut mods = Vec::with_capacity(count);
    for _ in 0..count {
        let id = c.text()?;
        let version = c.text()?;
        if !torque_net::manifest::is_name(&id) {
            return Err(MessageError::Malformed("mod id"));
        }
        if !torque_net::modsync::is_version(&version) {
            return Err(MessageError::Malformed("mod version"));
        }
        let hash = c.hash()?;
        let download = match c.u8()? {
            0 => false,
            1 => true,
            _ => return Err(MessageError::Malformed("flag")),
        };
        let nfiles = c.u32()? as usize;
        // A file takes at least a path length, a path byte, a size and a hash.
        if nfiles > caps.files.saturating_sub(files_total)
            || nfiles.saturating_mul(2 + 1 + 4 + 32) > c.0.len()
            || (!download && nfiles != 0)
        {
            return Err(MessageError::TooLarge("files"));
        }
        files_total += nfiles;
        let mut files: Vec<crate::offer::FileOffer> = Vec::with_capacity(nfiles);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..nfiles {
            let path_len = usize::from(c.u16()?);
            if path_len == 0 || path_len > MAX_PATH_LEN {
                return Err(MessageError::BadPath(String::new()));
            }
            let raw = c.take(path_len)?;
            let path = std::str::from_utf8(raw)
                .map_err(|_| MessageError::BadPath(String::new()))?
                .to_string();
            if check_path(&path, &ModLimits::default()).is_err() {
                return Err(MessageError::BadPath(shown(&path)));
            }
            if is_script_path(&path) {
                return Err(MessageError::Script(shown(&path)));
            }
            if !seen.insert(path.to_ascii_lowercase()) {
                return Err(MessageError::BadPath(shown(&path)));
            }
            let size = c.u32()?;
            if u64::from(size) > caps.file_bytes {
                return Err(MessageError::TooLarge("file size"));
            }
            bytes_total = bytes_total.saturating_add(u64::from(size));
            if bytes_total > caps.total_bytes {
                return Err(MessageError::TooLarge("total size"));
            }
            let hash = c.hash()?;
            let nblocks = block_count(u64::from(size)) as usize;
            if nblocks.saturating_mul(BLOCK_HASH_LEN) > c.0.len() {
                return Err(MessageError::Malformed("cut short"));
            }
            let mut blocks = Vec::with_capacity(nblocks);
            for _ in 0..nblocks {
                let mut b = [0u8; BLOCK_HASH_LEN];
                b.copy_from_slice(c.take(BLOCK_HASH_LEN)?);
                blocks.push(b);
            }
            files.push(FileOffer {
                path,
                size,
                hash,
                blocks,
            });
        }
        mods.push(ModOffer {
            id,
            version,
            hash,
            download,
            files,
        });
    }
    if !c.0.is_empty() {
        return Err(MessageError::Malformed("trailing bytes"));
    }
    Ok(mods)
}

/// A request: a run of blocks of one file of one mod of the offer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Request {
    /// The mod's place in the offer.
    pub mod_index: u32,
    /// The file's place in the mod.
    pub file: u32,
    /// The first block.
    pub start: u32,
    /// How many blocks (1 to [`MAX_REQUEST_BLOCKS`]).
    pub count: u32,
}

/// The data message that answers `request`: the run's bytes.
pub fn encode_data(request: &Request, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 24);
    out.extend_from_slice(&MAGIC);
    out.push(KIND_DATA);
    out.extend_from_slice(&request.mod_index.to_le_bytes());
    out.extend_from_slice(&request.file.to_le_bytes());
    out.extend_from_slice(&request.start.to_le_bytes());
    out.extend_from_slice(&request.count.to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Reads a data message: the request it answers and the bytes (borrowed from `bytes`).
pub fn decode_data(bytes: &[u8]) -> Result<(Request, &[u8]), MessageError> {
    let mut c = Cursor(bytes);
    if c.take(4)? != MAGIC {
        return Err(MessageError::Malformed("magic"));
    }
    if c.u8()? != KIND_DATA {
        return Err(MessageError::Malformed("kind"));
    }
    let request = Request {
        mod_index: c.u32()?,
        file: c.u32()?,
        start: c.u32()?,
        count: c.u32()?,
    };
    Ok((request, c.0))
}

/// Whether `bytes` is an offer message (by its head).
pub fn is_offer(bytes: &[u8]) -> bool {
    bytes.len() > 4 && bytes[..4] == MAGIC && bytes[4] == KIND_OFFER
}

/// Whether `bytes` is a data message (by its head).
pub fn is_data(bytes: &[u8]) -> bool {
    bytes.len() > 4 && bytes[..4] == MAGIC && bytes[4] == KIND_DATA
}

#[cfg(test)]
#[path = "offer_tests.rs"]
mod tests;
