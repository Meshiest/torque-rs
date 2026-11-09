//! The messages a client sends on the upload channel: the block list and the data of a requested run.
//!
//! Both start with a tag byte and the upload id (little endian). The block list is the id and then every block's hash, back to
//! back. A data message is the id, the first block's number and the run's bytes. The reader works on borrowed bytes and checks
//! every length before it returns a slice, so it allocates nothing and cannot be made to read past a message.

use std::fmt;

use crate::blocks::{BLOCK_BYTES, BLOCK_HASH_LEN, BlockHash, MAX_RUN_BLOCKS};

const TAG_BLOCK_LIST: u8 = 1;
const TAG_DATA: u8 = 2;

/// The bytes before a block list's hashes or a run's data.
const LIST_HEADER: usize = 5;
const DATA_HEADER: usize = 9;

/// The most bytes of a message of the upload channel: a run of [`MAX_RUN_BLOCKS`] blocks and its header.
pub const MAX_MESSAGE: usize = MAX_RUN_BLOCKS as usize * BLOCK_BYTES + DATA_HEADER;

/// A message of the upload channel, borrowing the input it was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientMessage<'a> {
    /// The hash of each block of the upload.
    BlockList {
        /// The upload.
        id: u32,
        /// The hashes, [`BLOCK_HASH_LEN`] bytes each, in block order.
        hashes: &'a [u8],
    },
    /// The bytes of a run of blocks.
    Data {
        /// The upload.
        id: u32,
        /// The run's first block.
        start: u32,
        /// The run's bytes.
        bytes: &'a [u8],
    },
}

/// Why a message could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireError(pub &'static str);

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed upload message: {}", self.0)
    }
}

impl std::error::Error for WireError {}

/// The block-list message for upload `id`.
pub fn encode_block_list(id: u32, hashes: &[BlockHash]) -> Vec<u8> {
    let mut out = Vec::with_capacity(LIST_HEADER + hashes.len() * BLOCK_HASH_LEN);
    out.push(TAG_BLOCK_LIST);
    out.extend_from_slice(&id.to_le_bytes());
    for hash in hashes {
        out.extend_from_slice(hash);
    }
    out
}

/// The data message for the run of upload `id` that starts at block `start`.
pub fn encode_data(id: u32, start: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(DATA_HEADER + bytes.len());
    out.push(TAG_DATA);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Reads a message off `bytes`, all of them.
///
/// # Errors
/// A [`WireError`] for an unknown tag, a short message, a block list that is not a whole number of hashes, or a run of no
/// bytes or more than [`MAX_RUN_BLOCKS`] blocks.
pub fn decode(bytes: &[u8]) -> Result<ClientMessage<'_>, WireError> {
    let (&tag, rest) = bytes.split_first().ok_or(WireError("empty"))?;
    let (id, rest) = rest
        .split_first_chunk::<4>()
        .map(|(id, rest)| (u32::from_le_bytes(*id), rest))
        .ok_or(WireError("short"))?;
    match tag {
        TAG_BLOCK_LIST => {
            if !rest.len().is_multiple_of(BLOCK_HASH_LEN) {
                return Err(WireError("a block list is whole hashes"));
            }
            Ok(ClientMessage::BlockList { id, hashes: rest })
        }
        TAG_DATA => {
            let (start, data) = rest
                .split_first_chunk::<4>()
                .map(|(start, data)| (u32::from_le_bytes(*start), data))
                .ok_or(WireError("short"))?;
            if data.is_empty() || data.len() > MAX_RUN_BLOCKS as usize * BLOCK_BYTES {
                return Err(WireError("a run is 1 to 4 blocks"));
            }
            Ok(ClientMessage::Data {
                id,
                start,
                bytes: data,
            })
        }
        _ => Err(WireError("unknown tag")),
    }
}

#[cfg(test)]
#[path = "message_tests.rs"]
mod tests;
