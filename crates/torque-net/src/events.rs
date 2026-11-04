//! The event layer: the ordered and unordered event queues of a connection, the section of a data packet that carries them, and the
//! receiver's wait list (`eventWritePacket`, `eventReadPacket`, `postNetEvent` and the ack and loss handlers).
//!
//! **Wire** (exactly): `{1, class, data}* 0 {1, (flag seq==prev+1 | 0 + 7-bit seq), class, data}* 0` - the unordered events, then the
//! ordered ones, each section closed by a 0 flag. `prev` starts at -2, so the first ordered event of every packet carries its seq.
//! The class id is `ids.bits()` wide (4 for Torque's 13 events).
//!
//! **Sender** ([`EventQueue`]). `post` puts an ordered event (guarantee 0) in the ordered queue under the next sequence number and
//! any other in the unordered queue (sequence -1). `write_section` is `eventWritePacket`: each phase writes head events while the
//! stream is not full (`isFull`: the position past the rate's packet size), and the queue's [`SectionRule`] may test each head event
//! before it goes (Torque's, [`TORQUE_SECTION`], tests nothing). The real stream takes 12,000 bits whatever the rate, so a packet may
//! pass the rate's size as `eventWritePacket`'s does. An event that is refused stays at the head and blocks its queue. The ordered
//! queue sends nothing past `lastAcked + 126`. An event the packet chain acks goes
//! to the sorted acked list and `lastAcked` advances over the run that starts at `lastAcked + 1`; a lost packet puts its ordered
//! events back in sequence order, a guaranteed unordered event at the front of the unordered queue, and settles an unguaranteed one
//! as not delivered. **Additions:** an event no packet could ever carry is an error (rather than a stall), the closing flags are kept clear
//! of the hard limit, and `hold_ordered` exists for the host.
//!
//! **Receiver** ([`EventReceiver`]). An unordered event runs at once; an ordered one is widened (`(nextRecv & !0x7f) | seq7`, +128 if
//! below `nextRecv`, so it lies in `[nextRecv, nextRecv + 127]`) and waits in a sorted list until `nextRecv` reaches it. An unbounded
//! list that keeps duplicates lets a duplicate of a processed sequence (widened +128) wedge the channel for good, so this one
//! **drops an entry whose widened sequence is already waiting or is more than 126 ahead of `nextRecv`** (silently: a retransmit after a
//! lost ack is normal) and refuses a list past [`NET_WAIT_LIST`] as a protocol error. A class id at or above the count, and an event
//! whose direction is wrong for the reader, are protocol errors.
//!
//! **Hostile input.** Nothing here indexes with a wire value; counts come from the packet; the queue's caps hand a refused event back.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netEvent.cpp` (Copyright (c) 2012
//! GarageGames, LLC) and `console/consoleObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic,
//! Inc.); the code here is this crate's own.

use std::collections::VecDeque;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{
    NET_EVENT_PACKETS_IN_FLIGHT, NET_EVENT_QUEUE_ORDERED, NET_EVENT_QUEUE_UNORDERED,
    NET_EVENT_WINDOW, NET_WAIT_LIST,
};
use crate::classes::{Direction, Kind, NetClasses};
use crate::huffman::Table;
use crate::netstring::{NetStringEvent, SendStrings};
use crate::protocol::Role;
use crate::remote::RemoteCommandEvent;

/// The work callback of a decoder: told how many units of work are about to be done (one unit is one byte of wire input, at least one
/// for an element that takes fewer), it answers whether the per-tick network work budget can pay. A `false` stops the decode.
pub type ChargeFn<'a> = dyn FnMut(u64) -> bool + 'a;

/// A budget that pays for everything (tests, callers with none).
pub fn free_charge() -> impl FnMut(u64) -> bool {
    |_| true
}

/// The bits of an ordered event's sequence on the wire.
pub const SEQ_BITS: u8 = 7;
const SEQ_MASK: u32 = 0x7f;
/// `prevSeq` before the first ordered event of a packet (-2).
const PREV_START: u32 = 0xffff_fffe;

/// A sequence number is after another by wrapping distance.
fn seq_after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

fn seq_before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// The event classes' ids, derived from an [`EventRegistry`] by the class-sorting rule, and the ids of the Torque events this crate
/// codes. A game's own classes are looked up by name ([`EventIds::id_of`]); a game's own command classes (bodies in
/// `RemoteCommandEvent`'s form) are named with [`EventIds::with_command_classes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventIds {
    classes: Vec<(String, Direction)>,
    bits: u8,
    /// The ids of a game's own command classes, in the order it named them ([`RemoteCommandEvent::variant`] `n` is the class at
    /// `n - 1`).
    commands: Vec<u32>,
    pub connection_message: u32,
    pub net_string: u32,
    pub remote_command: u32,
    pub sim_data_block: u32,
    pub ghost_always_object: u32,
    pub path_manager: u32,
    pub sim_2d_audio: u32,
    pub sim_3d_audio: u32,
    pub lightning_strike: u32,
}

/// Torque's event classes with their directions (TGE 1.4's `IMPLEMENT_CO_NETEVENT_V1` / `IMPLEMENT_CO_CLIENTEVENT_V1` set, the
/// test event of `game/net/netTest.cc` among them). The ids come from the sort rule, never typed.
pub const TORQUE_EVENTS: [(&str, Direction); 13] = [
    ("ConnectionMessageEvent", Direction::Any),
    ("FileChunkEvent", Direction::Any),
    ("FileDownloadRequestEvent", Direction::Any),
    ("GhostAlwaysObjectEvent", Direction::Any),
    ("LightningStrikeEvent", Direction::ServerToClient),
    ("NetStringEvent", Direction::Any),
    ("PathManagerEvent", Direction::Any),
    ("RemoteCommandEvent", Direction::Any),
    ("SetMissionCRCEvent", Direction::ServerToClient),
    ("Sim2DAudioEvent", Direction::ServerToClient),
    ("Sim3DAudioEvent", Direction::ServerToClient),
    ("SimDataBlockEvent", Direction::ServerToClient),
    ("SimpleMessageEvent", Direction::Any),
];

/// The event classes a connection knows: Torque's own ([`TORQUE_EVENTS`]) unless a game says otherwise, and a game's own beside
/// them. Both sides must hold the same set (an id is a position in the sorted list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRegistry {
    classes: Vec<(String, Direction)>,
    commands: Vec<String>,
}

impl Default for EventRegistry {
    fn default() -> EventRegistry {
        EventRegistry::torque()
    }
}

impl EventRegistry {
    /// Torque's event classes.
    pub fn torque() -> EventRegistry {
        let mut r = EventRegistry::empty();
        for (name, direction) in TORQUE_EVENTS {
            r.register(name, direction);
        }
        r
    }

    /// No class at all (a game that lists every class itself).
    pub fn empty() -> EventRegistry {
        EventRegistry {
            classes: Vec::new(),
            commands: Vec::new(),
        }
    }

    /// Adds a class; a name already registered is kept once, with the direction given last.
    pub fn register(&mut self, name: &str, direction: Direction) -> &mut EventRegistry {
        match self.classes.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = direction,
            None => self.classes.push((name.to_string(), direction)),
        }
        self
    }

    /// Adds a command class: a class whose body is `RemoteCommandEvent`'s, read and written as a [`NetEvent::Remote`] whose
    /// [`RemoteCommandEvent::variant`] is its place among the command classes, counting from 1 in the order they were added.
    pub fn register_command(&mut self, name: &str, direction: Direction) -> &mut EventRegistry {
        self.register(name, direction);
        if !self.commands.iter().any(|n| n == name) {
            self.commands.push(name.to_string());
        }
        self
    }

    /// Takes a class out (a Torque event a game does not have); whether it was there.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.classes.len();
        self.classes.retain(|(n, _)| n != name);
        self.commands.retain(|n| n != name);
        self.classes.len() != before
    }

    /// The classes in registration order.
    pub fn classes(&self) -> &[(String, Direction)] {
        &self.classes
    }

    /// The ids of the registered classes ([`EventIds::from_classes`] over their derived table): an error when a name is empty or
    /// when a Torque event this crate codes is missing.
    pub fn ids(&self) -> Result<EventIds, NetError> {
        let reps: Vec<crate::classes::NetRep> = self
            .classes
            .iter()
            .map(|(name, direction)| crate::classes::NetRep {
                name: name.clone(),
                kind: Kind::Event,
                direction: *direction,
            })
            .collect();
        let classes =
            NetClasses::derive(&reps).map_err(|_| NetError::Invalid("event class names"))?;
        let commands: Vec<&str> = self.commands.iter().map(String::as_str).collect();
        EventIds::from_classes(&classes)?.with_command_classes(&commands)
    }
}

impl EventIds {
    /// Takes the event classes of a derived table ([`NetClasses`]); nothing is typed. The Torque events this crate codes must be
    /// among them.
    pub fn from_classes(classes: &NetClasses) -> Result<EventIds, NetError> {
        let events = classes.classes(Kind::Event);
        let id = |name: &str| {
            classes
                .id_of(Kind::Event, name)
                .ok_or(NetError::Invalid("event class missing from the table"))
        };
        Ok(EventIds {
            classes: events
                .iter()
                .map(|c| (c.name.clone(), c.direction))
                .collect(),
            bits: classes.id_bits(Kind::Event),
            commands: Vec::new(),
            connection_message: id("ConnectionMessageEvent")?,
            net_string: id("NetStringEvent")?,
            remote_command: id("RemoteCommandEvent")?,
            sim_data_block: id("SimDataBlockEvent")?,
            ghost_always_object: id("GhostAlwaysObjectEvent")?,
            path_manager: id("PathManagerEvent")?,
            sim_2d_audio: id("Sim2DAudioEvent")?,
            sim_3d_audio: id("Sim3DAudioEvent")?,
            lightning_strike: id("LightningStrikeEvent")?,
        })
    }

    /// The ids of Torque's own event classes ([`EventRegistry::torque`]); `None` only if the derivation refused the fixed list (it
    /// does not).
    pub fn torque() -> Option<EventIds> {
        EventRegistry::torque().ids().ok()
    }

    /// How many event classes exist (a class id at or above it is rejected).
    pub fn count(&self) -> u32 {
        self.classes.len() as u32
    }

    /// The width of a class id on the wire.
    pub fn bits(&self) -> u8 {
        self.bits
    }

    pub fn name(&self, id: u32) -> Option<&str> {
        self.classes.get(id as usize).map(|(name, _)| name.as_str())
    }

    pub fn direction(&self, id: u32) -> Option<Direction> {
        self.classes.get(id as usize).map(|&(_, d)| d)
    }

    /// The id of the class named `name` (how a game finds its own classes).
    pub fn id_of(&self, name: &str) -> Option<u32> {
        self.classes
            .iter()
            .position(|(n, _)| n == name)
            .map(|i| i as u32)
    }

    /// Names a game's own command classes (see [`EventRegistry::register_command`]); an error when one is not among the classes.
    pub fn with_command_classes(mut self, names: &[&str]) -> Result<EventIds, NetError> {
        self.commands = names
            .iter()
            .map(|name| {
                self.id_of(name)
                    .ok_or(NetError::Invalid("command class missing from the table"))
            })
            .collect::<Result<_, _>>()?;
        Ok(self)
    }

    /// The class of a command of `variant`: `RemoteCommandEvent` for 0, a game's command class after it; `None` past them.
    pub fn command_class(&self, variant: u8) -> Option<u32> {
        match variant {
            0 => Some(self.remote_command),
            n => self.commands.get(usize::from(n) - 1).copied(),
        }
    }

    /// The variant of a command whose class is `class`, if it is a command class.
    pub fn command_variant(&self, class: u32) -> Option<u8> {
        if class == self.remote_command {
            return Some(0);
        }
        let i = self.commands.iter().position(|&c| c == class)?;
        u8::try_from(i + 1).ok()
    }
}

/// What a pack or an unpack needs besides the stream: the Huffman table of the string coder, the class ids, the ghost-id width
/// (`ConnectionMessage` counts are `ghost_bits + 1` wide) and the datablock-id width (the audio events' profile id;
/// [`crate::profile::DEFAULT_DATABLOCK_BITS`] unless a game sets its own).
#[derive(Clone, Copy)]
pub struct EventCtx<'a> {
    pub table: &'a Table,
    pub ids: &'a EventIds,
    pub ghost_bits: u8,
    pub datablock_bits: u8,
}

/// `NetEvent::mGuaranteeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guarantee {
    /// 0: guaranteed and ordered (the queue with sequence numbers).
    Ordered,
    /// 1: guaranteed, unordered.
    Guaranteed,
    /// 2: sent once.
    Unguaranteed,
}

/// An event the queue can send.
pub trait WireEvent {
    /// The event's class id.
    fn class(&self, ids: &EventIds) -> u32;
    fn guarantee(&self) -> Guarantee {
        Guarantee::Ordered
    }
    /// Writes the body (after the class id). A write that does not fit overflows the stream, which the queue sees.
    fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>);
    /// [`WireEvent::pack`] with the objects an event may pack at write time (`GhostAlwaysObjectEvent::pack` calls the object's
    /// `packUpdate`, so the record reflects the object when the event is WRITTEN). The section writer passes its [`PackCtx`] through;
    /// an event that packs no object ignores it.
    fn pack_with(&self, w: &mut BitWriter, ctx: &EventCtx<'_>, objs: &mut dyn PackCtx) {
        let _ = objs;
        self.pack(w, ctx);
    }
}

/// What an event may reach while it is packed: the objects of the connection (`GhostAlwaysObjectEvent`'s record, the ghost index of
/// an object an event names). Implemented by `ghost::GhostPackCtx`; [`NoObjects`] stands in where there is none.
pub trait PackCtx {
    /// Writes the body of a `GhostAlwaysObjectEvent` after its index: `flag(live)` and, when the object exists and `seq` is still the
    /// connection's ghosting sequence, the class id and the object's full `packUpdate`; otherwise `flag(0)` alone.
    fn pack_ghost_always(&mut self, obj: u32, seq: u32, w: &mut BitWriter);

    /// `getGhostIndex` on this connection of the object with sim id `obj`, at write time: `None` for an id that is no object now and
    /// for an object that is not ghosted here or whose record is not acknowledged yet. The default knows no objects.
    fn ghost_index(&mut self, obj: u32) -> Option<u32> {
        let _ = obj;
        None
    }
}

/// A [`PackCtx`] with no objects: a ghost-always event packs as the empty form.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoObjects;

impl PackCtx for NoObjects {
    fn pack_ghost_always(&mut self, _obj: u32, _seq: u32, w: &mut BitWriter) {
        debug_assert!(false, "a ghost-always event written without a PackCtx");
        w.write_flag(false);
    }
}

/// `ConnectionMessageEvent`: raw 32-bit sequence, `int(3)` message, `int(ghost_bits + 1)` count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionMessage {
    pub seq: u32,
    pub message: u8,
    pub count: u32,
}

impl ConnectionMessage {
    pub fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        w.write_u32(self.seq);
        w.write_bits(u32::from(self.message) & 7, 3);
        w.write_bits(self.count, ctx.ghost_bits.saturating_add(1).min(32));
    }

    pub fn unpack(r: &mut BitReader, ctx: &EventCtx<'_>) -> Result<ConnectionMessage, NetError> {
        let seq = r.read_u32()?;
        let message = r.read_bits(3)? as u8;
        let count = r.read_bits(ctx.ghost_bits.saturating_add(1).min(32))?;
        Ok(ConnectionMessage {
            seq,
            message,
            count,
        })
    }
}

/// The events of this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetEvent {
    ConnectionMessage(ConnectionMessage),
    NetString(NetStringEvent),
    Remote(RemoteCommandEvent),
}

impl NetEvent {
    /// Reads the body of the event of class `class`; a class this crate has no decoder for is `Invalid`.
    pub fn unpack(class: u32, r: &mut BitReader, ctx: &EventCtx<'_>) -> Result<NetEvent, NetError> {
        let ids = ctx.ids;
        if class == ids.net_string {
            NetStringEvent::unpack(r, ctx).map(NetEvent::NetString)
        } else if class == ids.connection_message {
            ConnectionMessage::unpack(r, ctx).map(NetEvent::ConnectionMessage)
        } else if let Some(variant) = ids.command_variant(class) {
            let mut command = RemoteCommandEvent::unpack(r, ctx)?;
            command.variant = variant;
            Ok(NetEvent::Remote(command))
        } else {
            Err(NetError::Invalid("event class has no decoder"))
        }
    }

    /// What the connection does when this event settles (a packet carrying it was acked or lost): a delivered NetString sets its
    /// sender entry's flag.
    pub fn on_settled(&self, delivered: bool, strings: &mut SendStrings) {
        if let NetEvent::NetString(e) = self
            && delivered
        {
            strings.delivered(e.slot, e.tag);
        }
    }
}

impl WireEvent for NetEvent {
    fn class(&self, ids: &EventIds) -> u32 {
        match self {
            NetEvent::ConnectionMessage(_) => ids.connection_message,
            NetEvent::NetString(_) => ids.net_string,
            NetEvent::Remote(c) => ids.command_class(c.variant).unwrap_or(ids.remote_command),
        }
    }

    fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        match self {
            NetEvent::ConnectionMessage(e) => e.pack(w, ctx),
            NetEvent::NetString(e) => e.pack(w, ctx),
            NetEvent::Remote(e) => e.pack(w, ctx),
        }
    }
}

// ---- writing

/// Writes one unordered event: flag 1, the class id, the body.
pub fn put_unordered<E: WireEvent>(w: &mut BitWriter, ctx: &EventCtx<'_>, event: &E) {
    put_unordered_with(w, ctx, &mut NoObjects, event);
}

/// [`put_unordered`] with the objects the event may pack.
pub fn put_unordered_with<E: WireEvent>(
    w: &mut BitWriter,
    ctx: &EventCtx<'_>,
    objs: &mut dyn PackCtx,
    event: &E,
) {
    w.write_flag(true);
    w.write_bits(event.class(ctx.ids), ctx.ids.bits());
    event.pack_with(w, ctx, objs);
}

/// Writes one ordered event: flag 1, the seq flag (and the low 7 bits of `seq` when it is not `prev + 1`), the class id, the body.
/// `prev` is `PREV_START` (-2) before the first event of a packet and the last sequence written after.
pub fn put_ordered<E: WireEvent>(
    w: &mut BitWriter,
    ctx: &EventCtx<'_>,
    prev: &mut u32,
    seq: u32,
    event: &E,
) {
    put_ordered_with(w, ctx, &mut NoObjects, prev, seq, event);
}

/// [`put_ordered`] with the objects the event may pack.
pub fn put_ordered_with<E: WireEvent>(
    w: &mut BitWriter,
    ctx: &EventCtx<'_>,
    objs: &mut dyn PackCtx,
    prev: &mut u32,
    seq: u32,
    event: &E,
) {
    w.write_flag(true);
    let consecutive = seq == prev.wrapping_add(1);
    w.write_flag(consecutive);
    if !consecutive {
        w.write_bits(seq & SEQ_MASK, SEQ_BITS);
    }
    *prev = seq;
    w.write_bits(event.class(ctx.ids), ctx.ids.bits());
    event.pack_with(w, ctx, objs);
}

/// One event as it was read off the wire, in wire order (the datagram decoder re-encodes from this).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireItem<E> {
    pub ordered: bool,
    /// The sequence flag as read (ordered only).
    pub consecutive: bool,
    /// The 7-bit sequence as read or implied (ordered only).
    pub seq7: u8,
    pub class: u32,
    pub event: E,
}

/// Writes items read by [`EventReceiver::read_section`] back as they were: the unordered ones, a 0, the ordered ones, a 0. The seq
/// flag and the 7 bits are the ones read.
pub fn write_wire_items<E: WireEvent>(
    w: &mut BitWriter,
    ctx: &EventCtx<'_>,
    items: &[WireItem<E>],
) {
    for item in items.iter().filter(|i| !i.ordered) {
        put_unordered(w, ctx, &item.event);
    }
    w.write_flag(false);
    for item in items.iter().filter(|i| i.ordered) {
        w.write_flag(true);
        w.write_flag(item.consecutive);
        if !item.consecutive {
            w.write_bits(u32::from(item.seq7), SEQ_BITS);
        }
        w.write_bits(item.class, ctx.ids.bits());
        item.event.pack(w, ctx);
    }
    w.write_flag(false);
}

/// Writes into a scratch copy of the packet and commits it only if the copy is within the writer's hard limit with at least `reserve`
/// bits to spare for the flags that close the section (this crate's own bound; without it the stream could overflow).
fn commit<F: FnOnce(&mut BitWriter)>(w: &mut BitWriter, reserve: usize, put: F) -> bool {
    let mut scratch = w.clone();
    put(&mut scratch);
    if scratch.is_overflowed() || scratch.remaining_bits() < reserve {
        return false;
    }
    *w = scratch;
    true
}

/// The measure of an event a [`SectionRule`] asks for: the body alone, packed into a stream with no string buffer (so strings are
/// counted whole, never as a prefix-delta against the packet), in bits.
#[cfg(test)]
fn measure<E: WireEvent>(event: &E, ctx: &EventCtx<'_>) -> usize {
    measure_with(event, ctx, &mut NoObjects)
}

/// [`measure`] with the objects the event may pack.
fn measure_with<E: WireEvent>(event: &E, ctx: &EventCtx<'_>, objs: &mut dyn PackCtx) -> usize {
    let mut scratch = BitWriter::new();
    event.pack_with(&mut scratch, ctx, objs);
    scratch.bits()
}

/// What a section tests before it writes a head event, beyond `eventWritePacket`'s own tests (the stream not full, the ordered
/// window). A host may give a queue another rule ([`EventQueue::with_section_rule`]) or install one for every new queue
/// ([`install_section_rule`]).
#[derive(Debug, Clone, Copy)]
pub struct SectionRule {
    /// Whether the room tests need the head event's measure (its body packed alone, see [`SectionRule::unordered_fits`]); without
    /// it they are given 0.
    pub measures: bool,
    /// Whether the unordered head event may go: `(stream position in bits, packet_bits, measured body bits)`.
    pub unordered_fits: fn(usize, usize, usize) -> bool,
    /// Whether the ordered head event may go: `(stream position in bits, packet_bits, measured body bits)`.
    pub ordered_fits: fn(usize, usize, usize) -> bool,
    /// While the connection is ghosting: whether the unordered phase stops before the head event, `(stream position in bits,
    /// packet_bits)`.
    pub ghosting_stop: fn(usize, usize) -> bool,
}

fn always_fits(_: usize, _: usize, _: usize) -> bool {
    true
}

fn never_stops(_: usize, _: usize) -> bool {
    false
}

/// Torque's rule: no test beyond `eventWritePacket`'s own.
pub static TORQUE_SECTION: SectionRule = SectionRule {
    measures: false,
    unordered_fits: always_fits,
    ordered_fits: always_fits,
    ghosting_stop: never_stops,
};

static SECTION_RULE: std::sync::OnceLock<&'static SectionRule> = std::sync::OnceLock::new();

/// The rule a new [`EventQueue`] takes: the installed one, else [`TORQUE_SECTION`].
pub fn section_rule() -> &'static SectionRule {
    match SECTION_RULE.get() {
        Some(rule) => rule,
        None => &TORQUE_SECTION,
    }
}

/// Installs `rule` for every queue made after it. Only the first install takes; installing the same rule again is a no-op.
/// Returns false when another rule was installed first.
pub fn install_section_rule(rule: &'static SectionRule) -> bool {
    std::ptr::eq(*SECTION_RULE.get_or_init(|| rule), rule)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Unordered,
    Ordered,
}

/// How the section is written this time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionParams {
    /// The connection is ghosting (the rule's [`SectionRule::ghosting_stop`] applies).
    pub ghosting: bool,
    /// Send no ordered event. **Not Torque's behaviour**: a standard server sends events in its very first packets (before the
    /// client's first data packet, under the 200-byte initial rate). The host may use it to keep ordered sends
    /// back until the client's first packet. Unordered events still go.
    pub hold_ordered: bool,
    /// `bufferSize * 8`: the packet stream's size in bits, `curRate.packetSize * 8` (the loops of `eventWritePacket` test against
    /// it; the stream itself takes up to 12,000 bits whatever this is).
    pub packet_bits: usize,
    /// The fewest bits any data packet holds before its event section (header, rate prefix, game head). An event that could not
    /// be sent even in a packet that short is an error, not a stall.
    pub min_start_bits: usize,
}

/// What a section wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Written {
    pub unordered: usize,
    pub ordered: usize,
}

#[derive(Debug)]
struct Queued<E> {
    event: E,
    /// The ordered sequence number; meaningless for an unordered event.
    seq: u32,
}

/// An event the queue gave back: a post it refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PostError<E> {
    /// `mSendingEvents` is off: the connection is going away (the event is released).
    Closed(E),
    /// The queue is at its cap: keep the event and post it again later (backpressure; nothing is dropped).
    Full(E),
}

/// A packet's event that was acked or lost.
#[derive(Debug, PartialEq, Eq)]
pub struct Settled<E> {
    pub event: E,
    /// `notifyDelivered(conn, recved)`.
    pub delivered: bool,
}

/// One connection's send side of the event layer.
#[derive(Debug)]
pub struct EventQueue<E> {
    ordered: VecDeque<Queued<E>>,
    unordered: VecDeque<Queued<E>>,
    acked: Vec<Queued<E>>,
    in_flight: VecDeque<(u32, Vec<Queued<E>>)>,
    next_seq: u32,
    last_acked: u32,
    sending: bool,
    rule: &'static SectionRule,
}

impl<E: WireEvent> Default for EventQueue<E> {
    fn default() -> EventQueue<E> {
        EventQueue::new()
    }
}

impl<E: WireEvent> EventQueue<E> {
    /// A new queue: `mSendingEvents` on, the next sequence 0, nothing acked (`lastAcked` -1), the rule [`section_rule`] gives.
    pub fn new() -> EventQueue<E> {
        EventQueue {
            ordered: VecDeque::new(),
            unordered: VecDeque::new(),
            acked: Vec::new(),
            in_flight: VecDeque::new(),
            next_seq: 0,
            last_acked: u32::MAX,
            sending: true,
            rule: section_rule(),
        }
    }

    /// The queue with `rule` in place of the one it was made with.
    pub fn with_section_rule(mut self, rule: &'static SectionRule) -> EventQueue<E> {
        self.rule = rule;
        self
    }

    /// Stops accepting events (the connection is being removed).
    pub fn close(&mut self) {
        self.sending = false;
    }

    pub fn is_closed(&self) -> bool {
        !self.sending
    }

    /// Ordered events waiting to be sent.
    pub fn ordered_len(&self) -> usize {
        self.ordered.len()
    }

    pub fn unordered_len(&self) -> usize {
        self.unordered.len()
    }

    /// How many more ordered events the queue takes.
    pub fn room_ordered(&self) -> usize {
        NET_EVENT_QUEUE_ORDERED.saturating_sub(self.ordered.len())
    }

    pub fn room_unordered(&self) -> usize {
        NET_EVENT_QUEUE_UNORDERED.saturating_sub(self.unordered.len())
    }

    /// The sequence the next ordered event gets.
    pub fn next_seq(&self) -> u32 {
        self.next_seq
    }

    /// The highest ordered sequence acked in an unbroken run (-1 as `u32::MAX` at the start).
    pub fn last_acked(&self) -> u32 {
        self.last_acked
    }

    /// Packets whose events are waiting for an ack or a loss.
    pub fn packets_in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Whether nothing is queued, in flight or waiting for an earlier ack.
    pub fn is_idle(&self) -> bool {
        self.ordered.is_empty()
            && self.unordered.is_empty()
            && self.acked.is_empty()
            && self.in_flight.is_empty()
    }

    /// `postNetEvent`.
    pub fn post(&mut self, event: E) -> Result<(), PostError<E>> {
        if !self.sending {
            return Err(PostError::Closed(event));
        }
        if event.guarantee() == Guarantee::Ordered {
            if self.room_ordered() == 0 {
                return Err(PostError::Full(event));
            }
            let seq = self.next_seq;
            self.next_seq = self.next_seq.wrapping_add(1);
            self.ordered.push_back(Queued { event, seq });
        } else {
            if self.room_unordered() == 0 {
                return Err(PostError::Full(event));
            }
            self.unordered.push_back(Queued {
                event,
                seq: u32::MAX,
            });
        }
        Ok(())
    }

    /// Posts several events or none: either every one is queued (in order) or the whole batch comes back.
    pub fn post_batch(&mut self, events: Vec<E>) -> Result<(), Vec<E>> {
        let ordered = events
            .iter()
            .filter(|e| e.guarantee() == Guarantee::Ordered)
            .count();
        let other = events.len() - ordered;
        if !self.sending || ordered > self.room_ordered() || other > self.room_unordered() {
            return Err(events);
        }
        for event in events {
            // The room was checked above, so a post cannot be refused.
            let _refused = self.post(event).err();
        }
        Ok(())
    }

    /// `eventWritePacket`: the unordered phase, then the ordered phase, each closed by a 0 flag. `send_seq` is the data packet's
    /// sequence number, which [`EventQueue::on_notify`] is later asked about.
    ///
    /// Each loop stops once the stream is full (`bitNum > packet_bits`); the queue's [`SectionRule`] then tests the head event (the
    /// room tests, then, in phase 1 while ghosting, the ghosting stop). An event that is refused stays at the head and blocks its
    /// queue. The packet therefore may pass `packet_bits`, as in `eventWritePacket`; the writer's own limit (12,000 bits,
    /// `NET_MAX_DATAGRAM` bytes, the stream limit whatever the rate) is the hard bound, and the two closing flags are also kept clear
    /// of it (they could otherwise overflow the stream).
    ///
    /// Returns an error only for an event that could never be sent in any packet ([`SectionParams::min_start_bits`]).
    pub fn write_section(
        &mut self,
        w: &mut BitWriter,
        ctx: &EventCtx<'_>,
        send_seq: u32,
        params: SectionParams,
    ) -> Result<Written, NetError> {
        self.write_section_mode(w, ctx, &mut NoObjects, send_seq, params, false)
    }

    /// [`EventQueue::write_section`] where an event that no packet of `params` could carry **stalls** its queue (it stays at the head
    /// and the section ends there, as in `eventWritePacket`) instead of being an error. The host uses it while the peer's rate is still the initial
    /// one: a bigger packet may follow the peer's first rate pair. Events ahead of the stalled one still go out.
    pub fn write_section_stalling(
        &mut self,
        w: &mut BitWriter,
        ctx: &EventCtx<'_>,
        send_seq: u32,
        params: SectionParams,
    ) -> Result<Written, NetError> {
        self.write_section_mode(w, ctx, &mut NoObjects, send_seq, params, true)
    }

    /// [`EventQueue::write_section`] with the objects the events may pack at write time (the ghost layer's `GhostPackCtx`).
    pub fn write_section_with(
        &mut self,
        w: &mut BitWriter,
        ctx: &EventCtx<'_>,
        objs: &mut dyn PackCtx,
        send_seq: u32,
        params: SectionParams,
    ) -> Result<Written, NetError> {
        self.write_section_mode(w, ctx, objs, send_seq, params, false)
    }

    /// [`EventQueue::write_section_with`] with the stalling rule of [`EventQueue::write_section_stalling`].
    pub fn write_section_with_stalling(
        &mut self,
        w: &mut BitWriter,
        ctx: &EventCtx<'_>,
        objs: &mut dyn PackCtx,
        send_seq: u32,
        params: SectionParams,
    ) -> Result<Written, NetError> {
        self.write_section_mode(w, ctx, objs, send_seq, params, true)
    }

    fn write_section_mode(
        &mut self,
        w: &mut BitWriter,
        ctx: &EventCtx<'_>,
        objs: &mut dyn PackCtx,
        send_seq: u32,
        params: SectionParams,
        stall: bool,
    ) -> Result<Written, NetError> {
        let size = params.packet_bits;
        let rule = self.rule;
        let mut chain: Vec<Queued<E>> = Vec::new();
        let mut written = Written::default();
        let room_in_flight = self.in_flight.len() < NET_EVENT_PACKETS_IN_FLIGHT;

        // Phase 1: the unordered queue. Two bits stay free for the closing flags.
        while let Some(head) = self.unordered.front().filter(|_| room_in_flight) {
            if w.bits() > size {
                break;
            }
            let body = if rule.measures {
                measure_with(&head.event, ctx, objs)
            } else {
                0
            };
            if !(rule.unordered_fits)(w.bits(), size, body) {
                if !stall {
                    self.check_sendable(
                        &head.event,
                        ctx,
                        objs,
                        &params,
                        Phase::Unordered,
                        body,
                        w,
                    )?;
                }
                break;
            }
            if params.ghosting && (rule.ghosting_stop)(w.bits(), size) {
                break;
            }
            if !commit(w, 2, |s| put_unordered_with(s, ctx, objs, &head.event)) {
                if !stall {
                    self.check_sendable(
                        &head.event,
                        ctx,
                        objs,
                        &params,
                        Phase::Unordered,
                        body,
                        w,
                    )?;
                }
                break;
            }
            if let Some(q) = self.unordered.pop_front() {
                chain.push(q);
                written.unordered += 1;
            }
        }
        w.write_flag(false);

        // Phase 2: the ordered queue, one bit free for the closing flag.
        let mut prev = PREV_START;
        while let Some(head) = self
            .ordered
            .front()
            .filter(|_| room_in_flight && !params.hold_ordered)
        {
            if w.bits() > size {
                break;
            }
            let body = if rule.measures {
                measure_with(&head.event, ctx, objs)
            } else {
                0
            };
            if !(rule.ordered_fits)(w.bits(), size, body) {
                if !stall {
                    self.check_sendable(&head.event, ctx, objs, &params, Phase::Ordered, body, w)?;
                }
                break;
            }
            if seq_after(head.seq, self.last_acked.wrapping_add(NET_EVENT_WINDOW)) {
                break;
            }
            let mut p = prev;
            if !commit(w, 1, |s| {
                put_ordered_with(s, ctx, objs, &mut p, head.seq, &head.event)
            }) {
                if !stall {
                    self.check_sendable(&head.event, ctx, objs, &params, Phase::Ordered, body, w)?;
                }
                break;
            }
            prev = p;
            if let Some(q) = self.ordered.pop_front() {
                chain.push(q);
                written.ordered += 1;
            }
        }
        w.write_flag(false);

        if !chain.is_empty() {
            self.in_flight.push_back((send_seq, chain));
        }
        Ok(written)
    }

    /// An event the writer refused is an error only when no packet could ever take it: not even one whose game payload is as short as
    /// `params.min_start_bits` (it would otherwise stall for ever).
    #[allow(clippy::too_many_arguments)]
    fn check_sendable(
        &self,
        event: &E,
        ctx: &EventCtx<'_>,
        objs: &mut dyn PackCtx,
        params: &SectionParams,
        phase: Phase,
        body: usize,
        w: &BitWriter,
    ) -> Result<(), NetError> {
        let start = params.min_start_bits;
        let hard = w.bits() + w.remaining_bits();
        let mut alone = BitWriter::new();
        let mut prev = PREV_START;
        let (measure_fails, wire_bits) = match phase {
            Phase::Unordered => {
                put_unordered_with(&mut alone, ctx, objs, event);
                (
                    !(self.rule.unordered_fits)(start, params.packet_bits, body),
                    alone.bits() + 2,
                )
            }
            Phase::Ordered => {
                put_ordered_with(&mut alone, ctx, objs, &mut prev, 0, event);
                // The first ordered event of a packet follows the unordered phase's closing flag.
                (
                    !(self.rule.ordered_fits)(start + 1, params.packet_bits, body),
                    alone.bits() + 2,
                )
            }
        };
        if measure_fails || alone.is_overflowed() || start + wire_bits > hard {
            return Err(NetError::Cap {
                what: "event bits in a packet",
                count: (start + wire_bits) as u64,
                cap: params.packet_bits.min(hard) as u64,
            });
        }
        Ok(())
    }

    /// The fate of the data packet sent as `send_seq` (the packet layer's [`crate::protocol::Notify`]): its events are acked or put
    /// back. Settled events are appended to `out` for the host to notify.
    pub fn on_notify(&mut self, send_seq: u32, delivered: bool, out: &mut Vec<Settled<E>>) {
        let Some(at) = self.in_flight.iter().position(|(s, _)| *s == send_seq) else {
            return;
        };
        let Some((_, chain)) = self.in_flight.remove(at) else {
            return;
        };
        if delivered {
            for q in chain {
                if q.seq == u32::MAX && q.event.guarantee() != Guarantee::Ordered {
                    out.push(Settled {
                        event: q.event,
                        delivered: true,
                    });
                } else {
                    let at = self.acked.partition_point(|a| seq_before(a.seq, q.seq));
                    self.acked.insert(at, q);
                }
            }
            while self
                .acked
                .first()
                .is_some_and(|a| a.seq == self.last_acked.wrapping_add(1))
            {
                self.last_acked = self.last_acked.wrapping_add(1);
                let q = self.acked.remove(0);
                out.push(Settled {
                    event: q.event,
                    delivered: true,
                });
            }
        } else {
            for q in chain {
                match q.event.guarantee() {
                    Guarantee::Ordered => {
                        let at = self.ordered.partition_point(|o| seq_before(o.seq, q.seq));
                        self.ordered.insert(at, q);
                    }
                    Guarantee::Guaranteed => self.unordered.push_front(q),
                    Guarantee::Unguaranteed => out.push(Settled {
                        event: q.event,
                        delivered: false,
                    }),
                }
            }
        }
    }

    /// The connection is removed: every event still held, each settled as delivered (`notifyDelivered(conn, true)`).
    pub fn release_all(&mut self) -> Vec<Settled<E>> {
        self.sending = false;
        let mut out = Vec::new();
        let mut take = |q: Queued<E>| {
            out.push(Settled {
                event: q.event,
                delivered: true,
            })
        };
        for (_, chain) in self.in_flight.drain(..) {
            chain.into_iter().for_each(&mut take);
        }
        self.ordered.drain(..).for_each(&mut take);
        self.unordered.drain(..).for_each(&mut take);
        self.acked.drain(..).for_each(&mut take);
        out
    }
}

// ---- reading

/// Reads the body of an event of a class the reader has already checked.
pub trait EventUnpacker {
    type Event;
    fn unpack(
        &mut self,
        class: u32,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
    ) -> Result<Self::Event, NetError>;
}

/// Unpacks the events of this crate.
#[derive(Debug, Clone, Copy, Default)]
pub struct NetEventUnpacker;

impl EventUnpacker for NetEventUnpacker {
    type Event = NetEvent;

    fn unpack(
        &mut self,
        class: u32,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
    ) -> Result<NetEvent, NetError> {
        NetEvent::unpack(class, r, ctx)
    }
}

/// An event ready to run, in the order it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming<E> {
    /// The widened sequence of an ordered event; `None` for an unordered one.
    pub seq: Option<u32>,
    pub event: E,
}

/// What reading a section did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadStats {
    pub unordered: usize,
    pub ordered: usize,
    /// Ordered events dropped because their widened sequence was already waiting.
    pub duplicates: usize,
    /// Ordered events dropped because their widened sequence was more than 126 ahead.
    pub too_far: usize,
}

/// One connection's receive side of the event layer.
#[derive(Debug)]
pub struct EventReceiver<E> {
    next_recv: u32,
    wait: Vec<(u32, E)>,
}

impl<E> Default for EventReceiver<E> {
    fn default() -> EventReceiver<E> {
        EventReceiver::new()
    }
}

impl<E> EventReceiver<E> {
    pub fn new() -> EventReceiver<E> {
        EventReceiver {
            next_recv: 0,
            wait: Vec::new(),
        }
    }

    /// The next ordered sequence to run.
    pub fn next_recv(&self) -> u32 {
        self.next_recv
    }

    /// Ordered events waiting for an earlier one.
    pub fn waiting(&self) -> usize {
        self.wait.len()
    }

    /// Runs the events at the head of the wait list whose sequence is `nextRecv`.
    pub fn run_ready(&mut self, out: &mut Vec<Incoming<E>>) {
        while self.wait.first().is_some_and(|(s, _)| *s == self.next_recv) {
            let (seq, event) = self.wait.remove(0);
            self.next_recv = self.next_recv.wrapping_add(1);
            out.push(Incoming {
                seq: Some(seq),
                event,
            });
        }
    }

    /// Files an ordered event under its 7-bit sequence by the sequence widening, with drops for a duplicate or an entry too far ahead.
    fn file(&mut self, seq7: u32, event: E, stats: &mut ReadStats) -> Result<(), NetError> {
        let mut seq = (self.next_recv & !SEQ_MASK) | seq7;
        if seq_before(seq, self.next_recv) {
            seq = seq.wrapping_add(SEQ_MASK + 1);
        }
        if seq.wrapping_sub(self.next_recv) > NET_EVENT_WINDOW {
            stats.too_far += 1;
            return Ok(());
        }
        let at = self.wait.partition_point(|(s, _)| seq_before(*s, seq));
        if self.wait.get(at).is_some_and(|(s, _)| *s == seq) {
            stats.duplicates += 1;
            return Ok(());
        }
        if self.wait.len() >= NET_WAIT_LIST {
            return Err(NetError::Cap {
                what: "events waiting for an earlier one",
                count: self.wait.len() as u64 + 1,
                cap: NET_WAIT_LIST as u64,
            });
        }
        self.wait.insert(at, (seq, event));
        Ok(())
    }

    /// [`EventReceiver::read_section_charged`] with a budget that pays for everything.
    pub fn read_section<U>(
        &mut self,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
        reader: Role,
        unpacker: &mut U,
        out: &mut Vec<Incoming<E>>,
        log: Option<&mut Vec<WireItem<E>>>,
    ) -> Result<ReadStats, NetError>
    where
        U: EventUnpacker<Event = E>,
        E: Clone,
    {
        self.read_section_charged(r, ctx, reader, unpacker, out, log, &mut free_charge())
    }

    /// `eventReadPacket`. Unordered events are appended to `out` as they are read; ordered ones after the whole section, in sequence.
    /// `reader` is the role of the side that reads (it decides which direction is wrong). `log`, when given, receives every event
    /// as it was on the wire. On an error `out` keeps what was run before it.
    ///
    /// `charge` is told, for every event read, how many units of work it was (its bytes on the wire, at least one) and answers
    /// whether the per-tick network work budget can pay; a `false` stops the read with an error. One 1,500-byte datagram can carry
    /// about a thousand unordered commands, so a host that runs a script per event must also pay for that, before `resolve` and the
    /// call.
    #[allow(clippy::too_many_arguments)]
    pub fn read_section_charged<U>(
        &mut self,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
        reader: Role,
        unpacker: &mut U,
        out: &mut Vec<Incoming<E>>,
        mut log: Option<&mut Vec<WireItem<E>>>,
        charge: &mut ChargeFn,
    ) -> Result<ReadStats, NetError>
    where
        U: EventUnpacker<Event = E>,
        E: Clone,
    {
        let mut stats = ReadStats::default();
        let mut ordered_phase = false;
        let mut prev7: i32 = -2;
        loop {
            let started = r.position();
            if !r.read_flag()? {
                if ordered_phase {
                    break;
                }
                ordered_phase = true;
                continue;
            }
            let (mut consecutive, mut seq7) = (false, 0u32);
            if ordered_phase {
                consecutive = r.read_flag()?;
                seq7 = if consecutive {
                    ((prev7 + 1) & 0x7f) as u32
                } else {
                    r.read_bits(SEQ_BITS)?
                };
                prev7 = seq7 as i32;
            }
            let class = r.read_bits(ctx.ids.bits())?;
            if class >= ctx.ids.count() {
                return Err(NetError::Invalid("event class id"));
            }
            match (ctx.ids.direction(class), reader) {
                (Some(Direction::ServerToClient), Role::Server)
                | (Some(Direction::ClientToServer), Role::Client) => {
                    return Err(NetError::Invalid("event direction"));
                }
                _ => {}
            }
            let event = unpacker.unpack(class, r, ctx)?;
            if !charge((r.position() - started).div_ceil(8).max(1) as u64) {
                return Err(NetError::Invalid("work budget"));
            }
            if let Some(log) = log.as_deref_mut() {
                log.push(WireItem {
                    ordered: ordered_phase,
                    consecutive,
                    seq7: seq7 as u8,
                    class,
                    event: event.clone(),
                });
            }
            if ordered_phase {
                stats.ordered += 1;
                self.file(seq7, event, &mut stats)?;
            } else {
                stats.unordered += 1;
                out.push(Incoming { seq: None, event });
            }
        }
        self.run_ready(out);
        Ok(stats)
    }
}

#[cfg(test)]
mod tests;
