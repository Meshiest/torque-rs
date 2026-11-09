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
//! **Order of the checks** (a gate's, [`ManifestGate::check`]): the manifest first, then that a client can take the mods a server
//! requires ([`crate::modsync`]), then the ticket. The server never runs an
//! auth provider for a client whose build it already refuses.
//!
//! **A ticket is for one server and one connection.** The client makes its ticket after the server's challenge response arrives
//! (a server-chosen value comes back before the connect request goes out, so the request's bytes up to the block are Torque's own),
//! and hands its provider an [`AuthContext`]: the address it dialed and the challenge. The server rebuilds the same context from
//! the request and its own address ([`crate::handshake::ServerConfig::address`]) and hands it to the provider's check. A provider
//! that binds its tickets to the context cannot be replayed: a server that relays a client's ticket to another server fails there
//! (another address, another challenge), and a sniffed ticket fails on any other connection.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::NET_STRING_BYTES;
use crate::handshake::{ConnectFields, siphash24};
use crate::manifest::{MAX_MANIFEST_BYTES, Manifest, ManifestError};
use crate::modsync::{MAX_SECTION_BYTES, ModsSection, RequiredMods};

/// The word that starts a connect block.
pub const BLOCK_MAGIC: u32 = 0x4d46_5431;

/// The most sections a block holds.
pub const MAX_SECTIONS: usize = 4;

/// The longest auth ticket, in bytes.
pub const MAX_TICKET_BYTES: usize = 256;

/// The longest account id an [`Identity`] holds, in bytes.
pub const MAX_IDENTITY_BYTES: usize = 128;

/// The longest display name an [`Identity`] holds, in bytes.
pub const MAX_IDENTITY_NAME_BYTES: usize = 64;

/// Why an [`Identity`] could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    /// An empty id would make every player who gets one the same player.
    Empty,
    /// The id is longer than [`MAX_IDENTITY_BYTES`].
    TooLong(usize),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::Empty => f.write_str("an identity needs an id"),
            IdentityError::TooLong(n) => write!(
                f,
                "an identity's id is at most {MAX_IDENTITY_BYTES} bytes, this one has {n}"
            ),
        }
    }
}

impl std::error::Error for IdentityError {}

/// Who an auth provider verified a client as: an opaque account id and, optionally, a name to show.
///
/// **Only a provider makes one**, from [`AuthProvider::verify`] on the server, so it is never read from text a client sent. Two
/// identities are the same player exactly when their ids are equal (the name is for display and takes no part in equality,
/// ordering or hashing). The id's bytes mean nothing to the engine; keep them opaque.
///
/// **What it promises depends on the provider.** A real provider (a platform account, a signed token) gives the same id to the
/// same account on every connection, so what is kept under it (roles, props, an upload's resume) follows the player across a
/// reconnect and from one session to the next. A connection with no verified identity (no provider, or one that does not tell
/// accounts apart) has none: the server holds it by the connection itself, which is a new player each time it connects.
#[derive(Clone)]
pub struct Identity {
    id: Box<[u8]>,
    name: Option<Box<str>>,
}

impl Identity {
    /// An identity with this account id and no name. The id is at least one byte and at most [`MAX_IDENTITY_BYTES`].
    ///
    /// # Errors
    ///
    /// An empty id or one over the limit.
    pub fn new(id: impl AsRef<[u8]>) -> Result<Identity, IdentityError> {
        let id = id.as_ref();
        if id.is_empty() {
            return Err(IdentityError::Empty);
        }
        if id.len() > MAX_IDENTITY_BYTES {
            return Err(IdentityError::TooLong(id.len()));
        }
        Ok(Identity {
            id: id.into(),
            name: None,
        })
    }

    /// The same identity with a name to show: control characters and the bidirectional overrides (which reorder the text around
    /// them) are dropped, and the text is cut to [`MAX_IDENTITY_NAME_BYTES`] at a character boundary. A name with nothing left
    /// sets none.
    #[must_use]
    pub fn with_name(mut self, name: &str) -> Identity {
        let mut text = String::new();
        let shown = |c: &char| {
            !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        };
        for c in name.chars().filter(shown) {
            if text.len() + c.len_utf8() > MAX_IDENTITY_NAME_BYTES {
                break;
            }
            text.push(c);
        }
        self.name = (!text.is_empty()).then(|| text.into_boxed_str());
        self
    }

    /// The account id.
    pub fn id(&self) -> &[u8] {
        &self.id
    }

    /// The name to show, if the provider gave one. Not unique and not stable: a player may rename.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl PartialEq for Identity {
    fn eq(&self, other: &Identity) -> bool {
        self.id == other.id
    }
}

impl Eq for Identity {}

impl std::hash::Hash for Identity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish()
    }
}

const TAG_MANIFEST: u8 = 1;
const TAG_TICKET: u8 = 2;
const TAG_MODS: u8 = 3;

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

/// What a connect request carries after its arguments: the sender's manifest, the ticket its auth provider made and what it
/// says about taking mod content ([`ModsSection`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectBlock {
    /// The sender's manifest, when it sends one.
    pub manifest: Option<Manifest>,
    /// The auth ticket, when the sender has an auth provider.
    pub ticket: Option<Vec<u8>>,
    /// That the sender can take a server's mod content, and the most it will take (bytes and files), when its engine shares mods.
    /// It never lists the mods the sender has installed or cached.
    pub mods: Option<ModsSection>,
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
        let mods = self.mods.as_ref().map(ModsSection::encode);
        if let Some(ticket) = &self.ticket
            && ticket.len() > MAX_TICKET_BYTES
        {
            return Err(BlockError::Ticket(ticket.len()));
        }
        w.write_u32(BLOCK_MAGIC);
        w.write_u8(
            usize::from(manifest.is_some()) as u8
                + usize::from(self.ticket.is_some()) as u8
                + usize::from(mods.is_some()) as u8,
        );
        for (tag, bytes) in [
            (TAG_MANIFEST, manifest.as_deref()),
            (TAG_TICKET, self.ticket.as_deref()),
            (TAG_MODS, mods.as_deref()),
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
                TAG_MODS => MAX_SECTION_BYTES,
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
                TAG_MODS => block.mods = Some(ModsSection::decode(&bytes)?),
                _ => {}
            }
        }
        Ok(Some(block))
    }
}

/// What a ticket is made for: the server a client dials and the connection it makes. The client builds it from the address it
/// dialed and the server's challenge response, the server from its own address and the request's digest, so both sides hold the
/// same value exactly when the ticket was made for this server and this connection.
///
/// - `server` is the address clients reach the server by. On the client it is the address it dialed. On the server it is
///   [`ServerConfig::address`](crate::handshake::ServerConfig::address), which a host sets to its public address (a socket bound
///   to every interface, a NAT or a relay hides the address clients dial, so a server cannot always read it off its socket). A
///   server that was never told one passes the unspecified address with port 0, and a provider that binds to `server` must then
///   refuse ([`SharedSecret`] does): a ticket that names no server could be taken to any.
/// - `sequence` and `challenge` are the connect's sequence and the keyed challenge digest the server returned for it. The digest
///   is the server's own keyed hash of the client's source address, the sequence and a time epoch, so another server's digest
///   differs, and a connection from another source address or with another sequence gets another one. The client never sees
///   the key, so it cannot make the digest of a connection it does not have.
///
///   What this does not stop: the digest is not a one-time value. A connect request that is sniffed on the path can be sent
///   again with the client's source address forged, by anyone who can forge it, for as long as its digest verifies (between
///   [`CHALLENGE_PERIOD_MS`](crate::handshake::CHALLENGE_PERIOD_MS) and twice that) and a server holding the same sequence is
///   not already serving that address. It opens a connection nobody can use (the replies go to the address that was forged), as the identity of that client. After
///   the window the digest is refused. A provider that needs more (a ticket that works once) must keep that state itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthContext {
    /// The address clients reach the server by.
    pub server: SocketAddr,
    /// The connect request's sequence.
    pub sequence: u32,
    /// The challenge digest the server returned for that sequence.
    pub challenge: [u32; 4],
}

impl AuthContext {
    /// The context as bytes, for a provider that signs it: the same bytes on both sides for the same context, and no two different
    /// contexts share them.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(40);
        match self.server.ip() {
            IpAddr::V4(ip) => {
                out.push(4);
                out.extend_from_slice(&ip.octets());
            }
            IpAddr::V6(ip) => {
                out.push(6);
                out.extend_from_slice(&ip.octets());
            }
        }
        out.extend_from_slice(&self.server.port().to_le_bytes());
        out.extend_from_slice(&self.sequence.to_le_bytes());
        for word in self.challenge {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out
    }

    /// Whether the server's address is unknown (it was never told one).
    pub fn server_unknown(&self) -> bool {
        self.server.ip().is_unspecified()
    }
}

/// Makes a client's connect block once the server's challenge response is in, when the block holds a ticket for the connection
/// ([`crate::client::Client::set_block_source`]). `Ok(None)` sends no block; an `Err` is the reason the connect ends (shown to the
/// player).
pub struct BlockSource(Box<MakeBlock>);

type MakeBlock = dyn FnMut(&AuthContext) -> Result<Option<ConnectBlock>, String>;

impl BlockSource {
    /// A source that calls `make` for each connection.
    pub fn new(
        make: impl FnMut(&AuthContext) -> Result<Option<ConnectBlock>, String> + 'static,
    ) -> BlockSource {
        BlockSource(Box::new(make))
    }

    /// The block for the connection `cx` names.
    pub fn make(&mut self, cx: &AuthContext) -> Result<Option<ConnectBlock>, String> {
        (self.0)(cx)
    }
}

impl fmt::Debug for BlockSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BlockSource")
    }
}

/// A server's judgment of a connect request, made before its scripts see the request.
pub trait ConnectGate: fmt::Debug + Send + Sync {
    /// `Ok` accepts the request (the [`Identity`] an auth provider verified, if any); `Err` refuses it with the reason the client
    /// shows (cut at 255 bytes on the wire). `cx` is the connection the request belongs to ([`AuthContext`]).
    fn check(
        &self,
        from: SocketAddr,
        cx: &AuthContext,
        fields: &ConnectFields,
    ) -> Result<Option<Identity>, Vec<u8>>;

    /// Whether the request this gate accepted lists bulk channels and they are this server's own (the same names), so the
    /// server may write the channels' section for the connection and tell the client so in its accept. The built-in
    /// [`ManifestGate`] compares the channel names; a gate that does not says `false` (the default), and its connections never
    /// get channels.
    fn matches_channels(&self, _fields: &ConnectFields) -> bool {
        false
    }

    /// What the request this gate accepted says about taking content from the server (its mods section), for the server to send
    /// its required mods to a client that can take them. `None` (the default): the client cannot.
    /// The canonical token of `ticket` (its auth provider's [`AuthProvider::ticket_id`]), which a replay check remembers.
    fn ticket_id(&self, ticket: &[u8]) -> Vec<u8> {
        ticket.to_vec()
    }

    fn mods_support(&self, _fields: &ConnectFields) -> Option<ModsSection> {
        None
    }
}

/// Auth tickets: the client's half makes one for the server it connects to, the server's half verifies it. A game supplies one
/// provider type used on both sides (a Steam session ticket, a signed token, a shared secret); the engine's
/// `net().auth(provider)` registers it.
///
/// **The contract.** A ticket must be good for one server and one connection and nothing else: a provider binds it to the
/// [`AuthContext`] (sign `cx.encode()`, or put `cx`'s address and challenge in the token), or uses its platform's own audience or
/// identity field for the server (a Steam ticket names the server it was made for) and checks it. `verify` must then refuse a
/// ticket whose binding is not the `cx` it is given. A provider that ignores `cx` lets any server a client joins replay the
/// client's ticket to any other server that accepts the same credentials.
pub trait AuthProvider: fmt::Debug + Send + Sync {
    /// The client's ticket for the connection `cx` names (at most [`MAX_TICKET_BYTES`]). An `Err` is why there is none (not
    /// signed in), shown to the player; the connect ends there.
    fn ticket(&self, cx: &AuthContext) -> Result<Vec<u8>, String>;

    /// The server's check of a ticket from `from` for the connection `cx` names (an empty ticket when the client sent none): the
    /// [`Identity`] it stands for, or the reason it is refused (cut at 255 bytes on the wire). A slow check belongs to a
    /// provider that answers from a cache; this runs inside the handshake.
    ///
    /// The identity must come from what the provider verified (a signature, a platform's answer), never from unchecked text in
    /// the ticket, and its id must be the same for the same account on every connection: roles and ownership are kept under
    /// it. A provider that cannot tell accounts apart should not be a provider: a connection with no identity is already held
    /// by the connection.
    fn verify(&self, from: SocketAddr, cx: &AuthContext, ticket: &[u8])
    -> Result<Identity, String>;

    /// The canonical token a ticket stands for, which a server's replay check remembers (two encodings of one token are one
    /// ticket). The bytes themselves unless the provider's tickets have more than one encoding.
    fn ticket_id(&self, ticket: &[u8]) -> Vec<u8> {
        ticket.to_vec()
    }
}

/// The standard gate: the server's manifest the client's must satisfy, then the auth provider's verdict.
#[derive(Debug, Clone)]
pub struct ManifestGate {
    manifest: Manifest,
    auth: Option<Arc<dyn AuthProvider>>,
    required: RequiredMods,
}

impl ManifestGate {
    /// A gate that holds clients to `manifest`.
    pub fn new(manifest: Manifest) -> ManifestGate {
        ManifestGate {
            manifest,
            auth: None,
            required: RequiredMods::default(),
        }
    }

    /// The same gate that holds clients to the mods in `required` (the list may be set later, through a clone of the handle).
    #[must_use]
    pub fn with_required(mut self, required: RequiredMods) -> ManifestGate {
        self.required = required;
        self
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

    /// The mods clients must hold, shared with the gate: setting the list changes what the gate judges from then on.
    pub fn required(&self) -> &RequiredMods {
        &self.required
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
    fn ticket_id(&self, ticket: &[u8]) -> Vec<u8> {
        self.auth
            .as_ref()
            .map_or_else(|| ticket.to_vec(), |auth| auth.ticket_id(ticket))
    }

    fn mods_support(&self, fields: &ConnectFields) -> Option<ModsSection> {
        fields.block.as_ref().and_then(|b| b.mods.clone())
    }

    fn matches_channels(&self, fields: &ConnectFields) -> bool {
        let theirs = fields
            .block
            .as_ref()
            .and_then(|b| b.manifest.as_ref())
            .map(|m| &m.channels);
        self.manifest.channels.count() > 0
            && theirs.is_some_and(|t| self.manifest.channels.same_as(t))
    }

    fn check(
        &self,
        from: SocketAddr,
        cx: &AuthContext,
        fields: &ConnectFields,
    ) -> Result<Option<Identity>, Vec<u8>> {
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
        // The server's mods are sent to the client after the accept and the client picks what to fetch: the gate only turns away
        // a client that cannot take content at all.
        if !self.required.is_empty() && block.and_then(|b| b.mods.as_ref()).is_none() {
            return Err(reason(
                "server requires mods, and your client cannot download them",
            ));
        }
        match &self.auth {
            None => Ok(None),
            Some(auth) => {
                let ticket = block.and_then(|b| b.ticket.as_deref()).unwrap_or_default();
                auth.verify(from, cx, ticket)
                    .map(Some)
                    .map_err(|why| reason(&why))
            }
        }
    }
}

/// A provider for tests and private games only: the ticket is a name and a keyed hash of the name and the connection it is for
/// ([`AuthContext`]: the server's address and the challenge) under a shared 128-bit secret, so a server holding the secret accepts
/// a name it signed for this server and this connection and nothing else. The identity is the name.
///
/// A ticket made for one server is refused by another (its address differs), and a ticket made for one connection is refused on
/// another (its challenge differs), so a server that relays what a client sent it, or anyone who sniffs a ticket, gets nothing
/// that works elsewhere. A server that was never told its own address ([`AuthContext::server_unknown`]) refuses every ticket.
///
/// It is still not for a public server: every server and client holds the secret itself, so anyone with a copy can make a ticket
/// for any name. The identity is the name, as id and as display name, so one name is one player on every connection.
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

    /// The keyed hash of `name` for the connection `cx` names (the length first, so a name and a context cannot trade bytes).
    fn mac(&self, name: &[u8], cx: &AuthContext) -> [u8; 8] {
        let mut data = Vec::with_capacity(name.len() + 48);
        data.extend_from_slice(&(name.len() as u16).to_le_bytes());
        data.extend_from_slice(name);
        data.extend_from_slice(&cx.encode());
        siphash24(&self.key, &data).to_le_bytes()
    }
}

impl AuthProvider for SharedSecret {
    fn ticket(&self, cx: &AuthContext) -> Result<Vec<u8>, String> {
        if self.name.is_empty() {
            return Err("not signed in".to_string());
        }
        let mut ticket = self.name.clone();
        ticket.extend_from_slice(&self.mac(&self.name, cx));
        Ok(ticket)
    }

    fn verify(
        &self,
        _from: SocketAddr,
        cx: &AuthContext,
        ticket: &[u8],
    ) -> Result<Identity, String> {
        let Some(split) = ticket.len().checked_sub(8) else {
            return Err("sign in to join this server".to_string());
        };
        if cx.server_unknown() {
            return Err(
                "this server cannot check sign-ins (it does not know its own address)".to_string(),
            );
        }
        let (name, mac) = ticket.split_at(split);
        // Every byte is compared, whatever the first difference: the check takes the same time for a near miss as for a wild one.
        let want = self.mac(name, cx);
        let diff = mac.iter().zip(&want).fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if name.is_empty() || diff != 0 {
            return Err("your sign-in was not accepted".to_string());
        }
        let identity = Identity::new(name).map_err(|e| e.to_string())?;
        Ok(identity.with_name(&String::from_utf8_lossy(name)))
    }
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
