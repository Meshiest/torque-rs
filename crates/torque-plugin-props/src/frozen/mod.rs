//! Frozen props over the `props-frozen` bulk channel: a server stops ghosting frozen props to a client past a count and sends
//! their static state (id, datablock, quantized pose, owner) over a bulk channel instead; the client keeps them in a table,
//! draws them as still copies of their shapes' static batches and collides with them as with ghosts.
//!
//! A ghost costs a table entry on the client (at most a connection's ghost limit), a record in packets and a place in the scope
//! walk, for a prop that never moves; a frozen prop on the channel costs 27 bytes once and nothing after. The server's half
//! ([`server`]) deals each client the frozen props and does the handover; the messages ([`messages`]) are what goes over the
//! channel; the client's half ([`client`]) is its table, the channel's logic and the stills it hands the renderer.
//!
//! It stands on two engine seams that are not about props: the scope control (`engine.net().scope_control()`), which keeps an
//! object out of one connection's ghosts, and the stills (`engine.net().stills()`), shapes a client draws as it draws its
//! resting ghosts without a ghost of them.

pub mod client;
pub mod messages;
pub mod server;

use torque_net::chunk::ChannelLimits;

/// The name of the channel in the manifest.
pub const CHANNEL: &str = "props-frozen";

/// The most ops a server puts in one message.
pub const MAX_OPS_PER_SEND: usize = 1024;

/// The bytes the channel may have queued for a client and not yet acknowledged before the server waits to send more.
pub const SEND_BACKLOG: usize = 64 << 10;

/// The bytes of each packet the channel's chunks take at most.
pub const PACKET_BUDGET: usize = 400;

/// The channel's limits: messages of up to [`messages::MAX_MESSAGE_BYTES`] bytes, a backlog of a megabyte.
#[must_use]
pub fn limits() -> ChannelLimits {
    ChannelLimits {
        max_message: messages::MAX_MESSAGE_BYTES,
        max_backlog: 1 << 20,
        ..ChannelLimits::new(PACKET_BUDGET)
    }
}
