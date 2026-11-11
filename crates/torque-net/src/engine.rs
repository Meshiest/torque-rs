//! Engine mode's own framing: the engine's protocol, which owes TGE nothing on the wire.
//!
//! It runs over a [`Link`] (unreliable, unordered datagrams of a size the link reports each tick, with backpressure and the
//! facts the connect flow reads) and puts its own sequence numbers, acks, lanes and fragmentation on top, so the same bytes run
//! over QUIC datagrams, WebTransport, WebRTC, WebSockets, plain UDP and the memory link. Security, address validation and
//! congestion control come from the link where it has them; on a plain link the [`Gate`] runs a stateless cookie round trip
//! first, and plain links are for LAN and development only.
//!
//! - The framing, re-exported here: the datagrams ([`Hello`], [`Cookie`], `Opening`, `Data` and `TokenData`, [`Reject`],
//!   [`Disconnect`]; [`parse`] reads one), the control messages ([`Connect`], [`Accept`]) and the packet [`Body`] with its
//!   lane sections ([`Fragment`], [`Unreliable`]).
//! - [`link`]: the link contract.
//! - [`session`]: one peer's session on either side: the connect flow, the lanes (control, RPC, unreliable, then the stream
//!   lanes the host writes: ghosts, moves, channels), their budgets, and the notify of packets that carried stream data.
//! - [`gate`]: what a plain link's server does with datagrams from addresses that have no session.
//! - [`table`]: a server's sessions: routing by source, the gate in front of plain links, the per-source caps the host owes.
//! - [`order`]: the wide packet numbers, in send order, that the layers above a session key their records by.
//! - [`bridge`]: the connect message as the connect fields, block and ticket context both modes judge a connect by.
//!
//! **Versioned.** Byte 0 of every datagram is [`ENGINE_TAG`]: the engine family in the high nibble, the framing version in the
//! low one. Any change to these bytes is a new [`ENGINE_FRAMING`], and every version's goldens are kept
//! (`tests/engine_golden_v1.rs`). No TGE datagram starts with the engine family, and it lies in QUIC's long-header range, so
//! plain engine UDP never shares a port with QUIC.
//!
//! **Every decoder reads hostile input**, returns a value or a [`crate::NetError`] and never panics; what a peer can make a
//! session hold is bounded by [`LaneLimits`] and [`session::MAX_EVENTS`].
//!
//! **What the host owes.** These are state machines; the host that owns the sockets and the session table must also:
//! - cap the established sessions one source address (and network) may hold, and the sessions in all: the gate bounds only
//!   the pending ones, and a validated link's server sessions are made by the host;
//! - route each datagram by its source to that source's session, and only datagrams without one to the gate, then
//!   [`Gate::settle`] a source once its session is established or closed;
//! - drain every session's events each tick ([`Session::next_event`]): a session that holds too many closes;
//! - draw nonces, session ids and gate keys from a cryptographic random source, and refuse plain links on public addresses.

pub mod bridge;
pub(crate) mod framing;
pub mod gate;
mod lanes;
pub mod link;
pub mod order;
pub mod session;
pub mod table;

pub use framing::{
    Accept, Body, COOKIE_BYTES, Connect, Control, Cookie, CookieValue, Datagram, Disconnect,
    Fragment, Header, Hello, Kind, Lane, MAX_ACCEPT_WIDTH, MIN_ACCEPT_PACKET, Prefix, Reject,
    RejectCode, STR_BYTES, StreamLane, TableDigests, Unreliable, Versions, Widths, one_line, parse,
};
pub use gate::{Gate, GateConfig, GateOutcome};
pub use lanes::{ACK_BITS, MESSAGE_WINDOW, PACKET_WINDOW, REORDER_THRESHOLD, SendError};
pub use link::{ChannelBinding, Link, LinkFacts, LinkKind, NoRoom, ServerIdentity};
pub use order::PacketOrder;
pub use session::{
    CloseReason, Delivery, Event, LaneBudgets, LaneLimits, NoStreams, PacketFates, Polled, Session,
    SessionConfig, State, StreamSource,
};
pub use table::{Arrival, Peer, SessionTable, Stage, TableLimits, canonical};

/// The engine framing version this build speaks.
pub const ENGINE_FRAMING: u8 = 1;

/// The high nibble of byte 0 of every engine datagram.
pub const ENGINE_FAMILY: u8 = 0xe0;

/// Byte 0 of every datagram of this build's framing.
pub const ENGINE_TAG: u8 = ENGINE_FAMILY | ENGINE_FRAMING;

/// The size a `Hello` is padded to, and the size every `Opening` on a plain link is padded to (or the link's datagram size,
/// if that is smaller): no answer before the address is proven is larger than what it answers.
pub const HELLO_BYTES: usize = 1_200;

/// The packet size limit before an accept sets one.
pub const DEFAULT_MAX_PACKET: u16 = 1_200;

/// The largest datagram a session builds or reads.
pub const MAX_PACKET: usize = u16::MAX as usize;
