//! The WebTransport session browsers need, as the least HTTP/3 that carries one (RFC 9114 frames and settings, RFC 9204
//! QPACK with the static table only, RFC 9220 extended CONNECT, RFC 9297 HTTP datagrams, draft-ietf-webtrans-http3), written
//! sans-IO over a connection's streams and fuzzed as hostile input (`tests/h3_fuzz.rs`). A desktop client may open the same
//! session ([`Dial`]), so the native tests exercise the path browsers take.
//!
//! [`Session`] is the state machine; the endpoints (`server`, `client`) drive it: they open its streams, hand it the peer's
//! bytes and carry out its [`Action`]s. The engine's datagrams then ride the connection's DATAGRAM frames with the session's
//! quarter stream id in front ([`Session::datagram_prefix`], [`Session::unwrap_datagram`]).

pub mod drive;
pub mod frames;
pub mod huffman;
pub mod qpack;
pub mod session;

pub use drive::{Driver, Outcome};
pub use session::{Action, Dial, End, MAX_STREAM_BUFFER, Origins, Session, Setup, judge};

/// The ALPN of HTTP/3, which a WebTransport session runs on (RFC 9114 section 3.1).
pub const ALPN_H3: &[u8] = b"h3";
