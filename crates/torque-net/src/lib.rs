//! Torque-style networking, server side.
//!
//! Std only, no dependency. Pure state machines and codecs over bytes: nothing here opens a socket, runs a script or reads a file.
//! What a game changes about the protocol (its event classes, its handshake strings and versions, its ghosting rules) it passes in
//! through [`profile`] and the hooks named there; the defaults are plain Torque.
//!
//! **What is here:**
//! - [`bits`]: the `BitStream` as a writer and a reader (bits LSB first, nothing byte-aligned): `int(n)`, flags, raw
//!   `u32`/`f32`/`u8`, ranged integers and floats, signed integers, `ColorI`, normal vectors, affine transforms, compressed
//!   points, and the per-packet 256-byte string buffer.
//! - [`library`]: the table of float conversions the bit stream, the moves and the audio events call through (TGE 1.4's
//!   expressions by default; a host may install its own).
//! - [`huffman`]: the string coder's code tree (`HuffmanProcessor::buildTables`, with its tie-breaking) built from a 256-entry
//!   frequency table that is passed in (Torque's standard one is [`huffman::TORQUE_CHAR_FREQS`]), and [`strcoder`],
//!   `writeString`/`readString` over it with the shared-prefix buffer.
//! - [`cap`]: the ONE helper every decoder allocates through (a wire count against the bits left AND a named cap).
//! - [`crc`]: CRC-32 (the standard one and the raw running state the camera checksum uses).
//! - [`protocol`]: the packet layer (header, sequence and ack window, notify queue, rates, ping timeout, the send gate) and
//!   [`handshake`]: the handshake's datagrams (TGE's types 26 to 38), the keyed challenge digest, the pending and established tables
//!   and their caps; [`client`] is the other half of both (the test client, the datagram decoder, the client-to-server head);
//!   [`serverquery`]: the info packets of the server query (the LAN list's ping and info requests and their answers).
//! - [`events`]: the event layer (queues, the packet section, the receiver's wait list), [`netstring`]: the 32-slot string table and `NetStringEvent`,
//!   [`remote`]: `RemoteCommandEvent`, the sender and the tag expansion; [`decode`] runs a list of datagrams through all of it.
//! - [`ghost`]: the NetObject model and the ghost manager: the three-region id table, scope, priorities, the ghost section of a
//!   data packet, `GhostRef` notify chains, `activateGhosting` / `resetGhosting`, a reference ghost client; the host implements `ObjectSource`.
//! - [`control`], [`moves`], [`gameevents`]: the server head and the control section (`Camera::writePacketData`, the checksum, the
//!   compression point), the client's move block and the server's bounded move list, and the small events (audio, path manager,
//!   lightning).
//! - [`chunk`]: bulk channels, large static data in chunks under a per-packet budget, reliable and ordered, with deltas ([`chunk::patch`]).
//! - [`manifest`] and [`connect`]: the connection manifest (plugin versions, class and channel name tables), the
//!   auth ticket and the gate a server judges a connect request with before its scripts see it; both ride after the request's
//!   arguments, where a Torque server never reads.
//! - [`modsync`]: the mods a server requires, and the section of a client's connect block that says it takes mod content and its
//!   caps (it never lists the mods a client has); the gate turns away only a client that cannot take them.
//! - [`classes`]: the net classes' ids and widths derived by the class-sorting rule from the class representatives.
//! - [`engine`]: engine mode's own framing, which owes TGE nothing: the link contract, the connect messages (with a stateless
//!   cookie only on plain links), packets with prioritized lanes and budgets, and the sessions of both sides. Not used by the
//!   TGE path.
//! - [`profile`]: what a game sets (the event registry, the handshake's types, strings, versions, form and extension, the server
//!   query's version fields, the ghost limit, the datablock id width), with TGE 1.4's defaults.
//!
//! **Every decoder reads hostile input.** It takes bytes, returns a value or a [`NetError`], and never panics. The bit reader
//! returns `Err` on overrun (TGE's returns zeros).

pub mod bits;
pub mod cap;
pub mod chunk;
pub mod classes;
pub mod client;
pub mod connect;
pub mod control;
pub mod crc;
pub mod decode;
pub mod engine;
pub mod events;
pub mod gameevents;
pub mod ghost;
pub mod handshake;
pub mod huffman;
pub mod idhash;
pub mod library;
pub mod manifest;
pub mod modsync;
pub mod moves;
pub mod netstring;
pub mod profile;
pub mod protocol;
pub mod remote;
pub mod serverquery;
pub mod strcoder;

#[cfg(test)]
mod testids;

use std::fmt;

/// Why a decoder or a table refused its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    /// The stream ended inside a field.
    Overrun,
    /// A wire count is above its cap, or more than the bits left could hold. `what` names the count.
    Cap {
        what: &'static str,
        count: u64,
        cap: u64,
    },
    /// A field holds a value the format does not allow.
    Invalid(&'static str),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::Overrun => f.write_str("the stream ended inside a field"),
            NetError::Cap { what, count, cap } => {
                write!(f, "{what}: {count} is above the limit of {cap}")
            }
            NetError::Invalid(what) => write!(f, "invalid {what}"),
        }
    }
}

impl std::error::Error for NetError {}
