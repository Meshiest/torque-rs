//! Bulk channels: large, mostly static data sent from a server to a client in chunks that ride at the front of data packets.
//!
//! A *channel* carries *messages*: byte strings of up to the channel's [`ChannelLimits::max_message`], each a
//! [`Kind::Snapshot`] (the whole state a client should have) or a [`Kind::Delta`] (what changed since). A message is cut into
//! chunks that go into the packets a connection sends anyway, under a per-packet byte budget, so a long transfer cannot starve
//! the events and ghosts that share the packets. Delivery is reliable and ordered per channel: the sender learns each packet's
//! fate from the packet layer ([`crate::protocol::Notify`]) and sends the chunks of a lost packet again; the receiver assembles
//! chunks by byte range (they may arrive in any order, twice, or overlapping) and hands over whole messages in the order they
//! were queued.
//!
//! **Wire.** A connection that has channels enabled starts the payload of each server data packet with this section:
//!
//! | field | bits |
//! |---|---|
//! | any chunk follows | 1 |
//! | channel id | `ceil(log2(channels))` |
//! | kind (0 snapshot, 1 delta) | 1 |
//! | message sequence | 16 |
//! | message length | `bit_length(max_message)` |
//! | offset of the chunk | the same |
//! | chunk length | 12 |
//! | the chunk's bytes | 8 each |
//! | another chunk follows | 1 |
//!
//! (the fields from the channel id to the last flag repeat for each chunk). A connection with no channels has no section, so
//! its packets are the same bits as before channels existed.
//!
//! **Both directions.** A client's data packets start with the same section when the engine has client-to-server channels
//! (the roles swap: the client holds the [`ChunkSender`], the server a [`ChunkReceiver`] under the caps of [`upstream`]).
//!
//! **Hostile input.** [`ChunkReceiver::read`] takes bytes from a peer: every length is checked against the channel's limits and
//! the bits left before anything is allocated, a message is held only inside a small window of sequence numbers, and what a
//! channel holds is bounded by `max_message * window`. A violation is a [`NetError`] and the connection is dropped with the
//! packet error the host uses for any malformed packet; nothing here panics.
//!
//! **Limits on the sending side.** [`ChunkSender::enqueue`] refuses a message above the size cap and one that would take the
//! connection's backlog (the bytes queued and not yet acknowledged) above its cap, with a [`SendError`] and nothing queued.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::NetError;
use crate::bits::{BitReader, BitWriter, bit_length};
use crate::cap::NET_EVENT_PACKETS_IN_FLIGHT;

pub mod patch;
pub mod registry;
pub mod upstream;

/// The most channels one connection has.
pub const MAX_CHANNELS: usize = 64;
/// The width of a chunk's length field, and so the most bytes one chunk carries.
pub const CHUNK_LEN_BITS: u8 = 12;
/// The most bytes in one chunk.
pub const MAX_CHUNK_BYTES: usize = (1 << CHUNK_LEN_BITS) - 1;
/// The most chunks one packet's section carries.
pub const MAX_CHUNKS_PER_PACKET: usize = 64;
/// The messages of a channel in flight at once (the receiver holds this many sequence numbers).
pub const WINDOW: usize = 4;
/// The messages a connection may have queued in one channel.
pub const MAX_QUEUED_MESSAGES: usize = 64;
/// The most separate byte ranges a half-received message may be in (a hostile peer cannot make the receiver track more).
pub const MAX_RANGES: usize = 1_024;
/// The least the default for the bytes a receiver holds for half-received messages ([`shared_hold_cap`]) is.
pub const DEFAULT_HOLD_CAP: usize = 8 << 20;
/// The most that default is.
pub const MAX_HOLD_CAP: usize = 64 << 20;

/// What the channels that share a sender's total cap may queue across them (and what a receiver of them holds before the priority
/// channels' room is added): a window of messages of the largest size ([`WINDOW`] times it), between [`DEFAULT_HOLD_CAP`] and
/// [`MAX_HOLD_CAP`]. A sender holds its shared channels to this, so what an honest sender leaves half-received is never more
/// than the receiver allows.
pub fn shared_hold_cap(limits: &[ChannelLimits]) -> usize {
    let largest = limits.iter().map(|l| l.max_message).max().unwrap_or(0);
    largest
        .saturating_mul(WINDOW)
        .clamp(DEFAULT_HOLD_CAP, MAX_HOLD_CAP)
}

/// The backlog the priority channels of `limits` may have queued beyond the shared cap (each is held to its own `max_backlog`
/// and not to [`shared_hold_cap`]).
pub fn priority_allowance(limits: &[ChannelLimits]) -> usize {
    limits
        .iter()
        .filter(|l| l.priority)
        .map(|l| l.max_backlog)
        .fold(0, usize::saturating_add)
}

/// What a receiver of channels with these `limits` holds for half-received messages, unless it is told another cap: the shared
/// cap ([`shared_hold_cap`]) and the room of the priority channels ([`priority_allowance`]). A sender of the same channels
/// queues at most that much (its shared channels up to the first, its priority channels up to the second), so an honest sender
/// is never dropped for what it left half-received.
pub fn default_hold_cap(limits: &[ChannelLimits]) -> usize {
    shared_hold_cap(limits).saturating_add(priority_allowance(limits))
}

/// The size limit no channel may be configured above: the width of a length field.
pub const MAX_MESSAGE_LIMIT: usize = 1 << 30;

/// What a message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// All of the state a client should have.
    Snapshot,
    /// What changed since the last message.
    Delta,
}

/// What a channel is allowed to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelLimits {
    /// The most bytes of one packet the channel's chunks take, their headers included.
    pub budget: usize,
    /// The most bytes in one message.
    pub max_message: usize,
    /// The most bytes a connection may have queued in the channel and not yet acknowledged.
    pub max_backlog: usize,
    /// Whether the channel's chunks are written before the other channels' in every packet (still within its own budget), so
    /// a bulk transfer on another channel never delays it.
    pub priority: bool,
}

impl ChannelLimits {
    /// A message of up to 4 MiB, a backlog of up to 8 MiB, and `budget` bytes of each packet.
    pub fn new(budget: usize) -> ChannelLimits {
        ChannelLimits {
            budget,
            max_message: 4 << 20,
            max_backlog: 8 << 20,
            priority: false,
        }
    }

    /// The same, written before the other channels in each packet.
    pub fn with_priority(self) -> ChannelLimits {
        ChannelLimits {
            priority: true,
            ..self
        }
    }

    /// The same with a message cap of `bytes` (and a backlog of twice that).
    pub fn with_max_message(self, bytes: usize) -> ChannelLimits {
        ChannelLimits {
            max_message: bytes.clamp(1, MAX_MESSAGE_LIMIT),
            max_backlog: bytes.saturating_mul(2).min(MAX_MESSAGE_LIMIT),
            ..self
        }
    }

    /// The same with a backlog cap of `bytes`.
    pub fn with_max_backlog(self, bytes: usize) -> ChannelLimits {
        ChannelLimits {
            max_backlog: bytes,
            ..self
        }
    }

    fn size_bits(&self) -> u8 {
        bit_length(u32::try_from(self.max_message.min(MAX_MESSAGE_LIMIT)).unwrap_or(u32::MAX))
    }
}

/// Why a message was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The channel does not exist.
    UnknownChannel,
    /// The message is empty.
    Empty,
    /// The message is above the channel's size cap.
    TooLarge { len: usize, cap: usize },
    /// The connection's backlog (or its message count) would pass its cap: the client is not keeping up.
    Backlog,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::UnknownChannel => f.write_str("no such channel"),
            SendError::Empty => f.write_str("an empty message"),
            SendError::TooLarge { len, cap } => {
                write!(f, "a message of {len} bytes is above the cap of {cap}")
            }
            SendError::Backlog => f.write_str("the connection's backlog is full"),
        }
    }
}

impl std::error::Error for SendError {}

/// The bits of a channel id for `channels` channels.
pub fn id_bits(channels: usize) -> u8 {
    bit_length(u32::try_from(channels.saturating_sub(1)).unwrap_or(u32::MAX))
}

fn header_bits(id_bits: u8, limits: &ChannelLimits) -> usize {
    // channel, kind, sequence, total, offset, length, the flag after the chunk.
    usize::from(id_bits)
        + 1
        + 16
        + 2 * usize::from(limits.size_bits())
        + usize::from(CHUNK_LEN_BITS)
        + 1
}

// ---- the sender

/// A message being sent.
#[derive(Debug)]
struct Msg {
    seq: u16,
    /// Never reused on this sender, unlike `seq`, which wraps.
    id: u64,
    kind: Kind,
    data: Arc<[u8]>,
    /// The first byte never sent.
    fresh: usize,
    /// Ranges sent and lost, to go again.
    retry: VecDeque<(usize, usize)>,
    /// Bytes acknowledged.
    acked: usize,
}

impl Msg {
    fn done(&self) -> bool {
        self.acked >= self.data.len()
    }
}

/// A queued message, to ask whether it is still waiting for acknowledgement ([`ChunkSender::queued_len`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHandle {
    channel: usize,
    id: u64,
}

#[derive(Debug, Default)]
struct SendChan {
    next_seq: u16,
    queue: VecDeque<Msg>,
    queued_bytes: usize,
}

/// A range in a packet that is not yet acknowledged or lost.
#[derive(Debug, Clone, Copy)]
struct Sent {
    chan: usize,
    seq: u16,
    offset: usize,
    len: usize,
}

/// One connection's send side of every channel.
#[derive(Debug)]
pub struct ChunkSender {
    limits: Vec<ChannelLimits>,
    id_bits: u8,
    chans: Vec<SendChan>,
    in_flight: VecDeque<(u32, Vec<Sent>)>,
    rotate: usize,
    total_cap: usize,
    next_id: u64,
}

impl ChunkSender {
    /// A sender for channels with these `limits` (the channel id is the index).
    pub fn new(limits: &[ChannelLimits]) -> ChunkSender {
        let limits: Vec<ChannelLimits> = limits.iter().copied().take(MAX_CHANNELS).collect();
        ChunkSender {
            id_bits: id_bits(limits.len()),
            chans: limits.iter().map(|_| SendChan::default()).collect(),
            in_flight: VecDeque::new(),
            rotate: 0,
            total_cap: shared_hold_cap(&limits),
            next_id: 0,
            limits,
        }
    }

    /// The same sender with at most `bytes` queued across the channels that share the total cap ([`shared_hold_cap`] otherwise; keep it at or below what the receiver holds, less the room of the priority channels).
    #[must_use]
    pub fn with_total_backlog(mut self, bytes: usize) -> ChunkSender {
        self.total_cap = bytes;
        self
    }

    /// The bytes queued across all channels and not yet acknowledged.
    pub fn total_backlog(&self) -> usize {
        self.chans.iter().map(|c| c.queued_bytes).sum()
    }

    /// The bytes the channels that share the total cap have queued (every channel but the priority ones).
    fn shared_backlog(&self) -> usize {
        self.chans
            .iter()
            .zip(&self.limits)
            .filter(|(_, limits)| !limits.priority)
            .map(|(c, _)| c.queued_bytes)
            .sum()
    }

    /// How many channels there are.
    pub fn channels(&self) -> usize {
        self.limits.len()
    }

    /// Queues `data` on `channel`. Nothing is queued on an error.
    pub fn enqueue(&mut self, channel: usize, kind: Kind, data: Vec<u8>) -> Result<(), SendError> {
        self.enqueue_shared(channel, kind, Arc::from(data))
    }

    /// Queues a message that other connections queue too: the bytes are held once and shared. Nothing is queued on an error.
    pub fn enqueue_shared(
        &mut self,
        channel: usize,
        kind: Kind,
        data: Arc<[u8]>,
    ) -> Result<(), SendError> {
        self.enqueue_shared_with_handle(channel, kind, data)
            .map(|_| ())
    }

    /// [`enqueue_shared`](Self::enqueue_shared) that returns a handle to the queued message.
    pub fn enqueue_shared_with_handle(
        &mut self,
        channel: usize,
        kind: Kind,
        data: Arc<[u8]>,
    ) -> Result<MessageHandle, SendError> {
        let (Some(limits), Some(chan)) = (self.limits.get(channel), self.chans.get(channel)) else {
            return Err(SendError::UnknownChannel);
        };
        if data.is_empty() {
            return Err(SendError::Empty);
        }
        // A message the receiver could not hold whole is refused too.
        let cap = limits.max_message.min(self.total_cap);
        if data.len() > cap {
            return Err(SendError::TooLarge {
                len: data.len(),
                cap,
            });
        }
        let over_channel = chan.queue.len() >= MAX_QUEUED_MESSAGES
            || chan.queued_bytes.saturating_add(data.len()) > limits.max_backlog;
        // A priority channel is held to its own backlog and not to the budget the bulk channels share: a transfer that fills that
        // budget must not make a call's frames wait for it.
        let over_shared =
            !limits.priority && self.shared_backlog().saturating_add(data.len()) > self.total_cap;
        if over_channel || over_shared {
            return Err(SendError::Backlog);
        }
        let id = self.next_id;
        self.next_id += 1;
        let chan = &mut self.chans[channel];
        let seq = chan.next_seq;
        chan.next_seq = chan.next_seq.wrapping_add(1);
        chan.queued_bytes += data.len();
        chan.queue.push_back(Msg {
            seq,
            id,
            kind,
            data,
            fresh: 0,
            retry: VecDeque::new(),
            acked: 0,
        });
        Ok(MessageHandle { channel, id })
    }

    /// The length of the message `handle` names while it is still queued (not yet fully acknowledged, so its whole length is
    /// in [`backlog`](Self::backlog)), and 0 once it is acknowledged or dropped.
    pub fn queued_len(&self, handle: MessageHandle) -> usize {
        self.chans
            .get(handle.channel)
            .and_then(|c| c.queue.iter().find(|m| m.id == handle.id))
            .map_or(0, |m| m.data.len())
    }

    /// Drops the messages of `channel` that have not started going out (the newest ones: a message starts only after the one
    /// before it has nothing more to send), and returns the bytes dropped. A snapshot that replaces what was queued uses this
    /// first.
    pub fn drop_unstarted(&mut self, channel: usize) -> usize {
        let Some(chan) = self.chans.get_mut(channel) else {
            return 0;
        };
        let mut dropped = 0;
        while chan.queue.back().is_some_and(|m| m.fresh == 0) {
            if let Some(m) = chan.queue.pop_back() {
                dropped += m.data.len();
                chan.next_seq = m.seq;
            }
        }
        chan.queued_bytes -= dropped;
        dropped
    }

    /// The bytes queued on `channel` and not yet acknowledged.
    pub fn backlog(&self, channel: usize) -> usize {
        self.chans.get(channel).map_or(0, |c| c.queued_bytes)
    }

    /// The messages queued on `channel` and not yet acknowledged.
    pub fn queued(&self, channel: usize) -> usize {
        self.chans.get(channel).map_or(0, |c| c.queue.len())
    }

    /// Whether any chunk waits to go out (a first send or a send again).
    pub fn has_work(&self) -> bool {
        (0..self.chans.len()).any(|c| self.next_range(c).is_some())
    }

    /// Whether nothing is queued and nothing is in a packet that might still be lost.
    pub fn is_idle(&self) -> bool {
        self.chans.iter().all(|c| c.queue.is_empty()) && self.in_flight.is_empty()
    }

    /// The next range of `channel` to send: the oldest message in the window with a lost range or unsent bytes. Returns the
    /// message's place in the queue, the offset, and how many bytes the range has.
    fn next_range(&self, channel: usize) -> Option<(usize, usize, usize)> {
        let chan = self.chans.get(channel)?;
        for (i, m) in chan.queue.iter().enumerate().take(WINDOW) {
            if let Some(&(offset, len)) = m.retry.front() {
                return Some((i, offset, len));
            }
            if m.fresh < m.data.len() {
                return Some((i, m.fresh, m.data.len() - m.fresh));
            }
        }
        None
    }

    /// Writes the section: a flag, and when chunks fit in `room_bits` (the section's whole room, the flag included) the chunks
    /// the channels' budgets allow. `send_seq` names the packet the chunks go in. Returns the bytes of chunk data written.
    ///
    /// A packet that is then withheld or lost is reported by [`ChunkSender::on_notify`] as any other: its ranges go again.
    pub fn write(&mut self, w: &mut BitWriter, send_seq: u32, room_bits: usize) -> usize {
        let plan = self.plan(room_bits.saturating_sub(1));
        if plan.is_empty() {
            w.write_flag(false);
            return 0;
        }
        w.write_flag(true);
        let mut bytes = 0;
        for (i, sent) in plan.iter().enumerate() {
            let limits = &self.limits[sent.chan];
            let size_bits = limits.size_bits();
            // The plan was just made from the queue and nothing leaves it before the packet is written.
            let msg = self.chans[sent.chan]
                .queue
                .iter()
                .find(|m| m.seq == sent.seq)
                .expect("a planned range's message is queued");
            w.write_bits(sent.chan as u32, self.id_bits);
            w.write_flag(msg.kind == Kind::Delta);
            w.write_bits(u32::from(sent.seq), 16);
            w.write_bits(msg.data.len() as u32, size_bits);
            w.write_bits(sent.offset as u32, size_bits);
            w.write_bits(sent.len as u32, CHUNK_LEN_BITS);
            w.write_bytes(&msg.data[sent.offset..sent.offset + sent.len]);
            w.write_flag(i + 1 < plan.len());
            bytes += sent.len;
        }
        self.in_flight.push_back((send_seq, plan));
        while self.in_flight.len() > NET_EVENT_PACKETS_IN_FLIGHT {
            // A fate that never came is a loss.
            if let Some((seq, _)) = self.in_flight.front().cloned() {
                self.on_notify(seq, false);
            }
        }
        bytes
    }

    /// Picks the ranges for one packet and takes them off the queues.
    fn plan(&mut self, room_bits: usize) -> Vec<Sent> {
        let mut plan = Vec::new();
        let n = self.chans.len();
        let mut left = room_bits;
        // Priority channels go first, in id order; the others take turns at the front.
        let order: Vec<usize> = (0..n)
            .filter(|&c| self.limits[c].priority)
            .chain(
                (0..n)
                    .map(|k| (self.rotate + k) % n)
                    .filter(|&c| !self.limits[c].priority),
            )
            .collect();
        for c in order {
            let overhead = header_bits(self.id_bits, &self.limits[c]);
            let mut channel_left = (self.limits[c].budget.saturating_mul(8)).min(left);
            while plan.len() < MAX_CHUNKS_PER_PACKET {
                let Some((at, offset, avail)) = self.next_range(c) else {
                    break;
                };
                if channel_left < overhead + 8 {
                    break;
                }
                let len = avail
                    .min((channel_left - overhead) / 8)
                    .min(MAX_CHUNK_BYTES);
                let msg = &mut self.chans[c].queue[at];
                match msg.retry.front_mut() {
                    Some(front) => {
                        if len == front.1 {
                            msg.retry.pop_front();
                        } else {
                            *front = (front.0 + len, front.1 - len);
                        }
                    }
                    None => msg.fresh += len,
                }
                plan.push(Sent {
                    chan: c,
                    seq: msg.seq,
                    offset,
                    len,
                });
                let cost = overhead + len * 8;
                channel_left -= cost;
                left -= cost;
            }
        }
        if n > 0 {
            self.rotate = (self.rotate + 1) % n;
        }
        plan
    }

    /// The fate of the data packet sent as `send_seq`: its ranges are acknowledged, or queued to go again.
    pub fn on_notify(&mut self, send_seq: u32, delivered: bool) {
        let Some(at) = self.in_flight.iter().position(|(s, _)| *s == send_seq) else {
            return;
        };
        let Some((_, sent)) = self.in_flight.remove(at) else {
            return;
        };
        for s in sent {
            let Some(chan) = self.chans.get_mut(s.chan) else {
                continue;
            };
            let Some(msg) = chan.queue.iter_mut().find(|m| m.seq == s.seq) else {
                continue;
            };
            if delivered {
                msg.acked += s.len;
            } else {
                msg.retry.push_back((s.offset, s.len));
            }
        }
        for chan in &mut self.chans {
            while chan.queue.front().is_some_and(Msg::done) {
                if let Some(m) = chan.queue.pop_front() {
                    chan.queued_bytes -= m.data.len();
                }
            }
        }
    }
}

// ---- the receiver

/// A message that arrived whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub channel: usize,
    pub kind: Kind,
    pub data: Vec<u8>,
}

#[derive(Debug)]
struct Partial {
    kind: Kind,
    total: usize,
    /// When the first chunk arrived (the receiver's clock).
    started: u32,
    /// Allocated for the whole message when its first chunk arrives, and charged to the caps as such.
    buf: Vec<u8>,
    /// The byte ranges received, sorted, disjoint and not touching.
    ranges: Vec<(usize, usize)>,
}

impl Partial {
    fn complete(&self) -> bool {
        matches!(self.ranges.as_slice(), [(0, end)] if *end == self.total)
    }

    fn insert(&mut self, offset: usize, bytes: &[u8]) -> Result<(), NetError> {
        let end = offset + bytes.len();
        if self.buf.len() < end {
            self.buf.resize(end, 0);
        }
        self.buf[offset..end].copy_from_slice(bytes);
        let first = self.ranges.partition_point(|&(_, e)| e < offset);
        let mut start = offset;
        let mut stop = end;
        let mut last = first;
        while let Some(&(s, e)) = self.ranges.get(last) {
            if s > end {
                break;
            }
            start = start.min(s);
            stop = stop.max(e);
            last += 1;
        }
        self.ranges.splice(first..last, [(start, stop)]);
        if self.ranges.len() > MAX_RANGES {
            return Err(NetError::Cap {
                what: "chunk ranges of a message",
                count: self.ranges.len() as u64,
                cap: MAX_RANGES as u64,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct RecvChan {
    next: u16,
    window: VecDeque<Option<Partial>>,
}

/// One connection's receive side of every channel.
#[derive(Debug)]
pub struct ChunkReceiver {
    limits: Vec<ChannelLimits>,
    id_bits: u8,
    chans: Vec<RecvChan>,
    hold_cap: usize,
    /// The most bytes in one message, below the channels' own caps.
    message_cap: usize,
    /// The most half-received messages at once, across channels.
    max_partial: usize,
    shared: Option<Arc<SharedHold>>,
    /// This receiver's place in `shared`.
    holder: u64,
    /// The receiver's clock, in milliseconds ([`set_now`](ChunkReceiver::set_now)).
    now: u32,
    /// A half-received message may take this long (milliseconds) and, per byte of its size, this much more.
    age: Option<(u32, usize)>,
    /// Evicted, or a message went stale: nothing is held and every read is an error.
    dead: bool,
    /// The chunk bytes read so far (counting those that arrive twice).
    received: u64,
}

#[derive(Debug)]
struct Holder {
    held: usize,
    evicted: bool,
}

#[derive(Debug, Default)]
struct HoldState {
    total: usize,
    next: u64,
    holders: BTreeMap<u64, Holder>,
}

/// A byte budget several receivers draw on (the connections of a server): what all of them hold for half-received messages
/// together stays within it.
///
/// A chunk that would take the budget past its cap is never refused for the connection that happens to send it (that would let
/// connections that fill the budget get honest ones dropped, and a chunk refused after its packet was acknowledged is never sent
/// again). The holders that hold more than the newcomer would are **evicted** instead, the largest first: their bytes leave the
/// count at once, and their next read is an error (the host drops them). A newcomer that is itself the largest holder is the one
/// refused.
#[derive(Debug)]
pub struct SharedHold {
    state: Mutex<HoldState>,
    cap: usize,
    evictions: AtomicU64,
}

impl SharedHold {
    /// A budget of `cap` bytes.
    pub fn new(cap: usize) -> Arc<SharedHold> {
        Arc::new(SharedHold {
            state: Mutex::new(HoldState::default()),
            cap,
            evictions: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HoldState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The bytes held now by the receivers sharing the budget.
    pub fn held(&self) -> usize {
        self.lock().total
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// How many receivers have been evicted to make room.
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    fn join(&self) -> u64 {
        let mut st = self.lock();
        let id = st.next;
        st.next += 1;
        st.holders.insert(
            id,
            Holder {
                held: 0,
                evicted: false,
            },
        );
        id
    }

    fn leave(&self, id: u64) {
        let mut st = self.lock();
        if let Some(h) = st.holders.remove(&id) {
            st.total -= h.held;
        }
    }

    fn is_evicted(&self, id: u64) -> bool {
        self.lock().holders.get(&id).is_none_or(|h| h.evicted)
    }

    /// Records that `id` holds `held` bytes now (an evicted holder holds none).
    fn set_held(&self, id: u64, held: usize) {
        let mut st = self.lock();
        let Some(h) = st.holders.get_mut(&id) else {
            return;
        };
        if h.evicted {
            return;
        }
        let before = std::mem::replace(&mut h.held, held);
        st.total = st.total - before + held;
    }

    /// Makes room for `id` to hold `new_held` bytes: evicts the other holders that hold more than that, the largest first,
    /// while the budget would be past its cap. `false` when it would still be (`id` is the heaviest).
    fn reserve(&self, id: u64, new_held: usize) -> bool {
        let mut st = self.lock();
        let mine = st.holders.get(&id).map_or(0, |h| h.held);
        let mut others = st.total - mine;
        if others.saturating_add(new_held) <= self.cap {
            return true;
        }
        let mut heavier: Vec<(usize, u64)> = st
            .holders
            .iter()
            .filter(|(k, h)| **k != id && !h.evicted && h.held > new_held)
            .map(|(k, h)| (h.held, *k))
            .collect();
        heavier.sort_by(|a, b| b.cmp(a));
        for (held, k) in heavier {
            if others.saturating_add(new_held) <= self.cap {
                break;
            }
            if let Some(h) = st.holders.get_mut(&k) {
                h.held = 0;
                h.evicted = true;
            }
            st.total -= held;
            others -= held;
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
        others.saturating_add(new_held) <= self.cap
    }
}

impl ChunkReceiver {
    /// A receiver for channels with these `limits` (the same, in the same order, as the sender's).
    pub fn new(limits: &[ChannelLimits]) -> ChunkReceiver {
        let limits: Vec<ChannelLimits> = limits.iter().copied().take(MAX_CHANNELS).collect();
        ChunkReceiver {
            id_bits: id_bits(limits.len()),
            chans: limits.iter().map(|_| RecvChan::default()).collect(),
            hold_cap: default_hold_cap(&limits),
            message_cap: usize::MAX,
            max_partial: usize::MAX,
            shared: None,
            holder: 0,
            now: 0,
            age: None,
            dead: false,
            received: 0,
            limits,
        }
    }

    /// The same receiver that also draws on `shared`: a chunk that would take what all the receivers of the budget hold past
    /// its cap evicts the larger holders first (see [`SharedHold`]), and what this receiver holds is given back when it is
    /// dropped.
    #[must_use]
    pub fn with_shared_hold(mut self, shared: Arc<SharedHold>) -> ChunkReceiver {
        self.holder = shared.join();
        self.shared = Some(shared);
        self
    }

    /// The same receiver giving up on a half-received message that is not whole `base_ms` milliseconds after its first chunk
    /// plus `1000 / min_rate` milliseconds for each of its bytes (a sender that moves at least `min_rate` bytes a second is
    /// never stale). A message is ordered behind the ones before it, so a stale one ends the connection ([`read`](Self::read) and
    /// [`sweep`](Self::sweep) fail) and what it held is released.
    #[must_use]
    pub fn with_max_age(mut self, base_ms: u32, min_rate: usize) -> ChunkReceiver {
        self.age = Some((base_ms, min_rate.max(1)));
        self
    }

    /// Sets the receiver's clock (milliseconds; only differences matter) for the next reads.
    pub fn set_now(&mut self, now: u32) {
        self.now = now;
    }

    /// Whether the receiver is evicted or ended by a stale message: it holds nothing and reads nothing more.
    pub fn is_dead(&self) -> bool {
        self.dead
            || self
                .shared
                .as_ref()
                .is_some_and(|s| s.is_evicted(self.holder))
    }

    /// Looks for an eviction and for stale messages at `now`, and releases what a dead receiver held. An error is the reason the
    /// connection is to be dropped. A host calls this every tick, so a connection that goes quiet does not keep its bytes.
    pub fn sweep(&mut self, now: u32) -> Result<(), NetError> {
        self.now = now;
        self.check_alive()
    }

    fn check_alive(&mut self) -> Result<(), NetError> {
        if !self.dead
            && self
                .shared
                .as_ref()
                .is_some_and(|s| s.is_evicted(self.holder))
        {
            self.release();
            return Err(NetError::Invalid(
                "evicted to make room for other connections",
            ));
        }
        if self.dead {
            return Err(NetError::Invalid("a connection that was ended"));
        }
        if let Some((base, rate)) = self.age {
            let now = self.now;
            let stale = self
                .chans
                .iter()
                .flat_map(|c| c.window.iter().flatten())
                .any(|p| {
                    let allowed =
                        u64::from(base) + (p.total as u64).saturating_mul(1_000) / rate as u64;
                    u64::from(now.wrapping_sub(p.started)) > allowed
                });
            if stale {
                self.release();
                return Err(NetError::Invalid("a message was not completed in time"));
            }
        }
        Ok(())
    }

    /// Drops everything held and ends the receiver.
    fn release(&mut self) {
        self.dead = true;
        for c in &mut self.chans {
            c.window.clear();
        }
        self.sync_shared();
    }

    /// The same receiver refusing a message above `bytes` (on top of each channel's own cap), before anything of it is held.
    #[must_use]
    pub fn with_message_cap(mut self, bytes: usize) -> ChunkReceiver {
        self.message_cap = bytes;
        self
    }

    /// The same receiver refusing a chunk that would start a message when `count` are already half received (across all
    /// channels).
    #[must_use]
    pub fn with_max_partial(mut self, count: usize) -> ChunkReceiver {
        self.max_partial = count;
        self
    }

    /// The chunk bytes read so far, those that arrived twice included.
    pub fn received_bytes(&self) -> u64 {
        self.received
    }

    /// The same receiver holding at most `bytes` for half-received messages across all its channels
    /// ([`default_hold_cap`] otherwise). A chunk that would take it past that is refused. A channel whose `max_message` is
    /// above it can never deliver a whole message.
    #[must_use]
    pub fn with_hold_cap(mut self, bytes: usize) -> ChunkReceiver {
        self.hold_cap = bytes;
        self
    }

    /// The bytes held for half-received messages (what is allocated for them, whole messages at a time).
    pub fn held_bytes(&self) -> usize {
        if self.is_dead() {
            return 0;
        }
        self.buffered()
    }

    fn buffered(&self) -> usize {
        self.chans
            .iter()
            .flat_map(|c| c.window.iter().flatten())
            .map(|p| p.buf.capacity())
            .sum()
    }

    /// Reads a packet's section and appends the messages it completed to `out`, in order. On an error what was completed
    /// before it is in `out` and the connection should be dropped.
    pub fn read(
        &mut self,
        r: &mut BitReader<'_>,
        out: &mut Vec<Delivered>,
    ) -> Result<(), NetError> {
        self.check_alive()?;
        let result = self.read_section(r, out);
        self.sync_shared();
        result
    }

    /// What the receiver holds is told to the shared budget, if there is one.
    fn sync_shared(&mut self) {
        if let Some(shared) = &self.shared {
            shared.set_held(self.holder, self.buffered());
        }
    }

    fn partial_count(&self) -> usize {
        self.chans
            .iter()
            .map(|c| c.window.iter().flatten().count())
            .sum()
    }

    fn read_section(
        &mut self,
        r: &mut BitReader<'_>,
        out: &mut Vec<Delivered>,
    ) -> Result<(), NetError> {
        if !r.read_flag()? {
            return Ok(());
        }
        for _ in 0..MAX_CHUNKS_PER_PACKET {
            self.read_chunk(r, out)?;
            if !r.read_flag()? {
                return Ok(());
            }
        }
        Err(NetError::Cap {
            what: "chunks in a packet",
            count: MAX_CHUNKS_PER_PACKET as u64 + 1,
            cap: MAX_CHUNKS_PER_PACKET as u64,
        })
    }

    fn read_chunk(
        &mut self,
        r: &mut BitReader<'_>,
        out: &mut Vec<Delivered>,
    ) -> Result<(), NetError> {
        let channel = r.read_bits(self.id_bits)? as usize;
        let Some(limits) = self.limits.get(channel).copied() else {
            return Err(NetError::Invalid("channel id"));
        };
        let kind = if r.read_flag()? {
            Kind::Delta
        } else {
            Kind::Snapshot
        };
        let seq = r.read_bits(16)? as u16;
        let size_bits = limits.size_bits();
        let total = r.read_bits(size_bits)? as usize;
        let offset = r.read_bits(size_bits)? as usize;
        let len = r.read_bits(CHUNK_LEN_BITS)? as usize;
        let message_cap = limits.max_message.min(self.message_cap);
        if total == 0 || total > message_cap {
            return Err(NetError::Cap {
                what: "channel message length",
                count: total as u64,
                cap: message_cap as u64,
            });
        }
        if len == 0 || offset + len > total {
            return Err(NetError::Invalid("chunk range"));
        }
        let bytes = r.read_bytes(len)?;
        self.received = self.received.saturating_add(len as u64);
        let next = self.chans[channel].next;
        let distance = seq.wrapping_sub(next);
        if distance >= 0x8000 {
            // A message already delivered: a chunk sent again after its ack was taken for lost.
            return Ok(());
        }
        let distance = usize::from(distance);
        if distance >= WINDOW {
            return Err(NetError::Cap {
                what: "message sequence window",
                count: distance as u64,
                cap: WINDOW as u64 - 1,
            });
        }
        // A message's buffer is allocated whole when its first chunk comes, and charged as such: what the receiver holds stays
        // under its cap, and what a sender claims is what it pays for.
        let existing = self.chans[channel]
            .window
            .get(distance)
            .and_then(Option::as_ref);
        if let Some(p) = existing
            && (p.kind != kind || p.total != total)
        {
            return Err(NetError::Invalid("chunk of another message"));
        }
        if existing.is_none() && self.partial_count() >= self.max_partial {
            return Err(NetError::Cap {
                what: "half-received messages",
                count: self.partial_count() as u64 + 1,
                cap: self.max_partial as u64,
            });
        }
        let held = self
            .buffered()
            .saturating_add(if existing.is_none() { total } else { 0 });
        if held > self.hold_cap {
            return Err(NetError::Cap {
                what: "bytes held for half-received messages",
                count: held as u64,
                cap: self.hold_cap as u64,
            });
        }
        if let Some(shared) = &self.shared
            && !shared.reserve(self.holder, held)
        {
            // Every other holder holds less than this one would: it is the heaviest.
            return Err(NetError::Cap {
                what: "bytes held for half-received messages across connections",
                count: held as u64,
                cap: shared.cap as u64,
            });
        }
        let chan = &mut self.chans[channel];
        while chan.window.len() <= distance {
            chan.window.push_back(None);
        }
        let now = self.now;
        let partial = chan.window[distance].get_or_insert_with(|| Partial {
            kind,
            total,
            started: now,
            buf: Vec::with_capacity(total),
            ranges: Vec::new(),
        });
        partial.insert(offset, &bytes)?;
        while chan
            .window
            .front()
            .is_some_and(|p| p.as_ref().is_some_and(Partial::complete))
        {
            if let Some(Some(p)) = chan.window.pop_front() {
                out.push(Delivered {
                    channel,
                    kind: p.kind,
                    data: p.buf,
                });
                chan.next = chan.next.wrapping_add(1);
            }
        }
        Ok(())
    }
}

impl Drop for ChunkReceiver {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.leave(self.holder);
        }
    }
}

#[cfg(test)]
#[path = "chunk_tests.rs"]
mod tests;
