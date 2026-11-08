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
//! **Hostile input.** [`ChunkReceiver::read`] takes bytes from a peer: every length is checked against the channel's limits and
//! the bits left before anything is allocated, a message is held only inside a small window of sequence numbers, and what a
//! channel holds is bounded by `max_message * window`. A violation is a [`NetError`] and the connection is dropped with the
//! packet error the host uses for any malformed packet; nothing here panics.
//!
//! **Limits on the sending side.** [`ChunkSender::enqueue`] refuses a message above the size cap and one that would take the
//! connection's backlog (the bytes queued and not yet acknowledged) above its cap, with a [`SendError`] and nothing queued.

use std::collections::VecDeque;

use crate::NetError;
use crate::bits::{BitReader, BitWriter, bit_length};
use crate::cap::NET_EVENT_PACKETS_IN_FLIGHT;

pub mod patch;
pub mod registry;

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
/// The least the default for the bytes a receiver holds for half-received messages ([`default_hold_cap`]) is.
pub const DEFAULT_HOLD_CAP: usize = 8 << 20;
/// The most that default is.
pub const MAX_HOLD_CAP: usize = 64 << 20;

/// What a receiver of channels with these `limits` holds for half-received messages, unless it is told another cap: a window of
/// messages of the largest size ([`WINDOW`] times it), between [`DEFAULT_HOLD_CAP`] and [`MAX_HOLD_CAP`]. A sender of the same
/// channels queues at most this much across them (its total backlog), so what an honest sender leaves half-received, which is
/// never more than it has queued, is never more than the receiver holds.
pub fn default_hold_cap(limits: &[ChannelLimits]) -> usize {
    let largest = limits.iter().map(|l| l.max_message).max().unwrap_or(0);
    largest
        .saturating_mul(WINDOW)
        .clamp(DEFAULT_HOLD_CAP, MAX_HOLD_CAP)
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
}

impl ChannelLimits {
    /// A message of up to 4 MiB, a backlog of up to 8 MiB, and `budget` bytes of each packet.
    pub fn new(budget: usize) -> ChannelLimits {
        ChannelLimits {
            budget,
            max_message: 4 << 20,
            max_backlog: 8 << 20,
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
    kind: Kind,
    data: Vec<u8>,
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
            total_cap: default_hold_cap(&limits),
            limits,
        }
    }

    /// The same sender with at most `bytes` queued across all its channels ([`default_hold_cap`] otherwise; keep it at or below what the receiver holds).
    #[must_use]
    pub fn with_total_backlog(mut self, bytes: usize) -> ChunkSender {
        self.total_cap = bytes;
        self
    }

    /// The bytes queued across all channels and not yet acknowledged.
    pub fn total_backlog(&self) -> usize {
        self.chans.iter().map(|c| c.queued_bytes).sum()
    }

    /// How many channels there are.
    pub fn channels(&self) -> usize {
        self.limits.len()
    }

    /// Queues `data` on `channel`. Nothing is queued on an error.
    pub fn enqueue(&mut self, channel: usize, kind: Kind, data: Vec<u8>) -> Result<(), SendError> {
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
        if over_channel || self.total_backlog().saturating_add(data.len()) > self.total_cap {
            return Err(SendError::Backlog);
        }
        let chan = &mut self.chans[channel];
        let seq = chan.next_seq;
        chan.next_seq = chan.next_seq.wrapping_add(1);
        chan.queued_bytes += data.len();
        chan.queue.push_back(Msg {
            seq,
            kind,
            data,
            fresh: 0,
            retry: VecDeque::new(),
            acked: 0,
        });
        Ok(())
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
        for k in 0..n {
            let c = (self.rotate + k) % n;
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
}

impl ChunkReceiver {
    /// A receiver for channels with these `limits` (the same, in the same order, as the sender's).
    pub fn new(limits: &[ChannelLimits]) -> ChunkReceiver {
        let limits: Vec<ChannelLimits> = limits.iter().copied().take(MAX_CHANNELS).collect();
        ChunkReceiver {
            id_bits: id_bits(limits.len()),
            chans: limits.iter().map(|_| RecvChan::default()).collect(),
            hold_cap: default_hold_cap(&limits),
            limits,
        }
    }

    /// The same receiver holding at most `bytes` for half-received messages across all its channels
    /// ([`default_hold_cap`] otherwise). A chunk that would take it past that is refused. A channel whose `max_message` is
    /// above it can never deliver a whole message.
    #[must_use]
    pub fn with_hold_cap(mut self, bytes: usize) -> ChunkReceiver {
        self.hold_cap = bytes;
        self
    }

    /// The bytes held for half-received messages.
    pub fn held_bytes(&self) -> usize {
        self.chans
            .iter()
            .flat_map(|c| c.window.iter().flatten())
            .map(|p| p.buf.len())
            .sum()
    }

    /// Reads a packet's section and appends the messages it completed to `out`, in order. On an error what was completed
    /// before it is in `out` and the connection should be dropped.
    pub fn read(
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
        if total == 0 || total > limits.max_message {
            return Err(NetError::Cap {
                what: "channel message length",
                count: total as u64,
                cap: limits.max_message as u64,
            });
        }
        if len == 0 || offset + len > total {
            return Err(NetError::Invalid("chunk range"));
        }
        let bytes = r.read_bytes(len)?;
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
        // A chunk far into a message makes its buffer that long at once: what the receiver holds stays under its cap.
        let existing = self.chans[channel]
            .window
            .get(distance)
            .and_then(Option::as_ref);
        if let Some(p) = existing
            && (p.kind != kind || p.total != total)
        {
            return Err(NetError::Invalid("chunk of another message"));
        }
        let have = existing.map_or(0, |p| p.buf.len());
        let held = self
            .held_bytes()
            .saturating_add((offset + len).saturating_sub(have));
        if held > self.hold_cap {
            return Err(NetError::Cap {
                what: "bytes held for half-received messages",
                count: held as u64,
                cap: self.hold_cap as u64,
            });
        }
        let chan = &mut self.chans[channel];
        while chan.window.len() <= distance {
            chan.window.push_back(None);
        }
        let partial = chan.window[distance].get_or_insert_with(|| Partial {
            kind,
            total,
            buf: Vec::new(),
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

#[cfg(test)]
#[path = "chunk_tests.rs"]
mod tests;
