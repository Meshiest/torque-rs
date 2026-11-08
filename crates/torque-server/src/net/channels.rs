//! Bulk channels on the server: the registry plugins add them to, the hub a game sends through, and the layer that puts the
//! chunks into each connection's data packets.
//!
//! A channel is for large data that mostly does not change (a map's static data, a terrain patch): the client gets a
//! snapshot when it is enabled, then deltas, and the data is cut into chunks that share the packets with events and ghosts under
//! the channel's per-packet byte budget ([`torque_net::chunk`]). What a client gets is the game's decision, per client:
//! [`ChunkChannel::snapshot`] is asked for each client's own snapshot, and a delta is sent to the clients a closure picks
//! ([`ChannelHub::send_each`]).
//!
//! **Where the bytes go.** [`ChannelLayer`] wraps the host's payload layer. For a connection with channels enabled it writes the
//! channels' section first in each data packet and then lets the wrapped layer write the rest into what is left, so the
//! channels' share is bounded (a channel's budget, and [`ChannelLayer::share_percent`] of the packet for all of them) and the
//! events and ghosts keep the rest; every [`ChannelLayer::yield_every`]th packet carries no chunks at all, so an event too large
//! for the room the channels leave still gets a packet. A connection with channels not enabled, and a host that registered no
//! channel (it has no layer), write the bits they always did.
//!
//! **Enabling.** A peer that does not read the section would fail on it, so a connection starts with channels off. An engine
//! with channels registered puts their names in its client's manifest, and a server whose gate accepted a manifest that listed
//! channels (so its own channels are the same) turns them on for that connection as it accepts it ([`PayloadLayer::mark_channels`])
//! and ends its accept with a word that says so; the client's connection turns its reader on when it reads that word
//! ([`ClientChannels::set_active`]). Data packets only flow after the accept, so both sides are on before the first one. A server
//! with no gate, or a client of a game with other channels, never gets the word: nothing is written and nothing is read.
//! [`ChannelHub::enable`] and [`ClientChannels::set_active`] are the manual way (a test, or a game that has its own way to know
//! its peer); a manual `enable` needs the client active first, or the first packets it writes are misread, so a game that
//! enables by hand does it for clients it knows are reading.
//!
//! **Limits and refusals.** A message above the channel's size cap, or one that would take a client's backlog above its cap, is
//! refused with a [`SendError`] and nothing is queued; the game then drops the delta and sends a fresh snapshot
//! ([`ChannelHub::resync`]), which also replaces the messages that had not started to go out.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
pub use torque_net::chunk::registry::{
    ChannelEntry, ChannelError, ChannelRegistry, ChunkChannel, MAX_BUDGET, MIN_BUDGET,
};
use torque_net::chunk::{ChunkReceiver, ChunkSender, Kind, SendError};
use torque_net::handshake::ConnId;
use torque_net::protocol::{Notify, Received};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::NetGame;
use super::NetShared;
use super::payload::{ConnCtx, Followup, PacketOutcome, PayloadLayer, Quiet};

// ---- the server's hub

struct ConnChannels {
    sender: ChunkSender,
    /// Data packets written since the last one that carried no chunks.
    since_yield: u32,
}

struct HubState {
    registry: ChannelRegistry,
    logic: Vec<Option<Box<dyn ChunkChannel>>>,
    conns: HashMap<ConnId, ConnChannels>,
    /// Whether a connection the host marks as an engine peer (the handshake's extension) gets channels at once.
    enable_peers: bool,
    /// Connections the layer knows (accepted), enabled or not.
    known: Vec<ConnId>,
}

/// What a game sends through: shared with the layer ([`ChannelLayer`]).
#[derive(Clone)]
pub struct ChannelHub {
    state: Rc<RefCell<HubState>>,
}

/// The clients a send was refused for, with why.
pub type Refused = Vec<(ConnId, SendError)>;

impl ChannelHub {
    /// A hub for `registry` (each channel's server logic made).
    pub fn new(registry: ChannelRegistry) -> ChannelHub {
        ChannelHub {
            state: Rc::new(RefCell::new(HubState {
                logic: registry.make_all(),
                registry,
                conns: HashMap::new(),
                enable_peers: false,
                known: Vec::new(),
            })),
        }
    }

    /// The id of channel `name`.
    pub fn id(&self, name: &str) -> Option<usize> {
        self.state.borrow().registry.id(name)
    }

    /// Whether a connection the host marks as an engine peer (a client that asked for the handshake's extension) has channels
    /// enabled at once, with its snapshots queued. Off by default, and not needed by an engine's own clients (the manifest turns
    /// channels on for them). It is a manual opt-in for a game whose clients are all known to read the section: their reader
    /// must be active ([`ClientChannels::set_active`]) before the first data packet, or the packets are misread.
    pub fn set_enable_peers(&self, on: bool) {
        self.state.borrow_mut().enable_peers = on;
    }

    /// Turns channels on for `client` and queues each channel's snapshot for it (what its logic answers). Does nothing for a
    /// client that is not connected or is already on.
    pub fn enable(&self, client: ConnId) {
        {
            let mut st = self.state.borrow_mut();
            if st.conns.contains_key(&client) || !st.known.contains(&client) {
                return;
            }
            let limits = st.registry.limits();
            st.conns.insert(
                client,
                ConnChannels {
                    sender: ChunkSender::new(&limits),
                    since_yield: 0,
                },
            );
        }
        let channels = self.state.borrow().registry.len();
        for channel in 0..channels {
            let _ = self.queue_snapshot(client, channel, false);
        }
    }

    /// The connections the host has accepted, with channels on or off.
    pub fn connected(&self) -> Vec<ConnId> {
        self.state.borrow().known.clone()
    }

    /// Whether channels are on for `client`.
    pub fn is_enabled(&self, client: ConnId) -> bool {
        self.state.borrow().conns.contains_key(&client)
    }

    /// The clients whose channels are on.
    pub fn clients(&self) -> Vec<ConnId> {
        let st = self.state.borrow();
        let mut ids: Vec<ConnId> = st.conns.keys().copied().collect();
        ids.sort();
        ids
    }

    /// Asks the logic of `channel` for `client`'s snapshot and queues it. The logic is out of the hub for the call.
    fn queue_snapshot(
        &self,
        client: ConnId,
        channel: usize,
        replace: bool,
    ) -> Result<(), SendError> {
        let logic = self
            .state
            .borrow_mut()
            .logic
            .get_mut(channel)
            .and_then(Option::take);
        let Some(mut logic) = logic else {
            return Err(SendError::UnknownChannel);
        };
        let data = logic.snapshot(client);
        let mut st = self.state.borrow_mut();
        st.logic[channel] = Some(logic);
        let Some(data) = data else {
            return Ok(());
        };
        let Some(conn) = st.conns.get_mut(&client) else {
            return Err(SendError::UnknownChannel);
        };
        if replace {
            conn.sender.drop_unstarted(channel);
        }
        conn.sender.enqueue(channel, Kind::Snapshot, data)
    }

    /// Replaces what has not started going out to `client` on `channel` with a fresh snapshot (the logic's), after a refusal or
    /// whenever the client's state changed too much for a delta.
    pub fn resync(&self, client: ConnId, channel: &str) -> Result<(), SendError> {
        let Some(id) = self.id(channel) else {
            return Err(SendError::UnknownChannel);
        };
        self.queue_snapshot(client, id, true)
    }

    /// Queues `data` for `client` on `channel`.
    pub fn send_to(
        &self,
        client: ConnId,
        channel: &str,
        kind: Kind,
        data: Vec<u8>,
    ) -> Result<(), SendError> {
        let mut st = self.state.borrow_mut();
        let Some(id) = st.registry.id(channel) else {
            return Err(SendError::UnknownChannel);
        };
        match st.conns.get_mut(&client) {
            Some(c) => c.sender.enqueue(id, kind, data),
            None => Err(SendError::UnknownChannel),
        }
    }

    /// Queues for each enabled client the bytes `pick` returns for it (`None`: that client does not get this message).
    /// Returns the clients that were refused, which a game answers with [`resync`](Self::resync) or by dropping them.
    pub fn send_each(
        &self,
        channel: &str,
        kind: Kind,
        mut pick: impl FnMut(ConnId) -> Option<Vec<u8>>,
    ) -> Refused {
        let mut refused = Vec::new();
        let Some(id) = self.id(channel) else {
            return refused;
        };
        for client in self.clients() {
            let Some(data) = pick(client) else {
                continue;
            };
            let mut st = self.state.borrow_mut();
            if let Some(c) = st.conns.get_mut(&client)
                && let Err(e) = c.sender.enqueue(id, kind, data)
            {
                refused.push((client, e));
            }
        }
        refused
    }

    /// Queues `data` for every enabled client.
    pub fn broadcast(&self, channel: &str, kind: Kind, data: &[u8]) -> Refused {
        self.send_each(channel, kind, |_| Some(data.to_vec()))
    }

    /// The bytes queued for `client` on `channel` and not yet acknowledged.
    pub fn backlog(&self, client: ConnId, channel: &str) -> usize {
        let st = self.state.borrow();
        st.registry
            .id(channel)
            .zip(st.conns.get(&client))
            .map_or(0, |(id, c)| c.sender.backlog(id))
    }

    /// Whether nothing is queued for `client` and nothing is in a packet that might yet be lost.
    pub fn is_idle(&self, client: ConnId) -> bool {
        self.state
            .borrow()
            .conns
            .get(&client)
            .is_none_or(|c| c.sender.is_idle())
    }

    // The layer's side.

    fn accepted(&self, conn: ConnId) {
        self.state.borrow_mut().known.push(conn);
    }

    fn peer(&self, conn: ConnId) {
        let on = self.state.borrow().enable_peers;
        if on {
            self.enable(conn);
        }
    }

    fn dropped(&self, conn: ConnId) {
        let mut st = self.state.borrow_mut();
        st.known.retain(|c| *c != conn);
        st.conns.remove(&conn);
    }

    fn has_work(&self, conn: ConnId) -> bool {
        self.state
            .borrow()
            .conns
            .get(&conn)
            .is_some_and(|c| c.sender.has_work())
    }
}

/// The payload layer that carries the channels: the host's own layer, with each enabled connection's chunk section first.
pub struct ChannelLayer {
    inner: Box<dyn PayloadLayer>,
    hub: ChannelHub,
    /// The share of a packet all the channels together may take, in percent.
    pub share_percent: usize,
    /// Every this many data packets one carries no chunks (0: never).
    pub yield_every: u32,
}

impl ChannelLayer {
    /// A layer over `inner` that writes `hub`'s channels: half of each packet at most, and every fourth packet
    /// without chunks.
    pub fn new(inner: Box<dyn PayloadLayer>, hub: ChannelHub) -> ChannelLayer {
        ChannelLayer {
            inner,
            hub,
            share_percent: 50,
            yield_every: 4,
        }
    }
}

impl PayloadLayer for ChannelLayer {
    fn on_game_packet(
        &mut self,
        ctx: &mut ConnCtx<'_>,
        received: &Received,
        payload: &mut BitReader<'_>,
    ) -> PacketOutcome {
        // Channels run from the server to the client: a client's packets carry no section.
        self.inner.on_game_packet(ctx, received, payload)
    }

    fn on_notify(&mut self, ctx: &mut ConnCtx<'_>, notify: Notify) {
        if let Some(c) = self.hub.state.borrow_mut().conns.get_mut(&ctx.conn) {
            c.sender.on_notify(notify.send_seq, notify.delivered);
        }
        self.inner.on_notify(ctx, notify);
    }

    fn on_accept(&mut self, conn: ConnId, obj: ObjId, ghost_bits: u8) {
        self.hub.accepted(conn);
        self.inner.on_accept(conn, obj, ghost_bits);
    }

    fn mark_peer(&mut self, conn: ConnId) {
        self.inner.mark_peer(conn);
        self.hub.peer(conn);
    }

    fn mark_channels(&mut self, conn: ConnId) {
        self.inner.mark_channels(conn);
        self.hub.enable(conn);
    }

    fn write(&mut self, ctx: &mut ConnCtx<'_>, w: &mut BitWriter) -> Result<(), NetError> {
        {
            let mut st = self.hub.state.borrow_mut();
            if let Some(c) = st.conns.get_mut(&ctx.conn) {
                let limit = ctx.protocol.packet_limit_bits();
                let share = limit / 100 * self.share_percent.min(100);
                let room = share
                    .min(limit.saturating_sub(w.bits()))
                    .min(w.remaining_bits());
                let yielding = self.yield_every > 0 && c.since_yield + 1 >= self.yield_every;
                let room = if yielding { room.min(1) } else { room };
                let written = c.sender.write(w, ctx.protocol.last_send_seq(), room.max(1));
                c.since_yield = if yielding || written == 0 {
                    0
                } else {
                    c.since_yield + 1
                };
            }
        }
        self.inner.write(ctx, w)
    }

    fn busy(&self, conn: ConnId) -> bool {
        self.inner.busy(conn) || self.hub.has_work(conn)
    }

    fn take_calls(&mut self, conn: ConnId) -> Vec<Followup> {
        self.inner.take_calls(conn)
    }

    fn take_work(&mut self, conn: ConnId) -> u32 {
        self.inner.take_work(conn)
    }

    fn on_drop(&mut self, conn: ConnId, vm: &mut Vm) {
        self.hub.dropped(conn);
        self.inner.on_drop(conn, vm);
    }
}

/// Puts the channels of `registry` in the host's packets and returns the hub a game sends through. A registry with no channel
/// changes nothing (no layer is added).
pub fn install<G: NetGame>(
    shared: &Rc<RefCell<NetShared<G>>>,
    registry: ChannelRegistry,
) -> Option<ChannelHub> {
    if registry.is_empty() {
        return None;
    }
    let hub = ChannelHub::new(registry);
    let mut sh = shared.borrow_mut();
    // The handshake may now tell a client whose channels match the gate's that this server writes them.
    sh.hs.config_mut().channels = true;
    let inner = std::mem::replace(&mut sh.payload, Box::new(Quiet));
    sh.payload = Box::new(ChannelLayer::new(inner, hub.clone()));
    Some(hub)
}

// ---- the client's side

/// A client's channels, shared between the connection that reads them and whatever turns them on: an extension of the client's
/// script engine when the engine has channels. They are off until [`set_active`](Self::set_active) (a server that does not write
/// the section would be misread): the connection turns them on when the server's accept says it writes them, and
/// [`reset`](Self::reset) starts them over for each connection, since a connection's message numbers and half-received messages
/// belong to it alone.
///
/// A channel's [`receive`](ChunkChannel::receive) may call back into this (`reset`, `set_active`, `is_active`, `held_bytes`):
/// nothing is borrowed while it runs.
pub struct ClientChannels {
    registry: ChannelRegistry,
    receiver: RefCell<ChunkReceiver>,
    /// Each channel's logic; a slot is empty while its `receive` runs.
    logic: RefCell<Vec<Option<Box<dyn ChunkChannel>>>>,
    active: Cell<bool>,
}

impl ClientChannels {
    /// The client's channels for `registry`, off.
    pub fn new(registry: &ChannelRegistry) -> ClientChannels {
        ClientChannels {
            receiver: RefCell::new(ChunkReceiver::new(&registry.limits())),
            logic: RefCell::new(registry.make_all()),
            registry: registry.clone(),
            active: Cell::new(false),
        }
    }

    /// The names in id order.
    pub fn names(&self) -> Vec<&str> {
        self.registry.names()
    }

    /// Whether the connection reads the section.
    pub fn is_active(&self) -> bool {
        self.active.get()
    }

    /// Turns the section's reading on or off (for a server that wrote it).
    pub fn set_active(&self, on: bool) {
        self.active.set(on);
    }

    /// Starts over for a new connection: off, and no message numbers or half-received messages left from the last one. The
    /// channels' logic keeps its state (the next snapshot replaces it).
    pub fn reset(&self) {
        *self.receiver.borrow_mut() = ChunkReceiver::new(&self.registry.limits());
        self.active.set(false);
    }

    /// Reads a packet's section when active, and gives each message that arrived whole to its channel. An error means the
    /// server sent something no server of this engine sends: the connection should be dropped.
    pub fn read(&self, r: &mut BitReader<'_>) -> Result<(), NetError> {
        if !self.active.get() {
            return Ok(());
        }
        let mut delivered = Vec::new();
        let result = self.receiver.borrow_mut().read(r, &mut delivered);
        for d in delivered {
            let taken = self
                .logic
                .borrow_mut()
                .get_mut(d.channel)
                .and_then(Option::take);
            if let Some(mut logic) = taken {
                logic.receive(d.kind, &d.data);
                self.logic.borrow_mut()[d.channel] = Some(logic);
            }
        }
        result
    }

    /// The bytes held for messages half received.
    pub fn held_bytes(&self) -> usize {
        self.receiver.borrow().held_bytes()
    }
}

#[cfg(test)]
#[path = "channels_tests.rs"]
mod tests;
