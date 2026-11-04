//! The events of the host's connections in one type that the connection's [`EventQueue`](torque_net::events::EventQueue) and
//! receiver carry: `net`'s [`NetEvent`] (NetString, remote commands, `ConnectionMessage`), Torque's small events
//! ([`TorqueEvent`]: the audio events, `PathManagerEvent`, `LightningStrikeEvent`), the datablock events a chain posts, the
//! ghost-always events, and the game's own events ([`HostEvent::Game`]).
//!
//! A game says what its events are and what the layer does with them through [`GameEvents`]: how they are read, what a client's
//! event does on the server, what posting one or its settling sets off on the connection's game state, and what a dropped
//! connection lets go of.

use std::fmt;
use std::marker::PhantomData;

use torque_net::NetError;
use torque_net::bits::BitReader;
use torque_net::bits::BitWriter;
use torque_net::events::{
    ChargeFn, EventCtx, EventIds, EventUnpacker, Guarantee, NetEvent, PackCtx, WireEvent,
    free_charge,
};
use torque_net::gameevents::{PathCount, PathManagerEvent};
use torque_net::ghost::GhostEvent;
use torque_net::handshake::ConnId;
use torque_net::netstring::SendStrings;
use torque_ts_vm::sim::ObjId;

pub use torque_net::gameevents::GameEvent as TorqueEvent;

use super::datablocks::DbEvent;
use super::ghosts::{GhostGame, GhostHub};
use super::payload::Followup;

/// `SimDataBlockEvent`'s engine class name.
pub const DATABLOCK_CLASS: &str = "SimDataBlockEvent";

/// An event of a host connection; `G` is the game's events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent<G> {
    Net(NetEvent),
    /// Torque's small events: the audio events (server to client), `PathManagerEvent`, `LightningStrikeEvent`.
    Torque(TorqueEvent),
    /// A datablock event a chain posted (server to client): rendered from the live datablock when its packet is built
    /// ([`super::datablocks`]).
    DataBlock(DbEvent),
    /// A ghost-always event the ghost layer posted (server to client): its record is packed from the live object when the event
    /// is written.
    Ghost(GhostEvent),
    /// One of the game's events.
    Game(G),
}

/// The id of the event class named `name` in the derived table. `None` when the table lacks it.
pub fn class_id(ids: &EventIds, name: &str) -> Option<u32> {
    (0..ids.count()).find(|&id| ids.name(id) == Some(name))
}

impl<G> HostEvent<G> {
    /// What the connection does when this event settles (a packet carrying it was acked or lost) apart from the game's: a
    /// delivered NetString sets its sender entry's flag.
    pub fn on_settled(&self, delivered: bool, strings: &mut SendStrings) {
        if let HostEvent::Net(e) = self {
            e.on_settled(delivered, strings);
        }
    }
}

impl<G: WireEvent> WireEvent for HostEvent<G> {
    fn class(&self, ids: &EventIds) -> u32 {
        match self {
            HostEvent::Net(e) => e.class(ids),
            HostEvent::Torque(e) => e.class(ids),
            HostEvent::DataBlock(_) => class_id(ids, DATABLOCK_CLASS).unwrap_or(0),
            HostEvent::Ghost(e) => e.class(ids),
            HostEvent::Game(e) => e.class(ids),
        }
    }

    fn guarantee(&self) -> Guarantee {
        match self {
            // The inner event's own (unordered ones: the ghost-related and move events).
            HostEvent::Net(e) => e.guarantee(),
            HostEvent::Game(e) => e.guarantee(),
            // Torque's small events, the datablock and the ghost-always events are `GuaranteedOrdered`.
            _ => Guarantee::Ordered,
        }
    }

    fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        match self {
            HostEvent::Net(e) => e.pack(w, ctx),
            HostEvent::Torque(e) => e.pack(w, ctx),
            HostEvent::DataBlock(cell) => cell.0.pack(w, ctx.table),
            HostEvent::Ghost(e) => e.pack(w, ctx),
            HostEvent::Game(e) => e.pack(w, ctx),
        }
    }

    fn pack_with(&self, w: &mut BitWriter, ctx: &EventCtx<'_>, objs: &mut dyn PackCtx) {
        match self {
            HostEvent::Ghost(e) => e.pack_with(w, ctx, objs),
            HostEvent::Game(e) => e.pack_with(w, ctx, objs),
            other => other.pack(w, ctx),
        }
    }
}

/// What the layer tells the game about a game event a client sent.
pub struct ReceiveCx<'a, S: GhostGame> {
    pub conn: ConnId,
    /// The connection's object.
    pub obj: ObjId,
    /// The network time.
    pub now: u32,
    /// What runs after the packet's events (script calls, console lines).
    pub followups: &'a mut Vec<Followup>,
    /// The ghost hub, when the host has one.
    pub hub: Option<&'a GhostHub<S>>,
}

/// What a game adds to the host's events: its event classes and what the event layer does with them. `Self` is the game's state
/// of the whole layer, [`GameEvents::Conn`] its state of one connection.
pub trait GameEvents: Default + 'static {
    /// The game's events.
    type Event: WireEvent + Clone + fmt::Debug + Eq;
    /// One connection's state of the game's events.
    type Conn: Default;

    /// The most entries a `PathManagerEvent` read may hold.
    fn path_count() -> PathCount {
        PathCount::default()
    }

    /// Reads the body of the game's event class `class`; `Ok(None)` when the class is not the game's (the reader tries `net`'s
    /// next).
    fn unpack(
        class: u32,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
        charge: &mut ChargeFn,
    ) -> Result<Option<Self::Event>, NetError>;

    /// A game event a client sent (the section reader has refused the classes that only go to a client). An error drops the
    /// connection.
    fn on_received<S: GhostGame>(
        &mut self,
        conn: &mut Self::Conn,
        event: Self::Event,
        cx: &mut ReceiveCx<'_, S>,
    ) -> Option<NetError>;

    /// A game event went on the connection's ordered queue.
    fn on_posted(conn: &mut Self::Conn, event: &Self::Event) {
        let _ = (conn, event);
    }

    /// A game event settled (its packet was acked or lost): an event it pulls goes on the queue behind any earlier pull.
    fn on_settled(
        conn: &mut Self::Conn,
        event: &Self::Event,
        delivered: bool,
    ) -> Option<Self::Event> {
        let _ = (conn, event, delivered);
        None
    }

    /// The connection is going, before its queue lets go of its events.
    fn on_drop(conn: &mut Self::Conn) {
        let _ = conn;
    }
}

/// Reads the body of any event of the host's classes with the game's `H`. The payment for the bytes is the section reader's (it
/// charges each event by its size), so the codecs here are given a callback that pays for everything.
pub struct HostUnpacker<H>(PhantomData<fn() -> H>);

impl<H> Default for HostUnpacker<H> {
    fn default() -> HostUnpacker<H> {
        HostUnpacker(PhantomData)
    }
}

impl<H> Clone for HostUnpacker<H> {
    fn clone(&self) -> HostUnpacker<H> {
        *self
    }
}

impl<H> Copy for HostUnpacker<H> {}

impl<H> fmt::Debug for HostUnpacker<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostUnpacker")
    }
}

impl<H: GameEvents> EventUnpacker for HostUnpacker<H> {
    type Event = HostEvent<H::Event>;

    fn unpack(
        &mut self,
        class: u32,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
    ) -> Result<HostEvent<H::Event>, NetError> {
        let mut free = free_charge();
        if class == ctx.ids.path_manager {
            return PathManagerEvent::unpack_with(r, &mut free, H::path_count())
                .map(|e| HostEvent::Torque(TorqueEvent::PathManager(e)));
        }
        if let Some(e) = TorqueEvent::unpack(class, r, ctx, &mut free)? {
            return Ok(HostEvent::Torque(e));
        }
        match H::unpack(class, r, ctx, &mut free)? {
            Some(e) => Ok(HostEvent::Game(e)),
            None => NetEvent::unpack(class, r, ctx).map(HostEvent::Net),
        }
    }
}
