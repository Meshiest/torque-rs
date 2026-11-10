//! The server's side of client-to-server channels: what it takes from a client that it does not trust.
//!
//! A connection whose channels are enabled and whose engine has upstream channels starts the payload of each client data packet
//! with the same section a server writes ([`crate::chunk`]), ahead of the moves, so the same [`ChunkSender`](super::ChunkSender)
//! runs on the client and the same [`ChunkReceiver`] on the server. Everything in it is hostile input, so the receiving side
//! runs under caps ([`UpstreamCaps`]) that an honest client stays far below:
//!
//! - **Held bytes**: what one connection holds for half-received messages ([`UpstreamCaps::hold_per_connection`]) and what all
//!   connections hold together ([`UpstreamCaps::hold_total`], one [`UpstreamServer`] for the server).
//! - **Message size**: a message above [`UpstreamCaps::max_message`], or above what a connection may hold, is refused from its
//!   first chunk, before anything of it is held.
//! - **Messages in flight**: [`UpstreamCaps::max_partial`] half-received messages at once (the window of each channel bounds
//!   them in any case).
//! - **Rate**: a token bucket of chunk bytes and one of completed messages per connection, counted in the milliseconds the
//!   caller passes (the simulation clock, so a run repeats).
//!
//! A cap that is passed is a [`NetError`], and the host drops the connection with the packet error it uses for any malformed
//! packet. Nothing is delivered from a packet that went over a rate cap.

use std::sync::Arc;

use super::{ChannelLimits, ChunkReceiver, Delivered, SharedHold};
use crate::NetError;
use crate::bits::BitReader;

/// The most a connection may be allowed to hold for half-received messages, whatever a server configures.
pub const MAX_UPSTREAM_HOLD: usize = 16 << 20;
/// What a connection may hold unless the server sets another cap.
pub const DEFAULT_UPSTREAM_HOLD: usize = 2 << 20;
/// What all connections may hold together unless the server sets another cap.
pub const DEFAULT_UPSTREAM_TOTAL: usize = 64 << 20;
/// The most bytes one upstream message may have, whatever a channel asks for.
pub const MAX_UPSTREAM_MESSAGE: usize = MAX_UPSTREAM_HOLD;

/// What a server takes from its clients' upstream channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpstreamCaps {
    /// What one connection holds for half-received messages (at most [`MAX_UPSTREAM_HOLD`]).
    pub hold_per_connection: usize,
    /// What all connections hold together.
    pub hold_total: usize,
    /// The most bytes in one message (and never more than `hold_per_connection`).
    pub max_message: usize,
    /// The most half-received messages one connection has at once; `None` is the channels' windows
    /// ([`WINDOW`](super::WINDOW) each).
    pub max_partial: Option<usize>,
    /// Chunk bytes a connection may send a second, counting those sent again.
    pub bytes_per_second: usize,
    /// The bytes a connection may send at once after a quiet time.
    pub byte_burst: usize,
    /// A half-received message may take this long (milliseconds) before its first byte counts, and then...
    pub message_age_ms: u32,
    /// ...this slow a sender is never stale: a message of `n` bytes may take `n / message_min_rate` seconds more.
    pub message_min_rate: usize,
    /// Messages a connection may complete a second.
    pub messages_per_second: usize,
    /// The messages it may complete at once after a quiet time.
    pub message_burst: usize,
}

impl Default for UpstreamCaps {
    fn default() -> UpstreamCaps {
        UpstreamCaps {
            hold_per_connection: DEFAULT_UPSTREAM_HOLD,
            hold_total: DEFAULT_UPSTREAM_TOTAL,
            max_message: MAX_UPSTREAM_MESSAGE,
            max_partial: None,
            message_age_ms: 30_000,
            message_min_rate: 1_024,
            bytes_per_second: 256 << 10,
            byte_burst: 256 << 10,
            messages_per_second: 64,
            message_burst: 128,
        }
    }
}

impl UpstreamCaps {
    /// The effective message cap: the configured one, and never more than a connection holds.
    pub fn message_cap(&self) -> usize {
        self.max_message.min(self.hold_per_connection)
    }

    /// The caps as the server applies them: a connection holds at most [`MAX_UPSTREAM_HOLD`] and a quarter of the server-wide
    /// budget, so no one connection can fill it and it takes several to crowd the others.
    fn clamped(mut self) -> UpstreamCaps {
        self.hold_per_connection = self
            .hold_per_connection
            .min(MAX_UPSTREAM_HOLD)
            .min((self.hold_total / 4).max(1));
        self
    }
}

/// A token bucket in thousandths of a token, so a rate below one a millisecond still refills.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    milli: u64,
    last: Option<u32>,
    rate: u64,
    burst: u64,
}

impl Bucket {
    fn new(rate: usize, burst: usize) -> Bucket {
        let burst = (burst as u64).max(1);
        Bucket {
            milli: burst.saturating_mul(1_000),
            last: None,
            rate: rate as u64,
            burst,
        }
    }

    /// Takes `n` tokens at time `now` (milliseconds); `false`, taking nothing, when they are not there.
    fn take(&mut self, now: u32, n: u64) -> bool {
        if let Some(last) = self.last {
            let elapsed = u64::from(now.wrapping_sub(last));
            self.milli = self
                .milli
                .saturating_add(self.rate.saturating_mul(elapsed))
                .min(self.burst.saturating_mul(1_000));
        }
        self.last = Some(now);
        let cost = n.saturating_mul(1_000);
        if cost > self.milli {
            return false;
        }
        self.milli -= cost;
        true
    }
}

/// The server-wide state of the upstream channels: the caps and the budget all connections draw on.
#[derive(Debug, Clone)]
pub struct UpstreamServer {
    caps: UpstreamCaps,
    shared: Arc<SharedHold>,
}

impl UpstreamServer {
    /// A server under `caps`.
    pub fn new(caps: UpstreamCaps) -> UpstreamServer {
        let caps = caps.clamped();
        UpstreamServer {
            shared: SharedHold::new(caps.hold_total),
            caps,
        }
    }

    pub fn caps(&self) -> &UpstreamCaps {
        &self.caps
    }

    /// The bytes all connections hold now for half-received messages.
    pub fn held_total(&self) -> usize {
        self.shared.held()
    }

    /// How many connections were evicted to make room for others.
    pub fn evictions(&self) -> u64 {
        self.shared.evictions()
    }

    /// The reader for one connection of channels with these `limits` (the client's upstream channels, the same on both sides).
    pub fn connection(&self, limits: &[ChannelLimits]) -> UpstreamReader {
        let caps = self.caps;
        let mut receiver = ChunkReceiver::new(limits)
            .with_hold_cap(
                caps.hold_per_connection
                    .saturating_add(super::priority_allowance(limits)),
            )
            .with_message_cap(caps.message_cap())
            .with_max_age(caps.message_age_ms, caps.message_min_rate)
            .with_shared_hold(Arc::clone(&self.shared));
        if let Some(count) = caps.max_partial {
            receiver = receiver.with_max_partial(count);
        }
        UpstreamReader {
            receiver,
            bytes: Bucket::new(caps.bytes_per_second, caps.byte_burst),
            messages: Bucket::new(caps.messages_per_second, caps.message_burst),
        }
    }
}

/// One connection's reader of the upstream section, under the server's caps.
#[derive(Debug)]
pub struct UpstreamReader {
    receiver: ChunkReceiver,
    bytes: Bucket,
    messages: Bucket,
}

impl UpstreamReader {
    /// Reads a client packet's section at `now` (milliseconds) and appends the messages it completed to `out`, in order. An
    /// error means the connection is to be dropped; nothing of the packet is then in `out`.
    pub fn read(
        &mut self,
        r: &mut BitReader<'_>,
        now: u32,
        out: &mut Vec<Delivered>,
    ) -> Result<(), NetError> {
        let start = out.len();
        self.receiver.set_now(now);
        let before = self.receiver.received_bytes();
        let result = self.receiver.read(r, out);
        let verdict = result.and_then(|()| {
            let bytes = self.receiver.received_bytes() - before;
            if !self.bytes.take(now, bytes) {
                return Err(NetError::Cap {
                    what: "upstream channel bytes a second",
                    count: bytes,
                    cap: self.bytes.burst,
                });
            }
            let done = (out.len() - start) as u64;
            if !self.messages.take(now, done) {
                return Err(NetError::Cap {
                    what: "upstream channel messages a second",
                    count: done,
                    cap: self.messages.burst,
                });
            }
            Ok(())
        });
        if verdict.is_err() {
            out.truncate(start);
        }
        verdict
    }

    /// The chunk bytes this connection has sent that no earlier chunk had carried (bytes sent again do not count): a count that
    /// only grows, for a host that wants to know a connection is still delivering.
    pub fn fresh_bytes(&self) -> u64 {
        self.receiver.fresh_bytes()
    }

    /// [`fresh_bytes`](Self::fresh_bytes) of the channel with this id (the order of the limits the reader was made with).
    pub fn fresh_bytes_of(&self, channel: usize) -> u64 {
        self.receiver.fresh_bytes_of(channel)
    }

    /// The bytes this connection holds for half-received messages.
    pub fn held_bytes(&self) -> usize {
        self.receiver.held_bytes()
    }

    /// Whether the connection was evicted or ended by a stale message: it holds nothing and is to be dropped.
    pub fn is_dead(&self) -> bool {
        self.receiver.is_dead()
    }

    /// Looks for an eviction or a stale message at `now` (milliseconds), freeing what a dead connection held. Call it each tick.
    pub fn sweep(&mut self, now: u32) -> Result<(), NetError> {
        self.receiver.sweep(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_refills_with_the_clock_and_never_past_its_burst() {
        let mut b = Bucket::new(1_000, 500);
        assert!(b.take(0, 500));
        assert!(!b.take(0, 1));
        assert!(b.take(100, 100));
        assert!(!b.take(100, 1));
        // A long quiet time fills it to the burst and no further.
        assert!(!b.take(1_000_000, 501));
        assert!(b.take(1_000_000, 500));
    }

    #[test]
    fn the_clock_wrapping_is_a_refill_not_a_panic() {
        let mut b = Bucket::new(1_000, 10);
        assert!(b.take(u32::MAX - 5, 10));
        assert!(b.take(4, 1));
    }

    #[test]
    fn the_message_cap_never_passes_what_a_connection_holds() {
        let caps = UpstreamCaps {
            hold_per_connection: 1 << 20,
            max_message: 8 << 20,
            ..UpstreamCaps::default()
        };
        assert_eq!(caps.message_cap(), 1 << 20);
        let server = UpstreamServer::new(UpstreamCaps {
            hold_per_connection: usize::MAX,
            ..UpstreamCaps::default()
        });
        assert_eq!(server.caps().hold_per_connection, MAX_UPSTREAM_HOLD);
    }

    #[test]
    fn a_connection_holds_the_priority_channels_room_beyond_its_cap() {
        let limits = [
            ChannelLimits::new(100),
            ChannelLimits::new(100)
                .with_max_backlog(65_536)
                .with_priority(),
        ];
        let server = UpstreamServer::new(UpstreamCaps::default());
        let reader = server.connection(&limits);
        assert_eq!(
            reader.receiver.hold_cap,
            DEFAULT_UPSTREAM_HOLD + 65_536,
            "an honest client queues its bulk up to the cap and its priority channel up to its own backlog"
        );
        let plain = server.connection(&limits[..1]);
        assert_eq!(plain.receiver.hold_cap, DEFAULT_UPSTREAM_HOLD);
    }
}
