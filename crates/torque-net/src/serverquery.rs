//! The server query's info packets (`handleInfoPacket` and the handlers it calls): what a client's "Query LAN"
//! and its single-server queries send, and what a server answers. Both halves, bytes in and bytes out; the host
//! reads the values from the scripts' variables and decides whether to answer.
//!
//! **Dispatch.** `processPacketReceiveEvent` hands every non-game datagram whose type is at most `GameHeartbeat` (22) to the info
//! handler ([`QueryProfile::is_info_type`]; a profile may move the bound). The handler reads `U8 flags, U32 key` and switches on
//! the type:
//!
//! | type | handler | answer |
//! |---|---|---|
//! | 14 [`GAME_PING_REQUEST`] | `handleGamePingRequest` | 16 [`GAME_PING_RESPONSE`]: the version string, the protocol, the minimum protocol, the build number ([`QueryProfile`]), the name cut at 24 bytes |
//! | 18 [`GAME_INFO_REQUEST`] | `handleGameInfoRequest` | 20 [`GAME_INFO_RESPONSE`]: game type, mission type, mission name, status, players, max, bots, CPU MHz, info, `onServerInfoQuery()` |
//!
//! Types 16 and 20 are the client's side; there is no handler for TGE's master-server types 2..12 or the heartbeat (22), and any
//! other info type is handed back to the host with its flags and key ([`Parsed::Other`]). A reply echoes the request's
//! flags and key. Strings are Huffman `writeString(s, 255)` unless flags bit 1 ([`FLAG_NO_STRING_COMPRESS`]) asks for
//! `writeCString` (a byte of length, `strlen & 0xff`, then the bytes); the info reply's last string is always the long form (`U16`
//! length, `strlen & 0xffff`, then the bytes). A reply is built in a 1,500-byte packet stream: a write past 12,000 bits is
//! dropped and the datagram is `(bits + 7) / 8` bytes ([`REPLY_BITS`]).
//!
//! **Who answers.** The ping needs `allowConnections`, a `$Server::ServerType` that is not `SinglePlayer` and flags bit 0
//! ([`FLAG_OFFLINE_QUERY`]) clear; the info request needs `allowConnections` and bit 0 clear ([`answers`]).
//!
//! **Additions.** A request shorter than 6 bytes is silence. The replies are rate limited
//! ([`InfoLimiter`]: per source, in all, and the `onServerInfoQuery()` runs); a standard server answers every request.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `app/net/serverQuery.cpp` and
//! `sim/netInterface.cpp` (each Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::net::IpAddr;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{
    self, NET_INFO_REPLIES_PER_S, NET_INFO_REPLIES_PER_SRC_PER_S, NET_INFO_SCRIPT_RUNS_PER_S,
    NET_MAX_DATAGRAM, NET_STRING_BYTES,
};
use crate::handshake::{RateWindow, SourceLimiter};
use crate::huffman::Table;
use crate::profile::QueryProfile;

pub const GAME_PING_REQUEST: u8 = 14;
pub const GAME_PING_RESPONSE: u8 = 16;
pub const GAME_INFO_REQUEST: u8 = 18;
pub const GAME_INFO_RESPONSE: u8 = 20;
/// `GameHeartbeat`: the last info type ([`QueryProfile::info_type_max`]'s default); nothing here sends or answers it.
pub const GAME_HEARTBEAT: u8 = 22;

/// Flags bit 0: an offline query, which a server does not answer.
pub const FLAG_OFFLINE_QUERY: u8 = 1;
/// Flags bit 1: the strings are `writeCString`s instead of Huffman strings.
pub const FLAG_NO_STRING_COMPRESS: u8 = 2;

/// The bytes of the server name the ping answer keeps (`strncpy(buf, name, 24)`).
pub const SERVER_NAME_BYTES: usize = 24;

/// Status bit 0: `$Server::Dedicated`.
pub const STATUS_DEDICATED: u8 = 1;
/// Status bit 1: `$Pref::Server::Password` is not empty.
pub const STATUS_PASSWORDED: u8 = 2;

/// The bits a reply may take: a packet stream (1,500 bytes); a write past them is dropped.
pub const REPLY_BITS: usize = NET_MAX_DATAGRAM * 8;

/// The bytes of a request: type, flags, key.
pub const REQUEST_BYTES: usize = 6;

/// A request the server answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ping,
    Info,
}

/// A request's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub kind: Kind,
    pub flags: u8,
    pub key: u32,
}

impl Request {
    /// Whether the strings of the reply are Huffman strings (flags bit 1 clear).
    pub fn compressed(&self) -> bool {
        self.flags & FLAG_NO_STRING_COMPRESS == 0
    }
}

/// What an info-type datagram is to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parsed {
    /// One of the two requests a server answers.
    Request(Request),
    /// An info type the server does nothing with (a response, an unused type): its type, flags and key, for a host that answers a
    /// type of its own.
    Other { ty: u8, flags: u8, key: u32 },
    /// Shorter than its 6 bytes (silence).
    Short,
}

/// Reads an info-type datagram (the caller checked [`QueryProfile::is_info_type`]).
pub fn parse_request(data: &[u8]) -> Parsed {
    let mut r = BitReader::new(data);
    let (Ok(ty), Ok(flags), Ok(key)) = (r.read_u8(), r.read_u8(), r.read_u32()) else {
        return Parsed::Short;
    };
    let kind = match ty {
        GAME_PING_REQUEST => Kind::Ping,
        GAME_INFO_REQUEST => Kind::Info,
        _ => return Parsed::Other { ty, flags, key },
    };
    Parsed::Request(Request { kind, flags, key })
}

/// Whether a server answers `request`: `allow` is `doesAllowConnections()`, `single_player` whether `$Server::ServerType` is
/// `SinglePlayer` ignoring case (the info request does not look at it).
pub fn answers(request: &Request, allow: bool, single_player: bool) -> bool {
    if !allow || request.flags & FLAG_OFFLINE_QUERY != 0 {
        return false;
    }
    match request.kind {
        Kind::Ping => !single_player,
        Kind::Info => true,
    }
}

/// The client's request datagram (`sendPacket`): the type, the flags and `(session << 16) | (key & 0xffff)`.
pub fn request(ty: u8, flags: u8, session: u32, key: u32) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_u8(ty);
    w.write_u8(flags);
    w.write_u32((session << 16) | (key & 0xffff));
    w.into_bytes()
}

fn reply_head(ty: u8, request: &Request) -> BitWriter {
    let mut w = BitWriter::with_limit(REPLY_BITS);
    w.write_u8(ty);
    w.write_u8(request.flags);
    w.write_u32(request.key);
    w
}

fn until_nul(s: &[u8]) -> &[u8] {
    s.split(|&b| b == 0).next().unwrap_or(&[])
}

/// `writeCString`: a byte of length (`strlen & 0xff`), then that many bytes.
pub fn write_c_string(w: &mut BitWriter, s: &[u8]) {
    let s = until_nul(s);
    let len = s.len() & 0xff;
    w.write_u8(len as u8);
    w.write_bytes(s.get(..len).unwrap_or_default());
}

/// `writeLongCString`: a `U16` of length (`strlen & 0xffff`), then that many bytes.
pub fn write_long_c_string(w: &mut BitWriter, s: &[u8]) {
    let s = until_nul(s);
    let len = s.len() & 0xffff;
    w.write_bits(len as u32, 16);
    w.write_bytes(s.get(..len).unwrap_or_default());
}

fn write_string(w: &mut BitWriter, table: &Table, compressed: bool, s: &[u8]) {
    if compressed {
        table.write(w, s, NET_STRING_BYTES);
    } else {
        write_c_string(w, s);
    }
}

/// The ping answer (type 16) for a server named `name` (the name is cut at [`SERVER_NAME_BYTES`]), with the version fields of
/// `profile`.
pub fn ping_response(
    table: &Table,
    request: &Request,
    name: &[u8],
    profile: &QueryProfile,
) -> Vec<u8> {
    let compressed = request.compressed();
    let mut w = reply_head(GAME_PING_RESPONSE, request);
    write_string(&mut w, table, compressed, &profile.version_string);
    w.write_u32(profile.protocol_current);
    w.write_u32(profile.protocol_min);
    w.write_u32(profile.version);
    let name = until_nul(name);
    write_string(
        &mut w,
        table,
        compressed,
        name.get(..name.len().min(SERVER_NAME_BYTES))
            .unwrap_or_default(),
    );
    w.into_bytes()
}

/// What the info answer carries, in its order (the host reads them from the variables).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfoFields {
    pub game_type: Vec<u8>,
    pub mission_type: Vec<u8>,
    pub mission_name: Vec<u8>,
    /// [`STATUS_DEDICATED`] | [`STATUS_PASSWORDED`].
    pub status: u8,
    pub players: u8,
    pub max_players: u8,
    pub bots: u8,
    /// The CPU figure in MHz (`Platform::SystemInfo.processor.mhz`; 0 for none).
    pub cpu_mhz: u16,

    /// `$Pref::Server::Info`.
    pub info: Vec<u8>,
    /// What `onServerInfoQuery()` returned.
    pub content: Vec<u8>,
}

/// The info answer (type 20).
pub fn info_response(table: &Table, request: &Request, f: &InfoFields) -> Vec<u8> {
    let compressed = request.compressed();
    let mut w = reply_head(GAME_INFO_RESPONSE, request);
    write_string(&mut w, table, compressed, &f.game_type);
    write_string(&mut w, table, compressed, &f.mission_type);
    write_string(&mut w, table, compressed, &f.mission_name);
    w.write_u8(f.status);
    w.write_u8(f.players);
    w.write_u8(f.max_players);
    w.write_u8(f.bots);
    w.write_bits(u32::from(f.cpu_mhz), 16);
    write_string(&mut w, table, compressed, &f.info);
    write_long_c_string(&mut w, &f.content);
    w.into_bytes()
}

// ---- the client's reading (tests) ---------------------------------------------------------------------------

fn read_c_string(r: &mut BitReader) -> Result<Vec<u8>, NetError> {
    let len = usize::from(r.read_u8()?);
    let len = cap::count("c string", len as u64, NET_STRING_BYTES, r.bits_left(), 8)?;
    r.read_bytes(len)
}

fn read_long_c_string(r: &mut BitReader) -> Result<Vec<u8>, NetError> {
    let len = r.read_bits(16)?;
    let len = cap::count(
        "long c string",
        u64::from(len),
        u16::MAX as usize,
        r.bits_left(),
        8,
    )?;
    r.read_bytes(len)
}

fn read_string(r: &mut BitReader, table: &Table, compressed: bool) -> Result<Vec<u8>, NetError> {
    if compressed {
        table.read(r)
    } else {
        read_c_string(r)
    }
}

fn read_head(r: &mut BitReader, ty: u8) -> Result<(u8, u32), NetError> {
    if r.read_u8()? != ty {
        return Err(NetError::Invalid("datagram type"));
    }
    Ok((r.read_u8()?, r.read_u32()?))
}

/// A ping answer as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PingResponse {
    pub flags: u8,
    pub key: u32,
    pub version: Vec<u8>,
    pub protocol: u32,
    pub min_protocol: u32,
    pub version_number: u32,
    pub name: Vec<u8>,
}

/// Reads a ping answer (type 16); its strings in the form its flags say.
pub fn parse_ping_response(table: &Table, data: &[u8]) -> Result<PingResponse, NetError> {
    let mut r = BitReader::new(data);
    let (flags, key) = read_head(&mut r, GAME_PING_RESPONSE)?;
    let compressed = flags & FLAG_NO_STRING_COMPRESS == 0;
    Ok(PingResponse {
        flags,
        key,
        version: read_string(&mut r, table, compressed)?,
        protocol: r.read_u32()?,
        min_protocol: r.read_u32()?,
        version_number: r.read_u32()?,
        name: read_string(&mut r, table, compressed)?,
    })
}

/// An info answer as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoResponse {
    pub flags: u8,
    pub key: u32,
    pub fields: InfoFields,
}

/// Reads an info answer (type 20).
pub fn parse_info_response(table: &Table, data: &[u8]) -> Result<InfoResponse, NetError> {
    let mut r = BitReader::new(data);
    let (flags, key) = read_head(&mut r, GAME_INFO_RESPONSE)?;
    let compressed = flags & FLAG_NO_STRING_COMPRESS == 0;
    let game_type = read_string(&mut r, table, compressed)?;
    let mission_type = read_string(&mut r, table, compressed)?;
    let mission_name = read_string(&mut r, table, compressed)?;
    let status = r.read_u8()?;
    let players = r.read_u8()?;
    let max_players = r.read_u8()?;
    let bots = r.read_u8()?;
    let cpu_mhz = r.read_bits(16)? as u16;
    let info = read_string(&mut r, table, compressed)?;
    let content = read_long_c_string(&mut r)?;
    Ok(InfoResponse {
        flags,
        key,
        fields: InfoFields {
            game_type,
            mission_type,
            mission_name,
            status,
            players,
            max_players,
            bots,
            cpu_mhz,
            info,
            content,
        },
    })
}

// ---- the rate limits ---------------------------------------------------------------------------------------------------------

/// Why the limiter refused a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The source had [`NET_INFO_REPLIES_PER_SRC_PER_S`] this second.
    Source,
    /// The server sent [`NET_INFO_REPLIES_PER_S`] this second.
    Global,
    /// [`NET_INFO_SCRIPT_RUNS_PER_S`] info replies ran the script this second.
    Script,
}

/// The rate limits of the info replies: per source IP (a bounded table), in all, and the info replies that
/// run `onServerInfoQuery()`.
#[derive(Debug)]
pub struct InfoLimiter {
    per_source: SourceLimiter,
    replies: RateWindow,
    scripts: RateWindow,
    pub per_source_limit: u32,
    pub global_limit: u32,
    pub script_limit: u32,
}

impl Default for InfoLimiter {
    fn default() -> InfoLimiter {
        InfoLimiter {
            per_source: SourceLimiter::default(),
            replies: RateWindow::default(),
            scripts: RateWindow::default(),
            per_source_limit: NET_INFO_REPLIES_PER_SRC_PER_S,
            global_limit: NET_INFO_REPLIES_PER_S,
            script_limit: NET_INFO_SCRIPT_RUNS_PER_S,
        }
    }
}

impl InfoLimiter {
    /// Counts one reply to `ip` at `now` (milliseconds), `script` when it runs the host's script (the info answer's), or says
    /// which limit refuses it (a refused reply is counted only by the limits checked before the one that refused it).
    pub fn allow(&mut self, ip: IpAddr, script: bool, now: u32) -> Result<(), Refused> {
        if !self.per_source.allow(ip, now, self.per_source_limit) {
            return Err(Refused::Source);
        }
        if !self.replies.allow(now, self.global_limit) {
            return Err(Refused::Global);
        }
        if script && !self.scripts.allow(now, self.script_limit) {
            return Err(Refused::Script);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
