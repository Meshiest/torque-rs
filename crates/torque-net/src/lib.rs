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
//! - [`testrng`]: the seeded random source of tests and fixed-seed fuzz loops.
//! - [`fair`]: the ONE place a stranger's budget is shared out: the source network ([`fair::NetworkKey`]), per-second budgets
//!   charged per address, per network and in all ([`fair::FairLimiter`]), and the connections one source may hold
//!   ([`fair::SourceCaps`]).
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
//!
//! # Maintaining
//!
//! **A stranger's budget is shared out through [`fair`], never by its own count.** A per-second budget is a
//! [`fair::FairLimiter`] (per address, per network, and with a total one network takes at most an eighth of), and a
//! table of connections admits one more through [`fair::SourceCaps`] over a [`fair::SourceCount`] of what it holds, keyed
//! by the peer's own address (`fair::tests::one_network_takes_at_most_its_share_of_a_total_and_others_still_get_through`,
//! `a_steady_source_stays_limited_under_a_flood_of_rotating_sources`,
//! `the_caps_count_pending_and_all_connections_per_ip_and_per_network`). No total may refuse what a spoofed flood can spend:
//! challenge and cookie answers have none.
//!
//! **A full pending table follows one rule, [`fair::choose_eviction`]**, in the TGE handshake, the engine gate and the
//! session table alike: a network over its fair share (at least one) pays, else a slot past the evict age, else the newcomer
//! is refused; an honest network's only young slot is never evicted
//! (`fair::tests::the_eviction_rule_takes_from_a_network_over_its_share_or_an_old_slot_and_never_a_young_only_one`,
//! `handshake::tests::an_honest_networks_only_pending_slot_is_never_evicted_while_young`,
//! `ten_addresses_holding_every_pending_slot_cannot_lock_out_another_network`,
//! `an_eviction_takes_from_a_network_over_its_share_never_an_honest_client_elsewhere`,
//! `a_requester_whose_network_holds_its_share_is_refused_and_nothing_is_evicted`,
//! `engine::table::tests::pending_sessions_from_one_address_are_capped_and_a_network_over_its_share_is_evicted`); a source
//! holds at most its caps of connections (`a_source_holds_at_most_4_connections_per_address_and_8_per_network`).
//!
//! **IPv6 sources are keyed by /48 for their share of a budget and by /64 for the connections they hold**
//! ([`fair::NetworkKey::sharing`], [`fair::NetworkKey::of`];
//! `fair::tests::a_network_shares_budgets_by_its_v6_48_and_holds_connections_by_its_v6_64`).
//!
//! **No reply to an unproven source is more than three times its request** but within the small amplified budget
//! (`serverquery::tests::spoofed_short_info_requests_reflect_at_most_three_times_their_bytes_and_padded_ones_are_answered`),
//! and the info answer's script output is answered from for a second (`the_info_scripts_output_is_answered_from_for_a_second`).
//!
//! **Seeded tests and fuzz loops draw from [`testrng::TestRng`]**, never from a source of their own.
//!
//! **A Ready runs the script once a sequence only by the policy's rule** (`ghost::tests::
//! a_repeated_ready_runs_the_script_once_when_the_policy_says_so`).

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
pub mod fair;
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
pub mod testrng;

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
