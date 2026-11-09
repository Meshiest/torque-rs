//! Files a client sends to a server: a protocol where the server pulls what the client offers, block by block, under limits it
//! sets and checks, and where every byte a client sends is hostile.
//!
//! A client **offers** an upload: its kind, a short description, its size and the SHA-256 of its content. The server checks the
//! size, the player's quota, how many uploads the player has going and how many staged bytes it holds, and asks the kind's own
//! rules ([`UploadServer::begin`]'s `admit`). If all agree, the client sends the hash of each 64 KiB block, and the server
//! *requests runs* of at most 4 blocks, 2 at a time, as fast as its `rate` and `server_rate` allow. The client sends only what
//! was requested. Each block is checked against its hash before a byte of it is kept, the finished content is hashed again,
//! and only then does the kind's sink get it, as [`UntrustedBytes`]: a read-only view the sink parses under its own caps.
//!
//! Data nobody asked for, a block whose hash does not match, and a message that is not well formed are a [`Violation`]; the host
//! drops that connection. An upload cut off by a disconnect is kept for the kind's `resume` time, still counted against the
//! player's quota, and a begin of the same content by the same player asks only for the blocks that are missing.
//!
//! This crate is the state machines: [`UploadServer`] and [`UploadClient`] take the time and the messages as arguments and
//! return what to send. They know nothing of sockets, scripts or RPCs; `torque-engine` wires them to the engine's RPCs and a
//! client-to-server channel (`NetRegistry::upload`). The block hashing is the same the mods download uses.
//!
//! ```
//! use torque_net::handshake::ConnId;
//! use torque_transfer::{Offer, Out, Subject, UploadClient, UploadServer, UploadSpec, StagingConfig};
//!
//! let mut server = UploadServer::new(vec![("notes".into(), UploadSpec::default())], StagingConfig::default())?;
//! let mut client = UploadClient::new();
//! let (conn, now) = (ConnId(1), 0);
//! let player = Subject::Connection(conn);
//!
//! let begin = client.begin("notes", b"hello".to_vec(), Box::new(b"some bytes".to_vec()))?;
//! let offer = Offer { kind: &begin.kind, meta: &begin.meta, size: begin.size, hash: begin.hash };
//! let accepted = server.begin(now, conn, &player, &offer, |_meta| Ok(()))?;
//! let handle = begin.handle;
//! for message in client.accepted(handle, accepted.into())? {
//!     server.on_message(now, conn, &message)?;
//! }
//! let mut done = false;
//! while !done {
//!     for out in server.poll(now) {
//!         match out {
//!             Out::Request { id, start, count, .. } => {
//!                 let data = client.request(id, start, count)?;
//!                 server.on_message(now, conn, &data)?;
//!             }
//!             Out::Finish { id } => {
//!                 assert_eq!(server.content(id).map(|c| c.len()), Some(10));
//!                 server.complete(id, Ok(()));
//!                 done = true;
//!             }
//!             _ => {}
//!         }
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Maintaining
//!
//! **Module map.** `blocks`: sizes and hashes (shared with the mods download). `spec`: [`UploadSpec`] and [`Refusal`] with its
//! player-facing text. `message`: the two client messages. `staging`: the pool (memory and folder caps), [`UntrustedBytes`].
//! `server` (and `server/`): [`UploadServer`]; `admit` (offers, resume, evictions, quota), `receive` (messages, delivered bytes, disconnects), `schedule` (expiry, the final hash, requests), `session`, `bucket`. `client`: [`UploadClient`] and [`Source`]. `fuzz`:
//! the harness every sink's fuzz test uses.
//!
//! **Invariants and their tests.**
//! - Staging never holds more than its caps, a kind's uploads that hold their room never pass `server_in_flight`, and a player
//!   never holds more than the per-player bytes (`tests/hostile.rs`, `TORQUE_FUZZ_UPLOAD`; `server_tests.rs`). An upload that
//!   gave up its room keeps its staging, which the staging caps bound.
//! - Nothing is kept before its hash is checked, and the whole content is checked before the sink sees it
//!   (`a_bad_block_hash_is_a_violation_and_nothing_is_kept`, `a_content_that_does_not_match_its_hash_drops_the_connection`).
//! - Data nobody asked for is a violation; data of an upload the server just closed is tolerated, once per request
//!   (`unrequested_data_is_a_violation`, `late_data_of_a_closed_upload_is_tolerated_once_per_request`), whichever way the upload
//!   ended (`an_upload_that_goes_quiet_is_ended_with_a_reason_and_its_data_on_the_way_is_not_a_violation`,
//!   `every_way_an_upload_ends_leaves_room_for_the_data_on_its_way`). A client that breaks the rules loses all its uploads
//!   (`a_client_that_breaks_the_rules_loses_every_upload_it_had_and_cannot_resume_them`).
//! - A slow connection that keeps delivering is not ended and a trickle is
//!   (`bytes_that_keep_arriving_keep_a_slow_upload_alive_whatever_the_specs_rate_is`, `a_trickle_does_not_keep_an_upload_alive`);
//!   a quiet or waiting upload gives its room up and needs it back
//!   (`an_upload_waiting_for_a_resume_gives_up_its_room_and_needs_it_back_to_resume`).
//! - An upload that has a connection is not resumed (`an_upload_that_has_a_connection_is_not_resumed`), and one client does not
//!   begin one content twice (`the_same_content_is_not_begun_twice_while_the_first_is_going`).
//! - Players with no identity leave no records (`players_with_no_identity_leave_no_records_when_their_connection_ends`).
//! - A request the host could not send costs nothing (`a_request_the_host_could_not_send_is_neither_a_sign_of_life_nor_a_cost`).
//! - The kind's rules are asked before anything is evicted or staged (`the_kinds_own_rules_are_asked_before_anything_is_evicted_or_staged`).
//! - Staging deletes only files named like its own (`only_files_named_like_staging_files_are_deleted`).
//! - A resume requests only the missing blocks (`tests/link.rs`); it never extends the deadline and an upload is resumed at most
//!   eight times (`disconnecting_and_resuming_in_a_loop_cannot_keep_an_upload_alive_past_its_deadline`).
//! - Waiting uploads count toward a player's limit and per-player staged bytes, and give way oldest first
//!   (`waiting_uploads_count_toward_the_players_limit_and_the_oldest_gives_way`,
//!   `staged_bytes_per_player_stay_capped_and_waiting_uploads_give_way_for_them`).
//! - A refused offer holds nothing (`a_refused_offer_holds_nothing`).
//! - Every read of a message is checked against the bytes left before it allocates (`message_tests.rs`).
//! - Peak memory of a 16 MiB upload is its staging cap plus 2 MiB (`tests/memory.rs`).
//!
//! **Extension points.** A sink is the host's: `UploadServer::content` after [`Out::Finish`], then `complete`. Per-network
//! fairness (many connections from one address) is the host's to enforce before it calls [`UploadServer::begin`].
//!
//! **Checks.** `cargo test -p torque-transfer`; the fuzz as `TORQUE_FUZZ_UPLOAD=100000 cargo test -p torque-transfer --test
//! hostile`; a sink's own as `TORQUE_FUZZ_UPLOAD_<NAME>=100000`.

mod blocks;
mod client;
pub mod fuzz;
mod message;
mod server;
mod spec;
mod staging;

pub use blocks::{
    BLOCK_BYTES, BLOCK_HASH_LEN, BlockHash, Hash, MAX_OUTSTANDING_RUNS, MAX_RUN_BLOCKS,
    block_count, block_hash, block_hashes, content_hash, run_bytes,
};
pub use client::{
    Accept, BeginRequest, ClientError, FileSource, Handle, MAX_CLIENT_UPLOADS, Source, Status,
    UploadClient,
};
pub use message::{ClientMessage, MAX_MESSAGE, WireError, decode, encode_block_list, encode_data};
pub use server::{
    Accepted, Counters, MAX_PLAYERS, MAX_RESUMES, MAX_SESSIONS, Millis, Offer, Out, SLOWEST_LINK,
    Subject, UploadServer, Violation,
};
pub use spec::{
    MAX_KIND_NAME, MAX_META_BYTES, MAX_UPLOAD_BYTES, Quota, Refusal, SpecError, UploadSpec,
    duration_text,
};
pub use staging::{ContentReader, StagingConfig, UntrustedBytes};
