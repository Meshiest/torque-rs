//! The client's half: what it offers ([`Source`]), and the state of each upload it began ([`UploadClient`]).
//!
//! The client sends nothing but what the server asks for. It hashes the content once to make the begin request and the block
//! list, then answers each requested run with that run's bytes. The server is hostile as far as the client is concerned: a
//! request outside the content, for more than a run, or past what the client could be asked in all ends that upload with an
//! error, and a block that no longer matches its hash (the file changed under the upload) is never sent.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::blocks::{
    BLOCK_BYTES, BlockHash, Hash, MAX_OUTSTANDING_RUNS, MAX_RUN_BLOCKS, block_count, block_hash,
    run_bytes,
};
use crate::message::{encode_block_list, encode_data};
use crate::spec::{MAX_KIND_NAME, MAX_META_BYTES, MAX_UPLOAD_BYTES};

/// The content a client offers.
pub trait Source {
    /// The content's size.
    fn len(&self) -> u64;

    /// Whether there is nothing to send.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `buf` from `offset` (inside the content).
    ///
    /// # Errors
    /// The reader's: a file that changed or went away.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl Source for Vec<u8> {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        read_slice(self, offset, buf)
    }
}

impl Source for Arc<[u8]> {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        read_slice(self, offset, buf)
    }
}

fn read_slice(bytes: &[u8], offset: u64, buf: &mut [u8]) -> io::Result<()> {
    let from = usize::try_from(offset).unwrap_or(usize::MAX);
    let part = from
        .checked_add(buf.len())
        .and_then(|end| bytes.get(from..end))
        .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
    buf.copy_from_slice(part);
    Ok(())
}

/// A file offered as it is on disk, read as the server asks.
pub struct FileSource {
    file: File,
    len: u64,
}

impl FileSource {
    /// Opens `path`.
    ///
    /// # Errors
    /// The file system's.
    pub fn open(path: &Path) -> io::Result<FileSource> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(FileSource { file, len })
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buf)
    }
}

/// Why the client's half could not do something. Each ends one upload, never the connection.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientError {
    /// The content is empty or above what any upload may be.
    BadSize,
    /// The kind's name or the metadata is too long.
    BadRequest(&'static str),
    /// The client has all the uploads it may keep at once.
    TooMany,
    /// The same content of the same kind is already being uploaded.
    Duplicate,
    /// The source could not be read, with the reason.
    Source(String),
    /// No such upload (it ended, or the server named one that does not exist).
    Unknown,
    /// The server asked for what an honest server does not.
    Protocol(&'static str),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::BadSize => f.write_str("there is nothing to upload, or it is too large"),
            ClientError::BadRequest(why) => write!(f, "the upload request is not valid: {why}"),
            ClientError::TooMany => f.write_str("too many uploads are going at once"),
            ClientError::Duplicate => f.write_str("that is already being uploaded"),
            ClientError::Source(why) => write!(f, "the content could not be read: {why}"),
            ClientError::Unknown => f.write_str("no such upload"),
            ClientError::Protocol(why) => write!(f, "the server's request was not valid: {why}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// An upload the client began.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(pub u32);

/// What the client sends to begin an upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeginRequest {
    /// The upload's handle on this client.
    pub handle: Handle,
    /// The kind's name.
    pub kind: String,
    /// The metadata that describes the upload.
    pub meta: Vec<u8>,
    /// The content's size.
    pub size: u64,
    /// The content's SHA-256.
    pub hash: Hash,
}

/// The server's accept of a begin request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accept {
    /// The upload's id on the server.
    pub id: u32,
    /// Whether the server holds the block list already (a resumed upload that got that far).
    pub has_list: bool,
    /// How many blocks the server holds.
    pub have: u32,
}

impl From<crate::server::Accepted> for Accept {
    fn from(a: crate::server::Accepted) -> Accept {
        Accept {
            id: a.id,
            has_list: a.has_list,
            have: a.have,
        }
    }
}

/// Where an upload is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The begin request is out; no answer yet.
    Waiting,
    /// The server is taking blocks.
    Sending {
        /// The bytes sent so far.
        sent: u64,
        /// The content's size.
        total: u64,
    },
    /// The server has the content and its sink took it.
    Done,
    /// The upload ended with this reason (the server's text, or ours).
    Failed(String),
}

/// The most uploads a client keeps at once.
pub const MAX_CLIENT_UPLOADS: usize = 8;

struct Upload {
    kind: String,
    hash: Hash,
    source: Box<dyn Source>,
    size: u64,
    blocks: Vec<BlockHash>,
    id: Option<u32>,
    sent: u64,
    status: Status,
}

impl Upload {
    fn is_over(&self) -> bool {
        matches!(self.status, Status::Failed(_) | Status::Done)
    }
}

/// The client's uploads.
#[derive(Default)]
pub struct UploadClient {
    uploads: HashMap<Handle, Upload>,
    by_id: HashMap<u32, Handle>,
    next: u32,
}

impl UploadClient {
    /// An empty client.
    pub fn new() -> UploadClient {
        UploadClient::default()
    }

    /// Starts an upload of `source`: reads it once for its hashes and returns the begin request to send.
    ///
    /// # Errors
    /// [`ClientError`] for an empty or oversized content, a long name or description, too many uploads going, or a source that
    /// cannot be read.
    pub fn begin(
        &mut self,
        kind: &str,
        meta: Vec<u8>,
        mut source: Box<dyn Source>,
    ) -> Result<BeginRequest, ClientError> {
        let size = source.len();
        if size == 0 || size > MAX_UPLOAD_BYTES {
            return Err(ClientError::BadSize);
        }
        if kind.is_empty() || kind.len() > MAX_KIND_NAME {
            return Err(ClientError::BadRequest("the name is 1 to 40 bytes"));
        }
        if meta.len() > MAX_META_BYTES {
            return Err(ClientError::BadRequest("the description is too long"));
        }
        self.uploads.retain(|_, u| !u.is_over());
        if self.uploads.len() >= MAX_CLIENT_UPLOADS {
            return Err(ClientError::TooMany);
        }
        let (blocks, hash) =
            hash_source(source.as_mut()).map_err(|e| ClientError::Source(e.to_string()))?;
        // The server keeps one upload of a content for a player: a second begin of it would find the first still going.
        if self
            .uploads
            .values()
            .any(|u| u.kind == kind && u.hash == hash)
        {
            return Err(ClientError::Duplicate);
        }
        let handle = Handle(self.next);
        self.next = self.next.wrapping_add(1);
        self.uploads.insert(
            handle,
            Upload {
                kind: kind.to_string(),
                hash,
                source,
                size,
                blocks,
                id: None,
                sent: 0,
                status: Status::Waiting,
            },
        );
        Ok(BeginRequest {
            handle,
            kind: kind.to_string(),
            meta,
            size,
            hash,
        })
    }

    /// The server accepted `handle`'s upload. Returns the messages to send on the upload channel: the block list, unless the
    /// server has it.
    ///
    /// # Errors
    /// [`ClientError::Unknown`] if the upload is not waiting for an answer.
    pub fn accepted(
        &mut self,
        handle: Handle,
        accept: Accept,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        let upload = self.uploads.get_mut(&handle).ok_or(ClientError::Unknown)?;
        if upload.status != Status::Waiting {
            return Err(ClientError::Unknown);
        }
        let have = u64::from(accept.have.min(upload.blocks.len() as u32)) * BLOCK_BYTES as u64;
        upload.sent = have.min(upload.size);
        upload.status = Status::Sending {
            sent: upload.sent,
            total: upload.size,
        };
        upload.id = Some(accept.id);
        self.by_id.insert(accept.id, handle);
        if accept.has_list {
            return Ok(Vec::new());
        }
        Ok(vec![encode_block_list(accept.id, &upload.blocks)])
    }

    /// The server refused `handle`'s upload.
    pub fn refused(&mut self, handle: Handle, reason: String) {
        if let Some(upload) = self.uploads.get_mut(&handle) {
            upload.status = Status::Failed(reason);
        }
    }

    /// The server asks for blocks `start..start + count` of upload `id`. Returns the data message to send.
    ///
    /// # Errors
    /// [`ClientError::Unknown`] for an upload that is not going (a late request after it ended: ignore it), and
    /// [`ClientError::Protocol`] or [`ClientError::Source`] when the upload cannot go on (it is failed with that reason).
    pub fn request(&mut self, id: u32, start: u32, count: u32) -> Result<Vec<u8>, ClientError> {
        let handle = *self.by_id.get(&id).ok_or(ClientError::Unknown)?;
        let upload = self.uploads.get_mut(&handle).ok_or(ClientError::Unknown)?;
        if !matches!(upload.status, Status::Sending { .. }) {
            return Err(ClientError::Unknown);
        }
        match build_run(upload, start, count) {
            Ok(message) => {
                upload.status = Status::Sending {
                    sent: upload.sent,
                    total: upload.size,
                };
                Ok(message)
            }
            Err(e) => {
                upload.status = Status::Failed(e.to_string());
                Err(e)
            }
        }
    }

    /// The server ended upload `id`: `Ok` when its sink took the content, else the reason.
    pub fn ended(&mut self, id: u32, result: Result<(), String>) {
        let Some(handle) = self.by_id.remove(&id) else {
            return;
        };
        if let Some(upload) = self.uploads.get_mut(&handle) {
            upload.status = match result {
                Ok(()) => Status::Done,
                Err(reason) => Status::Failed(reason),
            };
        }
    }

    /// Cancels `handle`'s upload if it has not ended. Returns the server's id for it, if the server accepted it (to tell the server).
    pub fn cancel(&mut self, handle: Handle) -> Option<u32> {
        let upload = self.uploads.get_mut(&handle)?;
        if upload.is_over() {
            return None;
        }
        upload.status = Status::Failed("Cancelled.".to_string());
        let id = upload.id.take()?;
        self.by_id.remove(&id);
        Some(id)
    }

    /// The connection ended: uploads still going fail, to be begun again (the server keeps what it has for a while).
    pub fn disconnected(&mut self) {
        self.by_id.clear();
        for upload in self.uploads.values_mut() {
            if !upload.is_over() {
                upload.status = Status::Failed("The connection ended.".to_string());
            }
        }
    }

    /// Where `handle`'s upload is.
    pub fn status(&self, handle: Handle) -> Option<&Status> {
        self.uploads.get(&handle).map(|u| &u.status)
    }
}

/// The bytes of the run `start..start + count` as a data message, after the checks a hostile server could fail.
fn build_run(upload: &mut Upload, start: u32, count: u32) -> Result<Vec<u8>, ClientError> {
    let id = upload.id.ok_or(ClientError::Unknown)?;
    let blocks = upload.blocks.len() as u64;
    if count == 0 || count > MAX_RUN_BLOCKS {
        return Err(ClientError::Protocol("a run is 1 to 4 blocks"));
    }
    if u64::from(start) + u64::from(count) > blocks {
        return Err(ClientError::Protocol("a run past the end"));
    }
    let (from, to) = run_bytes(upload.size, u64::from(start), u64::from(count));
    // An honest server asks for each block once, and for a few again when a connection resumed.
    let slack = (MAX_OUTSTANDING_RUNS as u64 + 1) * u64::from(MAX_RUN_BLOCKS) * BLOCK_BYTES as u64;
    if upload.sent.saturating_add(to - from) > upload.size.saturating_add(slack) {
        return Err(ClientError::Protocol("more asked for than the content"));
    }
    let mut bytes = vec![0u8; (to - from) as usize];
    upload
        .source
        .read_at(from, &mut bytes)
        .map_err(|e| ClientError::Source(e.to_string()))?;
    for (k, block) in bytes.chunks(BLOCK_BYTES).enumerate() {
        if upload.blocks.get(start as usize + k) != Some(&block_hash(block)) {
            return Err(ClientError::Source(
                "the content changed while it was sent".to_string(),
            ));
        }
    }
    upload.sent += to - from;
    Ok(encode_data(id, start, &bytes))
}

/// The hash of each block and of the whole content, one pass over `source`.
fn hash_source(source: &mut dyn Source) -> io::Result<(Vec<BlockHash>, Hash)> {
    let size = source.len();
    let mut blocks = Vec::with_capacity(block_count(size) as usize);
    let mut whole = Sha256::new();
    let mut buf = vec![0u8; BLOCK_BYTES];
    let mut at = 0;
    while at < size {
        let n = (size - at).min(BLOCK_BYTES as u64) as usize;
        source.read_at(at, &mut buf[..n])?;
        whole.update(&buf[..n]);
        blocks.push(block_hash(&buf[..n]));
        at += n as u64;
    }
    Ok((blocks, whole.finalize().into()))
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
