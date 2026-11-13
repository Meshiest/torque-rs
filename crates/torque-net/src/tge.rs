//! The parts of the connect handshake that only Torque's own protocol has, where a peer is told apart from the engine's by what it
//! adds to the bytes a Torque server and client already exchange. The engine's own protocol ([`crate::engine`]) has none of
//! them: every peer of its server is the engine's, and its connect says what it needs in its own messages.
//!
//! - [`HandshakeExtension`] and [`WideLimit`]: a connect argument a client appends to ask for a wider ghost-id width and a larger
//!   packet, and the word that answers it.
//! - [`CHANNELS_ACCEPT_WORD`] and [`connect_accept_with`]: the word that ends an accept when the client's manifest listed the same
//!   bulk channels as the server's gate.
//! - [`read_accept_tail`]: what a client reads after the form's fields of an accept.
//! - [`manifest_matches_channels`]: the check behind [`crate::connect::ConnectGate::matches_channels`].
//!
//! Each is re-exported at the path it had before ([`crate::profile`], [`crate::handshake`], [`crate::connect`]), and nothing here
//! changes a byte of what the Torque protocol puts on the wire.

use crate::bits::BitReader;
use crate::handshake::{ConnectFields, header};
use crate::manifest::Manifest;
use crate::profile::HandshakeProfile;

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
    /// The ghost-id width such a connection gets when it is wider than the standard one (at most [`NET_MAX_GHOST_BITS`](crate::cap::NET_MAX_GHOST_BITS)).
    pub ghost_bits: Option<u8>,
    /// The size such a connection's sections fill a packet to, in place of the rate's ([`crate::protocol::Protocol::set_packet_bytes`]).
    pub packet_bytes: Option<u32>,
    /// A bound on the network connections wider than the form's widest standard width ([`HandshakeForm::ghost_bits_range`](crate::profile::HandshakeForm::ghost_bits_range));
    /// `None` sets none.
    pub wide_limit: Option<WideLimit>,
}

impl HandshakeExtension {
    /// Takes the extension's argument off the end of a request's arguments; whether it was there.
    pub fn take_arg(&self, args: &mut Vec<Vec<u8>>) -> bool {
        if args.last().is_some_and(|a| *a == self.arg) {
            args.pop();
            true
        } else {
            false
        }
    }
}

/// How many network connections (the local one never counts) may hold a ghost-id width above the standard maximum at once, and
/// how many ghost slots (`2^width` a connection) their tables may hold in all: a connection that would pass either gets the
/// standard width instead. It bounds what strangers asking for the extension can make the server keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WideLimit {
    pub clients: usize,
    pub slots: u64,
}

/// The word that ends an accept sent to a client whose manifest listed bulk channels, when the server's gate matched them: from the
/// next packet on, the server writes the channels' section and the client reads it. Only a request that carried a manifest with
/// channels is ever answered with it, so a peer without plugins sees Torque's accept.
pub const CHANNELS_ACCEPT_WORD: u32 = 0x4c4e_4843;

/// [`connect_accept`](crate::handshake::connect_accept) that also says `channels` (see [`CHANNELS_ACCEPT_WORD`]), after the
/// extension's word.
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

/// What follows the form's fields of an accept, read as a client of `profile` reads it: whether the extension's word came (only
/// when the profile has an extension), and whether [`CHANNELS_ACCEPT_WORD`] ended the accept. A missing or other word says no.
pub fn read_accept_tail(profile: &HandshakeProfile, r: &mut BitReader) -> (bool, bool) {
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
    (extension, channels)
}

/// Whether the request `fields` lists bulk channels and they are `ours` (the same names): the server may then write the channels'
/// section for the connection and say so in its accept. A server with no channels matches nothing.
pub fn manifest_matches_channels(ours: &Manifest, fields: &ConnectFields) -> bool {
    let theirs = fields
        .block
        .as_ref()
        .and_then(|b| b.manifest.as_ref())
        .map(|m| &m.channels);
    ours.channels.count() > 0 && theirs.is_some_and(|t| ours.channels.same_as(t))
}

#[cfg(test)]
#[path = "tge_tests.rs"]
mod tests;
