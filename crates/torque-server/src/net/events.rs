//! The event layer of the host: the [`PayloadLayer`] that carries `torque_net::events` over each connection's data
//! packets, the NetString tables with their references on the VM's tag table, the client's remote commands as
//! [`Followup::ServerCmd`], and the sender the `commandTo*` natives post through ([`EventLayer::post_command`]).
//!
//! | piece | Torque |
//! |---|---|
//! | the server-to-client head: move ack, four clear flags, then events, then the ghost flag | `GameConnection::writePacket` |
//! | the client-to-server head: camera flag, control checksum, move start and count, the moves, the view | `readPacket` |
//! | events | `eventWritePacket`, `eventReadPacket` (the `torque_net::events` crate) |
//! | `NetStringEvent::process` | the receiver's slot is written here and nowhere else |
//! | `RemoteCommandEvent::process` | the client's commands as followups |
//! | the connection's destructor releasing the tag references | [`PayloadLayer::on_drop`] |
//!
//! **The events are [`HostEvent`]s**: `net`'s `NetEvent`, Torque's small events, the datablock and ghost-always events and the
//! game's own in one type, so the one ordered queue and the one receiver carry them in order. The game's half of the layer
//! ([`GameEvents`]) keeps a state of its own for the layer and for each connection: what a client's game event does, what
//! posting a game event sets off, and the events a settled one pulls (they wait in the connection's held list while the ordered
//! queue has no room).
//!
//! **One state, borrowed briefly.** The layer, the natives and the tests share one `Rc<RefCell<EventState>>`. No method here
//! runs script, and none of them holds the borrow while it calls out; the natives take the network host's state first, let go
//! of it, then this one, and the layer never takes the host's state (so no order can deadlock).
//!
//! **Tag references (`checkString` / the table's destructor).** A sender entry owns one engine reference on the tag it names
//! (taken when the entry is re-keyed, released when it is re-keyed again or the connection is destroyed); a receiver handle
//! owns one on the tag its NetString event added. All are released in [`PayloadLayer::on_drop`], which the host calls from the
//! connection's `onRemove`. The NetString event's own transient reference (the tag held while the event is queued) is not
//! modelled: the text is copied into the event.
//!
//! **The host's limits.** A command that cannot be queued because the connection's ordered queue is full is a protocol error of
//! the connection ([`NET_QUEUE_FULL_REASON`]: the queues are bounded here). An event no packet could ever carry drops the peer
//! with `Packet Error.`; before the client's first packet such an event stalls instead. The datablocks-download-done message
//! ([`NetCodecs::datablocks_download_done_message`]: Torque's 7) runs `onDataBlocksDone`, or `onDataBlocksUpdated(count)`
//! when its sequence is a live round's (the datablock chain, [`super::datablocks`]), and message 1 is the ghost layer's ([`super::ghosts`]; the manager checks its sequence and the
//! script `onGhostAlwaysObjectsReceived` runs as a followup after the packet's events); every message the codecs do not reject
//! goes to a [`MessageHook`] too (tests); the ones they reject from a client ([`NetCodecs::rejects_client_message`]: Torque's
//! 0, 2, 3, 4 and 5, only a ghosting side sends them) are a `Packet Error.`.
//!
//! Credit: the layer follows TGE as Torque3D, its MIT-licensed release, keeps it: `sim/netEvent.cpp`, `sim/netConnection.cpp`,
//! `sim/netStringTable.cpp`, `sim/connectionStringTable.cpp` (`NetStringEvent`) and `app/net/net.cpp` (`RemoteCommandEvent`)
//! (`Copyright (c) 2012 GarageGames, LLC`), and `T3D/gameBase/gameConnection.cpp` and `T3D/player.cpp` (each `Copyright (c)
//! 2012 GarageGames, LLC`, `Copyright (C) 2015 Faust Logic, Inc.`).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use super::codecs::NetCodecs;
use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::control::ControlState;
use torque_net::events::{
    ConnectionMessage, EventCtx, EventIds, EventQueue, EventReceiver, Incoming, NetEvent,
    PostError, SectionParams, Settled,
};
use torque_net::ghost::manager::SECTION_MIN_BITS;
use torque_net::handshake::ConnId;
use torque_net::netstring::{RemoteStrings, SendStrings, mapping_line};
use torque_net::protocol::{Header, Notify, RatePrefix, Received, Role};
use torque_net::remote::body::{BodyError, CommandBody, encode_command};
use torque_net::remote::{SendError, Sent, TagLookup, function_name, resolve, send_command_with};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::datablocks::{
    ConnDatablocks, DatablockKeys, Transmit, net_classes_of, packet_room_bits_with,
};
use super::ghosts::{GhostCall, GhostGame, GhostHub, PackStrings, SourceEnv};
use super::hostevent::{GameEvents, HostEvent, HostUnpacker, ReceiveCx};
use super::payload::{ConnCtx, Followup, PacketOutcome, PayloadLayer};
use super::peer::PeerKind;
use torque_game::dbvalues::DbValues;

/// The reason a connection is dropped with when a command cannot be queued for it: its ordered queue is full
/// ([`torque_net::cap::NET_EVENT_QUEUE_ORDERED`] events the peer has not acknowledged).
pub const NET_QUEUE_FULL_REASON: &[u8] = b"Overflow.";
/// The console line printed with that drop (`{}` is the connection's object id).
pub fn queue_full_line(obj: ObjId) -> String {
    format!("engine: client {obj} dropped: its send queue is full")
}

/// Handles the `ConnectionMessage`s that are not a protocol error from a client (the layer itself runs the ghost and datablock
/// steps first).
pub trait MessageHook {
    /// A client's `ConnectionMessage` the codecs do not reject. May ask for a followup (a script call).
    fn on_message(
        &mut self,
        conn: ConnId,
        obj: ObjId,
        message: &ConnectionMessage,
    ) -> Option<Followup>;
}

/// The default: ignore (the base `handleConnectionMessage` does nothing more with them).
#[derive(Debug, Default, Clone, Copy)]
pub struct IgnoreMessages;

impl MessageHook for IgnoreMessages {
    fn on_message(&mut self, _: ConnId, _: ObjId, _: &ConnectionMessage) -> Option<Followup> {
        None
    }
}

/// One connection's event state.
struct ConnEvents<H: GameEvents> {
    obj: ObjId,
    queue: EventQueue<HostEvent<H::Event>>,
    receiver: EventReceiver<HostEvent<H::Event>>,
    send: SendStrings,
    remote: RemoteStrings,
    /// The tags of the commands that ride the engine protocol's RPCs: their own tables, so a command never changes what a ghost
    /// record's strings mean ([`EventLayer::encode_command`], [`EventLayer::decode_command`]).
    command_send: SendStrings,
    command_remote: RemoteStrings,
    /// The control state of the `GameConnection`: the move list and its ack, the move credit, the client's checksum, the field
    /// of view.
    control: ControlState,
    /// The client has sent a data packet (its rate pair has been read: the 200-byte initial rate is over).
    spoke: bool,
    /// The game's state of the connection.
    game: H::Conn,
    /// The datablock chain ([`super::datablocks`]).
    datablocks: ConnDatablocks,
    /// Game events a settled one pulled while the ordered queue had no room (a full queue is normal while the datablock chain
    /// holds events, and the queue's room counts only the unsent): they wait here, in order, until
    /// [`flush_held`](EventLayer::flush_held) finds room, and are never lost (a game bounds them by pulling only as its own
    /// events settle).
    held: VecDeque<HostEvent<H::Event>>,
    /// Work units the connection's renders used since the send pass last took them ([`PayloadLayer::take_work`]).
    work: u32,
}

struct EventState<H: GameEvents, S: GhostGame> {
    conns: HashMap<ConnId, ConnEvents<H>>,
    ids: Option<EventIds>,
    hook: Box<dyn MessageHook>,
    /// The game's state of the layer.
    game: H,
    /// The datablocks' `modifiedKey`s (shared with the `Vm`'s behaviour and observer).
    keys: DatablockKeys,
    /// The values the datablocks compute at add and preload time (shared with the host's behaviours).
    values: DbValues,
    /// The class ids of the datablock events (the sort rule over the codecs' datablock and event class names).
    classes: Rc<torque_net::classes::NetClasses>,
    /// The ghost hub: every connection's ghost manager lives there.
    hub: Option<GhostHub<S>>,
    /// The game's codecs (the event ids, the control format, the command classes).
    codecs: Rc<dyn NetCodecs>,
}

/// The layer with the game's events `H`. Cheap to clone; clones share the state.
pub struct EventLayer<H: GameEvents, S: GhostGame> {
    state: Rc<RefCell<EventState<H, S>>>,
}

impl<H: GameEvents, S: GhostGame> Clone for EventLayer<H, S> {
    fn clone(&self) -> EventLayer<H, S> {
        EventLayer {
            state: Rc::clone(&self.state),
        }
    }
}

/// One connection as the game's half of the layer sees it ([`EventLayer::with_game`]).
pub struct GameConn<'a, H: GameEvents> {
    pub conn: ConnId,
    /// The connection's object.
    pub obj: ObjId,
    /// The game's state of the connection.
    pub game: &'a mut H::Conn,
    queue: &'a mut EventQueue<HostEvent<H::Event>>,
}

impl<H: GameEvents> GameConn<'_, H> {
    /// The connection is going: its queue takes nothing.
    pub fn is_closed(&self) -> bool {
        self.queue.is_closed()
    }

    /// The ordered events the queue takes now.
    pub fn room_ordered(&self) -> usize {
        self.queue.room_ordered()
    }

    /// Posts `events` on the ordered queue, all or none, in order, and tells the game of each of its own
    /// ([`GameEvents::on_posted`]); false when the queue had no room for all of them.
    pub fn post_batch(&mut self, events: Vec<HostEvent<H::Event>>) -> bool {
        let posted: Vec<H::Event> = events
            .iter()
            .filter_map(|e| match e {
                HostEvent::Game(g) => Some(g.clone()),
                _ => None,
            })
            .collect();
        if self.queue.post_batch(events).is_err() {
            return false;
        }
        for e in &posted {
            H::on_posted(self.game, e);
        }
        true
    }
}

/// The tag table as the sender's `validateSendString` sees it.
struct VmTags<'a>(&'a Vm);

impl TagLookup for VmTags<'_> {
    fn tag_text(&self, id: u32) -> Option<Vec<u8>> {
        self.0.tags().lookup(id).map(<[u8]>::to_vec)
    }

    fn table_size(&self) -> u32 {
        self.0.tags().size()
    }
}

/// The fewest bits of a data packet before its event section: the header with no ack bytes (25), an empty rate prefix (2) and
/// the server head with nothing set (`head_bits`, [`NetCodecs::head_min_bits`]).
pub fn min_start_bits_with(head_bits: usize) -> usize {
    let mut w = BitWriter::new();
    Header {
        connect_bit: false,
        seq: 0,
        ack: 0,
        ptype: torque_net::protocol::PacketType::Data,
        ack_bytes: 0,
        ack_mask: 0,
    }
    .write(&mut w);
    RatePrefix::default().write(&mut w);
    w.bits() + head_bits
}

impl<H: GameEvents, S: GhostGame> EventLayer<H, S> {
    /// A layer with a game's codecs.
    pub fn with_codecs(codecs: Rc<dyn NetCodecs>) -> EventLayer<H, S> {
        EventLayer {
            state: Rc::new(RefCell::new(EventState {
                conns: HashMap::new(),
                ids: codecs.event_ids(),
                hook: Box::new(IgnoreMessages),
                game: H::default(),
                keys: DatablockKeys::new(),
                values: DbValues::new(),
                classes: Rc::new(net_classes_of(&*codecs)),
                hub: None,
                codecs,
            })),
        }
    }

    /// Gives the layer the ghost hub: connections accepted from now on have a ghost manager, and the packets carry the ghost
    /// section.
    pub fn set_hub(&self, hub: GhostHub<S>) {
        self.state.borrow_mut().hub = Some(hub);
    }

    /// Replaces the event ids (a game's own table).
    pub fn set_ids(&self, ids: EventIds) {
        self.state.borrow_mut().ids = Some(ids);
    }

    pub fn ids(&self) -> Option<EventIds> {
        self.state.borrow().ids.clone()
    }

    /// Replaces the handler of the `ConnectionMessage`s the layer does not own.
    pub fn set_hook(&self, hook: Box<dyn MessageHook>) {
        self.state.borrow_mut().hook = hook;
    }

    /// Whether the layer holds state for `conn`.
    pub fn has(&self, conn: ConnId) -> bool {
        self.state.borrow().conns.contains_key(&conn)
    }

    /// Ordered and unordered events waiting to be sent, and packets waiting for an ack.
    pub fn queued(&self, conn: ConnId) -> Option<(usize, usize, usize)> {
        let st = self.state.borrow();
        let c = st.conns.get(&conn)?;
        Some((
            c.queue.ordered_len(),
            c.queue.unordered_len(),
            c.queue.packets_in_flight(),
        ))
    }

    /// The ordered events the receiver holds out of sequence (its wait list, bounded by `NET_WAIT_LIST`).
    pub fn waiting(&self, conn: ConnId) -> Option<usize> {
        let st = self.state.borrow();
        Some(st.conns.get(&conn)?.receiver.waiting())
    }

    /// The tags the connection's tables hold references on: the sender's entries and the receiver's handles.
    pub fn held_tags(&self, conn: ConnId) -> Option<(usize, usize)> {
        let st = self.state.borrow();
        let c = st.conns.get(&conn)?;
        let sender = (0..32u8).filter(|&s| c.send.tag_of(s).is_some()).count();
        Some((sender, c.remote.mapped()))
    }

    /// `sendRemoteCommand` for one connection: queues the NetString events of the command's new tags, then the command, and
    /// takes the references the table now holds. `args` are the script's strings (a tag is `\x01<id>`), the first being the
    /// command's tag.
    pub fn post_command(
        &self,
        vm: &mut Vm,
        conn: ConnId,
        secure: bool,
        args: &[Vec<u8>],
    ) -> Result<Sent, SendError> {
        let mut st = self.state.borrow_mut();
        let variant = st.codecs.command_variant(secure);
        let c = st.conns.get_mut(&conn).ok_or(SendError::Closed)?;
        // `send_command` posts on a `net` queue of `NetEvent`s: it fills a scratch one (checking its own room, which is the
        // whole cap), and the events it made go on the connection's queue in order, all or none, after the connection's room is
        // checked.
        if c.queue.is_closed() {
            return Err(SendError::Closed);
        }
        // `send_command` runs on a copy of the sender's string table and a scratch queue: its own checks (a bad command name,
        // too many arguments, not a tag) are silent errors whatever the queue holds, and only a command that passes them needs
        // room on the connection's queue. The table is replaced by the copy once the events are taken, so a refused command
        // leaves it as it was.
        let mut trial = c.send.clone();
        let mut scratch: EventQueue<NetEvent> = EventQueue::new();
        let sent = send_command_with(
            &mut scratch,
            &mut trial,
            &VmTags(vm),
            args,
            NetEvent::NetString,
            |mut command| {
                command.variant = variant;
                NetEvent::Remote(command)
            },
        )?;
        let events: Vec<HostEvent<H::Event>> = scratch
            .release_all()
            .into_iter()
            .map(|s| HostEvent::Net(s.event))
            .collect();
        if events.len() > c.queue.room_ordered() {
            return Err(SendError::Backpressure);
        }
        c.send = trial;
        let _ = c.queue.post_batch(events);
        for &tag in &sent.acquired {
            let text = vm.tags().lookup(tag).map(<[u8]>::to_vec);
            if let Some(text) = text {
                let _ = vm.tags_mut().add_string(&text);
            }
        }
        for &tag in &sent.released {
            release(vm, tag);
        }
        Ok(sent)
    }

    /// The body of `sendRemoteCommand` for one connection, for the engine protocol, where a command is an RPC and no event is
    /// queued: the same checks, the same slots in the connection's own table of command tags, and the tags' texts inside the
    /// body ([`CommandBody`]). The tables hold no reference on the tags: a text is read as it is sent.
    ///
    /// # Errors
    /// [`BodyError::Send`] for a command `sendRemoteCommand` refuses or a connection that is closing, [`BodyError::TooLarge`] for
    /// one whose body would not fit an RPC.
    pub fn encode_command(
        &self,
        vm: &Vm,
        conn: ConnId,
        secure: bool,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, BodyError> {
        let mut st = self.state.borrow_mut();
        let variant = st.codecs.command_variant(secure);
        let c = st
            .conns
            .get_mut(&conn)
            .filter(|c| !c.queue.is_closed())
            .ok_or(BodyError::Send(SendError::Closed))?;
        let mut trial = c.command_send.clone();
        let body = encode_command(&mut trial, &VmTags(vm), variant, args)?;
        c.command_send = trial;
        Ok(body)
    }

    /// A client's remote command that arrived as a body (`RemoteCommandEvent::process` and the NetString events before it): the
    /// tags it names are mapped into the connection's table of command tags, and the command, expanded, comes back as the
    /// followup that calls `serverCmd<name>`. `None` when the command is not a tag (nothing happens, silently) or the connection
    /// is gone.
    ///
    /// # Errors
    /// A [`NetError`] when the body is not one a sender of this engine makes, or the tag table is full: the host drops the client.
    pub fn decode_command(
        &self,
        vm: &mut Vm,
        conn: ConnId,
        body: &[u8],
    ) -> Result<Option<Followup>, NetError> {
        let command = CommandBody::decode(body)?;
        let mut guard = self.state.borrow_mut();
        let st = &mut *guard;
        let Some(c) = st.conns.get_mut(&conn) else {
            return Ok(None);
        };
        for (slot, text) in &command.strings {
            let id = vm
                .tags_mut()
                .add_string(text)
                .ok_or(NetError::Invalid("tag table full"))?;
            if let Some(old) = c.command_remote.set(*slot, id, text) {
                release(vm, old);
            }
        }
        let prefix = st.codecs.command_prefix(command.variant, true);
        Ok(
            resolve(&command.script_args(), &c.command_remote).map(|r| Followup::ServerCmd {
                obj: c.obj,
                name: function_name(prefix, &r.name),
                args: r.args,
            }),
        )
    }

    /// The sender table comes back from a packet's packs (`packNetStringHandleU`): the NetString events its misses made go on
    /// the ordered queue in order (each rides a later packet than the record that sent the text), and the references the
    /// re-keyed entries took and let go of are applied to the tag table as [`EventLayer::post_command`] applies a command's. An
    /// event the queue cannot take (full or closing) is dropped: the entry then stays unacknowledged and the name keeps going
    /// as text.
    fn strings_packed(vm: &mut Vm, c: &mut ConnEvents<H>, strings: PackStrings) {
        c.send = strings.send;
        for event in strings.events {
            let _ = c.queue.post(HostEvent::Net(NetEvent::NetString(event)));
        }
        for tag in strings.acquired {
            let text = vm.tags().lookup(tag).map(<[u8]>::to_vec);
            if let Some(text) = text {
                let _ = vm.tags_mut().add_string(&text);
            }
        }
        for tag in strings.released {
            release(vm, tag);
        }
    }

    fn settle(c: &mut ConnEvents<H>, settled: Vec<Settled<HostEvent<H::Event>>>) {
        let mut pulled = Vec::new();
        for s in settled {
            s.event.on_settled(s.delivered, &mut c.send);
            if let HostEvent::Game(e) = &s.event
                && let Some(next) = H::on_settled(&mut c.game, e, s.delivered)
            {
                pulled.push(HostEvent::Game(next));
            }
        }
        // A pull goes behind any earlier one that is still held, then as many as the queue takes go now.
        c.held.extend(pulled);
        Self::flush_held(c);
    }

    /// Posts the held events, in order, while the ordered queue has room. A closed queue (the connection is going) drops them.
    fn flush_held(c: &mut ConnEvents<H>) {
        while let Some(event) = c.held.pop_front() {
            match c.queue.post(event) {
                Ok(()) => {}
                Err(PostError::Full(event)) => {
                    c.held.push_front(event);
                    break;
                }
                Err(PostError::Closed(_)) => {
                    c.held.clear();
                    break;
                }
            }
        }
    }

    /// Posts `events` on the connection's ordered queue, all or none, in order; the game hears of each of its own
    /// ([`GameEvents::on_posted`]).
    pub fn post_events(
        &self,
        conn: ConnId,
        events: Vec<HostEvent<H::Event>>,
    ) -> Result<(), SendError> {
        let mut st = self.state.borrow_mut();
        let c = st.conns.get_mut(&conn).ok_or(SendError::Closed)?;
        if c.queue.is_closed() {
            return Err(SendError::Closed);
        }
        let mut game = GameConn::<H> {
            conn,
            obj: c.obj,
            game: &mut c.game,
            queue: &mut c.queue,
        };
        if game.post_batch(events) {
            Ok(())
        } else {
            Err(SendError::Backpressure)
        }
    }

    /// Runs `f` on the game's state of the layer and of every connection.
    pub fn with_game<R>(&self, f: impl FnOnce(&mut H, &mut [GameConn<'_, H>]) -> R) -> R {
        let mut guard = self.state.borrow_mut();
        let st = &mut *guard;
        let mut conns: Vec<GameConn<'_, H>> = st
            .conns
            .iter_mut()
            .map(|(id, c)| GameConn {
                conn: *id,
                obj: c.obj,
                game: &mut c.game,
                queue: &mut c.queue,
            })
            .collect();
        f(&mut st.game, &mut conns)
    }

    /// Game events a settle pulled that wait for room in `conn`'s ordered queue.
    pub fn held(&self, conn: ConnId) -> Option<usize> {
        Some(self.state.borrow().conns.get(&conn)?.held.len())
    }

    /// Runs `f` on the connection's control state (the move list, the credit, the field of view); None when the layer has no
    /// such connection.
    pub fn with_control<R>(
        &self,
        conn: ConnId,
        f: impl FnOnce(&mut ControlState) -> R,
    ) -> Option<R> {
        let mut st = self.state.borrow_mut();
        Some(f(&mut st.conns.get_mut(&conn)?.control))
    }

    /// The move bookkeeping of one server tick batch of `steps` 32 ms steps with no `processTick` between (the server tick and
    /// the move credit pass; the process list's pass, [`NetControl`](super::pump::NetControl), runs the object's `processTick`
    /// between [`take_moves`](Self::take_moves) and [`finish_moves`](Self::finish_moves)): a connection with a control object
    /// has its moves taken and cleared for each step; the credit is raised once after the batch, capped at 30, and a connection
    /// with no control object has its list emptied. Returns the moves consumed.
    pub fn tick_moves(&self, conn: ConnId, steps: u32, has_control: bool) -> usize {
        let mut consumed = 0;
        if has_control {
            for _ in 0..steps {
                let taken = self.take_moves(conn);
                consumed += taken.len();
                self.finish_moves(conn, taken.len());
            }
        }
        self.end_batch(conn, steps, has_control);
        consumed
    }

    /// `getMoveList` for one step: `n = min(size, credit)`, the credit drops by `n` and the list is cut to its first `n`.
    /// Returns those moves for the control object's `processTick`.
    pub fn take_moves(&self, conn: ConnId) -> Vec<torque_net::moves::Move> {
        self.with_control(conn, |cs| {
            let mut credit = cs.credit;
            let taken = cs.moves.take_for_tick(&mut credit);
            cs.credit = credit;
            taken
        })
        .unwrap_or_default()
    }

    /// `clearMoves(processed)`: the moves `processTick` ran on leave the list.
    pub fn finish_moves(&self, conn: ConnId, processed: usize) {
        self.with_control(conn, |cs| cs.moves.clear_moves(processed));
    }

    /// The pass after a batch of `steps` steps: the credit rises and empties the list of a connection with no control
    /// object only when a step ran.
    pub fn end_batch(&self, conn: ConnId, steps: u32, has_control: bool) {
        self.with_control(conn, |cs| {
            if steps > 0 {
                cs.add_credit(steps);
                if !has_control {
                    cs.moves.clear();
                }
            }
        });
    }

    /// The datablocks' keys.
    pub fn keys(&self) -> DatablockKeys {
        self.state.borrow().keys.clone()
    }

    /// The computed values the packers read.
    pub fn values(&self) -> DbValues {
        self.state.borrow().values.clone()
    }

    /// Shares the host's computed values with the layer.
    pub fn set_values(&self, values: DbValues) {
        self.state.borrow_mut().values = values;
    }

    /// A connection's datablock state: (key, max key, sequence, events in flight, the queued call).
    pub fn datablock_state(&self, conn: ConnId) -> Option<(i32, i32, i32, usize, Option<i32>)> {
        let st = self.state.borrow();
        let d = &st.conns.get(&conn)?.datablocks;
        Some((d.key, d.max_key, d.seq, d.in_flight(), d.queued()))
    }

    /// `GameConnection::transmitDataBlocks(seq)` for `conn`: starts the chain, or queues the call behind the one that runs.
    /// Returns what happened and the group positions looked at (the caller charges them). A queue with no room at all is
    /// `Backpressure` and nothing changes.
    pub fn transmit_datablocks(
        &self,
        vm: &Vm,
        conn: ConnId,
        seq: i32,
    ) -> Result<(Transmit, usize), SendError> {
        let mut guard = self.state.borrow_mut();
        let st = &mut *guard;
        let c = st.conns.get_mut(&conn).ok_or(SendError::Closed)?;
        if c.queue.is_closed() {
            return Err(SendError::Closed);
        }
        // A chain starts on a queue with no room at all only when 4,096 ordered events are unacknowledged: the connection is
        // dropped like a command's would be. Anything less is enough: the window is posted as far as the room goes and the rest
        // is held ([`ConnDatablocks::post_or_hold`]) and goes as acks free room. A call that only queues behind a running chain
        // posts nothing.
        if !c.datablocks.active() && c.queue.room_ordered() == 0 {
            return Err(SendError::Backpressure);
        }
        let (what, events, scanned) = c.datablocks.call(vm.objects(), &st.keys, &st.classes, seq);
        c.datablocks.post_or_hold(&mut c.queue, events);
        Ok((what, scanned))
    }

    /// A live round on every connection past its mission chain ([`ConnDatablocks::request_live`]): the datablocks each was
    /// not yet sent in their current state. Returns the connections reached (asked, queued or with nothing to send) and the
    /// group positions looked at (the caller charges them). A queue with no room holds the round's events until acks free
    /// room.
    pub fn update_datablocks(&self, vm: &Vm) -> (usize, usize) {
        let mut guard = self.state.borrow_mut();
        let st = &mut *guard;
        let (mut reached, mut scanned) = (0, 0);
        for c in st.conns.values_mut() {
            if c.queue.is_closed() {
                continue;
            }
            let (what, events, looked) =
                c.datablocks
                    .request_live(vm.objects(), &st.keys, &st.classes, &*st.codecs);
            scanned += looked;
            if what.reached() {
                reached += 1;
            }
            c.datablocks.post_or_hold(&mut c.queue, events);
        }
        (reached, scanned)
    }

    /// The key `id` was last sent with to `conn`, when a body of it was ([`ConnDatablocks::sent_key`]).
    pub fn datablock_sent_key(&self, conn: ConnId, id: ObjId) -> Option<i32> {
        self.state
            .borrow()
            .conns
            .get(&conn)?
            .datablocks
            .sent_key(id)
    }
}

/// `removeString(id, false)`: the engine reference a table entry held. A message from the table is the console's.
fn release(vm: &mut Vm, id: u32) {
    if let Some(message) = vm.tags_mut().remove_string(id, false) {
        vm.error(message);
    }
}

impl<H: GameEvents, S: GhostGame> PayloadLayer for EventLayer<H, S> {
    fn on_accept(&mut self, conn: ConnId, obj: ObjId, ghost_bits: u8) {
        if let Some(hub) = self.state.borrow().hub.clone() {
            hub.connect(conn, obj, ghost_bits);
        }
        let control = self.state.borrow().codecs.control_state();
        let head_bits = self.state.borrow().codecs.head_min_bits();
        let mut datablocks = ConnDatablocks::default();
        datablocks.room_bits = Some(packet_room_bits_with(head_bits));
        datablocks.set_window(self.state.borrow().codecs.datablock_window(false));
        datablocks.set_done_message(self.state.borrow().codecs.datablocks_done_message());
        self.state.borrow_mut().conns.insert(
            conn,
            ConnEvents {
                obj,
                queue: EventQueue::new(),
                receiver: EventReceiver::new(),
                send: SendStrings::new(),
                remote: RemoteStrings::new(),
                command_send: SendStrings::new(),
                command_remote: RemoteStrings::new(),
                control,
                spoke: false,
                game: H::Conn::default(),
                datablocks,
                held: VecDeque::new(),
                work: 0,
            },
        );
    }

    fn mark_peer(&mut self, conn: ConnId) {
        let window = self.state.borrow().codecs.datablock_window(true);
        if let Some(c) = self.state.borrow_mut().conns.get_mut(&conn) {
            c.datablocks.set_window(window);
        }
    }

    fn on_game_packet(
        &mut self,
        ctx: &mut ConnCtx<'_>,
        _received: &Received,
        payload: &mut BitReader<'_>,
    ) -> PacketOutcome {
        let mut out = PacketOutcome::nothing();
        let mut guard = self.state.borrow_mut();
        let st = &mut *guard;
        let Some(c) = st.conns.get_mut(&ctx.conn) else {
            return out;
        };
        c.spoke = true;
        payload.enable_string_buffer();
        // The client's head: the checksum of its control object, the moves (absorbed into the list by `lastMoveAck`'s rules),
        // the field of view.
        let block = match c.control.read_client(payload, &mut *ctx.charge) {
            Ok((block, _absorbed)) => block,
            Err(e) => {
                out.error = Some(e);
                return out;
            }
        };
        // `readPacket`, server branch, first: the camera position (`mCameraPos`) is `flag ? 0.0 : 1.0`.
        if let Some(h) = st.hub.as_ref() {
            h.set_camera_pos(ctx.conn, if block.camera_flag { 0.0 } else { 1.0 });
        }
        if let Some(byte) = block.fov {
            let camera = st
                .hub
                .as_ref()
                .and_then(|h| h.control_of(ctx.conn))
                .and_then(|o| torque_game::scene::camera::fov_object(ctx.vm, o));
            match camera {
                Some(id) => {
                    let mut target = torque_game::scene::camera::CameraFov {
                        vm: &mut *ctx.vm,
                        id,
                    };
                    c.control.apply_client_fov(byte, Some(&mut target));
                }
                None => c.control.apply_client_fov(byte, None),
            }
            if let Some(h) = st.hub.as_ref() {
                h.set_fov(ctx.conn, c.control.fov());
            }
        }

        let Some(ids) = st.ids.as_ref() else {
            out.error = Some(NetError::Invalid("event class table"));
            return out;
        };
        let ectx = EventCtx {
            datablock_bits: st.codecs.audio_id_bits(),
            table: &ctx.table,
            ids,
            ghost_bits: ctx.ghost_bits,
        };
        let mut incoming: Vec<Incoming<HostEvent<H::Event>>> = Vec::new();
        let read = {
            let pay = &mut *ctx.charge;
            let mut charge = |bytes: u64| pay(1 + bytes / 64);
            c.receiver.read_section_charged(
                payload,
                &ectx,
                Role::Server,
                &mut HostUnpacker::<H>::default(),
                &mut incoming,
                None,
                &mut charge,
            )
        };
        if let Err(e) = read {
            out.error = Some(e);
        }
        for inc in incoming {
            let event_error = match inc.event {
                HostEvent::Net(NetEvent::NetString(e)) => {
                    let id = ctx.vm.tags_mut().add_string(&e.text);
                    match id {
                        None => Some(NetError::Invalid("tag table full")),
                        Some(id) => {
                            if st.codecs.shows_string_mappings(ctx.vm) {
                                // Printed where the event ran, among the commands' output.
                                out.followups
                                    .push(Followup::Print(mapping_line(&e.text, e.slot)));
                            }
                            if let Some(old) = c.remote.set(e.slot, id, &e.text) {
                                release(ctx.vm, old);
                            }
                            None
                        }
                    }
                }
                HostEvent::Net(NetEvent::Remote(cmd)) => {
                    let args = cmd.script_args();
                    // The expansion of a tag argument copies up to 1,024 bytes: a unit each, before `resolve`.
                    let tags = args.iter().filter(|a| a.first() == Some(&1)).count();
                    if tags > 0 && !(ctx.charge)(tags as u64) {
                        Some(NetError::Invalid("work reservation"))
                    } else {
                        if let Some(r) = resolve(&args, &c.remote) {
                            let prefix = st.codecs.command_prefix(cmd.variant, true);
                            out.followups.push(Followup::ServerCmd {
                                obj: c.obj,
                                name: function_name(prefix, &r.name),
                                args: r.args,
                            });
                        }
                        None
                    }
                }
                HostEvent::Net(NetEvent::ConnectionMessage(m)) => match m.message {
                    n if st.codecs.rejects_client_message(n) => {
                        Some(NetError::Invalid("ghost bad connection message state"))
                    }
                    _ => {
                        // Server branch: the download-done message runs the script when it carries the sequence of the
                        // last `transmitDataBlocks`, or `onDataBlocksUpdated(count)` when it carries a live round's (each
                        // round's sequence answers once); the base handler then has nothing more to do with it.
                        if m.message == st.codecs.datablocks_download_done_message() {
                            if let Some(count) = c.datablocks.take_live_answer(m.seq as i32) {
                                out.followups.push(Followup::Call {
                                    object: Some(c.obj),
                                    name: b"onDataBlocksUpdated".to_vec(),
                                    args: vec![count.to_string().into_bytes()],
                                });
                            } else if m.seq as i32 == c.datablocks.seq {
                                out.followups.push(Followup::Call {
                                    object: Some(c.obj),
                                    name: b"onDataBlocksDone".to_vec(),
                                    args: vec![(m.seq as i32).to_string().into_bytes()],
                                });
                            }
                        }
                        // Message 1 is the ghost layer's: the client got the ghost-always objects. The script runs after the
                        // packet's events.
                        if m.message == 1
                            && let Some(hub) = st.hub.as_ref()
                        {
                            let _ = hub.on_message(ctx.conn, &m);
                            for call in hub.take_calls(ctx.conn) {
                                if call == GhostCall::AlwaysObjectsReceived {
                                    out.followups.push(Followup::Call {
                                        object: Some(c.obj),
                                        name: b"onGhostAlwaysObjectsReceived".to_vec(),
                                        args: Vec::new(),
                                    });
                                }
                            }
                        }
                        if let Some(f) = st.hook.on_message(ctx.conn, c.obj, &m) {
                            out.followups.push(f);
                        }
                        None
                    }
                },
                HostEvent::Game(e) => st.game.on_received(
                    &mut c.game,
                    e,
                    &mut ReceiveCx {
                        conn: ctx.conn,
                        obj: c.obj,
                        now: ctx.now,
                        followups: &mut out.followups,
                        hub: st.hub.as_ref(),
                    },
                ),
                // The other events go from the server to the client: the section reader refuses them by direction before they
                // get here.
                HostEvent::Torque(_) | HostEvent::DataBlock(_) | HostEvent::Ghost(_) => {
                    Some(NetError::Invalid("event direction"))
                }
            };
            if let Some(e) = event_error {
                out.error.get_or_insert(e);
                break;
            }
        }
        out
    }

    fn on_notify(&mut self, ctx: &mut ConnCtx<'_>, notify: Notify) {
        let mut guard = self.state.borrow_mut();
        let st = &mut *guard;
        let Some(c) = st.conns.get_mut(&ctx.conn) else {
            return;
        };
        let mut settled = Vec::new();
        c.queue
            .on_notify(notify.send_seq, notify.delivered, &mut settled);
        if let Some(hub) = st.hub.as_ref() {
            hub.on_notify(ctx.conn, notify.send_seq, notify.delivered);
        }
        if !notify.delivered {
            c.datablocks.mark_stale();
        }
        // `SimDataBlockEvent::notifyDelivered` reads no `delivered`: an ordered event settles when its packet is acked.
        let cells: Vec<_> = settled
            .iter()
            .filter_map(|s| match &s.event {
                HostEvent::DataBlock(cell) => Some(Rc::clone(&cell.0)),
                _ => None,
            })
            .collect();
        Self::settle(c, settled);
        for cell in cells {
            let posts =
                c.datablocks
                    .settled(ctx.vm.objects(), &st.keys, &st.classes, &*st.codecs, &cell);
            // A pull into a full queue is held, not dropped: the chain would never end.
            c.datablocks.post_or_hold(&mut c.queue, posts);
        }
        // A queued call that started at the chain's end looked at the group: one unit, and one more for each scan unit, as the
        // native charges.
        let scanned = c.datablocks.take_scan();
        if scanned > 0 {
            let units = 1 + (scanned / super::datablocks::NET_DATABLOCK_SCAN_PER_UNIT) as u32;
            c.work = c.work.saturating_add(units);
        }
        // The acks freed room: the events held earlier go now.
        c.datablocks.flush(&mut c.queue);
        Self::flush_held(c);
    }

    fn write(&mut self, ctx: &mut ConnCtx<'_>, w: &mut BitWriter) -> Result<(), NetError> {
        let mut guard = self.state.borrow_mut();
        let st = &mut *guard;
        let Some(c) = st.conns.get_mut(&ctx.conn) else {
            return Err(NetError::Invalid("no event state for the connection"));
        };
        c.datablocks.flush(&mut c.queue);
        Self::flush_held(c);
        // The plugins' datablocks go to the engine's own clients only (the local one, a peer granted the handshake's extension, any
        // client of a server that says all of them are).
        let peer = match st.hub.as_ref() {
            Some(hub) => hub.peer_kind(ctx.vm, ctx.conn),
            None => PeerKind::of(ctx.vm, false),
        };
        c.datablocks.set_plugin_classes(peer.sees_engine_classes());
        // The datablock events are rendered from the live datablocks before the section is written (they need the `Vm`, which a
        // pack has not). A game's prepack runs first ([`DbValues::before_pack`]); that needs the `Vm`.
        for id in c
            .datablocks
            .packing(ctx.vm.objects(), &st.keys, &*st.codecs)
        {
            st.values.before_pack(ctx.vm, id);
        }
        match c.datablocks.render_with(
            ctx.vm.objects(),
            &st.keys,
            &ctx.table,
            &st.values,
            &*st.codecs,
        ) {
            Ok((units, packed)) => {
                c.work = c.work.saturating_add(units);
                for (id, record) in packed {
                    st.codecs.datablock_packed(ctx.vm, &st.values, id, &*record);
                }
            }
            Err(e) => {
                c.work = c.work.saturating_add(e.units);
                ctx.vm.print(e.line(c.obj).as_bytes());
                return Err(NetError::Invalid("datablock event too big"));
            }
        }
        w.enable_string_buffer();
        let hub = st.hub.clone().filter(|h| h.has_conn(ctx.conn));
        // The head: the move ack, the damage flash, the control section (which sets the stream's compression point), the
        // camera object and the field of view.
        match hub.as_ref() {
            Some(hub) => hub.with(|h| {
                let ghost = h.conns.get(&ctx.conn);
                let index = |o: ObjId| ghost.and_then(|g| g.mgr.ghost_index(o));
                let adapter = ghost
                    .and_then(|g| g.control)
                    .and_then(|obj| st.codecs.peer_control_object(&*ctx.vm, obj, &index, peer));
                c.control.write_head(w, ctx.ghost_bits, adapter.as_deref());
            }),
            None => {
                c.control.write_head(w, ctx.ghost_bits, None);
            }
        }
        let Some(ids) = st.ids.as_ref() else {
            return Err(NetError::Invalid("event class table"));
        };
        let ectx = EventCtx {
            datablock_bits: st.codecs.audio_id_bits(),
            table: &ctx.table,
            ids,
            ghost_bits: ctx.ghost_bits,
        };
        let params = SectionParams {
            ghosting: false,
            hold_ordered: false,
            packet_bits: ctx.protocol.packet_limit_bits(),
            min_start_bits: min_start_bits_with(st.codecs.head_min_bits()),
        };
        let send_seq = ctx.protocol.last_send_seq();
        let params = SectionParams {
            ghosting: hub.as_ref().is_some_and(|h| h.is_ghosting(ctx.conn)),
            ..params
        };
        // Until the client has spoken the rate is the initial 200 bytes: what fits is sent and an event that does not fit waits
        // at the head of its queue for the client's first rate pair. After that an event no packet can carry is an error. A
        // ghost-always event packs its object when it is written, so the events go through the connection's pack context.
        let env = SourceEnv {
            vm: &*ctx.vm,
            table: Some(&*ctx.table),
            ghost_bits: ctx.ghost_bits,
        };
        let spoke = c.spoke;
        // The packs of the event and ghost sections map their net strings through the connection's sender table, lent to the
        // hub for the two sections and taken back whatever they return.
        if let Some(hub) = hub.as_ref() {
            hub.lend_strings(ctx.conn, std::mem::take(&mut c.send));
        }
        let queue = &mut c.queue;
        // The events leave room for the smallest ghost section (and for the lone flag written without a hub): a packet never
        // runs past the datagram bound.
        let tail = w.hold_back(SECTION_MIN_BITS);
        let events_written = match hub.as_ref() {
            Some(hub) => hub
                .with_pack_ctx(env, ctx.conn, |objs| {
                    if spoke {
                        queue.write_section_with(w, &ectx, objs, send_seq, params)
                    } else {
                        queue.write_section_with_stalling(w, &ectx, objs, send_seq, params)
                    }
                })
                .map_or(Ok(()), |r| r.map(drop)),
            None if spoke => queue.write_section(w, &ectx, send_seq, params).map(drop),
            None => queue
                .write_section_stalling(w, &ectx, send_seq, params)
                .map(drop),
        };
        w.release(tail);
        // The scope query's eye (`ShapeBase::onCameraScopeQuery` takes the scope object's `getEyeTransform`; a Player's animates
        // its nodes first on a server, as Torque3D's `Player::getEyeBaseTransform` does: the pose the muzzle and the mount points
        // read next).
        if let Some(h) = hub.as_ref() {
            let eye = h.scope_object_of(ctx.conn).and_then(|o| {
                ctx.vm.objects().contains(o).then(|| {
                    (
                        o,
                        torque_game::sim::player::methods::eye_transform(ctx.vm, o),
                    )
                })
            });
            h.set_scope_eye(ctx.conn, eye);
        }
        // The ghost part of a server packet: the flag, and once ghosting starts the records.
        let stats = match (&events_written, hub.as_ref()) {
            (Ok(()), Some(h)) => {
                let env = SourceEnv {
                    vm: &*ctx.vm,
                    table: Some(&*ctx.table),
                    ghost_bits: ctx.ghost_bits,
                };
                h.write_section(env, ctx.conn, w, params.packet_bits, send_seq)
            }
            _ => None,
        };
        if let Some(strings) = hub.as_ref().and_then(GhostHub::take_strings) {
            Self::strings_packed(ctx.vm, c, strings);
        }
        events_written?;
        if stats.is_none() {
            w.write_flag(false);
        }
        // A packet that still ran past the bound is the server's own fault, never the client's: the send pass withholds it (the
        // protocol then counts it lost and its events and ghost updates go again) instead of dropping the client.
        Ok(())
    }

    fn take_calls(&mut self, conn: ConnId) -> Vec<Followup> {
        let st = self.state.borrow();
        let (Some(hub), Some(c)) = (st.hub.as_ref(), st.conns.get(&conn)) else {
            return Vec::new();
        };
        hub.take_calls(conn)
            .into_iter()
            .filter_map(|call| match call {
                GhostCall::ModeSwitch(on) => {
                    let (name, args) = S::mode_switch_call(on)?;
                    Some(Followup::Call {
                        object: Some(c.obj),
                        name: name.to_vec(),
                        args,
                    })
                }
                GhostCall::AlwaysObjectsReceived => None,
            })
            .collect()
    }

    fn busy(&self, conn: ConnId) -> bool {
        let st = self.state.borrow();
        st.hub.as_ref().is_none_or(|h| !h.quiet(conn))
    }

    fn take_work(&mut self, conn: ConnId) -> u32 {
        self.state
            .borrow_mut()
            .conns
            .get_mut(&conn)
            .map_or(0, |c| std::mem::take(&mut c.work))
    }

    fn on_drop(&mut self, conn: ConnId, vm: &mut Vm) {
        if let Some(hub) = self.state.borrow().hub.clone() {
            hub.remove_conn(conn);
        }
        let Some(mut c) = self.state.borrow_mut().conns.remove(&conn) else {
            return;
        };
        // The game's state goes first: the settled events of the queue then pull nothing into a dead queue.
        H::on_drop(&mut c.game);
        let settled = c.queue.release_all();
        Self::settle(&mut c, settled);
        for tag in c.send.release_all() {
            release(vm, tag);
        }
        for id in c
            .remote
            .release_all()
            .into_iter()
            .chain(c.command_remote.release_all())
        {
            release(vm, id);
        }
    }
}
