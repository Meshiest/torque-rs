//! The reliability under the lanes: the packet window (sequence, ack and mask, which packets arrived and which were lost),
//! reliable messages cut into fragments that are resent by range when their packet is lost, and the unreliable lane.
//!
//! A fragment is resent by the byte range it covered, not as the same fragment, so a resend after the link's datagram size
//! shrank is cut to the new size. The receiver keeps a bit per byte of each message it is assembling, so a duplicate or an
//! overlapping fragment is harmless and the held bytes are bounded by the lane's cap before anything is allocated.

use std::collections::VecDeque;
use std::ops::Range;

use super::framing::{FRAGMENT_HEADER, Fragment, Header, Lane, UNRELIABLE_HEADER, Unreliable};
use crate::NetError;

/// Reliable messages of one lane in flight at once (sent and not all acknowledged); the receiver refuses ids further ahead.
pub const MESSAGE_WINDOW: u16 = 64;

/// Packets in flight at once: the oldest is counted lost when a newer one would pass it by this many.
pub const PACKET_WINDOW: u16 = 64;

/// Packets the ack mask covers.
pub const ACK_BITS: u16 = 32;

/// A packet is lost once a packet sent this many after it has been acknowledged (QUIC's packet threshold).
pub const REORDER_THRESHOLD: u16 = 3;

/// Messages a reliable lane queues at most, whatever their bytes (empty messages count too).
pub const MAX_QUEUED_MESSAGES: usize = 4_096;

/// Why a message was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// Larger than the lane takes.
    TooLarge,
    /// The lane's send queue is full; nothing was queued.
    Busy,
    /// The session is closed.
    Closed,
}

/// `a` is after `b` in sequence space.
pub(crate) fn newer(a: u16, b: u16) -> bool {
    let d = a.wrapping_sub(b);
    d != 0 && d < 0x8000
}

/// The byte range of one message one packet carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FragRef {
    pub(crate) id: u16,
    pub(crate) range: Range<u32>,
}

/// The one range `0..len`.
#[expect(
    clippy::single_range_in_vec_init,
    reason = "a list of unsent ranges that starts as one range"
)]
fn whole(len: u32) -> Vec<Range<u32>> {
    vec![0..len]
}

struct OutMessage {
    data: Vec<u8>,
    /// Ranges not yet sent (or sent and lost), sorted and disjoint. An empty message has the one range `0..0` until it is sent.
    unsent: Vec<Range<u32>>,
    in_flight: u32,
    acked: u32,
}

impl OutMessage {
    fn done(&self) -> bool {
        self.unsent.is_empty() && self.in_flight == 0 && self.acked as usize == self.data.len()
    }

    /// Puts `range` back among the unsent ones, merged in place with the neighbors it touches.
    fn give_back(&mut self, range: &Range<u32>) {
        self.in_flight -= 1;
        if range.is_empty() {
            if self.data.is_empty() && self.unsent.is_empty() {
                self.unsent.push(0..0);
            }
            return;
        }
        let mut at = self.unsent.partition_point(|r| r.start < range.start);
        self.unsent.insert(at, range.clone());
        if at > 0 && self.unsent[at - 1].end >= self.unsent[at].start {
            let r = self.unsent.remove(at);
            at -= 1;
            self.unsent[at].end = self.unsent[at].end.max(r.end);
        }
        while at + 1 < self.unsent.len() && self.unsent[at].end >= self.unsent[at + 1].start {
            let r = self.unsent.remove(at + 1);
            self.unsent[at].end = self.unsent[at].end.max(r.end);
        }
    }
}

/// The sending half of a reliable lane.
pub(crate) struct ReliableSend {
    front: u16,
    queue: VecDeque<OutMessage>,
    unacked: usize,
    cap: usize,
    max_message: usize,
}

impl ReliableSend {
    pub(crate) fn new(cap: usize, max_message: usize) -> ReliableSend {
        ReliableSend {
            front: 0,
            queue: VecDeque::new(),
            unacked: 0,
            cap,
            max_message: max_message.min(u32::MAX as usize),
        }
    }

    pub(crate) fn push(&mut self, data: Vec<u8>) -> Result<(), SendError> {
        if data.len() > self.max_message {
            return Err(SendError::TooLarge);
        }
        if self.unacked + data.len() > self.cap || self.queue.len() >= MAX_QUEUED_MESSAGES {
            return Err(SendError::Busy);
        }
        self.unacked += data.len();
        // `max_message` is at most `u32::MAX`.
        let len = data.len() as u32;
        self.queue.push_back(OutMessage {
            data,
            unsent: whole(len),
            in_flight: 0,
            acked: 0,
        });
        Ok(())
    }

    /// Bytes queued that the peer has not acknowledged.
    pub(crate) fn unacked(&self) -> usize {
        self.unacked
    }

    /// Writes fragments into `out` while they fit in `room` bytes, oldest message and lowest range first, within the window.
    pub(crate) fn write(
        &mut self,
        lane: Lane,
        room: usize,
        out: &mut Vec<u8>,
        sent: &mut Vec<(Lane, FragRef)>,
    ) {
        let mut left = room;
        for (i, m) in self
            .queue
            .iter_mut()
            .enumerate()
            .take(usize::from(MESSAGE_WINDOW))
        {
            // `i` is below the window, and a queued message's length fits a `u32` (`push`).
            let id = self.front.wrapping_add(i as u16);
            let total = m.data.len() as u32;
            while let Some(r) = m.unsent.first().cloned() {
                if left < FRAGMENT_HEADER || (left == FRAGMENT_HEADER && !r.is_empty()) {
                    return;
                }
                let fits = u32::from(u16::try_from(left - FRAGMENT_HEADER).unwrap_or(u16::MAX));
                let end = r.start + (r.end - r.start).min(fits);
                Fragment {
                    id,
                    total,
                    offset: r.start,
                    bytes: &m.data[r.start as usize..end as usize],
                }
                .write(out)
                .expect("a fragment is cut to a u16 length");
                left -= FRAGMENT_HEADER + (end - r.start) as usize;
                if end == r.end {
                    m.unsent.remove(0);
                } else {
                    m.unsent[0].start = end;
                }
                m.in_flight += 1;
                sent.push((
                    lane,
                    FragRef {
                        id,
                        range: r.start..end,
                    },
                ));
            }
        }
    }

    fn message(&mut self, id: u16) -> Option<&mut OutMessage> {
        let at = usize::from(id.wrapping_sub(self.front));
        self.queue.get_mut(at)
    }

    pub(crate) fn acked(&mut self, frag: &FragRef) {
        let Some(m) = self.message(frag.id) else {
            return;
        };
        let len = frag.range.end - frag.range.start;
        m.in_flight -= 1;
        m.acked += len;
        self.unacked -= len as usize;
        while self.queue.front().is_some_and(OutMessage::done) {
            self.queue.pop_front();
            self.front = self.front.wrapping_add(1);
        }
    }

    /// The packet that carried `frag` was lost, or never sent: its range goes back to be sent again.
    pub(crate) fn lost(&mut self, frag: &FragRef) {
        if let Some(m) = self.message(frag.id) {
            m.give_back(&frag.range);
        }
    }
}

struct InMessage {
    data: Vec<u8>,
    got: Vec<u64>,
    have: usize,
}

/// The receiving half of a reliable lane: messages are handed on whole and in order.
pub(crate) struct ReliableRecv {
    next: u16,
    slots: VecDeque<Option<InMessage>>,
    held: usize,
    cap: usize,
    max_message: usize,
}

impl ReliableRecv {
    pub(crate) fn new(cap: usize, max_message: usize) -> ReliableRecv {
        ReliableRecv {
            next: 0,
            slots: VecDeque::new(),
            held: 0,
            cap,
            max_message,
        }
    }

    /// Takes one fragment.
    ///
    /// # Errors
    /// The message is past the window, larger than the lane takes, would hold more than the lane's cap, or its total differs
    /// from an earlier fragment's.
    pub(crate) fn fragment(&mut self, f: &Fragment<'_>) -> Result<(), NetError> {
        let ahead = f.id.wrapping_sub(self.next);
        if ahead >= 0x8000 {
            return Ok(());
        }
        if ahead >= MESSAGE_WINDOW {
            return Err(NetError::Invalid("reliable message past the window"));
        }
        let total = f.total as usize;
        if total > self.max_message {
            return Err(NetError::Cap {
                what: "reliable message bytes",
                count: total as u64,
                cap: self.max_message as u64,
            });
        }
        let at = usize::from(ahead);
        if self.slots.len() <= at {
            self.slots.resize_with(at + 1, || None);
        }
        let slot = &mut self.slots[at];
        let m = match slot {
            Some(m) if m.data.len() != total => {
                return Err(NetError::Invalid("reliable message total"));
            }
            Some(m) => m,
            None => {
                if self.held + total > self.cap {
                    return Err(NetError::Cap {
                        what: "reliable bytes held",
                        count: (self.held + total) as u64,
                        cap: self.cap as u64,
                    });
                }
                self.held += total;
                slot.insert(InMessage {
                    data: vec![0; total],
                    got: vec![0; total.div_ceil(64)],
                    have: 0,
                })
            }
        };
        let start = f.offset as usize;
        m.data[start..start + f.bytes.len()].copy_from_slice(f.bytes);
        for i in start..start + f.bytes.len() {
            let (word, bit) = (i / 64, 1u64 << (i % 64));
            if m.got[word] & bit == 0 {
                m.got[word] |= bit;
                m.have += 1;
            }
        }
        Ok(())
    }

    /// The next whole message in order, if it has arrived.
    pub(crate) fn pop(&mut self) -> Option<Vec<u8>> {
        let Some(Some(m)) = self.slots.front() else {
            return None;
        };
        if m.have != m.data.len() {
            return None;
        }
        let m = self.slots.pop_front().flatten()?;
        self.held -= m.data.len();
        self.next = self.next.wrapping_add(1);
        Some(m.data)
    }
}

/// The sending half of the unreliable lane.
pub(crate) struct UnreliableSend {
    next: u16,
    queue: VecDeque<(u16, Vec<u8>)>,
    bytes: usize,
    cap: usize,
    max_message: usize,
    dropped: u64,
}

impl UnreliableSend {
    pub(crate) fn new(cap: usize, max_message: usize) -> UnreliableSend {
        UnreliableSend {
            next: 0,
            queue: VecDeque::new(),
            bytes: 0,
            cap,
            max_message: max_message.min(usize::from(u16::MAX)),
            dropped: 0,
        }
    }

    pub(crate) fn push(&mut self, data: Vec<u8>) -> Result<(), SendError> {
        if data.len() > self.max_message {
            return Err(SendError::TooLarge);
        }
        if self.bytes + data.len() > self.cap {
            return Err(SendError::Busy);
        }
        self.bytes += data.len();
        self.queue.push_back((self.next, data));
        self.next = self.next.wrapping_add(1);
        Ok(())
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Writes queued messages while they fit in `room`. A message that would not fit even in `packet_room` (all of a packet's
    /// payload, after the datagram size shrank) is dropped and counted; one that only does not fit this time waits.
    pub(crate) fn write(
        &mut self,
        room: usize,
        packet_room: usize,
        out: &mut Vec<u8>,
        taken: &mut Vec<(u16, Vec<u8>)>,
    ) {
        let mut left = room;
        while let Some((_, data)) = self.queue.front() {
            let need = UNRELIABLE_HEADER + data.len();
            if need > packet_room {
                let (_, data) = self.queue.pop_front().expect("the front was just seen");
                self.bytes -= data.len();
                self.dropped += 1;
                continue;
            }
            if need > left {
                return;
            }
            let (seq, data) = self.queue.pop_front().expect("the front was just seen");
            Unreliable { seq, bytes: &data }
                .write(out)
                .expect("an unreliable message is at most a u16 length (`new`)");
            left -= need;
            self.bytes -= data.len();
            taken.push((seq, data));
        }
    }

    /// Messages written into a packet the link refused go back to the front, in order.
    pub(crate) fn give_back(&mut self, taken: Vec<(u16, Vec<u8>)>) {
        for (seq, data) in taken.into_iter().rev() {
            self.bytes += data.len();
            self.queue.push_front((seq, data));
        }
    }
}

/// The receiving half of the unreliable lane: a message older than one already handed on is dropped.
#[derive(Default)]
pub(crate) struct UnreliableRecv {
    last: Option<u16>,
}

impl UnreliableRecv {
    pub(crate) fn accept(&mut self, seq: u16) -> bool {
        if self.last.is_some_and(|last| !newer(seq, last)) {
            return false;
        }
        self.last = Some(seq);
        true
    }
}

/// Which of the peer's packets arrived: the newest and a mask of the 32 before and including it.
#[derive(Default)]
pub(crate) struct Received {
    latest: Option<u16>,
    mask: u32,
}

impl Received {
    /// The packet was not seen before and is recent enough to be acknowledged.
    pub(crate) fn is_new(&self, seq: u16) -> bool {
        let Some(latest) = self.latest else {
            return true;
        };
        if newer(seq, latest) {
            return true;
        }
        let back = latest.wrapping_sub(seq);
        back < ACK_BITS && self.mask & (1 << back) == 0
    }

    pub(crate) fn note(&mut self, seq: u16) {
        let Some(latest) = self.latest else {
            self.latest = Some(seq);
            self.mask = 1;
            return;
        };
        if newer(seq, latest) {
            let ahead = u32::from(seq.wrapping_sub(latest));
            self.mask = self.mask.checked_shl(ahead).unwrap_or(0) | 1;
            self.latest = Some(seq);
        } else {
            self.mask |= 1 << latest.wrapping_sub(seq);
        }
    }

    pub(crate) fn header(&self, seq: u16) -> Header {
        Header {
            seq,
            ack: self.latest.unwrap_or(0),
            mask: self.mask,
        }
    }
}

/// One packet sent and not yet known to have arrived or been lost.
pub(crate) struct Record {
    pub(crate) seq: u16,
    pub(crate) sent_ms: u32,
    pub(crate) frags: Vec<(Lane, FragRef)>,
    /// A stream lane wrote into it, so the host hears whether it arrived.
    pub(crate) streams: bool,
}

/// The packets in flight.
#[derive(Default)]
pub(crate) struct Flight {
    pub(crate) next_seq: u16,
    records: VecDeque<Record>,
    /// The records an ack leaves in flight, swapped with `records` so an ack allocates nothing once warm.
    spare: VecDeque<Record>,
}

impl Flight {
    pub(crate) fn push(&mut self, record: Record) {
        self.next_seq = self.next_seq.wrapping_add(1);
        self.records.push_back(record);
    }

    /// Resolves what the peer's ack says, oldest first, into `out` (`true`: arrived). Returns the round trip of the newest
    /// packet it acknowledges. An ack of a packet never sent, or an empty mask, says nothing.
    pub(crate) fn on_ack(
        &mut self,
        ack: u16,
        mask: u32,
        now: u32,
        out: &mut Vec<(Record, bool)>,
    ) -> Option<u32> {
        if mask == 0 || self.next_seq.wrapping_sub(1).wrapping_sub(ack) >= 0x8000 {
            return None;
        }
        // The mask is not zero, so at most 31 trailing zeros.
        let newest = ack.wrapping_sub(mask.trailing_zeros() as u16);
        let mut rtt = None;
        std::mem::swap(&mut self.records, &mut self.spare);
        for r in self.spare.drain(..) {
            let back = ack.wrapping_sub(r.seq);
            let verdict = if newer(r.seq, ack) {
                None
            } else if back < ACK_BITS && mask & (1 << back) != 0 {
                if r.seq == newest {
                    rtt = Some(now.wrapping_sub(r.sent_ms));
                }
                Some(true)
            } else if back >= ACK_BITS
                || newer(newest, r.seq) && newest.wrapping_sub(r.seq) >= REORDER_THRESHOLD
            {
                Some(false)
            } else {
                None
            };
            match verdict {
                Some(arrived) => out.push((r, arrived)),
                None => self.records.push_back(r),
            }
        }
        rtt
    }

    /// Counts lost every packet older than `timeout`. Records are kept in the order they were sent, so the old ones are at
    /// the front.
    pub(crate) fn expire(&mut self, now: u32, timeout: u32, out: &mut Vec<(Record, bool)>) {
        while let Some(r) = self.records.front()
            && now.wrapping_sub(r.sent_ms) >= timeout
        {
            out.push((
                self.records.pop_front().expect("the front was just seen"),
                false,
            ));
        }
    }

    /// Counts lost the oldest packets the next one would pass by the window.
    pub(crate) fn make_room(&mut self, out: &mut Vec<(Record, bool)>) {
        while let Some(r) = self.records.front()
            && self.next_seq.wrapping_sub(r.seq) >= PACKET_WINDOW
        {
            out.push((
                self.records.pop_front().expect("the front was just seen"),
                false,
            ));
        }
    }
}
