//! The one place a wire count becomes an allocation. A decoder that reads a count from the wire asks
//! [`count`] for the number it may believe: it is at most the cap, and at most what the bits left could hold at `min_bits_each`
//! bits an element. Only then may it reserve (use [`vec_for`]); a count that fails is a [`NetError::Cap`], never a panic and
//! never an allocation.
//!
//! Credit: the constants named after Torque's take their values from Torque3D's MIT-licensed `sim/netInterface.h` and
//! `sim/netConnection.cpp` (Copyright (c) 2012 GarageGames, LLC each); the code here is this crate's own.

use crate::NetError;

/// `writeString`'s limit in the stream (`writeString(s, 255)`: the length is 8 bits).
pub const NET_STRING_BYTES: usize = 255;

/// The per-packet string buffer (`GameConnection::writePacket` clears 256 bytes at the start of each packet).
pub const NET_STRING_BUFFER: usize = 256;

/// The most bits one [`crate::bits::BitWriter`] takes unless its owner sets a lower limit: 256 MiB.
pub const NET_MAX_STREAM_BITS: usize = 1 << 31;

/// The largest datagram this layer looks at (`Net::MaxPacketDataSize`); a bigger one is dropped unread.
pub const NET_MAX_DATAGRAM: usize = 1_500;

/// The widest ghost index of any connection (2^24 ghosts), whatever a profile or an accept asks for. Every ghost index of a
/// connection is below `1 << its width`, so what a client keeps by index is bounded by it.
pub const NET_MAX_GHOST_BITS: u8 = 24;

/// Connections that have been accepted but have not yet carried a valid game packet (TGE's `MaxPendingConnects`).
pub const NET_MAX_PENDING_CONNECTS: usize = 20;

/// Pending connections one source IP address may hold at once.
pub const NET_PENDING_PER_ADDR: usize = 2;

/// How long a connection may stay pending before it is dropped.
pub const NET_PENDING_TIMEOUT_MS: u32 = 10_000;

/// Challenge replies one source IP address is given per second (a spoofed source can make the server answer a 5-byte request with
/// a 21-byte reply; this and [`NET_CHALLENGE_REPLIES_PER_S`] bound that).
pub const NET_CHALLENGES_PER_ADDR_PER_S: u32 = 4;

/// Challenge replies the whole server sends per second.
pub const NET_CHALLENGE_REPLIES_PER_S: u32 = 2_000;

/// Established connections allowed beyond `MaxPlayers` (the table holds `MaxPlayers + 4`).
pub const NET_CONNECTIONS_MARGIN: usize = 4;

/// Game packets (whose header passes) one connection is read from per second; the rest are dropped unread: 16 times the
/// most a client sends at TGE's fastest packet rate (32 a second), which leaves room for acks, pings and bursts.
pub const NET_PACKETS_PER_CONN_PER_S: u32 = 512;

/// Verified connect requests one source IP address gets per second (each can run the host's `onConnectRequest` or draw a reject;
/// the digest has no time in it, so a copied request would otherwise replay at line rate). A small burst lets a client retry
/// and a test harness run its cases.
pub const NET_CONNECT_SCRIPT_PER_SRC_PER_S: u32 = 4;

/// Disconnect datagrams one connection is looked at for per second (a separate budget, so forged disconnects cannot starve its
/// game packets); a 32-bit sequence cannot be guessed at this rate.
pub const NET_DISCONNECTS_PER_CONN_PER_S: u32 = 8;

/// Source addresses the per-address rate counters remember at once (the oldest is forgotten first).
pub const NET_LIMITER_ENTRIES: usize = 1_024;

/// Arguments a connect request may carry (more are rejected with a reject string).
pub const NET_MAX_CONNECT_ARGS: usize = 16;

/// The smallest packet size a peer's rate may ask for (a size of 0 does not mean "default" here).
pub const NET_MIN_PACKET_BYTES: u32 = 64;

/// Idle time before a ping, and the number of pings before `TimedOut` (TGE `PingTimeout`, retry count 15).
pub const NET_TIMEOUT_MS: u32 = 4_500;
pub const NET_TIMEOUT_TRIES: u32 = 15;

/// Arguments (the command tag included) a remote command may carry when this side sends one: the senders stop at 20.
pub const NET_MAX_ARGC_SEND: usize = 20;

/// Arguments a received remote command may carry: the whole 5-bit field (0..31). A fixed 20-slot object would let 21..31 land
/// on neighbouring memory; here they are ordinary entries of a `Vec`.
pub const NET_MAX_ARGC: usize = 31;

/// The bytes of one remote-command argument or one NetString (`writeString(s, 255)`).
pub const NET_ARG_BYTES: usize = NET_STRING_BYTES;

/// The bytes of the function name a received command is looked up under (`serverCmd` + the name): a shared 1,024-byte buffer
/// less its NUL.
pub const NET_COMMAND_NAME_BYTES: usize = 1_023;

/// Remote handles (and sender entries) of one connection's string table: a NetString slot is 5 bits.
pub const NET_REMOTE_SLOTS: usize = 32;

/// Ordered events a receiver holds back for a missing earlier one (an unbounded list that keeps duplicates would let a peer grow
/// it without limit; duplicates and entries more than [`NET_EVENT_WINDOW`] ahead are dropped, so at most 127 can be waiting and
/// this is the fallback bound).
pub const NET_WAIT_LIST: usize = 128;

/// The bytes of a tag expansion (a 1,024-byte buffer; the text takes at most this less one).
pub const NET_EXPAND_BYTES: usize = 1_024;

/// The sender's ordered-event window: an event is sent only while its sequence is at most `lastAcked + 126`.
pub const NET_EVENT_WINDOW: u32 = 126;

/// Events that may wait unsent in a connection's ordered / unordered queue; a post past it is refused (backpressure, the event is
/// handed back, never dropped).
pub const NET_EVENT_QUEUE_ORDERED: usize = 4_096;
pub const NET_EVENT_QUEUE_UNORDERED: usize = 1_024;

/// Data packets whose events are remembered for an ack or a loss at once (the send window is 30).
pub const NET_EVENT_PACKETS_IN_FLIGHT: usize = 64;

/// Ghost records in flight (written, not yet acked or lost) on one connection; at the cap the packet's ghost section ends early.
/// A full 1,500-byte packet holds at most about 480 records and the window is 30 packets.
pub const NET_GHOST_REFS: usize = 65_536;

/// The most ghosts one connection may hold, whatever its configured limit says (the id width is [`NET_MAX_GHOST_BITS`] at most).
pub const NET_GHOST_LIMIT: u32 = 1 << NET_MAX_GHOST_BITS;

/// Objects waiting to enter scope through the dependents recursion of one `objectInScope` (the recursion is bounded here).
pub const NET_GHOST_DEPENDENTS: usize = 65_536;

/// Work units one packet's ghost section may spend when the caller passes no budget of its own: 1 per object a scan visits or a ghost
/// the priority pass reads, `1 + bits / 32` per record.
pub const NET_GHOST_WORK_PER_PACKET: u64 = 1_000_000;

/// Moves a connection's server-side list holds at once. The per-tick truncation to
/// the move credit (at most [`NET_MOVE_CREDIT_MAX`]) keeps the first 30 and drops the rest, so a list bounded above 30 + 31 changes
/// nothing a legitimate client can see: when it is full the NEWEST arriving moves are dropped (`lastMoveAck` still advances).
pub const NET_MOVE_LIST: usize = 64;

/// The largest move count one packet carries: the field is 5 bits.
pub const NET_MOVES_PER_PACKET: usize = 31;

/// The most moves the client's writer sends in one packet (0x1e), and how often a move is sent before it is skipped.
pub const NET_MOVES_WRITE_MAX: usize = 30;
pub const NET_MOVE_SENDS: u8 = 4;

/// A connection's move credit starts at this and is capped at it (`incMoveCredit`).
pub const NET_MOVE_CREDIT_MAX: u32 = 30;

/// Entries of a `PathManagerEvent` (an entry is 288 bits, so the bits left bound it below this).
pub const NET_PATH_ENTRIES: usize = 4_096;

/// Info replies ([`crate::serverquery`]: ping, info and simple-ping answers) one source IP address is sent per second. One
/// `queryLanServers` asks a server at most 1 broadcast ping, 4 pings and 4 info requests; this leaves room for a few in a row.
pub const NET_INFO_REPLIES_PER_SRC_PER_S: u32 = 16;

/// Info replies the whole server sends per second: an info reply is at most 1,500 bytes for a 6-byte request, so this
/// bounds what spoofed requests can make the server send.
pub const NET_INFO_REPLIES_PER_S: u32 = 256;

/// Info requests a second whose reply runs the script's `onServerInfoQuery()` (the run also draws on the network's
/// script window).
pub const NET_INFO_SCRIPT_RUNS_PER_S: u32 = 32;

/// The number of elements a wire count of `count` may stand for: `count` if it is at most `cap` and the `bits_left` can hold
/// that many elements of `min_bits_each` bits (at least one bit each), else the error naming `what`.
pub fn count(
    what: &'static str,
    count: u64,
    cap: usize,
    bits_left: usize,
    min_bits_each: usize,
) -> Result<usize, NetError> {
    let each = min_bits_each.max(1);
    let by_bits = bits_left / each;
    let limit = cap.min(by_bits);
    match usize::try_from(count) {
        Ok(n) if n <= limit => Ok(n),
        _ => Err(NetError::Cap {
            what,
            count,
            cap: limit as u64,
        }),
    }
}

/// An empty `Vec` with room for the elements a wire count of `wanted` may stand for ([`count`]).
pub fn vec_for<T>(
    what: &'static str,
    wanted: u64,
    cap: usize,
    bits_left: usize,
    min_bits_each: usize,
) -> Result<Vec<T>, NetError> {
    count(what, wanted, cap, bits_left, min_bits_each).map(Vec::with_capacity)
}

#[cfg(test)]
#[path = "cap_tests.rs"]
mod tests;
