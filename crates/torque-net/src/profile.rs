//! What a game changes about the protocol, gathered in one place. Every value here has TGE 1.4's own as the default, so a host that
//! sets nothing speaks plain Torque; a game passes its own values (and fills the event registry) instead of editing the layers.
//!
//! - [`HandshakeProfile`]: the handshake's datagram types, the game string, the protocol versions, the reject strings, a
//!   [`HandshakeForm`] for what a game's connection class adds to the request and the accept, and an optional
//!   [`HandshakeExtension`] (a connect argument that asks for a wider ghost-id width and a larger packet, within a [`WideLimit`]).
//! - [`QueryProfile`]: the version fields of the server query's ping and info answers and the last info type.
//! - [`crate::protocol::RateConfig`] and [`crate::protocol::SendPacing`]: the packet rates and what a game adds to the send gate.
//! - [`crate::events::EventRegistry`]: the event classes (Torque's own by default; a game registers its own beside them, and its
//!   own command classes, [`crate::events::EventRegistry::register_command`]).
//! - [`crate::ghost::GhostPolicy`]: when the full scope query runs, the walks between queries, the admission of new ghosts, the
//!   order of the pending ghosts and an implied class form for new-ghost records.
//! - [`crate::control::ControlFormat`]: the control section's form (the moves' [`crate::moves::MoveFormat`], the first-person
//!   pair, the camera object's data); a game's control objects bring their own [`crate::control::ControlData`] and
//!   [`crate::control::ControlReader`].
//! - [`crate::gameevents::PathCount`]: how the path event's count is taken.
//! - [`crate::events::EventCtx::datablock_bits`]: the width of a datablock id in the audio events.

use std::fmt;
use std::ops::RangeInclusive;
use std::sync::Arc;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::NET_MAX_GHOST_BITS;
use crate::handshake::HandshakeTypes;

/// The ghost limit of a connection when nothing sets one: TGE's `MaxGhostCount` (`1 << GhostIdBitSize`, 12 bits).
pub const DEFAULT_GHOST_LIMIT: u32 = 1 << 12;

/// The width of a datablock id on the wire when nothing sets one: TGE's `DataBlockObjectIdBitSize`.
pub const DEFAULT_DATABLOCK_BITS: u8 = 10;

/// The reject strings of the handshake: wire text that the client's scripts match on, not prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectStrings {
    /// The class group or class CRC differs.
    pub invalid: Vec<u8>,
    /// The game string differs.
    pub game: Vec<u8>,
    /// The client's newest protocol is older than the server accepts.
    pub protocol_less: Vec<u8>,
    /// The client's oldest protocol is newer than the server speaks.
    pub protocol_greater: Vec<u8>,
    /// The password differs.
    pub password: Vec<u8>,
    /// The request carries more arguments than allowed.
    pub invalid_args: Vec<u8>,
    /// The connection table is full.
    pub server_full: Vec<u8>,
    /// What a client reports for an accept whose protocol it does not speak.
    pub protocol: Vec<u8>,
    /// What a client reports for an accept whose ghost-id width it cannot use.
    pub ghost_bits: Vec<u8>,
}

impl Default for RejectStrings {
    /// Torque's strings (`GameConnection::readConnectRequest` and `readConnectAccept`); an unusable ghost-id width is reported as
    /// an unusable protocol.
    fn default() -> RejectStrings {
        RejectStrings {
            invalid: b"CHR_INVALID".to_vec(),
            game: b"CHR_GAME".to_vec(),
            protocol_less: b"CHR_PROTOCOL_LESS".to_vec(),
            protocol_greater: b"CHR_PROTOCOL_GREATER".to_vec(),
            password: b"CHR_PASSWORD".to_vec(),
            invalid_args: b"CR_INVALID_ARGS".to_vec(),
            server_full: b"CR_SERVERFULL".to_vec(),
            protocol: b"CHR_PROTOCOL".to_vec(),
            ghost_bits: b"CHR_PROTOCOL".to_vec(),
        }
    }
}

/// A handshake extension: a client that appends [`HandshakeExtension::arg`] to its connect arguments asks for it, the server
/// takes the argument off before the host sees the arguments and answers with [`HandshakeExtension::accept_word`] after the
/// accept, and the connection gets the wider ghost-id width and the packet size named here. A peer without the extension never
/// sees any of it: a server ignores an argument it has no parameter for and a client stops reading the accept where its form ends.
///
/// Any client may name the argument, so the widths and sizes given here are what a stranger can make the server keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeExtension {
    /// The connect argument that asks for the extension (the last one of the request).
    pub arg: Vec<u8>,
    /// The word the server appends to the accept it sends a client that asked.
    pub accept_word: u32,
    /// The ghost-id width such a connection gets when it is wider than the standard one (at most [`NET_MAX_GHOST_BITS`]).
    pub ghost_bits: Option<u8>,
    /// The size such a connection's sections fill a packet to, in place of the rate's ([`crate::protocol::Protocol::set_packet_bytes`]).
    pub packet_bytes: Option<u32>,
    /// A bound on the network connections wider than the form's widest standard width ([`HandshakeForm::ghost_bits_range`]);
    /// `None` sets none.
    pub wide_limit: Option<WideLimit>,
}

/// How many network connections (the local one never counts) may hold a ghost-id width above the standard maximum at once, and
/// how many ghost slots (`2^width` a connection) their tables may hold in all: a connection that would pass either gets the
/// standard width instead. It bounds what strangers asking for the extension can make the server keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WideLimit {
    pub clients: usize,
    pub slots: u64,
}

/// What a game's connection class adds to the handshake beside Torque's fields (its `writeConnectRequest` and
/// `writeConnectAccept` and their readers), and the ghost-id widths it gives and takes. TGE's ([`TorqueForm`]) adds nothing: the
/// request ends with the arguments, the accept is the protocol alone, and a connection's ghost ids are as wide as its ghost limit
/// needs ([`crate::classes::ghost_id_bits`]).
pub trait HandshakeForm: fmt::Debug + Send + Sync {
    /// Writes what the connect request carries after the password ([`crate::handshake::ConnectFields::extra`]).
    fn write_request(&self, w: &mut BitWriter, extra: &[u8]) {
        let _ = (w, extra);
    }

    /// Reads what [`HandshakeForm::write_request`] wrote; it becomes [`crate::handshake::ConnectFields::extra`]. A short stream is an
    /// `Err` (the request is dropped).
    fn read_request(&self, r: &mut BitReader) -> Result<Vec<u8>, NetError> {
        let _ = r;
        Ok(Vec::new())
    }

    /// The bytes a fresh request of this form carries in [`crate::handshake::ConnectFields::extra`].
    fn default_request(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Writes what a server's accept of `protocol` carries after the protocol, for a connection given `ghost_bits`.
    fn write_accept(&self, w: &mut BitWriter, protocol: u32, ghost_bits: u8) {
        let _ = (w, protocol, ghost_bits);
    }

    /// Reads what a client takes from an accept of `protocol` after the protocol: the ghost-id width when the accept carries one.
    fn read_accept(&self, r: &mut BitReader, protocol: u32) -> Result<Option<u32>, NetError> {
        let _ = (r, protocol);
        Ok(None)
    }

    /// The ghost-id widths a server gives a standard client and a client takes from an accept that carries one; the server's width
    /// is `ceil(log2 ghost_limit)` within them.
    fn ghost_bits_range(&self) -> RangeInclusive<u8> {
        1..=NET_MAX_GHOST_BITS
    }
}

/// TGE's handshake form: nothing beside Torque's fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TorqueForm;

impl HandshakeForm for TorqueForm {}

/// What the handshake of a game says, on both sides.
#[derive(Debug, Clone)]
pub struct HandshakeProfile {
    /// The handshake's datagram types.
    pub types: HandshakeTypes,
    /// The game string a connect request names (`GameString`).
    pub game: Vec<u8>,
    /// The newest protocol version this side speaks (`CurrentProtocolVersion`).
    pub protocol_current: u32,
    /// The oldest protocol version this side accepts (`MinRequiredProtocolVersion`).
    pub protocol_min: u32,
    /// What the game's connection class adds to the request and the accept.
    pub form: Arc<dyn HandshakeForm>,
    pub rejects: RejectStrings,
    /// The extension a client may ask for; `None` for none.
    pub extension: Option<HandshakeExtension>,
}

impl Default for HandshakeProfile {
    /// TGE's handshake: its datagram types and game string, protocol 12 only, nothing beside Torque's fields, no extension.
    fn default() -> HandshakeProfile {
        HandshakeProfile {
            types: HandshakeTypes::TORQUE,
            game: b"Torque Game Engine Demo".to_vec(),
            protocol_current: 12,
            protocol_min: 12,
            form: Arc::new(TorqueForm),
            rejects: RejectStrings::default(),
            extension: None,
        }
    }
}

impl HandshakeProfile {
    /// The ghost-id width a server of `ghost_limit` gives a standard client: `ceil(log2 limit)` within the form's
    /// [`HandshakeForm::ghost_bits_range`] (and within 1..=[`NET_MAX_GHOST_BITS`]).
    pub fn ghost_bits(&self, ghost_limit: u32) -> u8 {
        let (min, max) = self.ghost_bits_bounds();
        crate::classes::ghost_id_bits(ghost_limit).clamp(min, max)
    }

    /// The form's widths held to 1..=[`NET_MAX_GHOST_BITS`]: `(min, max)` with `min <= max`.
    pub fn ghost_bits_bounds(&self) -> (u8, u8) {
        let range = self.form.ghost_bits_range();
        let max = (*range.end()).clamp(1, NET_MAX_GHOST_BITS);
        ((*range.start()).clamp(1, max), max)
    }

    /// The width a connection is given: [`HandshakeProfile::ghost_bits`], or for a peer that asked for the extension the wider of
    /// that and the extension's width.
    pub fn accept_ghost_bits(&self, ghost_limit: u32, extension: bool) -> u8 {
        let standard = self.ghost_bits(ghost_limit);
        match self.extension.as_ref().and_then(|e| e.ghost_bits) {
            Some(bits) if extension => standard.max(bits.clamp(1, NET_MAX_GHOST_BITS)),
            _ => standard,
        }
    }

    /// Takes the extension's argument off the end of a request's arguments; whether it was there (always false without an
    /// extension).
    pub fn take_extension_arg(&self, args: &mut Vec<Vec<u8>>) -> bool {
        let Some(ext) = self.extension.as_ref() else {
            return false;
        };
        if args.last().is_some_and(|a| *a == ext.arg) {
            args.pop();
            true
        } else {
            false
        }
    }
}

/// The version fields of the server query's ping answer (the CPU figure of the info answer is [`crate::serverquery::InfoFields`]'s,
/// 0 unless the host sets one), and which datagram types are info packets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryProfile {
    /// The query protocol's version string (Torque's `"VER1"`).
    pub version_string: Vec<u8>,
    /// The handshake's protocol versions ([`QueryProfile::for_handshake`]).
    pub protocol_current: u32,
    pub protocol_min: u32,
    /// The build number (`getVersionNumber`).
    pub version: u32,
    /// The last datagram type the info handler gets (TGE's `GameHeartbeat`, 22): every even type up to it is an info packet
    /// ([`QueryProfile::is_info_type`]).
    pub info_type_max: u8,
}

impl Default for QueryProfile {
    /// Torque's version string and protocol versions; build number 0 until the host sets one.
    fn default() -> QueryProfile {
        QueryProfile::for_handshake(&HandshakeProfile::default())
    }
}

impl QueryProfile {
    /// Torque's version string and info types, the protocol versions of `handshake`, build number 0.
    pub fn for_handshake(handshake: &HandshakeProfile) -> QueryProfile {
        QueryProfile {
            version_string: b"VER1".to_vec(),
            protocol_current: handshake.protocol_current,
            protocol_min: handshake.protocol_min,
            version: 0,
            info_type_max: crate::serverquery::GAME_HEARTBEAT,
        }
    }

    /// Whether the info handler gets a datagram whose first byte is `ty` (`processPacketReceiveEvent`: an even type up to
    /// [`QueryProfile::info_type_max`]; an odd first byte is a game packet).
    pub fn is_info_type(&self, ty: u8) -> bool {
        ty & 1 == 0 && ty <= self.info_type_max
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
