//! The bytes of engine framing 1. Byte-aligned, little-endian, every length before what it counts, strings UTF-8 with a one-byte
//! length. Every datagram starts with the framing tag ([`ENGINE_TAG`]) and a [`Kind`] byte.
//!
//! | Kind | Direction | After the tag and kind |
//! |------|-----------|------------------------|
//! | `Hello` | client to server, plain links only | versions (min, max), nonce `u64`, game `str8`, zeros up to [`HELLO_BYTES`] |
//! | `Cookie` | server to client | version, nonce `u64`, cookie (16 bytes) |
//! | `Opening` | client to server until accepted | nonce `u64`, cookie flag (0 or 1) and the cookie, then a packet body |
//! | `Data` | both ways, validated links | a packet body |
//! | `TokenData` | both ways, plain links | token `u64` (the client's nonce, or the session id once that side is established), then a packet body |
//! | `Reject` | server to client | token `u64` (the client's nonce), code, text `str8` |
//! | `Disconnect` | both ways | token `u64` (the session id, or the nonce before an accept), reason `str8` |
//!
//! A packet body is the sequence `u16`, the ack `u16`, the ack mask `u32` (bit `d` set: packet `ack - d` arrived; 0: none yet),
//! then one section per lane that has anything, in [`Lane`] order: the lane byte, a `u16` length and the content. A zero lane
//! byte ends the sections and the rest of the datagram must be zeros (padding). Reliable lanes hold fragments
//! (message id `u16`, total `u32`, offset `u32`, length `u16`, bytes); the unreliable lane holds messages (sequence `u16`,
//! length `u16`, bytes); the stream lanes hold what their writer wrote.
//!
//! `Hello` and `Reject` keep this layout in every framing version, so a server can tell any engine client which version it
//! speaks; they are read whatever the tag's version nibble says.

use super::{ENGINE_FAMILY, ENGINE_FRAMING, ENGINE_TAG, HELLO_BYTES};
use crate::NetError;
use crate::cap::NET_MAX_CONNECT_ARGS;
use crate::connect::MAX_TICKET_BYTES;

/// The bytes of a one-byte-length string.
pub const STR_BYTES: usize = 255;

/// The bytes of a packet body's header (sequence, ack, ack mask).
pub const BODY_HEADER: usize = 8;

/// The bytes a `Data` datagram spends before its body.
pub const DATA_PREFIX: usize = 2;

/// The bytes a `TokenData` datagram spends before its body.
pub const TOKEN_DATA_PREFIX: usize = 2 + 8;

/// The bytes an `Opening` datagram spends before its body.
pub const OPENING_PREFIX: usize = 2 + 8 + 1 + COOKIE_BYTES;

/// The bytes a lane section spends before its content.
pub const SECTION_HEADER: usize = 3;

/// The bytes a fragment of a reliable lane spends before its bytes.
pub const FRAGMENT_HEADER: usize = 12;

/// The bytes a message of the unreliable lane spends before its bytes.
pub const UNRELIABLE_HEADER: usize = 4;

/// The bytes of a plain-link cookie.
pub const COOKIE_BYTES: usize = 16;

/// What a datagram is (its second byte).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Hello = 1,
    Cookie = 2,
    Opening = 3,
    Data = 4,
    Reject = 5,
    Disconnect = 6,
    TokenData = 7,
}

impl Kind {
    fn from_byte(b: u8) -> Option<Kind> {
        Some(match b {
            1 => Kind::Hello,
            2 => Kind::Cookie,
            3 => Kind::Opening,
            4 => Kind::Data,
            5 => Kind::Reject,
            6 => Kind::Disconnect,
            7 => Kind::TokenData,
            _ => return None,
        })
    }
}

/// The lanes of a packet body, in the order they are written and given room: a higher lane is never delayed by a lower one.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lane {
    /// Connect and accept: reliable, ordered.
    Control = 1,
    /// RPC batches: reliable, ordered.
    Rpc = 2,
    /// Unreliable messages: sent once, never delivered after a newer one.
    Unreliable = 3,
    /// The ghost section, written by the host for each packet.
    Ghosts = 4,
    /// The move section, written by the host for each packet.
    Moves = 5,
    /// The bulk channels, written by the host for each packet.
    Channels = 6,
}

impl Lane {
    /// Every lane, highest priority first.
    pub const ALL: [Lane; 6] = [
        Lane::Control,
        Lane::Rpc,
        Lane::Unreliable,
        Lane::Ghosts,
        Lane::Moves,
        Lane::Channels,
    ];

    pub(crate) const fn index(self) -> usize {
        self as usize - 1
    }

    fn from_byte(b: u8) -> Option<Lane> {
        Lane::ALL.get(usize::from(b).wrapping_sub(1)).copied()
    }
}

/// The lanes whose content the host writes for each packet and resends from its own notify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamLane {
    Ghosts,
    Moves,
    Channels,
}

impl StreamLane {
    pub const ALL: [StreamLane; 3] = [StreamLane::Ghosts, StreamLane::Moves, StreamLane::Channels];

    #[must_use]
    pub const fn lane(self) -> Lane {
        match self {
            StreamLane::Ghosts => Lane::Ghosts,
            StreamLane::Moves => Lane::Moves,
            StreamLane::Channels => Lane::Channels,
        }
    }
}

/// The framing versions a peer speaks, oldest and newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Versions {
    pub min: u8,
    pub max: u8,
}

impl Versions {
    /// This build's only version.
    pub const CURRENT: Versions = Versions {
        min: ENGINE_FRAMING,
        max: ENGINE_FRAMING,
    };

    #[must_use]
    pub const fn contains(self, version: u8) -> bool {
        self.min <= version && version <= self.max
    }

    fn read(r: &mut Reader<'_>) -> Result<Versions, NetError> {
        let v = Versions {
            min: r.u8()?,
            max: r.u8()?,
        };
        if v.min == 0 || v.min > v.max || v.max > 0x0f {
            return Err(NetError::Invalid("framing version range"));
        }
        Ok(v)
    }

    fn write(self, w: &mut Writer) {
        w.u8(self.min);
        w.u8(self.max);
    }
}

impl Default for Versions {
    fn default() -> Versions {
        Versions::CURRENT
    }
}

/// A plain link's stateless cookie: the server's keyed digest of the client's address, its nonce and the time epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CookieValue(pub [u8; COOKIE_BYTES]);

/// Why a server refused a connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectCode {
    Game,
    Version,
    Password,
    Manifest,
    Mods,
    Ticket,
    /// The server's connect script refused.
    Script,
    Full,
    /// A code this build does not know (a newer server's).
    Other(u8),
}

impl RejectCode {
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            RejectCode::Game => 1,
            RejectCode::Version => 2,
            RejectCode::Password => 3,
            RejectCode::Manifest => 4,
            RejectCode::Mods => 5,
            RejectCode::Ticket => 6,
            RejectCode::Script => 7,
            RejectCode::Full => 8,
            RejectCode::Other(b) => b,
        }
    }

    #[must_use]
    pub const fn from_byte(b: u8) -> RejectCode {
        match b {
            1 => RejectCode::Game,
            2 => RejectCode::Version,
            3 => RejectCode::Password,
            4 => RejectCode::Manifest,
            5 => RejectCode::Mods,
            6 => RejectCode::Ticket,
            7 => RejectCode::Script,
            8 => RejectCode::Full,
            b => RejectCode::Other(b),
        }
    }
}

/// The client's first datagram on a plain link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub versions: Versions,
    pub nonce: u64,
    pub game: String,
}

impl Hello {
    /// The datagram, padded to [`HELLO_BYTES`] so that no answer is larger than it.
    ///
    /// # Errors
    /// The game string is longer than [`STR_BYTES`].
    pub fn encode(&self) -> Result<Vec<u8>, NetError> {
        let mut w = Writer::datagram(Kind::Hello, HELLO_BYTES);
        self.versions.write(&mut w);
        w.u64(self.nonce);
        w.str8(&self.game)?;
        w.buf.resize(HELLO_BYTES.max(w.buf.len()), 0);
        Ok(w.buf)
    }

    fn read(r: &mut Reader<'_>, len: usize) -> Result<Hello, NetError> {
        if len < HELLO_BYTES {
            return Err(NetError::Invalid("hello shorter than its padding"));
        }
        let hello = Hello {
            versions: Versions::read(r)?,
            nonce: r.u64()?,
            game: r.str8()?.to_owned(),
        };
        r.zeros()?;
        Ok(hello)
    }
}

/// The server's answer to a [`Hello`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cookie {
    pub version: u8,
    pub nonce: u64,
    pub cookie: CookieValue,
}

impl Cookie {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::datagram(Kind::Cookie, 27);
        w.u8(self.version);
        w.u64(self.nonce);
        w.bytes(&self.cookie.0);
        w.buf
    }

    fn read(r: &mut Reader<'_>) -> Result<Cookie, NetError> {
        let c = Cookie {
            version: r.u8()?,
            nonce: r.u64()?,
            cookie: r.cookie()?,
        };
        r.finish()?;
        Ok(c)
    }
}

/// The server's refusal, with one line of reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    /// The nonce of the client refused.
    pub token: u64,
    pub code: RejectCode,
    pub text: String,
}

impl Reject {
    /// A refusal whose text is cut to one line of at most [`STR_BYTES`] bytes.
    #[must_use]
    pub fn new(token: u64, code: RejectCode, text: &str) -> Reject {
        Reject {
            token,
            code,
            text: one_line(text),
        }
    }

    /// # Errors
    /// The text is longer than [`STR_BYTES`].
    pub fn encode(&self) -> Result<Vec<u8>, NetError> {
        let mut w = Writer::datagram(Kind::Reject, 12 + self.text.len());
        w.u64(self.token);
        w.u8(self.code.byte());
        w.str8(&self.text)?;
        Ok(w.buf)
    }

    fn read(r: &mut Reader<'_>) -> Result<Reject, NetError> {
        let reject = Reject {
            token: r.u64()?,
            code: RejectCode::from_byte(r.u8()?),
            text: r.line()?.to_owned(),
        };
        r.finish()?;
        Ok(reject)
    }
}

/// Either side's goodbye.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disconnect {
    /// The session id from the accept, or the client's nonce before one.
    pub token: u64,
    pub reason: String,
}

impl Disconnect {
    /// A goodbye whose reason is cut to one line of at most [`STR_BYTES`] bytes.
    #[must_use]
    pub fn new(token: u64, reason: &str) -> Disconnect {
        Disconnect {
            token,
            reason: one_line(reason),
        }
    }

    /// # Errors
    /// The reason is longer than [`STR_BYTES`].
    pub fn encode(&self) -> Result<Vec<u8>, NetError> {
        let mut w = Writer::datagram(Kind::Disconnect, 11 + self.reason.len());
        w.u64(self.token);
        w.str8(&self.reason)?;
        Ok(w.buf)
    }

    fn read(r: &mut Reader<'_>) -> Result<Disconnect, NetError> {
        let d = Disconnect {
            token: r.u64()?,
            reason: r.line()?.to_owned(),
        };
        r.finish()?;
        Ok(d)
    }
}

/// The client's connect message, the first message of the control lane.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Connect {
    pub versions: Versions,
    pub game: String,
    /// The connection manifest, as its own codec writes it.
    pub manifest: Vec<u8>,
    /// The auth ticket (empty for anonymous auth).
    pub ticket: Vec<u8>,
    /// The mods section.
    pub mods: Vec<u8>,
    pub password: String,
    /// The script's connect arguments, at most [`NET_MAX_CONNECT_ARGS`].
    pub args: Vec<String>,
}

/// The widths the session's ids are written in, set by the server's tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Widths {
    pub ghost_id: u8,
    pub ghost_class: u8,
    pub datablock_class: u8,
    pub event_class: u8,
}

/// Digests of the tables both sides must agree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TableDigests {
    pub channels: u64,
    pub rpcs: u64,
    pub uploads: u64,
}

/// The server's acceptance, a control-lane message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accept {
    pub session: u64,
    /// The largest packet either side sends (the link's own limit may be lower, tick by tick).
    pub max_packet: u16,
    /// Milliseconds between packets each way.
    pub packet_interval_ms: u16,
    /// Bytes a second the server sends this client at most.
    pub byte_rate: u32,
    pub widths: Widths,
    pub digests: TableDigests,
}

/// The smallest packet limit an accept may set.
pub const MIN_ACCEPT_PACKET: u16 = 128;

/// The widest id an accept may set.
pub const MAX_ACCEPT_WIDTH: u8 = 24;

const CONTROL_CONNECT: u8 = 1;
const CONTROL_ACCEPT: u8 = 2;
const CONTROL_CHALLENGE: u8 = 3;
const CONTROL_TICKET: u8 = 4;

/// A message of the control lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Connect(Box<Connect>),
    Accept(Accept),
    /// Server to client, after the connect: the nonce the server chose, which the client's auth ticket must be made for.
    Challenge(u64),
    /// Client to server, the answer to a challenge: the auth ticket made for it (empty: the client has none), at most
    /// [`MAX_TICKET_BYTES`].
    Ticket(Vec<u8>),
}

impl Control {
    /// # Errors
    /// A string is longer than [`STR_BYTES`], or the connect has more than [`NET_MAX_CONNECT_ARGS`] arguments.
    pub fn encode(&self) -> Result<Vec<u8>, NetError> {
        let mut w = Writer::default();
        match self {
            Control::Connect(c) => {
                if c.args.len() > NET_MAX_CONNECT_ARGS {
                    return Err(NetError::Cap {
                        what: "connect arguments",
                        count: c.args.len() as u64,
                        cap: NET_MAX_CONNECT_ARGS as u64,
                    });
                }
                w.u8(CONTROL_CONNECT);
                c.versions.write(&mut w);
                w.str8(&c.game)?;
                w.blob(&c.manifest)?;
                w.blob(&c.ticket)?;
                w.blob(&c.mods)?;
                w.str8(&c.password)?;
                // At most NET_MAX_CONNECT_ARGS, checked above.
                w.u8(c.args.len() as u8);
                for arg in &c.args {
                    w.str8(arg)?;
                }
            }
            Control::Accept(a) => {
                w.u8(CONTROL_ACCEPT);
                w.u64(a.session);
                w.u16(a.max_packet);
                w.u16(a.packet_interval_ms);
                w.u32(a.byte_rate);
                w.u8(a.widths.ghost_id);
                w.u8(a.widths.ghost_class);
                w.u8(a.widths.datablock_class);
                w.u8(a.widths.event_class);
                w.u64(a.digests.channels);
                w.u64(a.digests.rpcs);
                w.u64(a.digests.uploads);
            }
            Control::Challenge(nonce) => {
                w.u8(CONTROL_CHALLENGE);
                w.u64(*nonce);
            }
            Control::Ticket(ticket) => {
                if ticket.len() > MAX_TICKET_BYTES {
                    return Err(NetError::Cap {
                        what: "ticket bytes",
                        count: ticket.len() as u64,
                        cap: MAX_TICKET_BYTES as u64,
                    });
                }
                w.u8(CONTROL_TICKET);
                w.blob(ticket)?;
            }
        }
        Ok(w.buf)
    }

    /// # Errors
    /// The bytes are not a control message, or a field is out of range.
    pub fn decode(data: &[u8]) -> Result<Control, NetError> {
        let mut r = Reader::new(data);
        let message = match r.u8()? {
            CONTROL_CONNECT => {
                let mut c = Connect {
                    versions: Versions::read(&mut r)?,
                    game: r.str8()?.to_owned(),
                    manifest: r.blob()?.to_vec(),
                    ticket: r.blob()?.to_vec(),
                    mods: r.blob()?.to_vec(),
                    password: r.str8()?.to_owned(),
                    args: Vec::new(),
                };
                let argc = usize::from(r.u8()?);
                if argc > NET_MAX_CONNECT_ARGS {
                    return Err(NetError::Cap {
                        what: "connect arguments",
                        count: argc as u64,
                        cap: NET_MAX_CONNECT_ARGS as u64,
                    });
                }
                c.args.reserve_exact(argc);
                for _ in 0..argc {
                    c.args.push(r.str8()?.to_owned());
                }
                Control::Connect(Box::new(c))
            }
            CONTROL_ACCEPT => {
                let a = Accept {
                    session: r.u64()?,
                    max_packet: r.u16()?,
                    packet_interval_ms: r.u16()?,
                    byte_rate: r.u32()?,
                    widths: Widths {
                        ghost_id: r.u8()?,
                        ghost_class: r.u8()?,
                        datablock_class: r.u8()?,
                        event_class: r.u8()?,
                    },
                    digests: TableDigests {
                        channels: r.u64()?,
                        rpcs: r.u64()?,
                        uploads: r.u64()?,
                    },
                };
                let w = a.widths;
                if a.max_packet < MIN_ACCEPT_PACKET
                    || !(1..=1_000).contains(&a.packet_interval_ms)
                    || a.byte_rate == 0
                    || [w.ghost_id, w.ghost_class, w.datablock_class, w.event_class]
                        .iter()
                        .any(|&b| b > MAX_ACCEPT_WIDTH)
                {
                    return Err(NetError::Invalid("accept field"));
                }
                Control::Accept(a)
            }
            CONTROL_CHALLENGE => Control::Challenge(r.u64()?),
            CONTROL_TICKET => {
                let ticket = r.blob()?;
                if ticket.len() > MAX_TICKET_BYTES {
                    return Err(NetError::Cap {
                        what: "ticket bytes",
                        count: ticket.len() as u64,
                        cap: MAX_TICKET_BYTES as u64,
                    });
                }
                Control::Ticket(ticket.to_vec())
            }
            _ => return Err(NetError::Invalid("control message kind")),
        };
        r.finish()?;
        Ok(message)
    }
}

/// The header of a packet body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub seq: u16,
    pub ack: u16,
    pub mask: u32,
}

/// A packet body: its header and the content of each lane's section (empty: no section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Body<'a> {
    pub header: Header,
    pub sections: [&'a [u8]; 6],
}

impl<'a> Body<'a> {
    #[must_use]
    pub fn section(&self, lane: Lane) -> &'a [u8] {
        self.sections[lane.index()]
    }

    /// Any section at all (a packet with none is a bare ack and is not acked back).
    #[must_use]
    pub fn has_sections(&self) -> bool {
        self.sections.iter().any(|s| !s.is_empty())
    }

    /// Writes a body: the header, then each non-empty section in lane order, then zeros up to `pad_to` bytes in all.
    ///
    /// # Errors
    /// A section is longer than a `u16` length can say.
    pub fn write(&self, out: &mut Vec<u8>, pad_to: usize) -> Result<(), NetError> {
        out.extend_from_slice(&self.header.seq.to_le_bytes());
        out.extend_from_slice(&self.header.ack.to_le_bytes());
        out.extend_from_slice(&self.header.mask.to_le_bytes());
        for (lane, content) in Lane::ALL.iter().zip(self.sections) {
            if content.is_empty() {
                continue;
            }
            let len = u16_len(content.len(), "section bytes")?;
            out.push(*lane as u8);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(content);
        }
        if out.len() < pad_to {
            out.resize(pad_to, 0);
        }
        Ok(())
    }

    /// # Errors
    /// The body ends inside a field, a lane is unknown, repeated or out of order, a section is empty, or the padding is not
    /// zeros.
    pub fn read(data: &'a [u8]) -> Result<Body<'a>, NetError> {
        let mut r = Reader::new(data);
        let header = Header {
            seq: r.u16()?,
            ack: r.u16()?,
            mask: r.u32()?,
        };
        let mut sections: [&[u8]; 6] = [&[]; 6];
        let mut last = 0u8;
        while !r.is_empty() {
            let b = r.u8()?;
            if b == 0 {
                r.zeros()?;
                break;
            }
            let Some(lane) = Lane::from_byte(b) else {
                return Err(NetError::Invalid("lane"));
            };
            if b <= last {
                return Err(NetError::Invalid("lane order"));
            }
            last = b;
            let len = usize::from(r.u16()?);
            if len == 0 {
                return Err(NetError::Invalid("empty section"));
            }
            sections[lane.index()] = r.take(len)?;
        }
        Ok(Body { header, sections })
    }
}

/// One fragment of a reliable lane's message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fragment<'a> {
    pub id: u16,
    pub total: u32,
    pub offset: u32,
    pub bytes: &'a [u8],
}

impl Fragment<'_> {
    /// # Errors
    /// The fragment holds more than a `u16` length can say.
    pub fn write(&self, out: &mut Vec<u8>) -> Result<(), NetError> {
        let len = u16_len(self.bytes.len(), "fragment bytes")?;
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&self.total.to_le_bytes());
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(self.bytes);
        Ok(())
    }

    /// The fragments of one reliable section.
    ///
    /// # Errors
    /// A fragment ends early or reaches past its message's total.
    pub fn read_all(content: &[u8]) -> Result<Vec<Fragment<'_>>, NetError> {
        let mut r = Reader::new(content);
        let mut out = Vec::with_capacity(content.len() / FRAGMENT_HEADER);
        while !r.is_empty() {
            let f = Fragment {
                id: r.u16()?,
                total: r.u32()?,
                offset: r.u32()?,
                bytes: {
                    let len = usize::from(r.u16()?);
                    r.take(len)?
                },
            };
            if u64::from(f.offset) + f.bytes.len() as u64 > u64::from(f.total) {
                return Err(NetError::Invalid("fragment past its message"));
            }
            out.push(f);
        }
        Ok(out)
    }
}

/// One message of the unreliable lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreliable<'a> {
    pub seq: u16,
    pub bytes: &'a [u8],
}

impl Unreliable<'_> {
    /// # Errors
    /// The message holds more than a `u16` length can say.
    pub fn write(&self, out: &mut Vec<u8>) -> Result<(), NetError> {
        let len = u16_len(self.bytes.len(), "unreliable message bytes")?;
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(self.bytes);
        Ok(())
    }

    /// The messages of one unreliable section.
    ///
    /// # Errors
    /// A message ends early.
    pub fn read_all(content: &[u8]) -> Result<Vec<Unreliable<'_>>, NetError> {
        let mut r = Reader::new(content);
        let mut out = Vec::with_capacity(content.len() / UNRELIABLE_HEADER);
        while !r.is_empty() {
            let seq = r.u16()?;
            let len = usize::from(r.u16()?);
            out.push(Unreliable {
                seq,
                bytes: r.take(len)?,
            });
        }
        Ok(out)
    }
}

/// A datagram, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Datagram<'a> {
    Hello(Hello),
    Cookie(Cookie),
    Opening {
        nonce: u64,
        cookie: Option<CookieValue>,
        body: &'a [u8],
    },
    /// `Data` (no token) or `TokenData`.
    Data {
        token: Option<u64>,
        body: &'a [u8],
    },
    Reject(Reject),
    Disconnect(Disconnect),
}

/// Reads a datagram's kind and fixed fields (a packet body is left for [`Body::read`]).
///
/// # Errors
/// The datagram is not engine framing, is of another framing version (other than a `Hello` or a `Reject`), or a field is out
/// of range.
pub fn parse(data: &[u8]) -> Result<Datagram<'_>, NetError> {
    let mut r = Reader::new(data);
    let tag = r.u8()?;
    if tag & 0xf0 != ENGINE_FAMILY {
        return Err(NetError::Invalid("framing tag"));
    }
    let Some(kind) = Kind::from_byte(r.u8()?) else {
        return Err(NetError::Invalid("datagram kind"));
    };
    if tag != ENGINE_TAG && !matches!(kind, Kind::Hello | Kind::Reject) {
        return Err(NetError::Invalid("framing version"));
    }
    Ok(match kind {
        Kind::Hello => Datagram::Hello(Hello::read(&mut r, data.len())?),
        Kind::Cookie => Datagram::Cookie(Cookie::read(&mut r)?),
        Kind::Opening => {
            let nonce = r.u64()?;
            let cookie = match r.u8()? {
                0 => {
                    r.take(COOKIE_BYTES)?;
                    None
                }
                1 => Some(r.cookie()?),
                _ => return Err(NetError::Invalid("cookie flag")),
            };
            Datagram::Opening {
                nonce,
                cookie,
                body: r.rest(),
            }
        }
        Kind::Data => Datagram::Data {
            token: None,
            body: r.rest(),
        },
        Kind::TokenData => Datagram::Data {
            token: Some(r.u64()?),
            body: r.rest(),
        },
        Kind::Reject => Datagram::Reject(Reject::read(&mut r)?),
        Kind::Disconnect => Datagram::Disconnect(Disconnect::read(&mut r)?),
    })
}

/// What a packet datagram carries before its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    /// A client's packet until it has the accept (a zero cookie is written when there is none, so the prefix has one size).
    Opening {
        nonce: u64,
        cookie: Option<CookieValue>,
    },
    /// A packet on a validated link.
    Data,
    /// A packet on a plain link, with the token that shows it comes from the peer.
    TokenData { token: u64 },
}

impl Prefix {
    /// Its bytes.
    #[must_use]
    pub const fn size(self) -> usize {
        match self {
            Prefix::Opening { .. } => OPENING_PREFIX,
            Prefix::Data => DATA_PREFIX,
            Prefix::TokenData { .. } => TOKEN_DATA_PREFIX,
        }
    }

    pub fn write(self, out: &mut Vec<u8>) {
        match self {
            Prefix::Opening { nonce, cookie } => {
                out.extend_from_slice(&[ENGINE_TAG, Kind::Opening as u8]);
                out.extend_from_slice(&nonce.to_le_bytes());
                out.push(u8::from(cookie.is_some()));
                out.extend_from_slice(&cookie.map_or([0; COOKIE_BYTES], |c| c.0));
            }
            Prefix::Data => out.extend_from_slice(&[ENGINE_TAG, Kind::Data as u8]),
            Prefix::TokenData { token } => {
                out.extend_from_slice(&[ENGINE_TAG, Kind::TokenData as u8]);
                out.extend_from_slice(&token.to_le_bytes());
            }
        }
    }
}

fn u16_len(len: usize, what: &'static str) -> Result<u16, NetError> {
    u16::try_from(len).map_err(|_| NetError::Cap {
        what,
        count: len as u64,
        cap: u64::from(u16::MAX),
    })
}

/// `text` as one line (control characters become spaces) of at most [`STR_BYTES`] bytes, cut at a character boundary.
#[must_use]
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(STR_BYTES));
    for c in text.chars().map(|c| if c.is_control() { ' ' } else { c }) {
        if out.len() + c.len_utf8() > STR_BYTES {
            break;
        }
        out.push(c);
    }
    out
}

#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) buf: Vec<u8>,
}

impl Writer {
    fn datagram(kind: Kind, capacity: usize) -> Writer {
        let mut buf = Vec::with_capacity(capacity);
        buf.extend_from_slice(&[ENGINE_TAG, kind as u8]);
        Writer { buf }
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    fn str8(&mut self, s: &str) -> Result<(), NetError> {
        let len = u8::try_from(s.len()).map_err(|_| NetError::Cap {
            what: "string bytes",
            count: s.len() as u64,
            cap: STR_BYTES as u64,
        })?;
        self.buf.push(len);
        self.buf.extend_from_slice(s.as_bytes());
        Ok(())
    }

    fn blob(&mut self, b: &[u8]) -> Result<(), NetError> {
        let len = u32::try_from(b.len()).map_err(|_| NetError::Cap {
            what: "blob bytes",
            count: b.len() as u64,
            cap: u64::from(u32::MAX),
        })?;
        self.u32(len);
        self.bytes(b);
        Ok(())
    }
}

pub(crate) struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data }
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], NetError> {
        let Some((head, rest)) = self.data.split_at_checked(n) else {
            return Err(NetError::Overrun);
        };
        self.data = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], NetError> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u8(&mut self) -> Result<u8, NetError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, NetError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, NetError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, NetError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn cookie(&mut self) -> Result<CookieValue, NetError> {
        Ok(CookieValue(self.array()?))
    }

    fn str8(&mut self) -> Result<&'a str, NetError> {
        let len = usize::from(self.u8()?);
        std::str::from_utf8(self.take(len)?).map_err(|_| NetError::Invalid("UTF-8"))
    }

    /// A string that must be one line (a reason shown to a player).
    fn line(&mut self) -> Result<&'a str, NetError> {
        let s = self.str8()?;
        if s.chars().any(char::is_control) {
            return Err(NetError::Invalid("control character in a line"));
        }
        Ok(s)
    }

    /// A `u32`-length blob; the length is checked against the bytes left before anything is taken.
    fn blob(&mut self) -> Result<&'a [u8], NetError> {
        let len = self.u32()?;
        let left = self.data.len();
        if len as usize > left {
            return Err(NetError::Cap {
                what: "blob bytes",
                count: u64::from(len),
                cap: left as u64,
            });
        }
        self.take(len as usize)
    }

    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.data)
    }

    fn zeros(&mut self) -> Result<(), NetError> {
        if self.rest().iter().any(|&b| b != 0) {
            return Err(NetError::Invalid("padding"));
        }
        Ok(())
    }

    fn finish(&self) -> Result<(), NetError> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(NetError::Invalid("trailing bytes"))
        }
    }
}
