//! The datablock transfer of the host: `GameConnection::transmitDataBlocks`, `SimDataBlockEvent`'s pack and `notifyDelivered`,
//! `SimDataBlock::onAdd` and `onStaticModified`, and the datablock steps of `handleConnectionMessage`.
//!
//! **`modifiedKey`** ([`DatablockKeys`]). One counter, 0 at start, post-incremented: every `SimDataBlock`-class object takes
//! `counter++` when it is added (also one that gets no group, and one whose add fails later), and every `setDataField` that
//! matched a registered static field of a datablock takes one again (`onStaticModified`; the `Vm`'s static-write observer; a
//! same-value write included; the fields of an object that `datablock` is still building too, which only move the counter: the
//! add then gives the object its key), except where a game's rule takes the write itself ([`StaticWriteRule`]). The host's own
//! writes (preload) bump nothing. `deleteDataBlocks` sets the counter back to 0 and no connection's key is touched. Only the
//! order of the keys reaches the wire, never a value; the counter saturates at `i32::MAX` (unreachable: every bump is a script
//! step).
//!
//! **The chain** ([`ConnDatablocks`]). `transmitDataBlocks(seq)` stores `seq` (`atoi`), finds the first group position whose
//! key is above the connection's key and posts the events for the next window of positions (the codecs' [`datablock_window`]:
//! Torque's `DataBlockQueueCount` by default), each `{id, index = absolute position, total = group size now}`; no such position
//! posts the datablocks-done message at once (the codecs' [`datablocks_done_message`]: Torque's `DataBlocksDone`, 6). A settled
//! event (an ordered event only settles when acked) posts the event for `index + window` with the current size, and at
//! `index == size - 1` sets `key = max_key` and posts the datablocks-done message. The client's download-done message
//! ([`datablocks_download_done_message`]: Torque's 7) with the connection's sequence runs the script's `onDataBlocksDone`.
//!
//! **The host's choices.** One chain per connection: a call while a chain is active is queued, only the newest, and starts when
//! the chain ends; an event is rendered, not packed at the exact write: [`EventLayer::write`](super::events::EventLayer) has
//! the `Vm` and [`torque_net::events::WireEvent::pack`] has not, so the first packet build after events are posted renders
//! every cell of the chain that was never rendered (a window at once, not only the ones that packet carries), and a cell is
//! rendered again after a lost packet; an event over one packet drops the client with a console line; an object whose class is
//! not one of the codecs' networked datablock classes travels as the one 0 bit; an event the ordered queue has no room for is
//! held and posted as acks free room, and a chain started on a queue with no room at all drops the connection with `Overflow.`;
//! the download-done message compares the connection's sequence, which starts at 0.
//!
//! **Render time is not pack time.** The host effects of a pack happen when the event is rendered, which is when the first
//! packet build after the events were posted renders every cell of the window: a game's prepack ([`DbValues::before_pack`]) and
//! its pack effects run then, so a script that runs between the render of event 1 and the pack of event 40 (possible across
//! passes when a packet is lost and the cell is rendered again) sees the effect earlier than at the event's own pack.
//!
//! **Attach before the datablocks.** The keys and the static-write observer are installed when the network host is attached
//! ([`attach_host`](super::attach_host)); a host attached after datablocks were added would give each of them key 0 and send
//! none, so the server's attach refuses ([`AttachError::DatablocksAlreadyExist`](crate::server::AttachError)).
//!
//! **The view** ([`ObjectView`]). A value the host computed at add or preload time ([`DbValues`]: the emitter's ids, the
//! image's state records) is answered from there; the clamps written back into a member are in the static field itself; every
//! other name is answered as the static field says, or as the default a few names have (`special`).
//!
//! [`datablock_window`]: super::codecs::NetCodecs::datablock_window
//! [`datablocks_done_message`]: super::codecs::NetCodecs::datablocks_done_message
//! [`datablocks_download_done_message`]: super::codecs::NetCodecs::datablocks_download_done_message
//!
//! Credit: the chain follows Torque3D's MIT-licensed `T3D/gameBase/gameConnection.cpp` and `gameConnectionEvents.cpp`
//! (`Copyright (c) 2012 GarageGames, LLC`, `Copyright (C) 2015 Faust Logic, Inc.`), and the keys follow its
//! `console/simDatablock.cpp` (`Copyright (c) 2012 GarageGames, LLC`, `Copyright (C) 2015 Faust Logic, Inc.`).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use super::codecs::{DATABLOCKS_DONE, NetCodecs};
use super::datablock::{
    DatablockBody, DatablockEvent, DatablockRecord, DatablockView, DbPackError, DbValue,
    EVENT_SCRATCH_BITS,
};
use torque_net::bits::Affine;
use torque_net::classes::{Kind, NetClasses, NetRep};
use torque_net::events::{ConnectionMessage, EventQueue, NetEvent, PostError, WireEvent};
use torque_net::huffman::Table;
use torque_ts_vm::classes::EngineClass;
use torque_ts_vm::fields::{FieldType, Value as FieldValue};
use torque_ts_vm::geometry::Mat4;
use torque_ts_vm::sim::{ClassId, ObjId, Objects, SIM_DATA_BLOCK, SimObject};
use torque_ts_vm::vm::{Vm, VmError};

use super::hostevent::HostEvent;
use torque_game::dbvalues::DbValues;

/// Datablock events a chain keeps queued or unacked unless the codecs say otherwise ([`NetCodecs::datablock_window`]): Torque's
/// `DataBlockQueueCount`.
pub const DATABLOCK_QUEUE_COUNT: usize = 16;
/// The widest window a chain may keep: below the ordered channel's 126 events past the last acked one (`torque_net::events`),
/// with room for a command or two beside it.
pub const MAX_DATABLOCK_WINDOW: usize = 120;
/// The record entries one unit of the send pass's work pays for when an event is rendered (a unit is also charged per event).
pub const NET_DATABLOCK_ENTRIES_PER_UNIT: u64 = 16;
/// The group positions one unit of a `transmitDataBlocks` scan pays for.
pub const NET_DATABLOCK_SCAN_PER_UNIT: usize = 16;

/// The most bits one event may take in the event layer's own measure (no shared strings) for any packet to carry it: the packet
/// stream's largest size, less the fewest bits a data packet holds before its events and the closing flag. The event layer
/// refuses a bigger one with `Packet Error.` and no line of its own, so the host tests this first and says why (an event that
/// cannot fit one packet is refused with a console line and the client disconnected). `head_bits` is the server head with
/// nothing set ([`NetCodecs::head_min_bits`]).
pub fn packet_room_bits_with(head_bits: usize) -> usize {
    torque_net::protocol::MAX_PACKET_BYTES as usize * 8
        - super::events::min_start_bits_with(head_bits)
        - 1
}

/// Whether some packet can carry `event`: the bits it takes with the strings shared inside the packet are within the scratch
/// stream ([`EVENT_SCRATCH_BITS`]), and the bits it takes by themselves (the event layer measures a scratch stream with no
/// shared strings, `eventWritePacket`) are within `room` ([`packet_room_bits_with`]). Returns the bits of the first measure, or
/// [`DbPackError::TooBig`].
pub fn event_refusal_in(
    event: &DatablockEvent,
    table: &Table,
    classes: &NetClasses,
    room: usize,
) -> Result<usize, DbPackError> {
    let bits = event.measure(table, classes, EVENT_SCRATCH_BITS)?;
    let mut plain = torque_net::bits::BitWriter::new();
    event.pack(&mut plain, table, classes)?;
    if plain.bits() > room {
        return Err(DbPackError::TooBig {
            bits: plain.bits(),
            limit: room,
        });
    }
    Ok(bits)
}

// ---- the keys

#[derive(Debug, Default)]
struct KeyTable {
    counter: i32,
    keys: HashMap<ObjId, i32>,
}

/// The `modifiedKey` of every datablock object and the global counter. Cheap to clone; clones share the table.
#[derive(Debug, Clone, Default)]
pub struct DatablockKeys(Rc<RefCell<KeyTable>>);

impl DatablockKeys {
    pub fn new() -> DatablockKeys {
        DatablockKeys::default()
    }

    /// `db.modifiedKey = counter++`.
    pub fn bump(&self, id: ObjId) {
        let mut t = self.0.borrow_mut();
        let key = t.counter;
        t.counter = t.counter.saturating_add(1);
        t.keys.insert(id, key);
    }

    /// A write to a field of an object that is not registered yet: the counter moves, no object takes the value (its add
    /// overwrites the key).
    pub fn advance(&self) {
        let mut t = self.0.borrow_mut();
        t.counter = t.counter.saturating_add(1);
    }

    /// An object's key (0 for one that never took a key: not a datablock).
    pub fn key_of(&self, id: ObjId) -> i32 {
        self.0.borrow().keys.get(&id).copied().unwrap_or(0)
    }

    pub fn forget(&self, id: ObjId) {
        self.0.borrow_mut().keys.remove(&id);
    }

    /// `deleteDataBlocks`: the counter is 0 again. No object's key and no connection's key changes.
    pub fn reset(&self) {
        self.0.borrow_mut().counter = 0;
    }

    /// The next key the counter gives.
    pub fn counter(&self) -> i32 {
        self.0.borrow().counter
    }
}

/// `SimDataBlock::onAdd`'s key store and the removal of it.
struct KeyedClass {
    keys: DatablockKeys,
}

impl EngineClass for KeyedClass {
    fn on_add(&self, _vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        self.keys.bump(id);
        Ok(true)
    }

    fn on_remove(&self, _vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.keys.forget(id);
        Ok(())
    }
}

/// A game's rule for a static write to a datablock (`onStaticModified`): true when the game took the write and the key is not
/// bumped.
pub type StaticWriteRule = Rc<dyn Fn(&mut Vm, Option<ObjId>, ClassId) -> bool>;

/// Gives the `Vm` the key behaviour of `SimDataBlock` and the observer of static writes.
pub fn install_keys(vm: &mut Vm, keys: &DatablockKeys) {
    install_keys_with(vm, keys, None);
}

/// [`install_keys`] with a game's rule for the static writes it takes itself.
pub fn install_keys_with(vm: &mut Vm, keys: &DatablockKeys, rule: Option<StaticWriteRule>) {
    vm.set_class_behaviour("SimDataBlock", Rc::new(KeyedClass { keys: keys.clone() }));
    let keys = keys.clone();
    vm.set_static_write_observer(Some(Rc::new(
        move |vm: &mut Vm, id: Option<ObjId>, class: ClassId| {
            // The observer this one replaces (the simulation work: a PlayerData's Players rebuild their products).
            torque_game::sim::player::datablock_written(vm, id, class);
            let classes = &vm.objects().classes;
            if !classes.is_a(class, SIM_DATA_BLOCK) {
                return;
            }
            if rule.as_ref().is_some_and(|rule| rule(vm, id, class)) {
                return;
            }
            match id {
                Some(id) => keys.bump(id),
                // An object still being built takes the key of its add; its field writes only move the counter.
                None => keys.advance(),
            }
        },
    )));
}

// ---- class ids

/// The datablock classes and the events of `codecs`, with the ids the sort rule gives.
pub fn net_classes_of(codecs: &dyn NetCodecs) -> NetClasses {
    let mut reps: Vec<NetRep> = codecs
        .event_classes()
        .into_iter()
        .map(|(name, direction)| NetRep {
            name,
            kind: Kind::Event,
            direction,
        })
        .collect();
    reps.extend(codecs.datablock_classes().into_iter().map(|name| NetRep {
        name: name.to_string(),
        kind: Kind::DataBlock,
        direction: torque_net::classes::Direction::Any,
    }));
    NetClasses::derive(&reps).unwrap_or_else(|_| NetClasses::derive(&[]).expect("empty table"))
}

// ---- the events

/// The state of one `SimDataBlockEvent` a chain posted.
#[derive(Debug)]
pub struct DbCell {
    pub id: ObjId,
    pub index: u32,
    pub total: u32,
    pub seq: i32,
    classes: Rc<NetClasses>,
    state: RefCell<CellState>,
}

#[derive(Debug, Default)]
struct CellState {
    rendered: Option<DatablockEvent>,
    stale: bool,
}

impl DbCell {
    fn new(id: ObjId, index: u32, total: u32, seq: i32, classes: &Rc<NetClasses>) -> Rc<DbCell> {
        Rc::new(DbCell {
            id,
            index,
            total,
            seq,
            classes: Rc::clone(classes),
            state: RefCell::new(CellState::default()),
        })
    }

    fn needs_render(&self) -> bool {
        let s = self.state.borrow();
        s.rendered.is_none() || s.stale
    }

    /// The rendered event, or None for a cell that was never rendered.
    pub fn rendered(&self) -> Option<DatablockEvent> {
        self.state.borrow().rendered.clone()
    }

    /// Writes the event as rendered. A cell that was not (every cell of a connection is rendered before its section is written,
    /// so this cannot happen) and a pack that fails (only an unknown class id, which the fixed class table does not give) write
    /// the one 0 bit.
    pub fn pack(&self, w: &mut torque_net::bits::BitWriter, table: &Table) {
        let st = self.state.borrow();
        let Some(event) = st.rendered.as_ref() else {
            w.write_flag(false);
            return;
        };
        if event.pack(w, table, &self.classes).is_err() {
            w.write_flag(false);
        }
    }
}

/// A [`DbCell`] as a [`HostEvent`] carries it: equal when it is the same cell.
#[derive(Debug, Clone)]
pub struct DbEvent(pub Rc<DbCell>);

impl PartialEq for DbEvent {
    fn eq(&self, other: &DbEvent) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for DbEvent {}

/// An event the datablock chain posts: a cell's `SimDataBlockEvent`, or the datablocks-done `ConnectionMessage` that ends the
/// chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbPost {
    Cell(DbEvent),
    Message(ConnectionMessage),
}

impl<G> From<DbPost> for HostEvent<G> {
    fn from(post: DbPost) -> HostEvent<G> {
        match post {
            DbPost::Cell(cell) => HostEvent::DataBlock(cell),
            DbPost::Message(m) => HostEvent::Net(NetEvent::ConnectionMessage(m)),
        }
    }
}

fn message_event(seq: i32, message: u8) -> DbPost {
    DbPost::Message(ConnectionMessage {
        seq: seq as u32,
        message,
        count: 0,
    })
}

// ---- the connection's chain

#[derive(Debug)]
struct Chain {
    seq: i32,
    /// Made and not yet settled (posted, or held in [`ConnDatablocks::unposted`]).
    cells: Vec<Rc<DbCell>>,
}

/// One connection's datablock state: its key, its max key, its sequence, and the chain.
#[derive(Debug, Default)]
pub struct ConnDatablocks {
    /// The most bits one of its events may take ([`packet_room_bits_with`] of the codecs' head; none: Torque's head).
    pub room_bits: Option<usize>,
    pub key: i32,
    pub max_key: i32,
    pub seq: i32,
    chain: Option<Chain>,
    queued: Option<i32>,
    /// Events the chain made that the connection's ordered queue had no room for yet, in the order they go
    /// ([`post_or_hold`](Self::post_or_hold)). Their cells are in the chain, so the chain does not end while one is held:
    /// nothing is ever lost.
    unposted: VecDeque<DbPost>,
    /// Group positions a queued call looked at when it started at the end of a chain (inside [`settled`](Self::settled), which
    /// has no caller to charge them): taken by [`take_scan`](Self::take_scan) and charged to the send pass as work.
    scan_owed: usize,
    /// The events a chain keeps out ([`NetCodecs::datablock_window`]; none: [`DATABLOCK_QUEUE_COUNT`]).
    window: Option<usize>,
    /// The message that ends a chain ([`NetCodecs::datablocks_done_message`]; none: Torque's `DataBlocksDone`).
    done_message: Option<u8>,
}

/// What a `transmitDataBlocks` call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transmit {
    /// A chain started (or the datablocks-done message was posted at once): the number of events posted.
    Started(usize),
    /// A chain is active: the call waits for its end.
    Queued,
}

fn group(objects: &Objects) -> Vec<ObjId> {
    let group = objects.datablock_group();
    objects
        .members(group)
        .map(|m| (0..m.len()).filter_map(|i| m.get(i)).collect())
        .unwrap_or_default()
}

pub fn group_len(objects: &Objects) -> usize {
    objects
        .members(objects.datablock_group())
        .map_or(0, |m| m.len())
}

fn group_at(objects: &Objects, index: usize) -> Option<ObjId> {
    objects
        .members(objects.datablock_group())
        .and_then(|m| m.get(index))
}

impl ConnDatablocks {
    /// The events its chains keep out (set at the accept, before any chain), at most [`MAX_DATABLOCK_WINDOW`].
    pub fn set_window(&mut self, window: usize) {
        self.window = Some(window.clamp(1, MAX_DATABLOCK_WINDOW));
    }

    /// The events a chain keeps queued or unacked.
    pub fn window(&self) -> usize {
        self.window.unwrap_or(DATABLOCK_QUEUE_COUNT)
    }

    /// The message that ends its chains (set at the accept, before any chain).
    pub fn set_done_message(&mut self, message: u8) {
        self.done_message = Some(message);
    }

    fn done_message(&self) -> u8 {
        self.done_message.unwrap_or(DATABLOCKS_DONE)
    }

    /// Events made and not yet in the ordered queue.
    pub fn unposted(&self) -> usize {
        self.unposted.len()
    }

    /// Posts `events` behind the ones already held, as far as the queue has room, and holds the rest. The pulls an ack makes
    /// and the window a queued call starts are posted this way; a pull into a full queue waits here until
    /// [`flush`](Self::flush) finds room.
    pub fn post_or_hold<G>(&mut self, queue: &mut EventQueue<HostEvent<G>>, events: Vec<DbPost>)
    where
        HostEvent<G>: WireEvent,
    {
        self.unposted.extend(events);
        self.flush(queue);
    }

    /// Posts the held events, in order, while the queue has room. A closed queue (the connection is going) drops them.
    pub fn flush<G>(&mut self, queue: &mut EventQueue<HostEvent<G>>)
    where
        HostEvent<G>: WireEvent,
    {
        while let Some(post) = self.unposted.pop_front() {
            match queue.post(post.clone().into()) {
                Ok(()) => {}
                Err(PostError::Full(_)) => {
                    self.unposted.push_front(post);
                    break;
                }
                Err(PostError::Closed(_)) => {
                    self.unposted.clear();
                    break;
                }
            }
        }
    }

    /// Whether a chain is active.
    pub fn active(&self) -> bool {
        self.chain.is_some()
    }

    /// The call waiting for the chain's end.
    pub fn queued(&self) -> Option<i32> {
        self.queued
    }

    /// The cells posted and not yet settled.
    pub fn in_flight(&self) -> usize {
        self.chain.as_ref().map_or(0, |c| c.cells.len())
    }

    /// `transmitDataBlocks(seq)`: the events to post, and the group positions looked at (for the work charge).
    pub fn call(
        &mut self,
        objects: &Objects,
        keys: &DatablockKeys,
        classes: &Rc<NetClasses>,
        seq: i32,
    ) -> (Transmit, Vec<DbPost>, usize) {
        if self.chain.is_some() {
            self.queued = Some(seq);
            return (Transmit::Queued, Vec::new(), 0);
        }
        self.start(objects, keys, classes, seq)
    }

    fn start(
        &mut self,
        objects: &Objects,
        keys: &DatablockKeys,
        classes: &Rc<NetClasses>,
        seq: i32,
    ) -> (Transmit, Vec<DbPost>, usize) {
        self.seq = seq;
        let members = group(objects);
        let total = members.len();
        let first = members.iter().position(|&id| keys.key_of(id) > self.key);
        let scanned = first.map_or(total, |s| s + 1);
        let Some(first) = first else {
            return (
                Transmit::Started(1),
                vec![message_event(seq, self.done_message())],
                scanned,
            );
        };
        self.max_key = self.key;
        let end = total.min(first + self.window());
        let mut cells = Vec::with_capacity(end - first);
        for (index, &id) in members.iter().enumerate().take(end).skip(first) {
            cells.push(DbCell::new(id, index as u32, total as u32, seq, classes));
        }
        let events = cells
            .iter()
            .map(|c| DbPost::Cell(DbEvent(Rc::clone(c))))
            .collect::<Vec<_>>();
        let n = events.len();
        self.chain = Some(Chain { seq, cells });
        (Transmit::Started(n), events, scanned)
    }

    /// `SimDataBlockEvent::notifyDelivered` for `cell`: the events it posts. Ends the chain when its last cell has settled and
    /// starts the call that waited, if any.
    pub fn settled(
        &mut self,
        objects: &Objects,
        keys: &DatablockKeys,
        classes: &Rc<NetClasses>,
        cell: &Rc<DbCell>,
    ) -> Vec<DbPost> {
        let mut out = Vec::new();
        let Some(chain) = self.chain.as_mut() else {
            return out;
        };
        let Some(at) = chain.cells.iter().position(|c| Rc::ptr_eq(c, cell)) else {
            return out;
        };
        chain.cells.remove(at);
        // A connection whose sequence moved on is ignored; the host never moves it under a chain, so this is the chain's own.
        let seq = chain.seq;
        let size = group_len(objects);
        if size > 0 && cell.index as usize == size - 1 {
            self.key = self.max_key;
            out.push(message_event(seq, self.done_message()));
        }
        let next = (cell.index as usize).saturating_add(self.window());
        if next < size
            && let Some(id) = group_at(objects, next)
        {
            let pulled = DbCell::new(id, next as u32, size as u32, seq, classes);
            out.push(DbPost::Cell(DbEvent(Rc::clone(&pulled))));
            if let Some(chain) = self.chain.as_mut() {
                chain.cells.push(pulled);
            }
        }
        if self.chain.as_ref().is_some_and(|c| c.cells.is_empty()) {
            self.chain = None;
            if let Some(queued) = self.queued.take() {
                let (_, events, scanned) = self.start(objects, keys, classes, queued);
                self.scan_owed = self.scan_owed.saturating_add(scanned);
                out.extend(events);
            }
        }
        out
    }

    /// The group positions scanned by a queued call that started inside [`settled`](Self::settled), once.
    pub fn take_scan(&mut self) -> usize {
        std::mem::take(&mut self.scan_owed)
    }

    /// A packet of the connection was lost: its events are rendered again when they next go out.
    pub fn mark_stale(&mut self) {
        if let Some(chain) = &self.chain {
            for cell in &chain.cells {
                cell.state.borrow_mut().stale = true;
            }
        }
    }

    /// Renders the cells that were never rendered or went stale, as the pack at write time reads them: the datablock is looked
    /// up by id (gone, or not a datablock: the one 0 bit), `flag = conn.key < db.key`, then `max_key = max(max_key, db.key)`,
    /// then the record. Returns the work done in units, or the first event no packet can carry.
    pub fn render(
        &mut self,
        objects: &Objects,
        keys: &DatablockKeys,
        table: &Table,
        codecs: &dyn NetCodecs,
    ) -> Result<u32, RenderError> {
        self.render_with(objects, keys, table, &DbValues::new(), codecs)
            .map(|(units, _)| units)
    }

    /// The datablocks the next [`render_with`](Self::render_with) packs with a body (the others travel as one 0 bit): the host
    /// runs a game's prepack first ([`DbValues::before_pack`]), which needs the `Vm`.
    pub fn packing(
        &self,
        objects: &Objects,
        keys: &DatablockKeys,
        codecs: &dyn NetCodecs,
    ) -> Vec<ObjId> {
        let Some(chain) = self.chain.as_ref() else {
            return Vec::new();
        };
        chain
            .cells
            .iter()
            .filter(|c| c.needs_render())
            .filter(|c| {
                objects.get(c.id).is_some_and(|o| {
                    objects.classes.is_a(o.class(), SIM_DATA_BLOCK)
                        && self.key < keys.key_of(c.id)
                        && datablock_class(objects, o, codecs).is_some()
                })
            })
            .map(|c| c.id)
            .collect()
    }

    /// [`render`](Self::render) with the host's computed values; also the records rendered, for the codecs' after-pack step
    /// ([`NetCodecs::datablock_packed`]).
    pub fn render_with(
        &mut self,
        objects: &Objects,
        keys: &DatablockKeys,
        table: &Table,
        values: &DbValues,
        codecs: &dyn NetCodecs,
    ) -> Result<(u32, Packed), RenderError> {
        let Some(chain) = self.chain.as_ref() else {
            return Ok((0, Vec::new()));
        };
        let todo: Vec<Rc<DbCell>> = chain
            .cells
            .iter()
            .filter(|c| c.needs_render())
            .cloned()
            .collect();
        let mut units = 0u32;
        let mut packed = Vec::new();
        for cell in &todo {
            let (event, entries, flagged) = self.render_one(objects, keys, cell, values, codecs);
            if let Some(body) = &event.body {
                packed.push((cell.id, Rc::clone(&body.record)));
            }
            if let Some(key) = flagged {
                self.max_key = self.max_key.max(key);
            }
            let room = self
                .room_bits
                .unwrap_or_else(|| packet_room_bits_with(torque_net::control::HEAD_MIN_BITS));
            let bits = event_refusal_in(&event, table, &cell.classes, room);
            units = units.saturating_add(1).saturating_add(
                u32::try_from(entries as u64 / NET_DATABLOCK_ENTRIES_PER_UNIT).unwrap_or(u32::MAX),
            );
            if let Err(error) = bits {
                return Err(RenderError {
                    id: cell.id,
                    class: object_class_name(objects, cell.id),
                    error,
                    units,
                });
            }
            let mut st = cell.state.borrow_mut();
            st.rendered = Some(event);
            st.stale = false;
        }
        Ok((units, packed))
    }

    fn render_one(
        &self,
        objects: &Objects,
        keys: &DatablockKeys,
        cell: &DbCell,
        values: &DbValues,
        codecs: &dyn NetCodecs,
    ) -> (DatablockEvent, usize, Option<i32>) {
        let id_bits = codecs.datablock_id_bits();
        let none = (
            DatablockEvent {
                body: None,
                id_bits,
            },
            0,
            None,
        );
        let Some(object) = objects.get(cell.id) else {
            return none;
        };
        if !objects.classes.is_a(object.class(), SIM_DATA_BLOCK) {
            return none;
        }
        let key = keys.key_of(cell.id);
        if self.key >= key {
            return none;
        }
        let Some(class) = datablock_class(objects, object, codecs) else {
            return none;
        };
        let view = ObjectView {
            objects,
            object,
            values,
            affine: codecs.affine_fn(),
        };
        let Some(record) = codecs.datablock_record(class, cell.id, &view, values) else {
            return none;
        };
        let entries = record.entries();
        let body = DatablockBody::new(cell.id, class, cell.index, cell.total, id_bits, record);
        (
            DatablockEvent {
                body: Some(body),
                id_bits,
            },
            entries,
            Some(key),
        )
    }
}

/// The datablocks a render packed with a body, and their records (for [`NetCodecs::datablock_packed`]).
pub type Packed = Vec<(ObjId, Rc<dyn DatablockRecord>)>;

/// An event no packet can carry.
#[derive(Debug)]
pub struct RenderError {
    pub id: ObjId,
    pub class: String,
    pub error: DbPackError,
    pub units: u32,
}

impl RenderError {
    /// The console line.
    pub fn line(&self, client: ObjId) -> String {
        let why = match &self.error {
            DbPackError::TooBig { bits, limit } => {
                format!("it is {bits} bits and one packet carries {limit}")
            }
            other => other.to_string(),
        };
        format!(
            "engine: client {client} dropped: datablock {} ({}) cannot be sent: {why}",
            self.id, self.class
        )
    }
}

fn object_class_name(objects: &Objects, id: ObjId) -> String {
    objects.get(id).map_or_else(String::new, |o| {
        String::from_utf8_lossy(objects.classes.name(o.class())).into_owned()
    })
}

/// The networked datablock class of an object as the codecs name it (None for a class of the table they do not send).
pub fn datablock_class(
    objects: &Objects,
    object: &SimObject,
    codecs: &dyn NetCodecs,
) -> Option<&'static str> {
    std::str::from_utf8(objects.classes.name(object.class()))
        .ok()
        .and_then(|name| codecs.datablock_class(name))
}

// ---- the view

/// The names the packers read that the class table has no static field for and no computed value stands behind:
/// `AudioSampleEnvironment.roomHF` (the constructor zero-fills it), `ProjectileData.faceViewer` (the constructor sets 0) and
/// `stateScaleAnimation` (every element 1; also in the records, for an image that has them).
/// Anything else unknown is null.
fn special(field: &str, _index: usize) -> DbValue {
    match field {
        "roomHF" => DbValue::Int(0),
        "faceViewer" => DbValue::Bool(false),
        "stateScaleAnimation" => DbValue::Bool(true),
        _ => DbValue::Null,
    }
}

/// The components of one element of a field type (a flat index is `element * width + component`).
fn width(ty: FieldType) -> usize {
    match ty {
        FieldType::Point3F => 3,
        FieldType::ColorF | FieldType::ColorI => 4,
        FieldType::Point2F | FieldType::Point2I => 2,
        _ => 1,
    }
}

/// The host's getters for one datablock object.
pub struct ObjectView<'a> {
    pub objects: &'a Objects,
    pub object: &'a SimObject,
    /// The values computed at add and preload time (the emitter's ids, the image's state records).
    pub values: &'a DbValues,
    /// The host's matrix-to-transform conversion ([`NetCodecs::affine_fn`]).
    pub affine: AffineFn,
}

impl ObjectView<'_> {
    fn is_datablock(&self, id: ObjId) -> bool {
        self.objects
            .get(id)
            .is_some_and(|o| self.objects.classes.is_a(o.class(), SIM_DATA_BLOCK))
    }

    /// A datablock pointer as the packer reads it: the id, or none for an object that is gone or is no datablock.
    fn pointer(&self, target: Option<ObjId>) -> DbValue {
        DbValue::Id(target.filter(|&id| self.is_datablock(id)))
    }

    /// The values the host computed (not static fields, or static fields a packer reads from its own record): `None` when the
    /// object has no such value, and the static field answers.
    fn computed(&self, field: &str, index: usize) -> Option<DbValue> {
        let id = self.object.id();
        match field {
            "dataBlockIds" => {
                let ids = self.values.emitter_ids(id);
                Some(
                    ids.get(index)
                        .map_or(DbValue::Null, |&i| DbValue::Int(i64::from(i))),
                )
            }
            "dataBlockIdCount" => Some(DbValue::Int(self.values.emitter_ids(id).len() as i64)),
            _ => self.values.with_image(id, |image| {
                if field == "hasFlash" {
                    return Some(DbValue::Bool(image.has_flash));
                }
                // `stateTrans` is 11 values to a state, every other array one.
                let (state, k) = if field == "stateTrans" {
                    (index / 11, index % 11)
                } else {
                    (index, 0)
                };
                let s = image.states.get(state)?;
                Some(match field {
                    "stateName" => s
                        .name
                        .as_ref()
                        .map_or(DbValue::Null, |n| DbValue::Str(n.to_vec())),
                    "stateTrans" => DbValue::Int(i64::from(s.trans[k])),
                    "stateTimeoutValue" => DbValue::F32(s.timeout),
                    "stateWaitForTimeout" => DbValue::Bool(s.wait_for_timeout),
                    "stateFire" => DbValue::Bool(s.fire),
                    "stateEjectShell" => DbValue::Bool(s.eject_shell),
                    // The constructor sets every element; no field of the table holds it.
                    "stateScaleAnimation" => DbValue::Bool(true),
                    "stateDirection" => DbValue::Bool(s.direction),
                    "stateEnergyDrain" => DbValue::F32(s.energy_drain),
                    "stateLoadedFlag" => DbValue::Int(i64::from(s.loaded_flag)),
                    "stateSpinThread" => DbValue::Int(i64::from(s.spin_thread)),
                    "stateRecoil" => DbValue::Int(i64::from(s.recoil)),
                    "stateSequence" => DbValue::Int(i64::from(s.sequence)),
                    "stateSequenceVis" => DbValue::Int(i64::from(s.sequence_vis)),
                    "stateFlashSequence" => DbValue::Bool(s.flash),
                    "stateIgnoreLoadedForReady" => DbValue::Bool(s.ignore_loaded),
                    "stateEmitter" => self.pointer(s.emitter),
                    "stateEmitterTime" => DbValue::F32(s.emitter_time),
                    "stateEmitterNode" => DbValue::Int(i64::from(s.emitter_node)),
                    "stateSound" => self.pointer(s.sound),
                    _ => return None,
                })
            })?,
        }
    }

    /// Whether a text slot holds the text it started with and that was the empty one: a field never assigned (a NULL pointer).
    fn unset_text(&self, class: ClassId, slot: usize, now: &std::rc::Rc<[u8]>) -> bool {
        self.objects
            .classes
            .get(class)
            .and_then(|c| c.layout.initial.get(slot))
            .is_some_and(|first| match first {
                FieldValue::Text(first) => first.is_empty() && Rc::ptr_eq(now, first),
                _ => false,
            })
    }
}

impl DatablockView for ObjectView<'_> {
    fn get(&self, field: &str, index: usize) -> DbValue {
        if let Some(value) = self.computed(field, index) {
            return value;
        }
        let class = self.object.class();
        // `TSShapeConstructor` registers its sequence list as 127 fields `sequence0`..`sequence126` (one element each).
        let (name, index) = match field {
            "sequence" => (format!("sequence{index}"), 0),
            _ => (field.to_string(), index),
        };
        let Some((spec, slots)) = self.objects.classes.static_field(class, name.as_bytes()) else {
            return special(field, index);
        };
        let w = width(spec.ty);
        let (element, component) = (index / w, index % w);
        let Some(&slot) = slots.get(element) else {
            return DbValue::Null;
        };
        match self.object.slot(slot) {
            Some(FieldValue::Text(t)) => {
                if self.unset_text(class, slot, t) {
                    DbValue::Null
                } else {
                    DbValue::Str(t.to_vec())
                }
            }
            Some(FieldValue::Int(i)) => DbValue::Int(i64::from(*i)),
            Some(FieldValue::Float(f)) => DbValue::F32(*f),
            Some(FieldValue::Bool(b)) => DbValue::Bool(*b),
            Some(FieldValue::ColorI(c)) => c
                .get(component)
                .map_or(DbValue::Null, |&b| DbValue::Int(i64::from(b))),
            Some(FieldValue::ColorF(c)) => {
                c.get(component).map_or(DbValue::Null, |&f| DbValue::F32(f))
            }
            Some(FieldValue::Ints2(p)) => p
                .get(component)
                .map_or(DbValue::Null, |&i| DbValue::Int(i64::from(i))),
            Some(FieldValue::Floats2(p)) => {
                p.get(component).map_or(DbValue::Null, |&f| DbValue::F32(f))
            }
            Some(FieldValue::Floats3(p)) => {
                p.get(component).map_or(DbValue::Null, |&f| DbValue::F32(f))
            }
            Some(FieldValue::Matrix(m)) => DbValue::Xform((self.affine)(m)),
            Some(FieldValue::Object(Some(id))) => {
                DbValue::Id(self.is_datablock(*id).then_some(*id))
            }
            Some(FieldValue::Object(None)) => DbValue::Id(None),
            Some(FieldValue::IntList(_) | FieldValue::FloatList(_)) | None => DbValue::Null,
        }
    }
}

/// How a host turns a matrix field into the transform a record writes (`writeAffineTransform`): `None` is the identity.
pub type AffineFn = fn(&Mat4) -> Option<Affine>;

/// TGE's: the identity is `None`; any other matrix is its translation column and its rotation as a `QuatF` built from the
/// matrix and normalized (`QuatF::normalize`: each component divided by the length; a zero length gives the identity rotation).
pub fn tge_affine(m: &Mat4) -> Option<Affine> {
    if *m == torque_ts_vm::fields::IDENTITY {
        return None;
    }
    let q = torque_ts_vm::geometry::quat_from_matrix(m);
    let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let quaternion = if length == 0.0 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        q.map(|c| c / length)
    };
    Some(Affine {
        position: [m[0][3], m[1][3], m[2][3]],
        quaternion,
    })
}
