//! The QUIC link of the engine's protocol: encryption, address validation, congestion control and migration from QUIC
//! (RFC 9000, TLS 1.3 per RFC 9001, loss recovery per RFC 9002), the engine's datagrams on QUIC DATAGRAM frames (RFC 9221),
//! and the server's identity as the hash of its key.
//!
//! - [`QuicServer`] is a server [`Transport`](torque_server::net::transport::Transport): one UDP socket, a
//!   `quinn_proto::Endpoint`, one stand-in address per connection toward the host, Retry under load, per-address checks
//!   run again when a connection migrates. [`QuicServerTransport`] is the real one; the tests run the same code over an
//!   in-process [`Pipe`] with a [`FakeClock`].
//! - [`QuicClient`] is a client [`Datagrams`](torque_client::net::Datagrams): one connection to one server, the server's key
//!   pinned by its identity ([`Trust`]) or trusted on first use and remembered ([`KnownServers`]).
//! - [`ServerKey`] is the server's long-lived ECDSA P-256 key ([`IDENTITY_FILE`] in its data folder) and the self-signed
//!   certificate it presents; its identity is the SHA-256 of the key's SubjectPublicKeyInfo, `sha256:<hex>`.
//! - Both ends derive a [`ChannelBinding`] (the TLS exporter, RFC 8446 section 7.5) once the handshake is done, and the hosts
//!   mix it, with the identity, into the ticket context: a ticket relayed to another server fails.
//!
//! No async runtime: quinn-proto is sans-IO, and the hosts' non-blocking passes drive it (the server's receive pass and
//! waits, the client's socket reads), as the web transport drives str0m. No streams this crate (`max_concurrent_*_streams`
//! 0), no 0-RTT (early data is replayable, and a connect carries a ticket and a password), the fixed bit always set
//! (`grease_quic_bit` off), migration on. The ALPN is the provisional [`ALPN`] for natives; the HTTP/3 WebTransport session
//! browsers need replaces it with `h3` later without changing the engine's bytes.
//!
//! # Maintaining
//!
//! **Module map.** `identity`: the key, its file, the certificate and the identity hash (a bounded DER walk finds the
//! SubjectPublicKeyInfo). `tls`: the rustls and quinn configs ([`QuicLimits`], [`transport_config`], [`endpoint_config`]) and
//! the client's [`PinVerifier`]. `known`: the known-servers file. `io`: the socket trait ([`Io`]) with the UDP socket and the
//! test pipe. `clock`: the clock trait with the system's and the fake one. `stand_in`: the connections' stand-in addresses.
//! `retry`: the handshake rate gate. `server`, `client`: the two endpoints.
//!
//! **Flow.** Server: `Transport::recv` with an empty inbox (or `service`) runs `QuicServer::pump`: the socket's datagrams go to
//! `Endpoint::handle`; a new handshake is decided in `incoming` (refuse, Retry, ignore, accept); a connection's datagram goes
//! to its `Connection`, then `drive` moves it along (endpoint events, its events, transmits, the next timeout) and application
//! datagrams land in the inbox under the connection's stand-in. `send_to` queues a DATAGRAM frame and drives the connection
//! at once. The client mirrors it in `QuicClient::pump` and `drive`, with the verifier's result read at `Connected`.
//!
//! **Invariants and their tests.**
//! - A datagram the AEAD does not accept never reaches the host: of the bit-flipped datagrams the fuzz delivers, none
//!   arrives as an application datagram (`tests/quic_fuzz.rs`, `TORQUE_FUZZ_QUIC`).
//! - Before a peer's address is validated the server sends it at most three times the bytes it received from it; a Retry
//!   or a refusal costs the server no connection state (`quic_fuzz`: the amplification and the Retry-under-flood checks).
//! - Connections and buffers are bounded, and only established connections count: at most `max_connections` established,
//!   `connections_per_ip` per address, `handshakes_in_progress` not yet established (dropped past `handshake_timeout`),
//!   `inbox_per_connection` datagrams waiting per connection, and quinn-proto's datagram buffers at the configured sizes
//!   (`quic_fuzz`: the memory checks; `pair.rs`: `a_flood_of_unfinished_handshakes_never_fills_the_server_or_locks_an_address_out`,
//!   `a_flooding_peer_never_costs_a_quiet_peer_a_datagram`). Validated handshakes in progress are also shared per address and
//!   per network; unvalidated ones hold at most half the places, and a validated newcomer at the cap evicts the oldest
//!   unvalidated handshake, whoever's, but a validated one only of its own address or network
//!   (`a_validated_flood_from_one_network_never_evicts_another_addresss_handshake`,
//!   `a_forged_flood_never_keeps_a_validated_newcomer_out`).
//! - Nothing reaches the host from a connection that is not established or was told gone, and every connection that ends is
//!   in `take_gone` (`a_connection_closed_as_full_is_reported_gone_and_delivers_nothing`). A backlog is capped in bytes as
//!   well as datagrams, and a datagram over `QUIC_MAX_DATAGRAM` is dropped on receipt (`a_backlog_is_capped_in_bytes_too`).
//! - The known-servers file is written through a temporary file of this process's own, synced, then moved over the old
//!   one (`known::tests::a_save_leaves_no_temporary_file_and_two_savers_do_not_clobber_each_other`).
//! - A stand-in is given back only after the host has read that its connection is gone
//!   (`a_stand_in_is_not_reused_until_the_host_has_read_that_it_is_gone`).
//! - A known server is never forgotten without the user: at the cap a new one is refused with the file's name
//!   (`known::tests::at_the_cap_a_new_server_is_refused_and_no_pin_is_forgotten`,
//!   `a_full_known_servers_file_refuses_a_new_server_naming_the_file`).
//! - The identity is the hash of the key's SubjectPublicKeyInfo, the same from the key, the PEM and the certificate
//!   (`identity::tests`); a certificate that is not one has none and never panics.
//! - A client given an identity refuses any other with a message naming both; one with none trusts the first key it sees
//!   and refuses a change (`tls::PinVerifier`; the loopback test in `torque-app`, `tests/quic_link.rs`).
//! - Both ends derive the same channel binding, and it differs per connection (`tests/pair.rs`).
//! - An engine session pair over the QUIC link (fake clock, seeded RNG, lossless pipe) delivers the same events as over the
//!   memory link (`tests/pair.rs`).
//!
//! **Extension points.** [`QuicServer::set_address_check`] (a ban list checked on every handshake and migration); [`Io`] and
//! [`Clock`] for another socket or clock; [`QuicLimits`] for the caps. A WebTransport session (`h3`) goes beside the raw ALPN
//! in `tls` and a session layer between the connection and the inbox.
//!
//! **Checks.** `cargo test -p torque-net-quic` (the fuzz smoke at its default); `TORQUE_FUZZ_QUIC=100000 cargo test -p
//! torque-net-quic --release --test quic_fuzz`; `cargo clippy -p torque-net-quic --all-targets -- -D warnings`;
//! `cargo tree --workspace --all-features -i tokio` prints nothing; `cargo tree -i aws-lc-rs` shows only str0m's path.

pub mod clock;
pub mod identity;
pub mod io;
pub mod known;
pub mod retry;
pub mod stand_in;
pub mod tls;

mod client;
mod server;

pub use client::{
    CLIENT_INBOX, CLIENT_INBOX_BYTES, LinkState, QuicClient, QuicDatagrams, QuicWatch, Trust,
    client_socket,
};
pub use clock::{Clock, FakeClock, SystemClock};
pub use identity::{IDENTITY_FILE, IdentityError, ServerKey, identity_of_certificate};
pub use io::{Flight, Io, Pipe, PipeIo, UdpIo};
pub use known::{KNOWN_SERVERS_FILE, KnownError, KnownServers};
pub use server::{Counters, QuicServer, QuicServerTransport, READS_PER_PASS};
pub use tls::{
    ALPN, PinVerifier, QUIC_MAX_DATAGRAM, QuicLimits, endpoint_config, transport_config,
};
pub use torque_net::engine::{ChannelBinding, ServerIdentity};
