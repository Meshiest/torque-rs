//! The server's half of the `props-frozen` channel: which frozen props each client gets as ghosts and which over the channel,
//! and the handover between the two ([`sync`], once a tick).
//!
//! **Who gets what.** A client is ghosted at most `$Pref::Props::FrozenGhosts` frozen props. The first ones to become frozen
//! for it (in id order when it first connects) take the places; every frozen prop after them is sent to that client over the
//! channel and hidden from its ghosts ([`ScopeControl`]). A prop that is not frozen, is a child or a parent in the scene
//! hierarchy, or holds a joint is always a ghost, whatever the count. A place a ghost leaves (its prop is unfrozen or
//! deleted) is taken by a frozen prop that is new to the client, never by one already on the channel, so a prop does not
//! change hands for no reason.
//!
//! **Handover** (a prop switches ways without a frame where it is drawn twice or not at all; a client draws a prop it holds
//! both ways from its ghost):
//!
//! - ghost to channel (a prop freezes with the places full): the set op is sent first, the ghost is kept until the message is
//!   acknowledged, and only then hidden;
//! - channel to ghost (a prop thaws, is hung on a parent or given a joint): the prop is shown again, and the remove op is sent
//!   once the client has acknowledged its ghost's first record;
//! - a prop the client has never been sent as a ghost (every prop a client first hears of that is already frozen, and every
//!   prop on the server when a client connects) is hidden at once, so no ghost is made only to be taken away;
//! - deleted or moved: a deleted prop's remove op follows its kill record, and a frozen prop whose state changed is sent again.
//!
//! A message the hub refuses (the client's backlog is full) changes nothing: the same ops are tried again at the next tick.
//!
//! **Cost.** A tick costs what changed, not what exists. The world logs the props it touched (frozen, thawed, made, removed,
//! hung, given a joint, given another owner), [`Snapshot::refresh`] looks at those only (and at every frozen prop when the
//! scene hierarchy's links changed), and each client looks at the props that changed, the ones it still owes a message or an
//! acknowledgement for, and nothing else. A server whose props sit still does no work here. The ghosts a mass thaw owes
//! acknowledgements for are asked about a bounded number a tick ([`ACK_CHECKS_PER_TICK`]), and the props the clients look at
//! are bounded over all of them ([`DEAL_STEPS_PER_TICK`]): a client that joins, or a mass thaw, is spread over the ticks, the
//! clients taking turns first.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use torque_engine::VmPrefs;
use torque_engine::scope::{Audience, ScopeControl};
use torque_net::chunk::{Kind, MessageHandle, SendError};
use torque_net::handshake::ConnId;
use torque_plugin_hierarchy::ServerHierarchy;
use torque_server::net::channels::ChannelHub;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::messages::{self, Frozen, Op};
use super::{CHANNEL, MAX_OPS_PER_SEND, SEND_BACKLOG};
use crate::quant::Quantized;
use crate::server::{PropPrefs, ServerProps};
use crate::world::{Link, MAX_PROPS, PropWorld};

/// The most acknowledgements of returning ghosts a tick asks about, over all clients.
pub const ACK_CHECKS_PER_TICK: usize = 4096;

/// The most props whose scope is looked at again in a tick, over all clients, for want of a change to hear of.
pub const GATE_SWEEP_PER_TICK: usize = 64;

/// The most props the clients' deals look at in a tick, over all clients: what a client that joins, or a mass thaw, owes more
/// than this waits in the clients' lists for the ticks after, in turn.
pub const DEAL_STEPS_PER_TICK: usize = 4096;
/// What a client is owed or told, as the server sends it: the hub, and the scope control and ghosts a handover reads.
pub trait Port {
    /// The bytes queued for `conn` on the channel and not yet acknowledged.
    fn backlog(&self, conn: ConnId) -> usize;
    /// Queues a message for `conn`.
    fn send(&self, conn: ConnId, bytes: Arc<[u8]>) -> Result<MessageHandle, SendError>;
    /// Whether the message `handle` was fully acknowledged by `conn`.
    fn delivered(&self, conn: ConnId, handle: MessageHandle) -> bool;
    /// Keeps `prop` out of the ghosts of the connection `conn_obj`; false when the control had no room.
    fn hide(&self, conn_obj: ObjId, prop: u32) -> bool;
    /// Lets `prop` back into the ghosts of the connection `conn_obj`.
    fn show(&self, conn_obj: ObjId, prop: u32);
    /// Whether `conn_obj` acknowledged the first record of its ghost of `prop`.
    fn ghost_acked(&self, conn_obj: ObjId, prop: u32) -> bool;
    /// Which of `props` the connection `conn_obj` may be sent: its scope takes them (it would be ghosted them, apart from what
    /// plugins keep out) and no plugin but this one keeps them out of its ghosts.
    fn visible_many(&self, conn_obj: ObjId, props: &[u32]) -> Vec<bool>;
    /// What other plugins hid or showed since `cursor` (none: start now); the place to read from next, or an error with it
    /// when changes were forgotten.
    ///
    /// # Errors
    ///
    /// The log forgot changes since `cursor`; the error is where to read from now.
    fn scope_changes(
        &self,
        cursor: Option<u64>,
        out: &mut Vec<(ObjId, Audience)>,
    ) -> Result<u64, u64>;
    /// Whether the scope of `conn_obj` can change with no change of the objects (so the props it holds are looked at again
    /// from time to time). `via` are props to reach the ghosts' layer through (a connection object has no link of its own; every
    /// prop shares the server's one ghost layer, so the first of them that has a link is enough).
    fn scope_varies(&self, conn_obj: ObjId, via: &[u32]) -> bool;
}

/// The name this plugin hides props under in the scope control.
pub const SCOPE_USER: &str = "torque.props";

/// The server's own port: the channel hub, the scope control and the ghosts of the script engine.
struct LivePort<'a> {
    hub: &'a ChannelHub,
    scope: &'a ScopeControl,
    vm: &'a Vm,
}

impl Port for LivePort<'_> {
    fn backlog(&self, conn: ConnId) -> usize {
        self.hub.backlog(conn, CHANNEL)
    }

    fn send(&self, conn: ConnId, bytes: Arc<[u8]>) -> Result<MessageHandle, SendError> {
        self.hub
            .send_shared_with_handle(conn, CHANNEL, Kind::Delta, bytes)
    }

    fn delivered(&self, conn: ConnId, handle: MessageHandle) -> bool {
        self.hub.queued_message_len(conn, handle) == 0
    }

    fn hide(&self, conn_obj: ObjId, prop: u32) -> bool {
        self.scope
            .hide_as(self.vm, SCOPE_USER, prop, Audience::Connection(conn_obj))
            .is_ok()
    }

    fn show(&self, conn_obj: ObjId, prop: u32) {
        self.scope
            .show_as(SCOPE_USER, prop, Audience::Connection(conn_obj));
    }

    fn ghost_acked(&self, conn_obj: ObjId, prop: u32) -> bool {
        torque_game::scene::ext::get(self.vm.objects(), prop)
            .and_then(|e| e.ghost.clone())
            .is_some_and(|link| link.ghost_index_on(conn_obj, prop).is_some())
    }

    fn visible_many(&self, conn_obj: ObjId, props: &[u32]) -> Vec<bool> {
        let link = props.iter().find_map(|&p| {
            torque_game::scene::ext::get(self.vm.objects(), p).and_then(|e| e.ghost.clone())
        });
        let scoped = match &link {
            Some(link) => link.scopes(self.vm, conn_obj, props),
            None => vec![true; props.len()],
        };
        props
            .iter()
            .zip(scoped)
            .map(|(&p, in_scope)| {
                in_scope && !self.scope.is_hidden_by_other(SCOPE_USER, p, Some(conn_obj))
            })
            .collect()
    }

    fn scope_changes(
        &self,
        cursor: Option<u64>,
        out: &mut Vec<(ObjId, Audience)>,
    ) -> Result<u64, u64> {
        match cursor {
            None => Ok(self.scope.log_end()),
            Some(at) => self.scope.changes_since(SCOPE_USER, at, out),
        }
    }

    fn scope_varies(&self, conn_obj: ObjId, via: &[u32]) -> bool {
        via.iter()
            .find_map(|&prop| {
                torque_game::scene::ext::get(self.vm.objects(), prop)?
                    .ghost
                    .clone()
            })
            .is_some_and(|link| link.scope_varies(self.vm, conn_obj))
    }
}

/// What a frozen prop sent the channel's way is, for one client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Hidden from the client's ghosts, and the client does not have it yet (the hub refused the message).
    Unsent,
    /// The client was sent it; its ghost, if it has one, is kept until the message is acknowledged.
    Adding(MessageHandle),
    /// Hidden, and the client has it.
    Live,
    /// Shown again to the client's ghosts; the client's copy goes when its ghost's first record is acknowledged.
    Returning,
}

/// A prop on the channel for one client.
#[derive(Clone, Copy, Debug)]
struct Entry {
    slot: u16,
    sent: Option<Frozen>,
    stage: Stage,
}

/// What the server holds for one client.
#[derive(Debug)]
struct Client {
    obj: ObjId,
    /// The tick the client was first dealt: every prop it is dealt then is one it was never sent as a ghost.
    joined: u64,
    /// The frozen props it gets as ghosts.
    ghosted: BTreeSet<u32>,
    /// The frozen props it gets over the channel.
    channel: BTreeMap<u32, Entry>,
    /// The props to look at this tick: those that changed, and those a message the hub refused is owed for.
    todo: BTreeSet<u32>,
    /// The props that were never exposed to this client (it is new, or they are) and are not placed yet, with the tick that
    /// was true: a deal put off still hides them before a ghost is made, unless one was.
    fresh: BTreeMap<u32, u64>,
    /// Where the props it was not dealt yet start (its first deal is every prop there is, a few thousand a tick).
    seed: Option<u32>,
    /// The place in the snapshot's log of the next change it has not looked at.
    cursor: u64,
    /// The frozen props it is not dealt because its scope does not take them (or another plugin keeps them from it): looked
    /// at again when that changes.
    unseen: BTreeSet<u32>,
    /// Where the next look at the props it holds for want of a change in scope starts.
    sweep: u32,
    /// Whether its scope could change by itself when it was last looked at: when that stops (its scope is in place), every
    /// prop it holds or keeps from itself is looked at once more.
    varied: bool,
    /// The messages a prop's ghost waits on, with the props they carried (each is hidden once its message is acknowledged).
    adding: Vec<(MessageHandle, Vec<u32>)>,
    /// The props shown again that wait for their ghost to be acknowledged, and where the next look at them starts.
    awaiting: BTreeSet<u32>,
    ack_cursor: u32,
}

impl Client {
    fn new(obj: ObjId, joined: u64) -> Client {
        Client {
            obj,
            joined,
            ghosted: BTreeSet::new(),
            channel: BTreeMap::new(),
            todo: BTreeSet::new(),
            fresh: BTreeMap::new(),
            seed: Some(0),
            cursor: 0,
            unseen: BTreeSet::new(),
            sweep: 0,
            varied: false,
            adding: Vec::new(),
            awaiting: BTreeSet::new(),
            ack_cursor: 0,
        }
    }
}

/// What the channel has done, added up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrozenStats {
    /// Set and remove ops sent.
    pub sets: u64,
    pub removes: u64,
    /// Props that went from a ghost to the channel, and from the channel back to a ghost (the remove op queued).
    pub to_channel: u64,
    pub to_ghost: u64,
    /// Messages the hub refused.
    pub refused: u64,
    /// Hides the scope control refused for want of room (their props stay ghosts), as of the last tick.
    pub hide_refused: u64,
}

impl std::ops::AddAssign for FrozenStats {
    fn add_assign(&mut self, other: FrozenStats) {
        self.sets += other.sets;
        self.removes += other.removes;
        self.to_channel += other.to_channel;
        self.to_ghost += other.to_ghost;
        self.refused += other.refused;
        self.hide_refused = self.hide_refused.max(other.hide_refused);
    }
}

/// The work the sync has done, added up: what a tick costs is how these grow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrozenWork {
    /// Props the snapshot looked at again.
    pub props_looked_at: u64,
    /// Props a client's deal looked at (a prop is counted once for each client).
    pub client_steps: u64,
    /// Ghosts asked about for acknowledgement.
    pub ack_checks: u64,
}

/// One client's frozen props, as a test or a game asks about them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientCount {
    pub conn: ConnId,
    /// Frozen props it is ghosted.
    pub ghosted: usize,
    /// Frozen props on the channel for it, in any stage.
    pub channel: usize,
    /// Of those, the ones the client holds and no ghost of them is kept for it.
    pub live: usize,
}

/// What a frozen prop that may go over the channel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Eligible {
    slot: u16,
    data: u32,
    q: Quantized,
    owner: Option<u32>,
}

/// A prop whose being there, being eligible or state changed, and the tick it was first seen when that is the change.
#[derive(Clone, Copy, Debug)]
struct Change {
    id: u32,
    born: Option<u64>,
}

/// The most changes the log keeps for a client that is behind: past it the client is dealt every prop again.
const LOG_CAP: usize = 8 * MAX_PROPS as usize;

/// The props as the sync keeps them between ticks: which exist, which are frozen, which of those may go over the channel
/// (frozen, no parent or child in the scene hierarchy, no joint), and what changed this tick.
#[derive(Debug, Default)]
pub struct Snapshot {
    present: BTreeSet<u32>,
    frozen: BTreeSet<u32>,
    eligible: BTreeMap<u32, Eligible>,
    /// The props first seen this tick, and those whose being there or being eligible or whose state changed (read into the
    /// log at the end of the look).
    born: BTreeSet<u32>,
    changed: BTreeSet<u32>,
    /// What changed, tick after tick, for each client to read from where it is ([`Change`]); `log_base` is the place of its
    /// first.
    log: VecDeque<Change>,
    log_base: u64,
    /// The hierarchy's link revision the eligible props were last checked against.
    link_revision: Option<u64>,
    /// The first look is at every prop.
    seeded: bool,
}

impl Snapshot {
    /// The place the next change will have in the log.
    fn log_end(&self) -> u64 {
        self.log_base + self.log.len() as u64
    }

    /// Lets go of the changes every client has read (`read`: the place of the first change one has not; none: there is no
    /// client), and of the oldest when a client lags past [`LOG_CAP`] (it is dealt every prop again).
    fn prune(&mut self, read: Option<u64>) {
        let end = self.log_end();
        let upto = read.unwrap_or(end).min(end);
        let forced = end.saturating_sub(LOG_CAP as u64);
        let drop = (upto.max(forced).max(self.log_base) - self.log_base) as usize;
        self.log.drain(..drop);
        self.log_base += drop as u64;
    }

    /// Looks again at the props the world touched since the last tick (every prop the first time, and every frozen prop when
    /// the hierarchy's links changed). Gives the number of props looked at.
    pub fn refresh(
        &mut self,
        world: &mut PropWorld,
        hierarchy: Option<&ServerHierarchy>,
        tick: u64,
    ) -> usize {
        self.born.clear();
        self.changed.clear();
        let mut look: BTreeSet<u32> = world.take_touched();
        if !self.seeded {
            self.seeded = true;
            look.extend(world.props().keys().copied());
        }
        let links = hierarchy.map(ServerHierarchy::link_revision);
        if links != self.link_revision {
            self.link_revision = links;
            look.extend(self.frozen.iter().copied());
        }
        if look.is_empty() {
            return 0;
        }
        // The props that hold a joint: read once for the tick, and only when a look needs it.
        let jointed: BTreeSet<u32> = world.joints().values().flat_map(|j| j.props()).collect();
        for &id in &look {
            self.look_at(world, hierarchy, &jointed, id);
        }
        for &id in &self.changed {
            let born = self.born.contains(&id).then_some(tick);
            self.log.push_back(Change { id, born });
        }
        look.len()
    }

    fn look_at(
        &mut self,
        world: &PropWorld,
        hierarchy: Option<&ServerHierarchy>,
        jointed: &BTreeSet<u32>,
        id: u32,
    ) {
        let Some(p) = world.get(id) else {
            let was = self.present.remove(&id);
            self.frozen.remove(&id);
            if self.eligible.remove(&id).is_some() || was {
                self.changed.insert(id);
            }
            return;
        };
        if self.present.insert(id) {
            self.born.insert(id);
            self.changed.insert(id);
        }
        if p.rest.is_frozen() {
            self.frozen.insert(id);
        } else {
            self.frozen.remove(&id);
        }
        let may = p.rest.is_frozen()
            && p.link == Link::Free
            && !jointed.contains(&id)
            && !hierarchy.is_some_and(|h| h.parent(id).is_some() || h.child_count(id) > 0);
        let now = may.then(|| Eligible {
            slot: p.slot,
            data: p.data,
            q: p.q.stopped(),
            owner: p.owner.as_ref().and_then(|o| o.conn),
        });
        let before = self.eligible.get(&id).copied();
        if before != now {
            self.changed.insert(id);
            match now {
                Some(e) => self.eligible.insert(id, e),
                None => self.eligible.remove(&id),
            };
        }
    }
}

/// The server's state for the channel.
#[derive(Debug, Default)]
pub struct FrozenSync {
    clients: BTreeMap<ConnId, Client>,
    stats: FrozenStats,
    work: FrozenWork,
    snap: Snapshot,
    tick: u64,
    /// Where in the scope control's log the other plugins' changes were read to.
    scope_cursor: Option<u64>,
    /// Where the next tick's deals start among the clients, so none is always the last to be served.
    turn: usize,
    /// The console has been told that the scope control ran out of room.
    warned: bool,
}

impl FrozenSync {
    /// What the channel has done.
    #[must_use]
    pub fn stats(&self) -> FrozenStats {
        self.stats
    }

    /// The work done so far.
    #[must_use]
    pub fn work(&self) -> FrozenWork {
        self.work
    }

    /// Each client's counts, by connection.
    #[must_use]
    pub fn counts(&self) -> Vec<ClientCount> {
        self.clients
            .iter()
            .map(|(&conn, c)| ClientCount {
                conn,
                ghosted: c.ghosted.len(),
                channel: c.channel.len(),
                live: c
                    .channel
                    .values()
                    .filter(|e| e.stage == Stage::Live)
                    .count(),
            })
            .collect()
    }

    /// The slots of the props on the channel for the client whose connection object is `obj`, in any stage.
    #[must_use]
    pub fn channel_slots(&self, obj: ObjId) -> Vec<u16> {
        self.clients
            .values()
            .find(|c| c.obj == obj)
            .map_or_else(Vec::new, |c| c.channel.values().map(|e| e.slot).collect())
    }

    /// Forgets everything: the next tick looks at every prop again (the channel is not in use, so nothing was kept up to date).
    fn forget(&mut self, world: Option<&mut PropWorld>) {
        self.clients.clear();
        self.snap = Snapshot::default();
        if let Some(world) = world {
            world.take_touched();
        }
    }

    /// One tick: the snapshot brought up to date, then every client in `live` dealt with. `threshold` is the most frozen
    /// props a client is ghosted.
    pub fn run(
        &mut self,
        world: &mut PropWorld,
        hierarchy: Option<&ServerHierarchy>,
        live: &[(ConnId, ObjId)],
        threshold: usize,
        port: &dyn Port,
    ) {
        self.tick += 1;
        self.work.props_looked_at += self.snap.refresh(world, hierarchy, self.tick) as u64;
        self.clients
            .retain(|conn, c| live.iter().any(|&(l, obj)| l == *conn && obj == c.obj));
        let budget = (ACK_CHECKS_PER_TICK / live.len().max(1)).max(64);
        let tick = Tick {
            snap: &self.snap,
            number: self.tick,
            threshold,
            budget,
            sweep_share: (GATE_SWEEP_PER_TICK / live.len().max(1)).max(1),
            scope_changed: &[],
            regate: false,
            probe: self.snap.eligible.keys().take(8).copied().collect(),
            port,
        };
        let mut scope_changed = Vec::new();
        let regate = match port.scope_changes(self.scope_cursor, &mut scope_changed) {
            Ok(next) => {
                self.scope_cursor = Some(next);
                false
            }
            Err(next) => {
                self.scope_cursor = Some(next);
                true
            }
        };
        let tick = Tick {
            scope_changed: &scope_changed,
            regate,
            ..tick
        };
        let mut steps = DEAL_STEPS_PER_TICK;
        let start = self.turn % live.len().max(1);
        self.turn = self.turn.wrapping_add(1);
        for &(conn, obj) in live.iter().cycle().skip(start).take(live.len()) {
            let client = self
                .clients
                .entry(conn)
                .or_insert_with(|| Client::new(obj, tick.number));
            let mut stats = FrozenStats::default();
            let mut work = FrozenWork::default();
            deal(client, conn, &tick, &mut steps, &mut stats, &mut work);
            self.stats += stats;
            self.work.client_steps += work.client_steps;
            self.work.ack_checks += work.ack_checks;
        }
        self.snap
            .prune(self.clients.values().map(|c| c.cursor).min());
    }
}

/// The ops a client is owed this tick, and what to do to its state once the hub takes them.
#[derive(Default)]
struct Batch {
    items: Vec<(u32, Op, Act)>,
}

enum Act {
    /// A prop whose ghost is kept until the set op is acknowledged.
    Add { slot: u16, sent: Frozen },
    /// The state of a prop on the channel sent again (or a hidden one sent for the first time).
    Send { sent: Frozen },
    /// A prop whose entry ends with its remove op.
    Drop,
}

impl Batch {
    fn push(&mut self, id: u32, op: Op, act: Act) {
        self.items.push((id, op, act));
    }
}

/// What a tick deals every client from.
struct Tick<'a> {
    snap: &'a Snapshot,
    number: u64,
    /// The most frozen props a client is ghosted.
    threshold: usize,
    /// The most acknowledgements of returning ghosts one client's turn asks about.
    budget: usize,
    /// The most props a client's scope is looked at again for, when it can change by itself.
    sweep_share: usize,
    /// What other plugins hid or showed since the last tick, and whether the log forgot some (then every prop is looked at again).
    scope_changed: &'a [(ObjId, Audience)],
    regate: bool,
    /// A few props to find the ghosts' layer through ([`Port::scope_varies`]).
    probe: Vec<u32>,
    port: &'a dyn Port,
}

/// One client's tick: the props that changed or that it still owes, the ghosts it waits on and the message.
fn deal(
    client: &mut Client,
    conn: ConnId,
    tick: &Tick<'_>,
    steps: &mut usize,
    stats: &mut FrozenStats,
    work: &mut FrozenWork,
) {
    let mut batch = Batch::default();
    if client.joined == tick.number || client.cursor < tick.snap.log_base {
        // New, or so far behind that the log forgot what it has not read: it is dealt every prop there is.
        client.cursor = tick.snap.log_end();
        client.seed = Some(0);
        client.todo.clear();
        // The props it holds, ghosts of or keeps from itself may be gone or changed in what the log forgot, and the seed only
        // reaches the eligible ones: they are looked at again too.
        let held: Vec<u32> = client
            .channel
            .keys()
            .chain(&client.unseen)
            .chain(&client.ghosted)
            .copied()
            .collect();
        client.todo.extend(held);
    }
    rewatch_scope(client, tick);
    acknowledged_ghosts(client, tick, &mut batch, stats, work);
    delivered_messages(client, conn, tick, stats);
    // The list as it stood: what a step below puts on the client's list is for the next tick.
    let mut owed = std::mem::take(&mut client.todo);
    let mut now = Vec::new();
    read_changes(client, tick, steps, &mut owed, &mut now);
    seed(client, tick, steps, &mut owed, &mut now);
    // What is past the tick's budget stays on the list.
    let listed: Vec<u32> = owed.iter().take(*steps).copied().collect();
    for id in &listed {
        owed.remove(id);
    }
    *steps -= listed.len();
    client.todo.extend(owed);
    now.extend(listed);
    // The scope is asked once for all of them.
    let visible = tick.port.visible_many(client.obj, &now);
    for (id, visible) in now.into_iter().zip(visible) {
        work.client_steps += 1;
        step(client, id, visible, tick, &mut batch);
    }
    send(client, conn, batch, tick.port, stats);
}

/// Puts on the client's list the props it holds, or keeps from itself, that a change of scope may concern: those other plugins hid
/// or showed for it, all of them when the log of those forgot some, and, when its scope can change by itself, a few in turn.
fn rewatch_scope(client: &mut Client, tick: &Tick<'_>) {
    if tick.regate {
        let all: Vec<u32> = client
            .channel
            .keys()
            .chain(client.unseen.iter())
            .copied()
            .collect();
        client.todo.extend(all);
    }
    for &(id, audience) in tick.scope_changed {
        let mine = match audience {
            Audience::Everyone => true,
            Audience::Connection(c) => c == client.obj,
        };
        if mine && (client.channel.contains_key(&id) || client.unseen.contains(&id)) {
            client.todo.insert(id);
        }
    }
    let varies = tick.port.scope_varies(client.obj, &tick.probe);
    if std::mem::replace(&mut client.varied, varies) && !varies {
        // Its scope has settled (a scope object it lacked was set): what it was not sent may be taken now.
        let all: Vec<u32> = client
            .channel
            .keys()
            .chain(client.unseen.iter())
            .copied()
            .collect();
        client.todo.extend(all);
    }
    if tick.sweep_share == 0 || !varies {
        return;
    }
    let from = client.sweep;
    let mut next: Vec<u32> = client
        .channel
        .range(from..)
        .map(|(&id, _)| id)
        .take(tick.sweep_share)
        .chain(client.unseen.range(from..).copied().take(tick.sweep_share))
        .collect();
    next.sort_unstable();
    next.truncate(tick.sweep_share);
    client.sweep = next.last().map_or(0, |&last| last.wrapping_add(1));
    client.todo.extend(next);
}

/// Looks at the props the log says changed since the client last read it, as many as the tick's budget allows. The props the
/// client's seed has not reached are left to the seed, which finds them as they are by then; a prop made since is new to the
/// client, so it is hidden before a ghost of it is made.
fn read_changes(
    client: &mut Client,
    tick: &Tick<'_>,
    steps: &mut usize,
    owed: &mut BTreeSet<u32>,
    now: &mut Vec<u32>,
) {
    let snap = tick.snap;
    let below = client.seed.unwrap_or(u32::MAX);
    while client.cursor < snap.log_end() && *steps > 0 {
        let Change { id, born } = snap.log[(client.cursor - snap.log_base) as usize];
        client.cursor += 1;
        if id >= below {
            continue;
        }
        *steps -= 1;
        if let Some(since) = born.filter(|_| snap.eligible.contains_key(&id)) {
            client.fresh.insert(id, since);
        }
        owed.remove(&id);
        now.push(id);
    }
}

/// Deals a client that has not been dealt yet its next props, as many as the tick's budget allows: every prop it is dealt
/// this way was never exposed to it (its ghost is not made before the prop is hidden), unless a ghost of it was acknowledged
/// while it waited.
fn seed(
    client: &mut Client,
    tick: &Tick<'_>,
    steps: &mut usize,
    owed: &mut BTreeSet<u32>,
    now: &mut Vec<u32>,
) {
    let Some(from) = client.seed else {
        return;
    };
    let ids: Vec<u32> = tick
        .snap
        .eligible
        .range(from..)
        .take(*steps)
        .map(|(&id, _)| id)
        .collect();
    client.seed = match ids.last() {
        Some(&last) if ids.len() == *steps => last.checked_add(1),
        // No budget left: it starts here next time.
        None if *steps == 0 => Some(from),
        _ => None,
    };
    *steps -= ids.len();
    for id in ids {
        client.fresh.insert(id, client.joined);
        owed.remove(&id);
        now.push(id);
    }
}

/// The props shown again whose ghost the client has acknowledged leave the channel with a remove op.
fn acknowledged_ghosts(
    client: &mut Client,
    tick: &Tick<'_>,
    batch: &mut Batch,
    stats: &mut FrozenStats,
    work: &mut FrozenWork,
) {
    let looked: Vec<u32> = client
        .awaiting
        .range(client.ack_cursor..)
        .chain(client.awaiting.range(..client.ack_cursor))
        .take(tick.budget)
        .copied()
        .collect();
    for id in looked {
        work.ack_checks += 1;
        client.ack_cursor = id.wrapping_add(1);
        if !tick.port.ghost_acked(client.obj, id) {
            continue;
        }
        if let Some(entry) = client.channel.get(&id) {
            batch.push(id, Op::Remove { slot: entry.slot }, Act::Drop);
            stats.to_ghost += 1;
        }
    }
}

/// The props whose set op the client has acknowledged are hidden from its ghosts (the ghost they may have is kept until then).
fn delivered_messages(client: &mut Client, conn: ConnId, tick: &Tick<'_>, stats: &mut FrozenStats) {
    let mut waiting = Vec::new();
    for (handle, ids) in std::mem::take(&mut client.adding) {
        if !tick.port.delivered(conn, handle) {
            waiting.push((handle, ids));
            continue;
        }
        let mut refused = Vec::new();
        for id in ids {
            let Some(entry) = client.channel.get_mut(&id) else {
                continue;
            };
            if entry.stage != Stage::Adding(handle) {
                continue;
            }
            if tick.port.hide(client.obj, id) {
                entry.stage = Stage::Live;
                stats.to_channel += 1;
            } else {
                refused.push(id);
            }
        }
        // The control has no room for them: they stay ghosts, and are tried again.
        if !refused.is_empty() {
            waiting.push((handle, refused));
        }
    }
    client.adding = waiting;
}

/// What a prop is now, as a client's deal reads it.
struct Now {
    alive: bool,
    eligible: Option<Eligible>,
    /// The client may be sent it: its scope takes it and no other plugin keeps it from the client.
    visible: bool,
}

/// Brings one prop of one client up to date: what it is now against what the client holds of it.
fn step(client: &mut Client, id: u32, visible: bool, tick: &Tick<'_>, batch: &mut Batch) {
    let now = Now {
        alive: tick.snap.present.contains(&id),
        eligible: tick.snap.eligible.get(&id).copied(),
        visible,
    };
    if now.eligible.is_none() || client.channel.contains_key(&id) {
        client.fresh.remove(&id);
    }
    let Some(entry) = client.channel.get(&id).copied() else {
        return place(client, id, &now, tick, batch);
    };
    if !now.alive {
        return gone(client, id, entry, batch);
    }
    match entry.stage {
        Stage::Unsent => unsent(client, id, entry, &now, tick, batch),
        Stage::Adding(_) => adding(client, id, entry, &now, batch),
        Stage::Live => live(client, id, entry, &now, tick, batch),
        // Waits for its ghost's acknowledgement, whatever the prop became meanwhile.
        Stage::Returning => {}
    }
}

fn wire_of(client: &Client, e: &Eligible) -> Frozen {
    Frozen {
        data: e.data,
        mine: e.owner == Some(client.obj),
        q: e.q,
    }
}

/// A prop that has no entry: a frozen prop is given a place, anything else leaves the client's places.
fn place(client: &mut Client, id: u32, now: &Now, tick: &Tick<'_>, batch: &mut Batch) {
    let Some(e) = now.eligible else {
        client.ghosted.remove(&id);
        client.unseen.remove(&id);
        return;
    };
    let fresh_since = client.fresh.remove(&id);
    client.unseen.remove(&id);
    if client.ghosted.contains(&id) {
        return;
    }
    if !now.visible {
        // Out of the client's scope, or kept from it by another plugin: not sent, and looked at again when that changes.
        client.unseen.insert(id);
        return;
    }
    if client.ghosted.len() < tick.threshold {
        client.ghosted.insert(id);
        return;
    }
    let wire = wire_of(client, &e);
    let set = Op::Set {
        slot: e.slot,
        prop: wire,
    };
    // Never exposed to this client (it is new, or the prop is): hidden before a ghost is made. Otherwise it may have a ghost,
    // which stays until the client has the prop over the channel.
    let fresh = fresh_since
        .is_some_and(|since| since == tick.number || !tick.port.ghost_acked(client.obj, id));
    if !fresh {
        client.todo.insert(id);
        batch.push(
            id,
            set,
            Act::Add {
                slot: e.slot,
                sent: wire,
            },
        );
    } else if tick.port.hide(client.obj, id) {
        client.channel.insert(
            id,
            Entry {
                slot: e.slot,
                sent: None,
                stage: Stage::Unsent,
            },
        );
        client.todo.insert(id);
        batch.push(id, set, Act::Send { sent: wire });
    } else {
        // The scope control has no room: it is a ghost, as far as the table has room.
        client.ghosted.insert(id);
    }
}

/// A prop on the channel that is deleted: the client's copy goes (the object's ghost went with it).
fn gone(client: &mut Client, id: u32, entry: Entry, batch: &mut Batch) {
    client.awaiting.remove(&id);
    if entry.stage == Stage::Unsent {
        client.channel.remove(&id);
        return;
    }
    client.todo.insert(id);
    batch.push(id, Op::Remove { slot: entry.slot }, Act::Drop);
}

/// A prop hidden whose set op the hub has not taken: tried again, or its ghost comes back when it is no longer frozen.
fn unsent(
    client: &mut Client,
    id: u32,
    entry: Entry,
    now: &Now,
    tick: &Tick<'_>,
    batch: &mut Batch,
) {
    let Some(e) = now.eligible else {
        tick.port.show(client.obj, id);
        client.channel.remove(&id);
        return;
    };
    if !now.visible {
        tick.port.show(client.obj, id);
        client.channel.remove(&id);
        client.unseen.insert(id);
        return;
    }
    resend(client, id, entry, &e, batch, true);
}

/// A prop whose set op was sent while its ghost is kept: sent again when it changed, taken off when it is no longer frozen.
fn adding(client: &mut Client, id: u32, entry: Entry, now: &Now, batch: &mut Batch) {
    match now.eligible {
        Some(e) if now.visible => resend(client, id, entry, &e, batch, false),
        // No longer a frozen prop it may be sent, or out of the client's scope: the client's copy goes (a ghost it has is the
        // ghosts' to keep or not).
        _ => {
            client.todo.insert(id);
            batch.push(id, Op::Remove { slot: entry.slot }, Act::Drop);
        }
    }
}

/// A prop on the channel, hidden: sent again when it changed, its ghost brought back when it is no longer frozen.
fn live(client: &mut Client, id: u32, entry: Entry, now: &Now, tick: &Tick<'_>, batch: &mut Batch) {
    match now.eligible {
        None => {
            tick.port.show(client.obj, id);
            if let Some(e) = client.channel.get_mut(&id) {
                e.stage = Stage::Returning;
            }
            client.awaiting.insert(id);
        }
        Some(_) if !now.visible => {
            // Out of the client's scope: no ghost will come (the same scope decides), so the copy goes at once.
            tick.port.show(client.obj, id);
            client.todo.insert(id);
            batch.push(id, Op::Remove { slot: entry.slot }, Act::Drop);
        }
        Some(e) => resend(client, id, entry, &e, batch, false),
    }
}

/// Queues a set op for a prop on the channel when what the client was sent differs from what the prop is (or when it was
/// never sent: `unsent`).
fn resend(
    client: &mut Client,
    id: u32,
    entry: Entry,
    e: &Eligible,
    batch: &mut Batch,
    unsent: bool,
) {
    let wire = wire_of(client, e);
    if entry.sent == Some(wire) && !unsent {
        return;
    }
    client.todo.insert(id);
    batch.push(
        id,
        Op::Set {
            slot: entry.slot,
            prop: wire,
        },
        Act::Send { sent: wire },
    );
}

/// Sends `batch` in messages of at most [`MAX_OPS_PER_SEND`] ops, in order, and applies what each did to the client's state.
/// The first message the hub refuses ends the tick's sending: those props stay on the client's list for the next tick.
fn send(client: &mut Client, conn: ConnId, batch: Batch, port: &dyn Port, stats: &mut FrozenStats) {
    let mut left = batch.items.into_iter().peekable();
    while left.peek().is_some() {
        let chunk: Vec<(u32, Op, Act)> = left.by_ref().take(MAX_OPS_PER_SEND).collect();
        let ops: Vec<Op> = chunk.iter().map(|(_, op, _)| *op).collect();
        let sent = if port.backlog(conn) > SEND_BACKLOG {
            None
        } else {
            port.send(conn, Arc::from(messages::write(&ops))).ok()
        };
        let Some(handle) = sent else {
            stats.refused += 1;
            // This message and the ones after it are worked out again next tick: their props are on the list already.
            return;
        };
        let mut carried = Vec::new();
        for (id, op, act) in chunk {
            match op {
                Op::Set { .. } => stats.sets += 1,
                Op::Remove { .. } => stats.removes += 1,
            }
            client.todo.remove(&id);
            apply(client, id, act, handle, &mut carried);
        }
        if !carried.is_empty() {
            client.adding.push((handle, carried));
        }
    }
}

/// An act's change to the client's state, once its message is queued as `handle`; the props whose ghost waits on it are added
/// to `carried`.
fn apply(client: &mut Client, id: u32, act: Act, handle: MessageHandle, carried: &mut Vec<u32>) {
    match act {
        Act::Add { slot, sent } => {
            client.channel.insert(
                id,
                Entry {
                    slot,
                    sent: Some(sent),
                    stage: Stage::Adding(handle),
                },
            );
            carried.push(id);
        }
        Act::Send { sent } => {
            if let Some(e) = client.channel.get_mut(&id) {
                e.sent = Some(sent);
                e.stage = match e.stage {
                    Stage::Unsent | Stage::Live => Stage::Live,
                    Stage::Adding(_) => {
                        carried.push(id);
                        Stage::Adding(handle)
                    }
                    Stage::Returning => Stage::Returning,
                };
            }
        }
        Act::Drop => {
            client.channel.remove(&id);
            client.awaiting.remove(&id);
            // What the prop is now (it may be frozen again) is looked at next tick.
            client.todo.insert(id);
        }
    }
}

/// The tick's part ([module docs](self)): the snapshot kept up to date and every client dealt the frozen props, what changed
/// sent and ghosts hidden or shown.
pub fn sync(vm: &mut Vm, props: &ServerProps) {
    let threshold = vm.prefs::<PropPrefs>().frozen_ghosts as usize;
    let (Some(hub), Some(scope)) = (vm.extension::<ChannelHub>(), ScopeControl::of(vm)) else {
        // No channel here (a server of Torque's protocol alone): nothing is kept, and the log of touched props is not left to
        // grow.
        if let Ok(mut srv) = props.0.try_borrow_mut() {
            let srv = &mut *srv;
            srv.frozen.forget(srv.world.as_mut());
        }
        return;
    };
    let live: Vec<(ConnId, ObjId)> = hub
        .clients()
        .into_iter()
        .filter_map(|conn| Some((conn, hub.object_of(conn)?)))
        .collect();
    let hierarchy = ServerHierarchy::of(vm);
    let refused = {
        let Ok(mut srv) = props.0.try_borrow_mut() else {
            return;
        };
        let srv = &mut *srv;
        let Some(world) = srv.world.as_mut() else {
            return;
        };
        let port = LivePort {
            hub: &hub,
            scope: &scope,
            vm,
        };
        srv.frozen
            .run(world, hierarchy.as_ref(), &live, threshold, &port);
        srv.frozen.stats.hide_refused = scope.refused();
        let first = scope.refused() > 0 && !std::mem::replace(&mut srv.frozen.warned, true);
        first.then_some(scope.refused())
    };
    if let Some(total) = refused {
        vm.warn(format!(
            "props: the scope control has no room to hide more frozen props from a client ({total} refused so far): they stay ghosts"
        ));
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
