//! What a client adds to its connect request beyond Torque's fields, and what a server does with it before its scripts see the
//! request: the [`ConnectBlock`] (the connection [`Manifest`] and an auth ticket), the [`ConnectGate`] that judges a request, the
//! [`ManifestGate`] that is the standard gate, and the [`AuthProvider`] a game supplies for tickets.
//!
//! **Where it goes.** The block follows the request's arguments, after everything Torque's request and a game's
//! [`crate::profile::HandshakeForm`] carry. A server that reads Torque's request stops after the arguments, so a peer without a gate
//! never sees the block (its bytes are unread, not misread), and a client without a block sends exactly the request it sent before.
//! A server reads the block only when it has a gate ([`crate::handshake::ServerConfig::gate`]); a block that is there but garbled,
//! oversize or of an unknown format is refused with a reason, and a request with no block is judged as a peer with nothing
//! ([`Manifest::bare_like`]), so a plain client meets "server needs plugin torque.rope 2, you do not have it".
//!
//! **Wire.** `u32 BLOCK_MAGIC`, `u8` section count (at most [`MAX_SECTIONS`]), then each section as `u8 tag`, 16-bit length, bytes.
//! A tag this build does not know is skipped; a tag twice is invalid; the manifest section is at most [`MAX_MANIFEST_BYTES`] and the
//! ticket at most [`MAX_TICKET_BYTES`]. The magic is above the 16 arguments Torque allows, so a reader that took it for a count would
//! refuse it.
//!
//! **Order of the checks** (a gate's, [`ManifestGate::check`]): the manifest first, then the ticket. The server never runs an
//! auth provider for a client whose build it already refuses.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::NET_STRING_BYTES;
use crate::handshake::{ConnectFields, siphash24};
use crate::manifest::{MAX_MANIFEST_BYTES, Manifest, ManifestError};

/// The word that starts a connect block.
pub const BLOCK_MAGIC: u32 = 0x4d46_5431;

/// The most sections a block holds.
pub const MAX_SECTIONS: usize = 4;

/// The longest auth ticket, in bytes.
pub const MAX_TICKET_BYTES: usize = 256;

const TAG_MANIFEST: u8 = 1;
const TAG_TICKET: u8 = 2;

/// Why a connect block could not be written or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockError {
    /// The stream ended inside the block, or a count or length is above its cap.
    Stream(NetError),
    /// The manifest section is refused.
    Manifest(ManifestError),
    /// The ticket is longer than [`MAX_TICKET_BYTES`].
    Ticket(usize),
    /// A section appears twice.
    Repeated(u8),
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockError::Stream(e) => write!(f, "bad connect block ({e})"),
            BlockError::Manifest(e) => write!(f, "{e}"),
            BlockError::Ticket(n) => {
                write!(
                    f,
                    "ticket of {n} bytes is over the limit of {MAX_TICKET_BYTES}"
                )
            }
            BlockError::Repeated(tag) => write!(f, "bad connect block (section {tag} twice)"),
        }
    }
}

impl std::error::Error for BlockError {}

impl From<NetError> for BlockError {
    fn from(error: NetError) -> BlockError {
        BlockError::Stream(error)
    }
}

impl From<ManifestError> for BlockError {
    fn from(error: ManifestError) -> BlockError {
        BlockError::Manifest(error)
    }
}

/// What a connect request carries after its arguments: the sender's manifest and the ticket its auth provider made.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectBlock {
    /// The sender's manifest, when it sends one.
    pub manifest: Option<Manifest>,
    /// The auth ticket, when the sender has an auth provider.
    pub ticket: Option<Vec<u8>>,
}

impl ConnectBlock {
    /// Whether [`ConnectBlock::write`] would write this block: `Ok`, or what it would refuse. For a client to check before it sends
    /// (a block that cannot be written is left off the request, and the server then judges the request as one with nothing).
    pub fn validate(&self) -> Result<(), BlockError> {
        self.write(&mut BitWriter::new())
    }

    /// Writes the block into `w`. A manifest that cannot be encoded or a ticket over its limit is an `Err` and writes nothing.
    pub fn write(&self, w: &mut BitWriter) -> Result<(), BlockError> {
        let manifest = self.manifest.as_ref().map(Manifest::encode).transpose()?;
        if let Some(ticket) = &self.ticket
            && ticket.len() > MAX_TICKET_BYTES
        {
            return Err(BlockError::Ticket(ticket.len()));
        }
        w.write_u32(BLOCK_MAGIC);
        w.write_u8(
            usize::from(manifest.is_some()) as u8 + usize::from(self.ticket.is_some()) as u8,
        );
        for (tag, bytes) in [
            (TAG_MANIFEST, manifest.as_deref()),
            (TAG_TICKET, self.ticket.as_deref()),
        ] {
            if let Some(bytes) = bytes {
                w.write_u8(tag);
                w.write_bits(bytes.len() as u32, 16);
                w.write_bytes(bytes);
            }
        }
        Ok(())
    }

    /// Reads the block at the reader's position when it starts with [`BLOCK_MAGIC`]: `Ok(None)` (the reader unmoved) when it does
    /// not, an `Err` when it does and what follows is not a block.
    pub fn read(r: &mut BitReader) -> Result<Option<ConnectBlock>, BlockError> {
        let mut peek = r.clone();
        if peek.read_u32().ok() != Some(BLOCK_MAGIC) {
            return Ok(None);
        }
        r.read_u32()?;
        let sections = crate::cap::count(
            "connect block sections",
            u64::from(r.read_u8()?),
            MAX_SECTIONS,
            r.bits_left(),
            24,
        )?;
        let mut block = ConnectBlock::default();
        let mut seen = 0u8;
        for _ in 0..sections {
            let tag = r.read_u8()?;
            let len = r.read_bits(16)? as usize;
            let bound = match tag {
                TAG_MANIFEST => MAX_MANIFEST_BYTES,
                TAG_TICKET => MAX_TICKET_BYTES,
                _ => usize::MAX,
            };
            if len > bound {
                return Err(NetError::Cap {
                    what: "connect block section",
                    count: len as u64,
                    cap: bound as u64,
                }
                .into());
            }
            let bytes = r.read_bytes(len)?;
            if tag < 8 && seen & (1 << tag) != 0 {
                return Err(BlockError::Repeated(tag));
            }
            if tag < 8 {
                seen |= 1 << tag;
            }
            match tag {
                TAG_MANIFEST => block.manifest = Some(Manifest::decode(&bytes)?),
                TAG_TICKET => block.ticket = Some(bytes),
                _ => {}
            }
        }
        Ok(Some(block))
    }
}

/// A server's judgment of a connect request, made before its scripts see the request.
pub trait ConnectGate: fmt::Debug + Send + Sync {
    /// `Ok` accepts the request (the identity an auth provider verified, if any); `Err` refuses it with the reason the client
    /// shows (cut at 255 bytes on the wire).
    fn check(&self, from: SocketAddr, fields: &ConnectFields) -> Result<Option<Vec<u8>>, Vec<u8>>;

    /// Whether the request this gate accepted lists bulk channels and they are this server's own (the same names), so the
    /// server may write the channels' section for the connection and tell the client so in its accept. The built-in
    /// [`ManifestGate`] compares the channel names; a gate that does not says `false` (the default), and its connections never
    /// get channels.
    fn matches_channels(&self, _fields: &ConnectFields) -> bool {
        false
    }
}

/// Auth tickets: the client's half makes one for the server it connects to, the server's half verifies it. A game supplies one
/// provider type used on both sides (a Steam session ticket, a signed token, a shared secret); the engine's
/// `net().auth(provider)` registers it.
pub trait AuthProvider: fmt::Debug + Send + Sync {
    /// The client's ticket for a new connection (at most [`MAX_TICKET_BYTES`]). An `Err` is why there is none (not signed in),
    /// shown to the player; the connect does not start.
    fn ticket(&self) -> Result<Vec<u8>, String>;

    /// The server's check of a ticket from `from` (an empty one when the client sent none): the identity it stands for, or the
    /// reason it is refused (cut at 255 bytes on the wire). A slow check belongs to a provider that answers from a cache; this
    /// runs inside the handshake.
    fn verify(&self, from: SocketAddr, ticket: &[u8]) -> Result<Vec<u8>, String>;
}

/// The standard gate: the server's manifest the client's must satisfy, then the auth provider's verdict.
#[derive(Debug, Clone)]
pub struct ManifestGate {
    manifest: Manifest,
    auth: Option<Arc<dyn AuthProvider>>,
}

impl ManifestGate {
    /// A gate that holds clients to `manifest`.
    pub fn new(manifest: Manifest) -> ManifestGate {
        ManifestGate {
            manifest,
            auth: None,
        }
    }

    /// The same gate that also verifies tickets with `auth`.
    #[must_use]
    pub fn with_auth(mut self, auth: Arc<dyn AuthProvider>) -> ManifestGate {
        self.auth = Some(auth);
        self
    }

    /// The manifest clients are held to.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

/// `text` as a reason: at most [`NET_STRING_BYTES`] bytes, cut at a character boundary.
pub fn reason(text: &str) -> Vec<u8> {
    let mut end = text.len().min(NET_STRING_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.as_bytes()[..end].to_vec()
}

impl ConnectGate for ManifestGate {
    fn matches_channels(&self, fields: &ConnectFields) -> bool {
        let theirs = fields
            .block
            .as_ref()
            .and_then(|b| b.manifest.as_ref())
            .map(|m| &m.channels);
        self.manifest.channels.count() > 0
            && theirs.is_some_and(|t| self.manifest.channels.same_as(t))
    }

    fn check(&self, from: SocketAddr, fields: &ConnectFields) -> Result<Option<Vec<u8>>, Vec<u8>> {
        let block = fields.block.as_ref();
        let bare;
        let theirs = match block.and_then(|b| b.manifest.as_ref()) {
            Some(manifest) => manifest,
            None => {
                bare = self.manifest.bare_like();
                &bare
            }
        };
        if let Some(difference) = self.manifest.mismatch(theirs) {
            return Err(reason(&difference.to_string()));
        }
        match &self.auth {
            None => Ok(None),
            Some(auth) => {
                let ticket = block.and_then(|b| b.ticket.as_deref()).unwrap_or_default();
                auth.verify(from, ticket)
                    .map(Some)
                    .map_err(|why| reason(&why))
            }
        }
    }
}

/// A provider for tests and private games only: the ticket is a name and a keyed hash of it under a shared 128-bit secret, so a
/// server holding the secret accepts a name it signed and nothing else. The identity is the name.
///
/// It is not for a public server. The ticket is not bound to the server or to the client's address, so anyone who sniffs one can
/// replay it (as that name) to any server with the same secret, and every server and client holds the secret itself.
#[derive(Clone, PartialEq, Eq)]
pub struct SharedSecret {
    key: [u8; 16],
    name: Vec<u8>,
}

impl fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedSecret")
            .field("name", &String::from_utf8_lossy(&self.name))
            .finish_non_exhaustive()
    }
}

impl SharedSecret {
    /// A provider whose client half signs `name` with `key`; the server half only needs the key.
    pub fn new(key: [u8; 16], name: impl Into<Vec<u8>>) -> SharedSecret {
        SharedSecret {
            key,
            name: name.into(),
        }
    }
}

impl AuthProvider for SharedSecret {
    fn ticket(&self) -> Result<Vec<u8>, String> {
        if self.name.is_empty() {
            return Err("not signed in".to_string());
        }
        let mut ticket = self.name.clone();
        ticket.extend_from_slice(&siphash24(&self.key, &self.name).to_le_bytes());
        Ok(ticket)
    }

    fn verify(&self, _from: SocketAddr, ticket: &[u8]) -> Result<Vec<u8>, String> {
        let Some(split) = ticket.len().checked_sub(8) else {
            return Err("sign in to join this server".to_string());
        };
        let (name, mac) = ticket.split_at(split);
        // Every byte is compared, whatever the first difference: the check takes the same time for a near miss as for a wild one.
        let want = siphash24(&self.key, name).to_le_bytes();
        let diff = mac.iter().zip(&want).fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if name.is_empty() || diff != 0 {
            return Err("your sign-in was not accepted".to_string());
        }
        Ok(name.to_vec())
    }
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
