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
//! **Client to server.** A channel registered with a direction of `Direction::ToServer` carries messages the other way, in a
//! section at the front of each client data packet, ahead of the moves. It exists on the connections whose downstream channels
//! are enabled (the manifest matched, and the manifest lists the upstream channels too), and nowhere else, so a peer that does
//! not list one keeps its bytes. The server reads it as hostile input (`UpstreamCaps`: held bytes per connection and in all,
//! message size, messages in flight, rate); a cap that is passed drops the connection with the packet error. The client sends
//! through `ClientChannels::send`, which budgets the section so moves and events keep their room.
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
    ChannelEntry, ChannelError, ChannelRegistry, ChunkChannel, Direction, MAX_BUDGET, MIN_BUDGET,
    UPSTREAM_NAME_PREFIX,
};
pub use torque_net::chunk::upstream::UpstreamCaps;
use torque_net::chunk::upstream::{UpstreamReader, UpstreamServer};
use torque_net::chunk::{ChunkReceiver, ChunkSender, Kind, MessageHandle, SendError};
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
    /// The client-to-server channels' logic (one copy for all clients).
    up_logic: Vec<Option<Box<dyn ChunkChannel>>>,
    /// The caps and shared budget of the client-to-server channels; `None` when there are none.
    up: Option<UpstreamServer>,
    /// The readers of the connections with channels on, when there are client-to-server channels.
    up_conns: HashMap<ConnId, UpstreamReader>,
    /// The `GameConnection` of each accepted connection, to drop one the upstream caps ended.
    objs: HashMap<ConnId, ObjId>,
    /// Connections already asked to be dropped for a dead upstream reader.
    drop_asked: Vec<ConnId>,
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
                up_logic: registry.make_up(),
                up: (registry.up_len() > 0).then(|| UpstreamServer::new(UpstreamCaps::default())),
                up_conns: HashMap::new(),
                objs: HashMap::new(),
                drop_asked: Vec::new(),
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

    /// Sets what the server takes from clients' client-to-server channels. Call it before clients connect: a connection keeps
    /// the caps (and the budget shared with the others) it was enabled under.
    pub fn set_upstream_caps(&self, caps: UpstreamCaps) {
        let mut st = self.state.borrow_mut();
        if st.up.is_some() {
            st.up = Some(UpstreamServer::new(caps));
        }
    }

    /// The bytes `client` holds for half-received upstream messages.
    pub fn upstream_held(&self, client: ConnId) -> usize {
        self.state
            .borrow()
            .up_conns
            .get(&client)
            .map_or(0, UpstreamReader::held_bytes)
    }

    /// The bytes all clients hold together for half-received upstream messages.
    pub fn upstream_held_total(&self) -> usize {
        self.state
            .borrow()
            .up
            .as_ref()
            .map_or(0, UpstreamServer::held_total)
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
            if let Some(up) = &st.up {
                let reader = up.connection(&st.registry.up_limits());
                st.up_conns.insert(client, reader);
            }
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

    /// Queues a message that other clients are sent too (`data` is held once and shared) for `client` on `channel`.
    pub fn send_shared(
        &self,
        client: ConnId,
        channel: &str,
        kind: Kind,
        data: std::sync::Arc<[u8]>,
    ) -> Result<(), SendError> {
        self.send_shared_with_handle(client, channel, kind, data)
            .map(|_| ())
    }

    /// [`send_shared`](Self::send_shared) that returns a handle to the queued message, for
    /// [`queued_message_len`](Self::queued_message_len).
    pub fn send_shared_with_handle(
        &self,
        client: ConnId,
        channel: &str,
        kind: Kind,
        data: std::sync::Arc<[u8]>,
    ) -> Result<MessageHandle, SendError> {
        let mut st = self.state.borrow_mut();
        let Some(id) = st.registry.id(channel) else {
            return Err(SendError::UnknownChannel);
        };
        match st.conns.get_mut(&client) {
            Some(c) => c.sender.enqueue_shared_with_handle(id, kind, data),
            None => Err(SendError::UnknownChannel),
        }
    }

    /// The length of the message `handle` names while it is still queued for `client` and not fully acknowledged, else 0.
    pub fn queued_message_len(&self, client: ConnId, handle: MessageHandle) -> usize {
        self.state
            .borrow()
            .conns
            .get(&client)
            .map_or(0, |c| c.sender.queued_len(handle))
    }

    /// The bytes queued for `client` on every channel and not yet acknowledged.
    pub fn backlog_total(&self, client: ConnId) -> usize {
        self.state
            .borrow()
            .conns
            .get(&client)
            .map_or(0, |c| c.sender.total_backlog())
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

    fn accepted(&self, conn: ConnId, obj: ObjId) {
        let mut st = self.state.borrow_mut();
        st.known.push(conn);
        st.objs.insert(conn, obj);
    }

    /// Once a tick for a connection: its upstream reader looks for an eviction or a stale message and frees what it held.
    fn sweep_upstream(&self, conn: ConnId, now: u32) {
        if let Some(reader) = self.state.borrow_mut().up_conns.get_mut(&conn) {
            let _ = reader.sweep(now);
        }
    }

    /// The drop of `conn` when its upstream reader is dead (evicted for the room it held, or a message went stale), once.
    fn dead_drop(&self, conn: ConnId) -> Option<Followup> {
        let mut st = self.state.borrow_mut();
        if !st.up_conns.get(&conn).is_some_and(UpstreamReader::is_dead)
            || st.drop_asked.contains(&conn)
        {
            return None;
        }
        let obj = *st.objs.get(&conn)?;
        st.drop_asked.push(conn);
        Some(Followup::Drop {
            obj,
            reason: b"Packet Error.".to_vec(),
        })
    }

    fn peer(&self, conn: ConnId) {
        let on = self.state.borrow().enable_peers;
        if on {
            self.enable(conn);
        }
    }

    fn dropped(&self, conn: ConnId) {
        let had_up = {
            let mut st = self.state.borrow_mut();
            st.known.retain(|c| *c != conn);
            st.objs.remove(&conn);
            st.drop_asked.retain(|c| *c != conn);
            st.conns.remove(&conn);
            st.up_conns.remove(&conn).is_some()
        };
        if had_up {
            self.each_up_logic(|logic| logic.disconnected(conn));
        }
    }

    /// Runs `f` on each client-to-server channel's logic, with nothing borrowed while it runs.
    fn each_up_logic(&self, mut f: impl FnMut(&mut dyn ChunkChannel)) {
        let count = self.state.borrow().up_logic.len();
        for at in 0..count {
            let taken = self
                .state
                .borrow_mut()
                .up_logic
                .get_mut(at)
                .and_then(Option::take);
            if let Some(mut logic) = taken {
                f(logic.as_mut());
                if let Some(slot) = self.state.borrow_mut().up_logic.get_mut(at) {
                    *slot = Some(logic);
                }
            }
        }
    }

    /// Reads the upstream section of a data packet from `conn` and hands the messages that arrived whole to their channels'
    /// logic. An error means the client went over a cap or sent something no client of this engine sends.
    fn read_upstream(
        &self,
        conn: ConnId,
        now: u32,
        payload: &mut BitReader<'_>,
    ) -> Result<(), NetError> {
        let mut delivered = Vec::new();
        {
            let mut st = self.state.borrow_mut();
            let Some(reader) = st.up_conns.get_mut(&conn) else {
                return Ok(());
            };
            reader.read(payload, now, &mut delivered)?;
        }
        for d in delivered {
            let taken = self
                .state
                .borrow_mut()
                .up_logic
                .get_mut(d.channel)
                .and_then(Option::take);
            if let Some(mut logic) = taken {
                logic.receive_from(conn, d.kind, &d.data);
                if let Some(slot) = self.state.borrow_mut().up_logic.get_mut(d.channel) {
                    *slot = Some(logic);
                }
            }
        }
        Ok(())
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
        // A client of a connection with upstream channels starts its payload with their section.
        if let Err(error) = self.hub.read_upstream(ctx.conn, ctx.now, payload) {
            return PacketOutcome {
                followups: Vec::new(),
                error: Some(error),
            };
        }
        self.inner.on_game_packet(ctx, received, payload)
    }

    fn on_notify(&mut self, ctx: &mut ConnCtx<'_>, notify: Notify) {
        if let Some(c) = self.hub.state.borrow_mut().conns.get_mut(&ctx.conn) {
            c.sender.on_notify(notify.send_seq, notify.delivered);
        }
        self.inner.on_notify(ctx, notify);
    }

    fn on_accept(&mut self, conn: ConnId, obj: ObjId, ghost_bits: u8) {
        self.hub.accepted(conn, obj);
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
        self.hub.sweep_upstream(ctx.conn, ctx.now);
        {
            let mut st = self.hub.state.borrow_mut();
            // A connection whose channels all go the other way has no section to write.
            if let Some(c) = st
                .conns
                .get_mut(&ctx.conn)
                .filter(|c| c.sender.channels() > 0)
            {
                let limit = ctx.protocol.packet_limit_bits();
                let share = limit / 100 * self.share_percent.min(100);
                // A connection in a burst (it has bulk data to move and no events or ghosts yet) gives the channels the whole packet.
                let burst = ctx.protocol.is_burst();
                let share = if burst { limit } else { share };
                let room = share
                    .min(limit.saturating_sub(w.bits()))
                    .min(w.remaining_bits());
                let yielding =
                    !burst && self.yield_every > 0 && c.since_yield + 1 >= self.yield_every;
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
        let mut calls = self.inner.take_calls(conn);
        calls.extend(self.hub.dead_drop(conn));
        calls
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
    /// The send side of the client-to-server channels (none when the engine has none).
    up: RefCell<Option<UpSend>>,
    /// The backlog the upstream sender may queue, for a connection's next start.
    up_backlog: Cell<usize>,
}

/// A client's send side of the client-to-server channels.
struct UpSend {
    sender: ChunkSender,
    /// Data packets written since the last one that carried no chunks.
    since_yield: u32,
}

/// The share of a client packet the upstream section may take, in percent.
pub const UPSTREAM_SHARE_PERCENT: usize = 50;
/// Every this many client packets one carries no upstream chunks (the events of the packets in between may be large).
pub const UPSTREAM_YIELD_EVERY: u32 = 4;

impl ClientChannels {
    /// The client's channels for `registry`, off.
    pub fn new(registry: &ChannelRegistry) -> ClientChannels {
        let channels = ClientChannels {
            receiver: RefCell::new(ChunkReceiver::new(&registry.limits())),
            logic: RefCell::new(registry.make_all()),
            registry: registry.clone(),
            active: Cell::new(false),
            up: RefCell::new(None),
            up_backlog: Cell::new(torque_net::chunk::upstream::DEFAULT_UPSTREAM_HOLD),
        };
        channels.start_upstream();
        channels
    }

    /// A new sender for the client-to-server channels, if there are any.
    fn start_upstream(&self) {
        let limits = self.registry.up_limits();
        *self.up.borrow_mut() = (!limits.is_empty()).then(|| UpSend {
            sender: ChunkSender::new(&limits).with_total_backlog(self.up_backlog.get()),
            since_yield: 0,
        });
    }

    /// Sets the most bytes this client queues across its client-to-server channels, from the next connection on (default
    /// [`DEFAULT_UPSTREAM_HOLD`](torque_net::chunk::upstream::DEFAULT_UPSTREAM_HOLD), what a server holds for one connection
    /// unless it is set higher). A client that queues more than its server holds is dropped, so a game that raises the
    /// server's cap ([`UpstreamCaps::hold_per_connection`]) raises this to match.
    pub fn set_upstream_backlog(&self, bytes: usize) {
        self.up_backlog.set(bytes);
    }

    /// Queues a message on the client-to-server channel `name` for the server. Refused with a [`SendError`], and nothing
    /// queued, when the connection's channels are not on, there is no such channel, or the message or the backlog is over a cap.
    pub fn send(&self, name: &str, kind: Kind, data: Vec<u8>) -> Result<(), SendError> {
        let Some(id) = self.registry.up_id(name) else {
            return Err(SendError::UnknownChannel);
        };
        if !self.active.get() {
            return Err(SendError::UnknownChannel);
        }
        match self.up.borrow_mut().as_mut() {
            Some(up) => up.sender.enqueue(id, kind, data),
            None => Err(SendError::UnknownChannel),
        }
    }

    /// Whether the connection writes the upstream section: the channels are on and the engine has client-to-server ones.
    pub fn writes_upstream(&self) -> bool {
        self.active.get() && self.up.borrow().is_some()
    }

    /// The bytes queued on the client-to-server channels and not yet acknowledged.
    pub fn upstream_backlog(&self) -> usize {
        self.up
            .borrow()
            .as_ref()
            .map_or(0, |u| u.sender.total_backlog())
    }

    /// Whether nothing is queued upstream and nothing is in a packet that might yet be lost.
    pub fn upstream_idle(&self) -> bool {
        self.up.borrow().as_ref().is_none_or(|u| u.sender.is_idle())
    }

    /// Writes the upstream section at the front of a client data packet in `w` (the header and rate prefix are written, the
    /// moves are not), when the connection has one ([`writes_upstream`](Self::writes_upstream)). `send_seq` names the
    /// packet, `packet_bits` is its limit and `reserve_bits` what the moves need after the section. The chunks take at most
    /// [`UPSTREAM_SHARE_PERCENT`] of the packet and every [`UPSTREAM_YIELD_EVERY`]th packet none, so the moves and the events
    /// keep their room.
    pub fn write_upstream(
        &self,
        w: &mut BitWriter,
        send_seq: u32,
        packet_bits: usize,
        reserve_bits: usize,
    ) {
        if !self.active.get() {
            return;
        }
        let mut up = self.up.borrow_mut();
        let Some(up) = up.as_mut() else {
            return;
        };
        let share = packet_bits / 100 * UPSTREAM_SHARE_PERCENT;
        let room = share
            .min(
                packet_bits
                    .saturating_sub(w.bits())
                    .saturating_sub(reserve_bits),
            )
            .min(w.remaining_bits());
        let yielding = up.since_yield + 1 >= UPSTREAM_YIELD_EVERY;
        let room = if yielding { room.min(1) } else { room };
        let written = up.sender.write(w, send_seq, room.max(1));
        up.since_yield = if yielding || written == 0 {
            0
        } else {
            up.since_yield + 1
        };
    }

    /// The fate of the client data packet sent as `send_seq`: its upstream chunks are acknowledged or go again.
    pub fn on_notify(&self, send_seq: u32, delivered: bool) {
        if let Some(up) = self.up.borrow_mut().as_mut() {
            up.sender.on_notify(send_seq, delivered);
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
    /// channels' logic keeps its state (the next snapshot replaces it) unless it overrides [`ChunkChannel::reset`].
    pub fn reset(&self) {
        *self.receiver.borrow_mut() = ChunkReceiver::new(&self.registry.limits());
        self.start_upstream();
        self.active.set(false);
        self.each_logic(ChunkChannel::reset);
    }

    /// The connection ended: off, and every channel's logic is told ([`ChunkChannel::closed`]).
    pub fn closed(&self) {
        self.active.set(false);
        self.each_logic(ChunkChannel::closed);
    }

    /// Runs `f` on each channel's logic, with nothing borrowed while it runs.
    fn each_logic(&self, f: fn(&mut dyn ChunkChannel)) {
        let count = self.logic.borrow().len();
        for at in 0..count {
            let taken = self.logic.borrow_mut().get_mut(at).and_then(Option::take);
            if let Some(mut logic) = taken {
                f(logic.as_mut());
                if let Some(slot) = self.logic.borrow_mut().get_mut(at) {
                    *slot = Some(logic);
                }
            }
        }
    }

    /// Runs `f` on the logic of channel `name` when it is a `T` (the channel returns itself from
    /// [`ChunkChannel::as_any_mut`]); `None` when there is no such channel, it is of another type, or it is running right now.
    /// Nothing is borrowed while `f` runs.
    pub fn with_channel<T: 'static, R>(
        &self,
        name: &str,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let at = self.registry.id(name)?;
        let mut logic = self.logic.borrow_mut().get_mut(at).and_then(Option::take)?;
        let out = logic
            .as_any_mut()
            .and_then(|any| any.downcast_mut::<T>())
            .map(f);
        if let Some(slot) = self.logic.borrow_mut().get_mut(at) {
            *slot = Some(logic);
        }
        out
    }

    /// Reads a packet's section when active, and gives each message that arrived whole to its channel. An error means the
    /// server sent something no server of this engine sends: the connection should be dropped.
    pub fn read(&self, r: &mut BitReader<'_>) -> Result<(), NetError> {
        // A connection whose channels all go to the server has no downstream section.
        if !self.active.get() || !self.registry.has_downstream() {
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
