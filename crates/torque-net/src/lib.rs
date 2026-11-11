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
//! - [`stand_in`]: the reserved-range addresses a transport names its connections by toward the host (the web transport's
//!   `240.0.0.0/5`, the QUIC link's `248.0.0.0/5`): one IP per peer address, a port per connection.
//! - [`crc`]: CRC-32 (the standard one and the raw running state the camera checksum uses).
//! - [`protocol`]: the packet layer (header, sequence and ack window, notify queue, rates, ping timeout, the send gate) and
//!   [`handshake`]: the handshake's datagrams (TGE's types 26 to 38), the keyed challenge digest, the pending and established tables
//!   and their caps; [`client`] is the other half of both (the test client, the datagram decoder, the client-to-server head);
//!   [`serverquery`]: the info packets of the server query (the LAN list's ping and info requests and their answers).
//! - [`events`]: the event layer (queues, the packet section, the receiver's wait list), [`netstring`]: the 32-slot string table and `NetStringEvent`,
//!   [`remote`]: `RemoteCommandEvent`, the sender and the tag expansion (and [`remote::body`], a command as the byte body the engine's protocol sends as an RPC); [`decode`] runs a list of datagrams through all of it.
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
//! **A connection's identity is only ever the auth provider's verdict.** [`connect::Identity`] is made by
//! [`connect::AuthProvider::verify`] on the server and travels as the gate's `Ok` into [`handshake::PendingRequest::identity`] and
//! [`handshake::Connection::identity`]; nothing a client sends is read as one (its arguments, password and ticket text reach a
//! provider only to be checked). `None` is an anonymous player, held by the connection id, which a table never reuses. An
//! identity's id is never empty and at most [`connect::MAX_IDENTITY_BYTES`] (the type refuses it), so a provider cannot put every
//! player under one key, and its name is for display only (control and bidirectional override characters
//! dropped, cut to [`connect::MAX_IDENTITY_NAME_BYTES`]; equality and hashing use the id alone)
//! (`connect::tests::an_identity_needs_an_id_of_a_sane_size`, `an_identity_is_its_id_and_the_name_is_only_for_display`,
//! `a_display_name_loses_control_characters_and_is_cut_on_a_character`). Nothing on the wire carries an identity: a ticket
//! travels only when a provider is set, and the identity is what the server made of it.
//!
//! **Seeded tests and fuzz loops draw from [`testrng::TestRng`]**, never from a source of their own.
//!
//! **A ghost pass costs what changed, not what exists** (benches `ghost_section`, and `ghost_tick` in torque-server). A full
//! query over a source that answers [`ghost::ObjectSource::scope_stamp`], under a policy with [`ghost::GhostPolicy::plain_query`],
//! only counts the objects of a world it found whole the last time: the visits, the budget and the resume cursor are those of
//! scoping each, nothing else is skipped (`ghost::tests::a_query_that_only_counts_the_objects_it_found_before_gives_the_packets_of_one_that_scopes_them_again`).
//! A stamp moves whenever anything its query reads changes, and a policy that overrides a question of the walk
//! (`query_visit`, `admit`, `ghost_made`, `query_started`, `query_ended`) keeps `plain_query` false. What
//! [`ghost::ObjectSource::dependents`] answers is read by the query too: a source whose dependents change (a scene hierarchy's
//! links) moves its stamp with them. [`ghost::ObjectSource::first_record_bonus`] raises a ghost's first record only
//! (`ghost::tests::a_first_record_bonus_counts_for_the_first_record_alone`); it is 0 for every object of Torque's, and is
//! asked only for first records. [`ghost::ObjectSource::marks_on_ghost`] is asked for every ghost made and marks nothing for
//! Torque's objects. The priority sort stays a
//! stable sort of the whole pending list (the tie order is the next packet's), whatever it costs.
//!
//! **The bit layer is fast by words, never by changing a byte**: fields and byte runs are written and read a word or a run at a
//! time and equal the bit-by-bit forms (`bits::tests::bytes_and_fields_written_and_read_in_bulk_equal_the_same_written_bit_by_bit`;
//! bench `bits`), and a mark shares the string buffer with its writer until a string is written
//! (`bits::tests::a_mark_keeps_the_strings_the_stream_had_whatever_is_written_after_it`; bench `bits`' `mark_rewind_*`). Engine
//! sessions are measured by bench `engine_session` (time and allocations a pair-tick).
//!
//! **A Ready runs the script once a sequence only by the policy's rule** (`ghost::tests::
//! a_repeated_ready_runs_the_script_once_when_the_policy_says_so`).
//!
//! **The engine protocol's event table is Torque's plus its own classes** ([`events::EventRegistry::engine`]:
//! `DataBlockRemoveEvent`, server to client), derived on both sides by the one sort rule, so the ids of the classes after a new
//! one move and any change to it is a new [`engine::ENGINE_FRAMING`] (the goldens of `tests/engine_golden_v2.rs` and
//! torque-trace-hash's `wire_golden` pin the current version's bytes). Torque's table ([`events::EventRegistry::torque`]) never
//! changes: a stock peer has no class for the engine's events. A Torque class the engine codes keeps a field of
//! [`events::EventIds`]; an engine class is answered by an `Option` the table may lack ([`events::EventIds::datablock_remove`]).

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
pub mod stand_in;
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
