//! The simulation's event queue (`Sim::postEvent`, `gEventSequence`): the scheduled calls of `schedule`, and
//! the runtime's own delete events. Ordered by time, first in first out among equal times; every event takes
//! the next sequence number, which scripts see as the schedule id. The clock is virtual: the host moves it
//! with `Vm::advance_time`.
//!
//! Caps. TGE has none, so these are this crate's own ([`EventLimits`]): at most [`MAX_PENDING_EVENTS`] pending
//! events by default and [`MAX_PENDING_BYTES`] of arguments (`post` then refuses and `schedule` returns 0 with
//! a warning), and the number of events one `Vm::advance_time` fires, per call and per event time.
//!
//! Follows Torque3D's MIT-licensed `console/simManager.cpp` (Copyright (c) 2012 GarageGames, LLC) and
//! `console/simEvents.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::sim::ObjId;

/// The default for [`EventLimits::pending`]: about 300 bytes an event (plus the arguments, which
/// [`MAX_PENDING_BYTES`] bounds on top: up to 64 MiB more), so a script that schedules without end stops at
/// some 300 MiB (and at most 64 MiB of arguments) rather than at the host's memory.
///
/// A flood that fills the queue (a hostile script, or a bug) has a visible effect: past the cap every
/// `schedule` returns 0 with a warning, so script sites that keep the id for `cancel` or
/// `isEventPending` hold 0 and their heartbeats stop until events fire and room is made. TGE
/// would grow until it ran out of memory instead.
pub const MAX_PENDING_EVENTS: usize = 1 << 20;

/// The default for [`EventLimits::pending_bytes`]: the argument bytes the pending events hold together. The
/// events' own count does not bound memory (every call may carry 19 strings of up to 64 MiB), and a
/// script pays only 1 step per 64 bytes it queues, so the bytes have a cap of their own: one maximal
/// string's worth, far above what ordinary scripts queue (a few hundred bytes).
pub const MAX_PENDING_BYTES: usize = 64 << 20;

/// What an event does when it fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// A `schedule`: call `argv[0]` with the rest, as a method of the event's object when `on_object`,
    /// else as a global function (the object then only bounds the event's life).
    Call { argv: Vec<Vec<u8>>, on_object: bool },
    /// TGE's `DeleteThisEvent`, posted by `deleteObject` (it frees the object's memory a tick
    /// later). It does nothing here, but it takes a sequence number and counts as pending, as in TGE
    /// (deleting an object uses up an event id and one `getNumSchedules`).
    Delete,
    /// An event of the host's that does work the host knows (a C++ event class of a game's). The host's handler
    /// ([`Vm::set_native_event_handler`](crate::vm::Vm::set_native_event_handler)) gets `tag` and `arg` when it fires, as an
    /// outermost run of its own (its step budget, its charges), in due-time order with the scripts' calls. A script
    /// cannot cancel one (`cancel` says it was not created from script), it takes a sequence number and counts
    /// as pending like any other, and it is dropped when its object is unregistered.
    Native { tag: u32, arg: u32 },
}

impl EventKind {
    /// The bytes of arguments the event holds (what [`EventLimits::pending_bytes`] counts).
    pub fn bytes(&self) -> usize {
        match self {
            EventKind::Call { argv, .. } => argv.iter().map(Vec::len).sum(),
            EventKind::Delete | EventKind::Native { .. } => 0,
        }
    }

    /// Whether firing it can run script (a call, or a native event whose handler may make callbacks): the events a pass
    /// counts against its limits ([`EventLimits`]).
    pub fn runs_script(&self) -> bool {
        matches!(self, EventKind::Call { .. } | EventKind::Native { .. })
    }
}

/// A pending event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub seq: u32,
    /// When it fires, in milliseconds of simulation time (modulo 2^32, as TGE's `U32`).
    pub time: u32,
    /// The object it belongs to; it is dropped when that object is unregistered.
    pub dest: ObjId,
    pub kind: EventKind,
}

impl Event {
    /// Whether a script made it (`cancel` refuses the others).
    pub fn from_script(&self) -> bool {
        matches!(self.kind, EventKind::Call { .. })
    }
}

/// What a script can make the queue and the clock do. A host that slices advances by real time has
/// none here, so it counts: a script that schedules
/// itself again with a delay the clock reads as due (0, or a negative one once the clock is past 0)
/// would otherwise keep one call going for ever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventLimits {
    /// Events that can be pending at once; `schedule` returns 0 (with a warning) past it.
    pub pending: usize,
    /// Bytes of arguments the pending events hold together; `schedule` returns 0 (with a warning) past it.
    pub pending_bytes: usize,
    /// `Vm::advance_time`: events fired by one call, whatever their times.
    pub per_advance: usize,
    /// `Vm::advance_time`: events fired by one call that share one event time (a self-scheduling loop
    /// does).
    pub per_time: usize,
    /// `Vm::advance_time`: the steps (see `Vm::set_step_limit`) all the events of one call may use together;
    /// once the total reaches it no further event fires in that call. Every event is an outermost run with
    /// its own step budget, so the counts above alone allow `per_advance` times the step limit of work in
    /// one call. None (the default) is four times the step limit, and no bound when the host set no step
    /// limit (steps are not counted then). The event that crosses it has finished by then, so a call may
    /// use up to this much plus one event's budget.
    pub per_advance_steps: Option<u64>,
    /// `Vm::advance_time`: a time slice (as a game's `Sim::advanceToTime` may stop a pass after some real time),
    /// counted in steps instead of milliseconds. The events fire in batches of
    /// [`Limits::pass_batch`](crate::policy::Limits::pass_batch); after each batch, once the steps the call's events
    /// used together are MORE than this, the call stops quietly (not an error: the main loop goes on to the network),
    /// the remaining due events stay pending for the next call and the clock still moves to the target. None (the
    /// default, TGE's) is no slice: a call fires every due event; a server with a socket may set one. No slice when the
    /// host set no step limit (steps are not counted then).
    pub per_pass_steps: Option<u64>,
    /// `Vm::advance_time`: a time slice of the wall clock (for a host's bulk work): after each batch of
    /// [`Limits::pass_batch`](crate::policy::Limits::pass_batch), once the call has run this long, it stops as
    /// [`EventLimits::per_pass_steps`] stops it. None (the default) reads no clock. Unlike the steps it is not the same
    /// on every machine: a host sets it only for work whose pace no script depends on.
    pub per_pass_wall: Option<std::time::Duration>,
}

impl Default for EventLimits {
    /// 1M pending and 64 MiB of arguments; 1M and 100000 fired, and four step limits' worth of steps: far
    /// below a hang.
    fn default() -> Self {
        EventLimits {
            pending: MAX_PENDING_EVENTS,
            pending_bytes: MAX_PENDING_BYTES,
            per_advance: 1 << 20,
            per_time: 100_000,
            per_advance_steps: None,
            per_pass_steps: None,
            per_pass_wall: None,
        }
    }
}

/// The queue and the simulation clock.
#[derive(Debug, Clone)]
pub struct EventQueue {
    /// (time, posting order) -> event.
    pending: BTreeMap<(u32, u64), Event>,
    by_seq: HashMap<u32, (u32, u64)>,
    by_dest: HashMap<ObjId, HashSet<u32>>,
    next_seq: u32,
    next_order: u64,
    now: u32,
    max_pending: usize,
    max_bytes: usize,
    /// The argument bytes of the pending events.
    bytes: usize,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl EventQueue {
    /// An empty queue at time 0; the first sequence number is 1 (`Sim::init`).
    pub fn new() -> EventQueue {
        EventQueue {
            pending: BTreeMap::new(),
            by_seq: HashMap::new(),
            by_dest: HashMap::new(),
            next_seq: 1,
            next_order: 0,
            now: 0,
            max_pending: MAX_PENDING_EVENTS,
            max_bytes: MAX_PENDING_BYTES,
            bytes: 0,
        }
    }

    /// The simulation time (`Sim::getCurrentTime`).
    pub fn now(&self) -> u32 {
        self.now
    }

    pub(crate) fn set_now(&mut self, now: u32) {
        self.now = now;
    }

    /// Sets the most events, and the most bytes of arguments, that can be pending (events already pending
    /// stay).
    pub fn set_max_pending(&mut self, events: usize, bytes: usize) {
        self.max_pending = events;
        self.max_bytes = bytes;
    }

    /// The argument bytes the pending events hold.
    pub fn pending_bytes(&self) -> usize {
        self.bytes
    }

    /// Queues an event at `time` and returns its sequence number, or None when the most events that can be
    /// pending (default [`MAX_PENDING_EVENTS`]) are, or its arguments would take the pending bytes past
    /// their cap ([`MAX_PENDING_BYTES`]). Equal times keep posting order. The counter wraps past 2^32 like
    /// TGE's, and skips 0 (never an event) and the numbers still in use.
    pub fn post(&mut self, time: u32, dest: ObjId, kind: EventKind) -> Option<u32> {
        let size = kind.bytes();
        if self.pending.len() >= self.max_pending
            || self.bytes.saturating_add(size) > self.max_bytes
        {
            return None;
        }
        // Fewer than 2^32 numbers are in use (the cap is far below), so this finds a free one.
        let seq = loop {
            let seq = self.next_seq;
            self.next_seq = self.next_seq.wrapping_add(1);
            if seq != 0 && !self.by_seq.contains_key(&seq) {
                break seq;
            }
        };
        let key = (time, self.next_order);
        self.next_order = self.next_order.wrapping_add(1);
        self.pending.insert(
            key,
            Event {
                seq,
                time,
                dest,
                kind,
            },
        );
        self.by_seq.insert(seq, key);
        self.by_dest.entry(dest).or_default().insert(seq);
        self.bytes = self.bytes.saturating_add(size);
        Some(seq)
    }

    /// The pending event with this sequence number.
    pub fn get(&self, seq: u32) -> Option<&Event> {
        self.pending.get(self.by_seq.get(&seq)?)
    }

    /// Takes a pending event out of the queue.
    pub fn remove(&mut self, seq: u32) -> Option<Event> {
        let key = self.by_seq.remove(&seq)?;
        let event = self.pending.remove(&key)?;
        self.forget(&event);
        Some(event)
    }

    /// Takes an event that left `pending` and `by_seq` out of the other indexes.
    fn forget(&mut self, event: &Event) {
        self.bytes = self.bytes.saturating_sub(event.kind.bytes());
        self.forget_dest(event.dest, event.seq);
    }

    fn forget_dest(&mut self, dest: ObjId, seq: u32) {
        if let Some(seqs) = self.by_dest.get_mut(&dest) {
            seqs.remove(&seq);
            if seqs.is_empty() {
                self.by_dest.remove(&dest);
            }
        }
    }

    /// The number of pending events (`getNumSchedules`).
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Whether the earliest event can run script: a call, or a native event (not an engine delete event, which does
    /// nothing).
    pub fn next_runs_script(&self) -> bool {
        self.pending
            .first_key_value()
            .is_some_and(|(_, event)| event.kind.runs_script())
    }

    /// The time of the earliest event if it is due at `target` (not after it).
    pub fn next_due(&self, target: u32) -> Option<u32> {
        let (&(time, _), _) = self.pending.first_key_value()?;
        (time <= target).then_some(time)
    }

    /// The earliest event, if it is due at `target` (not after it).
    pub fn peek_due(&self, target: u32) -> Option<&Event> {
        let (&(time, _), event) = self.pending.first_key_value()?;
        (time <= target).then_some(event)
    }

    /// Every pending event in the order they would fire (by time, then posting order).
    pub fn iter(&self) -> impl Iterator<Item = &Event> {
        self.pending.values()
    }

    /// The sequence number the next posted event would get, before it skips 0 and numbers in use (`gEventSequence`).
    pub fn next_seq(&self) -> u32 {
        self.next_seq
    }

    /// Takes the earliest event due at `target` (time not after it), if any.
    pub fn take_due(&mut self, target: u32) -> Option<Event> {
        let (&key, _) = self.pending.first_key_value()?;
        if key.0 > target {
            return None;
        }
        let event = self.pending.remove(&key)?;
        self.by_seq.remove(&event.seq);
        self.forget(&event);
        Some(event)
    }

    /// Drops every pending event of an object (`SimObject::unregisterObject` cancels them). Returns how
    /// many there were.
    pub fn drop_dest(&mut self, dest: ObjId) -> usize {
        self.drop_dest_seqs(dest).len()
    }

    /// [`drop_dest`](Self::drop_dest), returning the sequence numbers of the events it dropped.
    pub fn drop_dest_seqs(&mut self, dest: ObjId) -> Vec<u32> {
        let Some(seqs) = self.by_dest.remove(&dest) else {
            return Vec::new();
        };
        let mut dropped = Vec::with_capacity(seqs.len());
        for seq in seqs {
            if let Some(event) = self
                .by_seq
                .remove(&seq)
                .and_then(|key| self.pending.remove(&key))
            {
                self.bytes = self.bytes.saturating_sub(event.kind.bytes());
            }
            dropped.push(seq);
        }
        dropped
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
