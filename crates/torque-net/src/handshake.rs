//! The connection handshake and the connection tables (`NetInterface::processPacketReceiveEvent`): the challenge request, the
//! challenge response, the connect request, the reject, the accept and the disconnect (TGE's datagram types 26, 30, 32, 34, 36 and
//! 38; a profile may number them otherwise, [`HandshakeTypes`]), the keyed challenge digest, the pending and established tables and the caps that bound them. A pure state machine over datagrams: bytes in,
//! bytes out, `now` from the caller's sim clock; the script that decides a connect request (`onConnectRequest`) stays with the host,
//! which answers a [`PendingRequest`] with [`Server::finish_request`].
//!
//! **Datagram.** Byte 0 bit 0 set is a game packet (looked up by source address); otherwise byte 0 is the type. Integers are raw
//! 32-bit words, strings are the Huffman form without the packet string buffer (at most 255 bytes).
//!
//! **No state before the digest returns.** A challenge request stores nothing: the reply carries a keyed hash (SipHash-2-4 under a
//! 128-bit key the caller draws from the OS, two runs for 128 bits) of the source address, port, the client's sequence and a time epoch
//! ([`CHALLENGE_PERIOD_MS`]; a digest verifies in its epoch and the next, so it lives one to two periods). A connect
//! request is looked at only after its digest verifies. The challenge and request counters are kept per address and per network
//! ([`crate::fair`]) in bounded tables that forget the least recently used entry first. The challenge digest of a standard
//! server is opaque to the client and is not copied.
//!
//! **Fair caps.** A source holds at most [`SourceCaps`] connections: pending ones per address and per network (a request past
//! them is silence) and all of them per address and per network (refused with
//! [`RejectStrings::too_many_from_source`](crate::profile::RejectStrings::too_many_from_source)). When
//! [`NET_MAX_PENDING_CONNECTS`] are pending, or the table is full, a requester whose network already holds its fair share of
//! the pending connections (those held over the networks holding them, the requester's counted) is refused; any other's
//! accept evicts the oldest pending connection of a network over its share ([`Finished::Accepted`]'s `evicted`), else one
//! at least [`NET_PENDING_EVICT_AGE_MS`] old, else it is refused. So addresses that are accepted and never speak cannot lock
//! others out, and cannot evict an honest client of another network; a pending connection expires after
//! [`NET_PENDING_TIMEOUT_MS`]. The caps count each
//! connection by its peer's own IP address ([`Connection::ip`]), which a host behind stand-in addresses passes in
//! ([`Server::handle_datagram_from`]).
//!
//! **Order of the reject strings** (the connect request's fields, in the order they are checked; the strings and the versions are
//! the [`HandshakeProfile`]'s): group and CRC (`CHR_INVALID`), game (`CHR_GAME`), current protocol below the oldest accepted
//! (`CHR_PROTOCOL_LESS`), minimum protocol above the newest spoken (`CHR_PROTOCOL_GREATER`), password (`CHR_PASSWORD`), more than 16
//! arguments (`CR_INVALID_ARGS`); then the server's gate when it has one ([`crate::connect`]: the build's manifest, then the auth
//! ticket); then the host's script decides. Everything else (bad digest, unknown class, switched-off
//! connections, a short datagram) is silence.
//!
//! **Deviations from Torque.** A repeated request is answered only if its digest verifies (the accept is not re-sent before the
//! check); an older connection from the same address is dropped only once the new request passed every check; what a profile's
//! form reads after the password is kept for the host, never checked.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netInterface.cpp` (Copyright (c) 2012
//! GarageGames, LLC) and `T3D/gameBase/gameConnection.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust
//! Logic, Inc.); the code here is this crate's own.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{
    self, NET_CHALLENGES_PER_ADDR_PER_S, NET_CHALLENGES_PER_NETWORK_PER_S,
    NET_CONNECT_SCRIPT_PER_SRC_PER_S, NET_CONNECTIONS_MARGIN, NET_DISCONNECTS_PER_CONN_PER_S,
    NET_MAX_CONNECT_ARGS, NET_MAX_DATAGRAM, NET_MAX_PENDING_CONNECTS, NET_NETWORK_SHARE,
    NET_PACKETS_PER_CONN_PER_S, NET_PENDING_EVICT_AGE_MS, NET_PENDING_TIMEOUT_MS, NET_STRING_BYTES,
};
use crate::connect::{AuthContext, ConnectBlock, ConnectGate, Identity};
use crate::fair::{
    CapHit, Eviction, FairLimiter, FairLimits, PendingSlot, SourceCaps, SourceCount, Stage,
    choose_eviction,
};

pub use crate::fair::{RateWindow, SourceLimiter};
use crate::huffman::Table;
use crate::profile::{DEFAULT_GHOST_LIMIT, HandshakeProfile};
use crate::protocol::{Dropped, Protocol, RateConfig, Received, Role, SendPacing, Timeout};

/// What [`Server::make_room`] found for one more pending connection.
enum Room {
    /// There is room.
    Free,
    /// This pending connection left to make room (the host drops it).
    Evicted(ConnId),
    /// No room, and nothing was removed.
    Full,
}

/// The first bytes of the handshake's datagrams (`NetInterface::PacketTypes`). [`HandshakeTypes::TORQUE`] is TGE's numbering; the
/// codecs are methods so a game's numbering travels with its [`HandshakeProfile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeTypes {
    pub challenge_request: u8,
    /// Defined beside the others; nothing here sends it.
    pub challenge_reject: u8,
    pub challenge_response: u8,
    pub connect_request: u8,
    pub connect_reject: u8,
    pub connect_accept: u8,
    pub disconnect: u8,
}

impl HandshakeTypes {
    /// TGE's numbering (`netInterface.h`): 26 challenge request, 28 challenge reject, 30 challenge response, 32 connect request,
    /// 34 reject, 36 accept, 38 disconnect.
    pub const TORQUE: HandshakeTypes = HandshakeTypes {
        challenge_request: 26,
        challenge_reject: 28,
        challenge_response: 30,
        connect_request: 32,
        connect_reject: 34,
        connect_accept: 36,
        disconnect: 38,
    };
}

impl Default for HandshakeTypes {
    fn default() -> HandshakeTypes {
        HandshakeTypes::TORQUE
    }
}

/// The only class a connect request may create.
pub const GAME_CONNECTION_CLASS: &[u8] = b"GameConnection";
/// The net class group and class CRC a game connection sends (CRC of group 0).
pub const CLASS_GROUP: u32 = 0;
pub const CLASS_CRC: u32 = 0xffff_ffff;

// ---- the keyed digest ---------------------------------------------------------------------------------------------------------

fn le64(bytes: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    for (slot, b) in a.iter_mut().zip(bytes) {
        *slot = *b;
    }
    u64::from_le_bytes(a)
}

fn sip_round(v: &mut [u64; 4]) {
    v[0] = v[0].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(13) ^ v[0];
    v[0] = v[0].rotate_left(32);
    v[2] = v[2].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(16) ^ v[2];
    v[0] = v[0].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(21) ^ v[0];
    v[2] = v[2].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(17) ^ v[2];
    v[2] = v[2].rotate_left(32);
}

/// SipHash-2-4 (Aumasson and Bernstein) of `data` under a 128-bit key.
pub fn siphash24(key: &[u8; 16], data: &[u8]) -> u64 {
    let k0 = le64(&key[..8]);
    let k1 = le64(&key[8..]);
    let mut v = [
        k0 ^ 0x736f_6d65_7073_6575,
        k1 ^ 0x646f_7261_6e64_6f6d,
        k0 ^ 0x6c79_6765_6e65_7261,
        k1 ^ 0x7465_6462_7974_6573,
    ];
    let (chunks, rest) = data.as_chunks::<8>();
    for chunk in chunks {
        let m = le64(chunk);
        v[3] ^= m;
        sip_round(&mut v);
        sip_round(&mut v);
        v[0] ^= m;
    }
    let last = le64(rest) | ((data.len() as u64 & 0xff) << 56);
    v[3] ^= last;
    sip_round(&mut v);
    sip_round(&mut v);
    v[0] ^= last;
    v[2] ^= 0xff;
    for _ in 0..4 {
        sip_round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

/// How long a challenge digest stays in one epoch (milliseconds on the caller's clock, which wraps every 2^32 ms: the server
/// counts the wraps itself, so an epoch never comes round again). A digest made in one epoch verifies in it and in the next, so a
/// digest lives between one and two periods and one older than that is refused, whatever the source. The clients only echo the
/// digest, so the period is the server's own business.
pub const CHALLENGE_PERIOD_MS: u32 = 120_000;

/// The challenge digest of epoch 0 ([`challenge_digest_at`]).
pub fn challenge_digest(key: &[u8; 16], from: SocketAddr, seq: u32) -> [u32; 4] {
    challenge_digest_at(key, from, seq, 0)
}

/// The challenge digest: four words, the keyed hash of the source address, port, the client's sequence and the epoch (the
/// caller's clock over [`CHALLENGE_PERIOD_MS`]).
pub fn challenge_digest_at(key: &[u8; 16], from: SocketAddr, seq: u32, epoch: u64) -> [u32; 4] {
    let mut data = Vec::with_capacity(32);
    data.push(0);
    match from.ip() {
        IpAddr::V4(ip) => {
            data.push(4);
            data.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            data.push(6);
            data.extend_from_slice(&ip.octets());
        }
    }
    data.extend_from_slice(&from.port().to_le_bytes());
    data.extend_from_slice(&seq.to_le_bytes());
    data.extend_from_slice(&epoch.to_le_bytes());
    let a = siphash24(key, &data);
    data[0] = 1;
    let b = siphash24(key, &data);
    [a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32]
}

// ---- datagram codecs (both halves) --------------------------------------------------------------------------------------------

/// What the connect request carries after the digest, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectFields {
    pub group: u32,
    pub crc: u32,
    pub game: Vec<u8>,
    pub current_protocol: u32,
    pub min_protocol: u32,
    pub password: Vec<u8>,
    /// What the profile's [`crate::profile::HandshakeForm`] carries after the password (empty in Torque).
    pub extra: Vec<u8>,
    pub args: Vec<Vec<u8>>,
    /// What follows the arguments: the sender's manifest and auth ticket ([`ConnectBlock`]). `None` for a plain Torque request,
    /// which is then the request it always was; a server reads it only when it has a [`ServerConfig::gate`].
    pub block: Option<Box<ConnectBlock>>,
}

impl ConnectFields {
    /// The fields a client of `profile` sends: its game string and protocol versions, its form's default bytes after the password,
    /// the given arguments.
    pub fn new(profile: &HandshakeProfile, password: &[u8], args: Vec<Vec<u8>>) -> ConnectFields {
        ConnectFields {
            group: CLASS_GROUP,
            crc: CLASS_CRC,
            game: profile.game.clone(),
            current_protocol: profile.protocol_current,
            min_protocol: profile.protocol_min,
            password: password.to_vec(),
            extra: profile.form.default_request(),
            args,
            block: None,
        }
    }

    /// These fields with `block` after the arguments.
    #[must_use]
    pub fn with_block(mut self, block: ConnectBlock) -> ConnectFields {
        self.block = Some(Box::new(block));
        self
    }

    /// [`ConnectFields::new`] for Torque's profile.
    pub fn standard(password: &[u8], args: Vec<Vec<u8>>) -> ConnectFields {
        ConnectFields::new(&HandshakeProfile::default(), password, args)
    }
}

fn header(ty: u8, seq: u32) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(ty);
    w.write_u32(seq);
    w
}

impl HandshakeTypes {
    /// The client's challenge request.
    pub fn challenge_request(&self, seq: u32) -> Vec<u8> {
        header(self.challenge_request, seq).into_bytes()
    }

    /// The server's challenge response.
    pub fn challenge_response(&self, seq: u32, digest: [u32; 4]) -> Vec<u8> {
        let mut w = header(self.challenge_response, seq);
        for word in digest {
            w.write_u32(word);
        }
        w.into_bytes()
    }

    /// A reject with its reason (cut at 255 bytes and the first NUL).
    pub fn connect_reject(&self, table: &Table, seq: u32, reason: &[u8]) -> Vec<u8> {
        let mut w = header(self.connect_reject, seq);
        table.write(&mut w, reason, NET_STRING_BYTES);
        w.into_bytes()
    }

    /// A disconnect with its reason.
    pub fn disconnect(&self, table: &Table, seq: u32, reason: &[u8]) -> Vec<u8> {
        let mut w = header(self.disconnect, seq);
        table.write(&mut w, reason, NET_STRING_BYTES);
        w.into_bytes()
    }

    /// The challenge response: `(seq, digest)`.
    pub fn parse_challenge_response(&self, data: &[u8]) -> Result<(u32, [u32; 4]), NetError> {
        let mut r = BitReader::new(data);
        if r.read_u8()? != self.challenge_response {
            return Err(NetError::Invalid("datagram type"));
        }
        let seq = r.read_u32()?;
        let mut digest = [0u32; 4];
        for word in &mut digest {
            *word = r.read_u32()?;
        }
        Ok((seq, digest))
    }

    /// The reject: `(seq, reason)`.
    pub fn parse_reject(&self, table: &Table, data: &[u8]) -> Result<(u32, Vec<u8>), NetError> {
        parse_reason(table, data, self.connect_reject)
    }

    /// A disconnect, read with a check of its reason: `(seq, reason)`.
    pub fn parse_disconnect_reason(
        &self,
        table: &Table,
        data: &[u8],
    ) -> Result<(u32, Vec<u8>), NetError> {
        parse_reason(table, data, self.disconnect)
    }

    /// A Disconnect as a server takes one (`processPacketReceiveEvent`): the sequence, then the reason read
    /// without a check (the connection is dropped whatever the reason reads as). An error reading the reason is not an error of the
    /// datagram: the reader returns what the bits give, and so does this one as far as it can. When the plain read fails, the reason
    /// is read as the string-buffer form: `sendDisconnectPacket` (`netInterface.cpp`) writes the reason into the global packet
    /// stream, whose position `getPacketStream` resets but whose string buffer it does not, so the reason may carry `writeString`'s
    /// flag bit before the text (and, when the flag is set, the 8-bit length of a prefix that is not known here).
    pub fn parse_disconnect(&self, table: &Table, data: &[u8]) -> Result<(u32, Vec<u8>), NetError> {
        let mut r = BitReader::new(data);
        if r.read_u8()? != self.disconnect {
            return Err(NetError::Invalid("datagram type"));
        }
        let seq = r.read_u32()?;
        let bits = data.len() * 8;
        // A reading is the writer's when it ends in the datagram's last byte (the datagram is the written bits rounded up to a byte).
        let ends = |r: &BitReader| bits.saturating_sub(r.position()) < 8;
        let mut plain = r.clone();
        let plain = table.read(&mut plain).map(|s| (s, ends(&plain)));
        let mut buffered = r;
        let buffered = (|| -> Result<(Vec<u8>, bool), NetError> {
            if buffered.read_flag()? {
                buffered.read_bits(8)?;
            }
            let s = table.read(&mut buffered)?;
            Ok((s, ends(&buffered)))
        })();
        let reason = match (plain, buffered) {
            (Ok((s, true)), _) => s,
            (_, Ok((s, true))) => s,
            (Ok((s, false)), _) => s,
            (_, Ok((s, false))) => s,
            _ => Vec::new(),
        };
        Ok((seq, c_string(reason)))
    }
}

/// The client's connect request as a client of `profile` writes it (`class` and `fields` as given, so a test can send a bad one).
pub fn connect_request(
    profile: &HandshakeProfile,
    table: &Table,
    seq: u32,
    digest: [u32; 4],
    class: &[u8],
    fields: &ConnectFields,
) -> Vec<u8> {
    let mut w = header(profile.types.connect_request, seq);
    for word in digest {
        w.write_u32(word);
    }
    table.write(&mut w, class, NET_STRING_BYTES);
    w.write_u32(fields.group);
    w.write_u32(fields.crc);
    table.write(&mut w, &fields.game, NET_STRING_BYTES);
    w.write_u32(fields.current_protocol);
    w.write_u32(fields.min_protocol);
    table.write(&mut w, &fields.password, NET_STRING_BYTES);
    profile.form.write_request(&mut w, &fields.extra);
    w.write_u32(fields.args.len() as u32);
    for arg in &fields.args {
        table.write(&mut w, arg, NET_STRING_BYTES);
    }
    if let Some(block) = &fields.block {
        // A block that cannot be written (a manifest over its limit, a ticket too long) is left off: the server then judges
        // the request as one with nothing, and says what it needs. Whoever builds a block checks it first
        // ([`ConnectBlock::validate`]; `Engine::connect_block` does), so this is a bug in the caller: loud in a debug build.
        let mark = w.mark();
        let written = block.write(&mut w);
        debug_assert!(written.is_ok(), "an unwritable connect block: {written:?}");
        if written.is_err() {
            w.rewind(mark);
        }
    }
    w.into_bytes()
}

/// The accept as a server of `profile` writes it: Torque's is the protocol alone; the profile's form may add its fields after it
/// ([`crate::profile::HandshakeForm::write_accept`]); a connection that asked for the profile's extension gets the extension's word
/// last.
pub fn connect_accept(
    profile: &HandshakeProfile,
    seq: u32,
    protocol: u32,
    ghost_bits: u8,
    extension: bool,
) -> Vec<u8> {
    connect_accept_with(profile, seq, protocol, ghost_bits, extension, false)
}

/// The word that ends an accept sent to a client whose manifest listed bulk channels, when the server's gate matched them: from the
/// next packet on, the server writes the channels' section and the client reads it. Only a request that carried a manifest with
/// channels is ever answered with it, so a peer without plugins sees Torque's accept.
pub const CHANNELS_ACCEPT_WORD: u32 = 0x4c4e_4843;

/// [`connect_accept`] that also says `channels` (see [`CHANNELS_ACCEPT_WORD`]), after the extension's word.
pub fn connect_accept_with(
    profile: &HandshakeProfile,
    seq: u32,
    protocol: u32,
    ghost_bits: u8,
    extension: bool,
    channels: bool,
) -> Vec<u8> {
    let mut w = header(profile.types.connect_accept, seq);
    w.write_u32(protocol);
    profile.form.write_accept(&mut w, protocol, ghost_bits);
    if let Some(ext) = profile.extension.as_ref().filter(|_| extension) {
        w.write_u32(ext.accept_word);
    }
    if channels {
        w.write_u32(CHANNELS_ACCEPT_WORD);
    }
    w.into_bytes()
}

/// `type, seq` of a datagram that is not a game packet.
pub fn peek_type(data: &[u8]) -> Result<(u8, u32), NetError> {
    let mut r = BitReader::new(data);
    let ty = r.read_u8()?;
    Ok((ty, r.read_u32()?))
}

/// What an accept says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accept {
    pub seq: u32,
    pub protocol: u32,
    /// The ghost-id width; `None` when the accept carries none (Torque's accept never does).
    pub ghost_bits: Option<u32>,
    /// The accept ended with the profile's extension word: the server granted the extension (only a request that named the
    /// extension's argument gets it).
    pub extension: bool,
    /// The accept ended with [`CHANNELS_ACCEPT_WORD`]: the server's gate matched the bulk channels the client's manifest
    /// listed, and it writes their section from its first data packet.
    pub channels: bool,
}

/// The accept, read as a client of `profile` reads it: the protocol, what the profile's form reads after it (the ghost-id width
/// when it carries one), and the extension's word when the profile has an extension (a missing or other word is no extension).
pub fn parse_accept(profile: &HandshakeProfile, data: &[u8]) -> Result<Accept, NetError> {
    let mut r = BitReader::new(data);
    if r.read_u8()? != profile.types.connect_accept {
        return Err(NetError::Invalid("datagram type"));
    }
    let seq = r.read_u32()?;
    let protocol = r.read_u32()?;
    let ghost_bits = profile.form.read_accept(&mut r, protocol)?;
    let mut next = None;
    let mut extension = false;
    if let Some(ext) = profile.extension.as_ref() {
        next = r.read_u32().ok();
        if next == Some(ext.accept_word) {
            extension = true;
            next = r.read_u32().ok();
        }
    }
    let channels = match next {
        Some(word) => word == CHANNELS_ACCEPT_WORD,
        None => r.read_u32().is_ok_and(|w| w == CHANNELS_ACCEPT_WORD),
    };
    Ok(Accept {
        seq,
        protocol,
        ghost_bits,
        extension,
        channels,
    })
}

/// A datagram of type `ty` that carries a reason: `(seq, reason)`.
fn parse_reason(table: &Table, data: &[u8], ty: u8) -> Result<(u32, Vec<u8>), NetError> {
    let mut r = BitReader::new(data);
    if r.read_u8()? != ty {
        return Err(NetError::Invalid("datagram type"));
    }
    let seq = r.read_u32()?;
    Ok((seq, c_string(table.read(&mut r)?)))
}

pub(crate) fn c_string(mut bytes: Vec<u8>) -> Vec<u8> {
    if let Some(nul) = bytes.iter().position(|&b| b == 0) {
        bytes.truncate(nul);
    }
    bytes
}

/// What the connect request's fields came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fields {
    Ok(ConnectFields),
    /// The request is refused with this reason.
    Reject(Vec<u8>),
}

/// What the server checks a request against.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// `allowConnections`: when off, every handshake datagram is dropped.
    pub allow_connections: bool,
    /// The game string, the versions, the reject strings and the extension.
    pub profile: HandshakeProfile,
    /// `$Pref::Server::Password`; empty means none.
    pub password: Vec<u8>,
    /// `$Pref::Server::MaxPlayers`; the table holds this plus [`NET_CONNECTIONS_MARGIN`].
    pub max_players: usize,
    /// The ghost limit: the connections' ghost-id width is [`HandshakeProfile::ghost_bits`] of it.
    pub ghost_limit: u32,
    /// The rates a new connection's packet layer starts with (`checkMaxRate`'s preferences).
    pub rates: RateConfig,
    /// What a game adds to the send gate of every connection the server makes; `None` is TGE's gate.
    pub pacing: Option<Box<dyn SendPacing>>,
    /// The connections one source IP address and one source network may hold, pending and in all ([`SourceCaps::default`]; a
    /// harness running many fake clients from one address raises them).
    pub caps: SourceCaps,
    /// Verified connect requests one source IP address gets per second ([`NET_CONNECT_SCRIPT_PER_SRC_PER_S`]).
    pub connect_requests_per_src_per_s: u32,
    /// The size every network connection's sections fill a packet to ([`Protocol::set_packet_bytes`]); None is the standard rule
    /// (the rate's size). A connection granted the profile's extension gets the extension's size instead.
    pub packet_bytes: Option<u32>,
    /// The size the local connection's sections fill a packet to (its packets stay in memory); None is the standard rule.
    pub local_packet_bytes: Option<u32>,
    /// A judgment of every request that passed the standard checks, before the host's scripts see it ([`ConnectGate`]); `None`
    /// is Torque's server, which reads nothing after the arguments.
    pub gate: Option<Arc<dyn ConnectGate>>,
    /// The address clients reach this server by (its public address: a socket bound to every interface, a NAT or a relay hides it),
    /// which the gate's auth provider binds tickets to ([`AuthContext::server`]). `None` when the host was never told one: a
    /// provider that binds to the address then refuses every ticket.
    pub address: Option<SocketAddr>,
    /// The host has the bulk channels' layer installed: a client the gate says has the same channels
    /// ([`ConnectGate::matches_channels`]) gets the accept's channels word ([`CHANNELS_ACCEPT_WORD`]). Off, no accept carries it.
    pub channels: bool,
}

impl ServerConfig {
    /// A server of Torque's profile.
    pub fn new(max_players: usize) -> ServerConfig {
        ServerConfig::with_profile(max_players, HandshakeProfile::default())
    }

    /// A server of a game's profile.
    pub fn with_profile(max_players: usize, profile: HandshakeProfile) -> ServerConfig {
        ServerConfig {
            allow_connections: true,
            profile,
            password: Vec::new(),
            max_players,
            ghost_limit: DEFAULT_GHOST_LIMIT,
            rates: RateConfig::standard(),
            pacing: None,
            caps: SourceCaps::default(),
            connect_requests_per_src_per_s: NET_CONNECT_SCRIPT_PER_SRC_PER_S,
            packet_bytes: None,
            local_packet_bytes: None,
            gate: None,
            address: None,
            channels: false,
        }
    }

    /// The established connections the table holds.
    pub fn max_connections(&self) -> usize {
        self.max_players.saturating_add(NET_CONNECTIONS_MARGIN)
    }
}

/// Reads and checks the connect request's fields after the class name, in the standard order. A short stream is an `Err` (silence).
pub fn read_connect_fields(
    r: &mut BitReader,
    table: &Table,
    config: &ServerConfig,
) -> Result<Fields, NetError> {
    let profile = &config.profile;
    let reject = |s: &Vec<u8>| Ok(Fields::Reject(s.clone()));
    let group = r.read_u32()?;
    let crc = r.read_u32()?;
    if group != CLASS_GROUP || crc != CLASS_CRC {
        return reject(&profile.rejects.invalid);
    }
    let game = c_string(table.read(r)?);
    if game != profile.game {
        return reject(&profile.rejects.game);
    }
    let current = r.read_u32()?;
    let min = r.read_u32()?;
    if current < profile.protocol_min {
        return reject(&profile.rejects.protocol_less);
    }
    if min > profile.protocol_current {
        return reject(&profile.rejects.protocol_greater);
    }
    let password = c_string(table.read(r)?);
    if !config.password.is_empty() && password != config.password {
        return reject(&profile.rejects.password);
    }
    let extra = profile.form.read_request(r)?;
    let argc = r.read_u32()?;
    if argc as usize > NET_MAX_CONNECT_ARGS {
        return reject(&profile.rejects.invalid_args);
    }
    // Every string takes 9 bits at least (a flag and a length).
    let argc = cap::count(
        "connect arguments",
        u64::from(argc),
        NET_MAX_CONNECT_ARGS,
        r.bits_left(),
        9,
    )?;
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        args.push(c_string(table.read(r)?));
    }
    // Only a server with a gate looks past the arguments.
    let block = if config.gate.is_some() {
        match ConnectBlock::read(r) {
            Ok(block) => block.map(Box::new),
            Err(error) => return Ok(Fields::Reject(crate::connect::reason(&error.to_string()))),
        }
    } else {
        None
    };
    Ok(Fields::Ok(ConnectFields {
        group,
        crc,
        game,
        current_protocol: current,
        min_protocol: min,
        password,
        extra,
        args,
        block,
    }))
}

// ---- the tables ---------------------------------------------------------------------------------------------------------------

/// A connection's identity in the table; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(pub u32);

/// One accepted connection.
#[derive(Debug)]
pub struct Connection {
    pub id: ConnId,
    pub addr: SocketAddr,
    /// The peer's own IP address, which the source caps count by: `addr`'s, or the real one a transport knows behind a
    /// stand-in address.
    pub ip: IpAddr,
    /// The client's connect sequence (the request's).
    pub sequence: u32,
    pub protocol: Protocol,
    /// The negotiated protocol version, `min(client, profile's current)`.
    pub protocol_version: u32,
    pub ghost_bits: u8,
    /// What the profile's form read after the request's password (empty in Torque).
    pub extra: Vec<u8>,
    /// What the gate's auth provider verified the client as, when there is one ([`Identity`]). `None` is an anonymous player,
    /// held by this connection's id alone: it is a new player on every connection.
    pub identity: Option<Identity>,
    /// Accepted, but no valid game packet has arrived yet.
    pub pending: bool,
    /// The server half of a local connection (`connectLocal`): never pinged or timed out (TGE's `checkTimeout` acts only
    /// on network connections), its packets carried in memory.
    pub local: bool,
    /// The connection has the profile's extension: the local connection, or a request that ended with the extension's argument.
    pub extension: bool,
    /// The client's manifest listed bulk channels and this server's gate accepted it, so the channels are the same on both
    /// sides: the accept told the client so ([`CHANNELS_ACCEPT_WORD`]) and the server writes the channels' section.
    pub channels: bool,
    /// What the client said about taking content from the server ([`ConnectGate::mods_support`]); `None` unless the connection has
    /// channels.
    pub mods: Option<crate::modsync::ModsSection>,
    /// The connection's packets ride an engine-mode session ([`crate::engine`]) the host keeps: this table never pings it or
    /// times it out, and its `protocol` only keeps the sequence numbers and the packet size its payload is written with.
    pub engine: bool,
    created: u32,
    packets: RateWindow,
    disconnects: RateWindow,
}

impl Connection {
    /// Where the connection is, as the source caps count it.
    #[must_use]
    pub fn stage(&self) -> Stage {
        if self.pending {
            Stage::Pending
        } else {
            Stage::Established
        }
    }
}

/// The verified connect requests a second: `per_ip` from one address, and `NET_NETWORK_SHARE / 2` times that from one network
/// (a few addresses of one network can each retry; the host's total is shared out per network on top).
#[must_use]
pub fn verified_limits(per_ip: u32) -> FairLimits {
    FairLimits::per_source(per_ip, per_ip.saturating_mul(NET_NETWORK_SHARE / 2))
}

/// A connect request that passed every check the protocol can make; the host runs `onConnectRequest` over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    pub addr: SocketAddr,
    /// The peer's own IP address ([`Connection::ip`]).
    pub ip: IpAddr,
    pub seq: u32,
    pub fields: ConnectFields,
    /// What the gate's auth provider verified the client as, when there is one.
    pub identity: Option<Identity>,
    /// `min(current, profile's current)`.
    pub protocol_version: u32,
    /// An older connection from the same address that this request replaces. It is still in the table: the host removes it once it admits the
    /// request (its own rate and script budget), so a request it drops keeps the old session; `finish_request` removes one the host left.
    pub replaced: Option<ConnId>,
    /// The request ended with the profile's extension argument (taken off `fields.args`).
    pub extension: bool,
}

/// What [`Server::insert_engine`] enters: an engine-mode connection the host accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineAccepted {
    pub addr: SocketAddr,
    /// The peer's own IP address ([`Connection::ip`]).
    pub ip: IpAddr,
    /// The ghost-id width the accept carried.
    pub ghost_bits: u8,
    /// What the gate's auth provider verified the client as, when there is one.
    pub identity: Option<Identity>,
    /// What the client said about taking content from the server.
    pub mods: Option<crate::modsync::ModsSection>,
}

/// The host's answer to a [`PendingRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Accept,
    /// Refuse with this reason (the script's return string; cut at 255 bytes).
    Reject(Vec<u8>),
}

/// What `finish_request` did.
#[derive(Debug)]
pub enum Finished {
    /// The connection is in the table. `replaced` is a connection from the same address that was still there and was removed.
    Accepted {
        conn: ConnId,
        reply: Vec<u8>,
        replaced: Option<ConnId>,
        /// The oldest pending connection, taken out of the table to make room (the pending table or the whole table was
        /// full). The host drops it as it drops one whose pending time ran out.
        evicted: Option<ConnId>,
    },
    Rejected {
        reply: Vec<u8>,
    },
    /// The accept came too late: a connection from the same address with the same or a newer sequence is already in the table.
    /// Nothing is sent.
    Refused,
}

/// Why a datagram was dropped without a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ignore {
    Empty,
    TooLong,
    /// The datagram ended inside a field.
    Malformed,
    /// `allowConnections` is off.
    Refused,
    RateLimited,
    BadDigest,
    /// A connect request older than the connection from that address.
    StaleSequence,
    UnknownClass,
    /// The source's address holds [`SourceCaps::pending_per_ip`] pending connections.
    PendingPerAddr,
    /// The source's network holds [`SourceCaps::pending_per_network`] pending connections.
    PendingPerNetwork,
    /// A game packet from an address with no connection.
    UnknownSource,
    /// A disconnect that did not carry the connection's sequence, or came from the wrong address.
    BadSequence,
    /// A handshake type the server does not handle here (client-side types; the info packets are answered by
    /// the host through [`crate::serverquery`] before a datagram gets here).
    UnknownType,
    /// The packet layer dropped it.
    Packet(Dropped),
}

/// What one datagram did.
#[derive(Debug)]
pub enum Handled {
    Ignored(Ignore),
    /// Send this to the source.
    Reply(Vec<u8>),
    /// A connect request for the host to decide.
    Request(Box<PendingRequest>),
    /// A game packet: `received` is the packet layer's result and `payload_bit` where its payload starts (see
    /// [`crate::protocol::reader_at`]).
    Game {
        conn: ConnId,
        received: Received,
        payload_bit: usize,
    },
    /// A valid header followed by a bad rate prefix: the host disconnects with `Packet Error.`.
    GameError {
        conn: ConnId,
        error: NetError,
    },
    /// The peer disconnected; the connection is out of the table.
    Disconnected {
        conn: ConnId,
        reason: Vec<u8>,
    },
}

/// What the time pass found.
#[derive(Debug)]
pub enum TimeoutEvent {
    /// Send this ping to the connection.
    Ping {
        conn: ConnId,
        to: SocketAddr,
        datagram: Vec<u8>,
    },
    /// Silent for 15 pings: removed from the table.
    TimedOut { conn: ConnId },
    /// Accepted but never heard from: removed from the table.
    PendingExpired { conn: ConnId },
}

/// The server side of the handshake and the connection table.
#[derive(Debug)]
pub struct Server {
    config: ServerConfig,
    key: [u8; 16],
    conns: BTreeMap<ConnId, Connection>,
    by_addr: HashMap<SocketAddr, ConnId>,
    next_id: u32,
    challenges: FairLimiter,
    verified: FairLimiter,
    /// The caller's clock counted past the 32-bit clock's wraps, starting at the first time the server was given (see
    /// [`Server::epoch`]).
    clock: u64,
    last_now: Option<u32>,
}

impl Server {
    /// A server whose challenge digests use `key` (drawn from the OS by the caller).
    pub fn new(config: ServerConfig, key: [u8; 16]) -> Server {
        Server {
            config,
            key,
            conns: BTreeMap::new(),
            by_addr: HashMap::new(),
            next_id: 1,
            challenges: FairLimiter::new(FairLimits::per_source(
                NET_CHALLENGES_PER_ADDR_PER_S,
                NET_CHALLENGES_PER_NETWORK_PER_S,
            )),
            verified: FairLimiter::new(verified_limits(NET_CONNECT_SCRIPT_PER_SRC_PER_S)),
            clock: 0,
            last_now: None,
        }
    }

    /// The challenge epoch at `now`: the caller's 32-bit clock is unwrapped into a 64-bit one that only goes forward (a step
    /// back of under half a wrap is a clock that is not quite monotonic and is ignored; a step forward of under half a wrap is
    /// time passing), so an epoch is never reached twice and a digest from before a wrap stays refused. Every datagram and
    /// every [`Server::poll_timeouts`] pass moves it, so a host that calls at least once in 24 days never loses a wrap.
    fn epoch(&mut self, now: u32) -> u64 {
        match self.last_now {
            None => {
                self.clock = u64::from(now);
                self.last_now = Some(now);
            }
            Some(last) => {
                let step = now.wrapping_sub(last);
                if step < 1 << 31 {
                    self.clock += u64::from(step);
                    self.last_now = Some(now);
                }
            }
        }
        self.clock / u64::from(CHALLENGE_PERIOD_MS)
    }

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut ServerConfig {
        &mut self.config
    }

    /// Replaces the digest key: digests handed out before no longer verify.
    pub fn set_key(&mut self, key: [u8; 16]) {
        self.key = key;
    }

    pub fn len(&self) -> usize {
        self.conns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.conns.is_empty()
    }

    /// The width a network connection is given ([`HandshakeProfile::accept_ghost_bits`]), within the extension's
    /// [`crate::profile::WideLimit`]: past it, the standard width.
    pub fn network_ghost_bits(&self, extension: bool) -> u8 {
        let profile = &self.config.profile;
        let bits = profile.accept_ghost_bits(self.config.ghost_limit, extension);
        let (_, standard_max) = profile.ghost_bits_bounds();
        let Some(limit) = profile.extension.as_ref().and_then(|e| e.wide_limit) else {
            return bits;
        };
        if bits <= standard_max {
            return bits;
        }
        let wide = self
            .conns
            .values()
            .filter(|c| !c.local && c.ghost_bits > standard_max);
        let (n, slots) = wide.fold((0usize, 0u64), |(n, s), c| {
            (n + 1, s.saturating_add(1u64 << c.ghost_bits.min(63)))
        });
        if n < limit.clients && slots.saturating_add(1u64 << bits) <= limit.slots {
            bits
        } else {
            profile.ghost_bits(self.config.ghost_limit)
        }
    }

    /// Connections accepted and not yet heard from.
    pub fn pending_len(&self) -> usize {
        self.conns.values().filter(|c| c.pending).count()
    }

    pub fn connection(&self, id: ConnId) -> Option<&Connection> {
        self.conns.get(&id)
    }

    pub fn connection_mut(&mut self, id: ConnId) -> Option<&mut Connection> {
        self.conns.get_mut(&id)
    }

    pub fn lookup(&self, addr: SocketAddr) -> Option<ConnId> {
        self.by_addr.get(&addr).copied()
    }

    pub fn ids(&self) -> Vec<ConnId> {
        self.conns.keys().copied().collect()
    }

    /// Takes a connection out of the table.
    pub fn remove(&mut self, id: ConnId) -> Option<Connection> {
        let conn = self.conns.remove(&id)?;
        if self.by_addr.get(&conn.addr) == Some(&id) {
            self.by_addr.remove(&conn.addr);
        }
        Some(conn)
    }

    /// The datagram that tells a connection's peer it is disconnected (`delete(reason)`), and its address.
    pub fn disconnect_datagram(
        &self,
        table: &Table,
        id: ConnId,
        reason: &[u8],
    ) -> Option<(SocketAddr, Vec<u8>)> {
        let conn = self.conns.get(&id)?;
        let types = &self.config.profile.types;
        Some((conn.addr, types.disconnect(table, conn.sequence, reason)))
    }

    /// Everything the server does with one received datagram, its source's own address being `from`'s.
    pub fn handle_datagram(
        &mut self,
        table: &Table,
        from: SocketAddr,
        data: &[u8],
        now: u32,
    ) -> Handled {
        self.handle_datagram_from(table, from, from.ip(), data, now)
    }

    /// [`Server::handle_datagram`] from `from`, whose peer's own IP address is `ip` (the real one a transport knows behind a
    /// stand-in address): the rate limits and the source caps count by `ip`.
    pub fn handle_datagram_from(
        &mut self,
        table: &Table,
        from: SocketAddr,
        ip: IpAddr,
        data: &[u8],
        now: u32,
    ) -> Handled {
        if data.is_empty() {
            return Handled::Ignored(Ignore::Empty);
        }
        if data.len() > NET_MAX_DATAGRAM {
            return Handled::Ignored(Ignore::TooLong);
        }
        if data[0] & 1 != 0 {
            return self.game_packet(from, data, now);
        }
        let types = self.config.profile.types;
        match data[0] {
            ty if ty == types.challenge_request => self.challenge_request(from, ip, data, now),
            ty if ty == types.connect_request => self.connect_request(table, from, ip, data, now),
            ty if ty == types.disconnect => self.disconnect_request(table, from, data, now),
            _ => Handled::Ignored(Ignore::UnknownType),
        }
    }

    fn challenge_request(
        &mut self,
        from: SocketAddr,
        ip: IpAddr,
        data: &[u8],
        now: u32,
    ) -> Handled {
        if !self.config.allow_connections {
            return Handled::Ignored(Ignore::Refused);
        }
        let Ok((_, seq)) = peek_type(data) else {
            return Handled::Ignored(Ignore::Malformed);
        };
        if self.challenges.allow(ip, now).is_err() {
            return Handled::Ignored(Ignore::RateLimited);
        }
        let epoch = self.epoch(now);
        let digest = challenge_digest_at(&self.key, from, seq, epoch);
        Handled::Reply(self.config.profile.types.challenge_response(seq, digest))
    }

    fn connect_request(
        &mut self,
        table: &Table,
        from: SocketAddr,
        ip: IpAddr,
        data: &[u8],
        now: u32,
    ) -> Handled {
        if !self.config.allow_connections {
            return Handled::Ignored(Ignore::Refused);
        }
        let mut r = BitReader::new(data);
        let (Ok(_), Ok(seq)) = (r.read_u8(), r.read_u32()) else {
            return Handled::Ignored(Ignore::Malformed);
        };
        let mut digest = [0u32; 4];
        for word in &mut digest {
            match r.read_u32() {
                Ok(w) => *word = w,
                Err(_) => return Handled::Ignored(Ignore::Malformed),
            }
        }
        // The current epoch's digest or the last one's (a handshake that straddles a change still succeeds); both are compared in full.
        let epoch = self.epoch(now);
        let differs = |epoch| {
            let expect = challenge_digest_at(&self.key, from, seq, epoch);
            digest
                .iter()
                .zip(&expect)
                .fold(0u32, |acc, (a, b)| acc | (a ^ b))
        };
        let (now_diff, last_diff) = (differs(epoch), differs(epoch.wrapping_sub(1)));
        if now_diff != 0 && last_diff != 0 {
            return Handled::Ignored(Ignore::BadDigest);
        }
        let limits = verified_limits(self.config.connect_requests_per_src_per_s);
        if self.verified.allow_within(limits, ip, now).is_err() {
            return Handled::Ignored(Ignore::RateLimited);
        }

        let mut replaced = None;
        if let Some(&id) = self.by_addr.get(&from)
            && let Some(existing) = self.conns.get_mut(&id)
        {
            if existing.sequence == seq {
                // The accept again, the same bytes (the first may have been lost): the channels word too.
                return Handled::Reply(connect_accept_with(
                    &self.config.profile,
                    seq,
                    existing.protocol_version,
                    existing.ghost_bits,
                    existing.extension,
                    existing.channels,
                ));
            }
            if existing.sequence > seq {
                return Handled::Ignored(Ignore::StaleSequence);
            }
            replaced = Some(id);
        }

        let class = match table.read(&mut r) {
            Ok(c) => c_string(c),
            Err(_) => return Handled::Ignored(Ignore::Malformed),
        };
        if class != GAME_CONNECTION_CLASS {
            return Handled::Ignored(Ignore::UnknownClass);
        }
        let mut fields = match read_connect_fields(&mut r, table, &self.config) {
            Ok(Fields::Ok(f)) => f,
            Ok(Fields::Reject(reason)) => {
                let types = &self.config.profile.types;
                return Handled::Reply(types.connect_reject(table, seq, &reason));
            }
            Err(_) => return Handled::Ignored(Ignore::Malformed),
        };

        // The source's caps, counted over the network connections but the one this request replaces. A full pending table is
        // no refusal (the accept evicts the oldest pending connection); a table full of established connections is.
        let others = || {
            self.conns
                .values()
                .filter(move |c| Some(c.id) != replaced && !c.local)
        };
        let count = SourceCount::of(ip, others().map(|c| (c.ip, c.stage())));
        let profile = &self.config.profile;
        match self.config.caps.admits(&count) {
            Ok(()) => {}
            Err(CapHit::PendingPerIp) => return Handled::Ignored(Ignore::PendingPerAddr),
            Err(CapHit::PendingPerNetwork) => return Handled::Ignored(Ignore::PendingPerNetwork),
            Err(CapHit::PerIp | CapHit::PerNetwork) => {
                let text = &profile.rejects.too_many_from_source;
                return Handled::Reply(profile.types.connect_reject(table, seq, text));
            }
        }
        if others().filter(|c| !c.pending).count() >= self.config.max_connections() {
            let full = &profile.rejects.server_full;
            return Handled::Reply(profile.types.connect_reject(table, seq, full));
        }

        // The gate (and an auth provider's check in it) runs last: a request the table could not take anyway, or one over a
        // source's pending limit, never costs the server a verification.
        let cx = AuthContext {
            server: self
                .config
                .address
                .unwrap_or_else(|| SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))),
            sequence: seq,
            challenge: digest,
        };
        let identity = match self
            .config
            .gate
            .as_ref()
            .map(|g| g.check(from, &cx, &fields))
        {
            None => None,
            Some(Ok(identity)) => identity,
            Some(Err(reason)) => {
                let types = &self.config.profile.types;
                return Handled::Reply(types.connect_reject(table, seq, &reason));
            }
        };

        let protocol_version = fields
            .current_protocol
            .min(self.config.profile.protocol_current);
        let extension = self.config.profile.take_extension_arg(&mut fields.args);
        Handled::Request(Box::new(PendingRequest {
            addr: from,
            ip,
            seq,
            fields,
            identity,
            protocol_version,
            replaced,
            extension,
        }))
    }

    /// The host's decision on a request: an accept enters the connection in the table (pending until its first game packet), a
    /// reject sends the reason.
    pub fn finish_request(
        &mut self,
        table: &Table,
        request: PendingRequest,
        decision: Decision,
        now: u32,
    ) -> Finished {
        let types = self.config.profile.types;
        let reject = |reason: &[u8]| Finished::Rejected {
            reply: types.connect_reject(table, request.seq, reason),
        };
        match decision {
            Decision::Reject(reason) => reject(&reason),
            Decision::Accept => {
                let mut replaced = None;
                if let Some(old) = self.by_addr.get(&request.addr).copied() {
                    if self
                        .conns
                        .get(&old)
                        .is_some_and(|c| c.sequence >= request.seq)
                    {
                        return Finished::Refused;
                    }
                    self.remove(old);
                    replaced = Some(old);
                }
                let evicted = match self.make_room(request.ip, now) {
                    Room::Free => None,
                    Room::Evicted(id) => Some(id),
                    Room::Full => return reject(&self.config.profile.rejects.server_full),
                };
                let id = self.fresh_id();
                let profile = &self.config.profile;
                // The host writes channels, and the gate that accepted the request found its channels the same as its own.
                let channels = self.config.channels
                    && self
                        .config
                        .gate
                        .as_ref()
                        .is_some_and(|g| g.matches_channels(&request.fields));
                let mods = if channels {
                    self.config
                        .gate
                        .as_ref()
                        .and_then(|g| g.mods_support(&request.fields))
                } else {
                    None
                };
                let ghost_bits = self.network_ghost_bits(request.extension);
                let mut protocol = Protocol::new(Role::Server, request.seq, self.config.rates, now);
                protocol.set_pacing(self.config.pacing.clone());
                protocol.set_packet_bytes(match profile.extension.as_ref() {
                    Some(ext) if request.extension => ext.packet_bytes.or(self.config.packet_bytes),
                    _ => self.config.packet_bytes,
                });
                let conn = Connection {
                    id,
                    addr: request.addr,
                    ip: request.ip.to_canonical(),
                    sequence: request.seq,
                    protocol,
                    protocol_version: request.protocol_version,
                    ghost_bits,
                    extra: request.fields.extra,
                    identity: request.identity,
                    pending: true,
                    local: false,
                    extension: request.extension,
                    channels,
                    mods,
                    engine: false,
                    created: now,
                    packets: RateWindow::default(),
                    disconnects: RateWindow::default(),
                };
                self.by_addr.insert(request.addr, id);
                self.conns.insert(id, conn);
                Finished::Accepted {
                    conn: id,
                    reply: connect_accept_with(
                        &self.config.profile,
                        request.seq,
                        request.protocol_version,
                        ghost_bits,
                        request.extension,
                        channels,
                    ),
                    replaced,
                    evicted,
                }
            }
        }
    }

    /// Room for one more pending connection from `ip` at `now`: when [`NET_MAX_PENDING_CONNECTS`] are pending or the table is
    /// full, [`choose_eviction`]'s rule over the evictable pending connections ([`NET_PENDING_EVICT_AGE_MS`] the age past
    /// which one may go however fair its network). Nothing is removed when no eviction would make room.
    fn make_room(&mut self, ip: IpAddr, now: u32) -> Room {
        let max = self.config.max_connections();
        if self.pending_len() < NET_MAX_PENDING_CONNECTS && self.conns.len() < max {
            return Room::Free;
        }
        // One eviction frees one slot of each: it makes room only if one is all that is short.
        if self.pending_len() > NET_MAX_PENDING_CONNECTS || self.conns.len() > max {
            return Room::Full;
        }
        let pending: Vec<PendingSlot<ConnId>> = self
            .conns
            .values()
            .filter(|c| c.pending && !c.local && !c.engine)
            .map(|c| PendingSlot {
                key: c.id,
                ip: c.ip,
                age_ms: u64::from(now.wrapping_sub(c.created)),
            })
            .collect();
        match choose_eviction(&pending, ip, u64::from(NET_PENDING_EVICT_AGE_MS)) {
            Eviction::Evict(id) => {
                self.remove(id);
                Room::Evicted(id)
            }
            Eviction::Refuse => Room::Full,
        }
    }

    /// Enters the server half of a local connection (`connectLocal`): established at once (no challenge, no request
    /// datagram, sequence 0 as `connectLocal` sets both sides'), under the pseudo-address `addr` no datagram comes from, outside the
    /// player count's and the pending limits, never pinged or timed out. The protocol version is `min(client, profile's current)`;
    /// the connection has the profile's extension when there is one, and fills its packets to [`ServerConfig::local_packet_bytes`].
    pub fn insert_local(&mut self, addr: SocketAddr, client_protocol: u32, now: u32) -> ConnId {
        if let Some(old) = self.by_addr.get(&addr).copied() {
            self.remove(old);
        }
        let id = self.fresh_id();
        let profile = &self.config.profile;
        let extension = profile.extension.is_some();
        let ghost_bits = profile.accept_ghost_bits(self.config.ghost_limit, extension);
        let protocol_version = client_protocol.min(profile.protocol_current);
        let mut protocol = Protocol::new(Role::Server, 0, self.config.rates, now);
        protocol.set_pacing(self.config.pacing.clone());
        // The local connection's packets stay in memory, read whole by its own client.
        protocol.set_in_memory(true);
        protocol.set_local(true);
        protocol.set_packet_bytes(self.config.local_packet_bytes);
        let conn = Connection {
            id,
            addr,
            ip: addr.ip().to_canonical(),
            sequence: 0,
            protocol,
            protocol_version,
            ghost_bits,
            extra: Vec::new(),
            identity: None,
            pending: false,
            local: true,
            extension,
            channels: false,
            mods: None,
            engine: false,
            created: now,
            packets: RateWindow::default(),
            disconnects: RateWindow::default(),
        };
        self.by_addr.insert(addr, id);
        self.conns.insert(id, conn);
        id
    }

    /// Enters a connection whose session the host accepted in engine mode ([`crate::engine`]): established at once (the
    /// session's connect flow already ran), under the peer's address, every peer the engine's own (the extension and the
    /// channels, when the host writes them). The host checks the player count first ([`ServerConfig::max_connections`]); an
    /// older connection from the same address is replaced. The table never pings or times it out: the session does.
    pub fn insert_engine(&mut self, accepted: EngineAccepted, now: u32) -> ConnId {
        if let Some(old) = self.by_addr.get(&accepted.addr).copied() {
            self.remove(old);
        }
        let id = self.fresh_id();
        let protocol_version = self.config.profile.protocol_current;
        let mut protocol = Protocol::new(Role::Server, 0, self.config.rates, now);
        protocol.set_engine_packet(0, 0);
        let conn = Connection {
            id,
            addr: accepted.addr,
            ip: accepted.ip.to_canonical(),
            sequence: 0,
            protocol,
            protocol_version,
            ghost_bits: accepted.ghost_bits,
            extra: Vec::new(),
            identity: accepted.identity,
            pending: false,
            local: false,
            extension: true,
            channels: self.config.channels,
            mods: accepted.mods,
            engine: true,
            created: now,
            packets: RateWindow::default(),
            disconnects: RateWindow::default(),
        };
        self.by_addr.insert(accepted.addr, id);
        self.conns.insert(id, conn);
        id
    }

    fn fresh_id(&mut self) -> ConnId {
        loop {
            let id = ConnId(self.next_id);
            self.next_id = self.next_id.wrapping_add(1);
            if !self.conns.contains_key(&id) {
                return id;
            }
        }
    }

    fn disconnect_request(
        &mut self,
        table: &Table,
        from: SocketAddr,
        data: &[u8],
        now: u32,
    ) -> Handled {
        let Ok((_, seq)) = peek_type(data) else {
            return Handled::Ignored(Ignore::Malformed);
        };
        let Some(&id) = self.by_addr.get(&from) else {
            return Handled::Ignored(Ignore::UnknownSource);
        };
        let Some(conn) = self.conns.get_mut(&id) else {
            return Handled::Ignored(Ignore::UnknownSource);
        };
        if !conn.disconnects.allow(now, NET_DISCONNECTS_PER_CONN_PER_S) {
            return Handled::Ignored(Ignore::RateLimited);
        }
        if conn.sequence != seq {
            return Handled::Ignored(Ignore::BadSequence);
        }
        let types = &self.config.profile.types;
        let Ok((_, reason)) = types.parse_disconnect_reason(table, data) else {
            return Handled::Ignored(Ignore::Malformed);
        };
        self.remove(id);
        Handled::Disconnected { conn: id, reason }
    }

    fn game_packet(&mut self, from: SocketAddr, data: &[u8], now: u32) -> Handled {
        let Some(&id) = self.by_addr.get(&from) else {
            return Handled::Ignored(Ignore::UnknownSource);
        };
        let Some(conn) = self.conns.get_mut(&id) else {
            return Handled::Ignored(Ignore::UnknownSource);
        };
        // Only a datagram whose header would be taken costs the connection's budget.
        if let Err(why) = conn.protocol.precheck(data) {
            return Handled::Ignored(Ignore::Packet(why));
        }
        if !conn.packets.allow(now, NET_PACKETS_PER_CONN_PER_S) {
            return Handled::Ignored(Ignore::RateLimited);
        }
        let mut r = BitReader::new(data);
        match conn.protocol.receive(&mut r, now) {
            Ok(received) => {
                if let Some(why) = received.dropped {
                    return Handled::Ignored(Ignore::Packet(why));
                }
                conn.pending = false;
                Handled::Game {
                    conn: id,
                    received,
                    payload_bit: r.position(),
                }
            }
            Err(error) => {
                conn.pending = false;
                Handled::GameError { conn: id, error }
            }
        }
    }

    /// The time pass: pings for idle connections, and the connections that timed out or never spoke, which leave the table.
    pub fn poll_timeouts(&mut self, now: u32) -> Vec<TimeoutEvent> {
        self.epoch(now);
        let mut events = Vec::new();
        let mut gone = Vec::new();
        for conn in self.conns.values_mut() {
            if conn.local || conn.engine {
                continue;
            }
            if conn.pending && now.wrapping_sub(conn.created) > NET_PENDING_TIMEOUT_MS {
                events.push(TimeoutEvent::PendingExpired { conn: conn.id });
                gone.push(conn.id);
                continue;
            }
            match conn.protocol.check_timeout(now) {
                Timeout::Alive => {}
                Timeout::SendPing => events.push(TimeoutEvent::Ping {
                    conn: conn.id,
                    to: conn.addr,
                    datagram: conn.protocol.ping_packet(now),
                }),
                Timeout::TimedOut => {
                    events.push(TimeoutEvent::TimedOut { conn: conn.id });
                    gone.push(conn.id);
                }
            }
        }
        for id in gone {
            self.remove(id);
        }
        events
    }
}

#[cfg(test)]
mod tests;
