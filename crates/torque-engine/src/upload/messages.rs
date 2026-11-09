//! The messages of the upload RPCs.

use crate::NetMessage;

/// A client's offer of an upload.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct BeginUpload {
    /// The kind's name.
    #[net(max_len = 40)]
    pub kind: String,
    /// The description the sink reads.
    #[net(max_len = 768)]
    pub meta: Vec<u8>,
    /// The content's size, low 32 bits.
    pub size_low: u32,
    /// The content's size, high 32 bits.
    pub size_high: u32,
    /// The content's SHA-256.
    pub hash: [u8; 32],
}

/// The server's answer to an offer.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct BeginReply {
    /// Whether the upload was accepted.
    pub accepted: bool,
    /// The upload's id.
    pub id: u32,
    /// Whether the server holds the block list already.
    pub has_list: bool,
    /// The blocks the server holds.
    pub have: u32,
    /// The reason, when it was not accepted.
    #[net(max_len = 255)]
    pub reason: String,
}

/// A client cancels an upload.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct CancelUpload {
    /// The upload's id.
    pub id: u32,
}

/// The server asks for a run of blocks.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct UploadRequest {
    /// The upload's id.
    pub id: u32,
    /// The first block.
    pub start: u32,
    /// How many.
    pub count: u32,
}

/// The server ends an upload.
#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
pub struct UploadEnd {
    /// The upload's id.
    pub id: u32,
    /// Whether the sink took the content.
    pub ok: bool,
    /// The reason, when it did not.
    #[net(max_len = 255)]
    pub reason: String,
}
