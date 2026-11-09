//! Plugin RPCs on the server and on a client: the carriers that put an [`RpcLink`]'s frames in the reserved channel
//! ([`RPC_CHANNEL`], TGE mode) or in the session's RPC lanes ([`RpcLanes`], engine mode), the per-connection links, the pump a host runs once a tick, and the
//! script natives (`rpcToClient`, `rpcToServer`, `rpcDrops`).
//!
//! **Server.** [`ServerRpc`] (an extension of the server's script engine, with the channel hub) holds one link for each client
//! whose channels are on. [`ServerRpc::pump`] runs once a tick: it takes the batches the channel received since the last tick,
//! reads them into the connection's link (a malformed one drops that client with the packet error), runs the calls that
//! passed the link's checks (permission, ghost references, the script's `onRpcCheck`) and hands what the handlers answered
//! to the carrier. Calls run after the tick's receive, so an RPC that names an object a client could see when it sent it
//! finds it.
//!
//! **Client.** [`ClientRpc`] (an extension of the client's script engine) holds the link of the current connection. Its pump runs
//! once a client tick, after the packets of the frame have been applied (so a notification that names a ghost from the same
//! packet finds it), and runs the notifications and the replies the server sent.
//!
//! **Script.** `rpcToServer("name", args..)` on a client and `rpcToClient(%client, "name", args..)` on the server send a call
//! whose arguments are the message's fields in order. They answer the empty string when the call was queued and a line of
//! text when it was not. `rpcDrops(%client, "name")` is the number of calls of an RPC a client's connection dropped for going
//! over its rate. A server script's `onRpcCheck(%client, %name)` denies a call unless it returns true. A handler
//! that is a script's is `rpcServer_<name>(%client, ..)` or `rpcClient_<name>(..)` ([`script_function`](torque_engine::rpc::script_function)).
//!
//! **Prefs.** `$Pref::Server::RpcKickDrops` (default 0: off) drops a client that has more than that many calls dropped in a
//! second; `$Pref::Net::RpcTimeout` (seconds, default 10) is how long a call waits for its reply.
//!
//! # Maintaining
//!
//! The rules and their tests are in `torque-engine`'s `rpc` module docs; this file is the glue. `ServerRpc::pump` and
//! `ClientRpc::pump` are the only places a link meets the network host: they take the reserved channel's batches
//! (`RpcChannel::take`), feed `RpcLink::receive`, run `take_work` outside every borrow (a handler may call back in, which is how
//! a notification is answered with a call), `complete`, `flush`. Keep that order and keep the `RefCell`s free while handlers run.
//! - A malformed batch ends the connection: the server drops the client (`Packet Error.`), the client asks its host to hang up
//!   (`ClientRpcHost::drop_malformed`); a client over its inbox share or whose replies pile up is dropped too, and others are
//!   untouched (`rpc_one_connection_cannot_crowd_out_the_others_inbox`, `a_malformed_batch_from_the_server_ends_the_connection_with_a_reason`).
//! - A script's `onRpcCheck` must answer true; script handlers and `rpcToClient`/`rpcToServer` use objects, and the host
//!   supplies the object/ghost mapping (`RpcServerHost::ghost_object`/`ghost_index`, `ClientRpcHost`).
//!
//! - A call of a `to_client` RPC marked for the realm (`RpcBuilder::realm`) goes to the `RealmInbox` the client's host installed
//!   before its own handler runs (`ClientHooks::realm`); without an inbox it is not queued. `ServerRpc::connections` and
//!   `conn_of` let a plugin address clients by connection (the client-script plugin's offer and natives).
//!
//! Checks: `cargo test -p torque-server --lib net::rpc`, `cargo test -p torque-example-minimal-engine --test rpc`, and the engine's
//! list in its `rpc` docs.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::rc::{Rc, Weak};

use torque_engine::permissions::{NoPermissions, Permissions, SubjectKey};
use torque_engine::rpc::{
    CONN_INBOX_CAP, CarrierFull, DEFAULT_SEND_QUEUE, DEFAULT_TIMEOUT_TICKS, Delivery,
    GhostResolver, NoGhosts, RPC_CHANNEL, RealmCall, RealmInbox, Received, RpcCarrier, RpcChannel,
    RpcEnv, RpcError, RpcHooks, RpcLink, RpcRef, RpcTable, Side,
};
use torque_ghosts::codec::NetWidths;
use torque_net::chunk::Kind;
use torque_net::engine::{Delivery as LaneDelivery, LaneLimits, Session};
use torque_net::handshake::ConnId;
use torque_ts_vm::fields::atob;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, Vm};

use super::NetGame;
use super::NetShared;
use super::channels::{ChannelHub, ClientChannels};
use super::commands::{arg, find_net_connection};
use super::conn::{DeleteMode, connection_of, delete_connection};
use super::permissions::subject_key_of_conn;

/// What the RPC host needs of the network host under it, whatever the game's parts are.
pub trait RpcServerHost {
    /// The ghost-index width of `conn`.
    fn ghost_bits(&self, conn: ConnId) -> u8;
    /// The `GameConnection` of `conn`.
    fn object_of(&self, conn: ConnId) -> Option<ObjId>;
    /// The connection of a `GameConnection` object.
    fn conn_of(&self, obj: ObjId) -> Option<ConnId>;
    /// The object ghost `index` of `conn` stands for, if the server sent that connection such a ghost.
    fn ghost_object(&self, conn: ConnId, index: u32) -> Option<ObjId>;
    /// The ghost index `object` has on `conn`, if it is ghosted there.
    fn ghost_index(&self, conn: ConnId, object: ObjId) -> Option<u32>;
    /// What `conn`'s permissions are kept under: its auth identity, else the connection.
    fn subject_key(&self, conn: ConnId) -> SubjectKey;
    /// Drops `conn` with `reason`.
    fn drop_conn(&self, vm: &mut Vm, conn: ConnId, reason: &[u8]);
}

/// The network host's [`RpcServerHost`].
pub struct SharedHost<G: NetGame>(pub Weak<RefCell<NetShared<G>>>);

impl<G: NetGame> RpcServerHost for SharedHost<G> {
    fn ghost_bits(&self, conn: ConnId) -> u8 {
        let Some(shared) = self.0.upgrade() else {
            return 0;
        };
        let sh = shared.borrow();
        sh.ghosts.as_ref().map_or(0, |g| g.conn_ghost_bits(conn))
    }

    fn object_of(&self, conn: ConnId) -> Option<ObjId> {
        self.0.upgrade()?.borrow().by_conn.get(&conn).copied()
    }

    fn conn_of(&self, obj: ObjId) -> Option<ConnId> {
        connection_of(&self.0.upgrade()?.borrow(), obj)
    }

    fn ghost_object(&self, conn: ConnId, index: u32) -> Option<ObjId> {
        let shared = self.0.upgrade()?;
        let sh = shared.borrow();
        sh.ghosts.as_ref()?.object_at(conn, index)
    }

    fn ghost_index(&self, conn: ConnId, object: ObjId) -> Option<u32> {
        let shared = self.0.upgrade()?;
        let sh = shared.borrow();
        sh.ghosts.as_ref()?.ghost_index(conn, object)
    }

    fn subject_key(&self, conn: ConnId) -> SubjectKey {
        match self.0.upgrade() {
            Some(shared) => subject_key_of_conn(&shared.borrow(), conn),
            None => SubjectKey::Connection(conn.0),
        }
    }

    fn drop_conn(&self, vm: &mut Vm, conn: ConnId, reason: &[u8]) {
        let Some(shared) = self.0.upgrade() else {
            return;
        };
        let obj = shared.borrow().by_conn.get(&conn).copied();
        if let Some(obj) = obj {
            let _ = delete_connection(vm, &shared, obj, Some(reason), DeleteMode::Immediate);
        }
    }
}

/// The ghosts a server sent each connection, as a call is checked against them.
struct HostGhosts<'a>(&'a dyn RpcServerHost);

impl GhostResolver for HostGhosts<'_> {
    fn knows(&self, conn: Option<ConnId>, index: u32) -> bool {
        self.resolve(conn, index).is_some()
    }

    fn resolve(&self, conn: Option<ConnId>, index: u32) -> Option<ObjId> {
        self.0.ghost_object(conn?, index)
    }

    fn index_of(&self, conn: Option<ConnId>, object: ObjId) -> Option<u32> {
        self.0.ghost_index(conn?, object)
    }
}

/// What the script says about a call: `onRpcCheck`, and the functions of script handlers.
struct ScriptHooks<'a>(&'a dyn RpcServerHost);

impl RpcHooks for ScriptHooks<'_> {
    /// A call the script cannot name the caller of is refused, and so is one `onRpcCheck` does not answer true to (an empty
    /// answer, `0`, `false` and a function that fails all deny). A game that defines no `onRpcCheck` has no say in it.
    fn allow(&mut self, vm: &mut Vm, caller: Option<ConnId>, rpc: &str) -> bool {
        let Some(obj) = caller.and_then(|c| self.0.object_of(c)) else {
            return false;
        };
        if vm.global_function_origin(b"onRpcCheck").is_none() {
            return true;
        }
        let args = [obj.to_string().into_bytes(), rpc.as_bytes().to_vec()];
        vm.invoke(None, b"onRpcCheck", &args)
            .is_ok_and(|answer| atob(&answer))
    }

    fn script(
        &mut self,
        vm: &mut Vm,
        caller: Option<ConnId>,
        function: &str,
        args: Vec<Vec<u8>>,
    ) -> Result<(), String> {
        let obj = caller
            .and_then(|c| self.0.object_of(c))
            .ok_or_else(|| "the caller has no connection object".to_string())?;
        if vm.global_function_origin(function.as_bytes()).is_none() {
            return Err(format!("{function} is not defined"));
        }
        let mut argv = Vec::with_capacity(args.len() + 1);
        argv.push(obj.to_string().into_bytes());
        argv.extend(args);
        vm.invoke(None, function.as_bytes(), &argv)
            .map(drop)
            .map_err(|e| format!("{function}: {e:?}"))
    }
}

/// The widths a connection's messages are written in.
fn widths(ghost_bits: u8) -> NetWidths {
    NetWidths::new(ghost_bits, NetWidths::engine_default().datablock_bits)
}

/// The server's end of the reserved channel for one client.
struct ServerCarrier {
    hub: ChannelHub,
    conn: ConnId,
}

impl RpcCarrier for ServerCarrier {
    fn unacked(&self) -> usize {
        self.hub.backlog(self.conn, RPC_CHANNEL)
    }

    fn send(&self, batch: Vec<u8>, _delivery: Delivery) -> Result<(), CarrierFull> {
        // The channel refuses a batch when its backlog or the connection's shared budget is full: the link keeps the frames.
        self.hub
            .send_to(self.conn, RPC_CHANNEL, Kind::Delta, batch)
            .map_err(|_| CarrierFull)
    }
}

// ---- engine mode's carrier: the session's RPC lanes

/// One connection's RPC batches between its links and its session in engine mode.
#[derive(Default)]
struct LaneQueue {
    /// Batches the link handed over that the session has not taken yet.
    out: VecDeque<(Vec<u8>, Delivery)>,
    out_bytes: usize,
    /// Reliable bytes the session holds unacknowledged, as the host last read them.
    unacked: usize,
    /// Batches the session received that the pump has not taken yet.
    inbox: Vec<Vec<u8>>,
    held: usize,
}

/// Engine mode's RPC carrier: each connection's RPC batches ride its session's own lanes (the reliable RPC lane, and the
/// unreliable lane for unreliable batches that fit it), not the reserved channel. The network host moves batches between
/// this queue and the sessions every pass ([`RpcLanes::take_out`], [`RpcLanes::give_back`], [`RpcLanes::received`]); a
/// link sees a queue with the session's backpressure. A server keeps one queue per connection (`Some(id)`), a client one
/// (`None`).
#[derive(Default)]
pub struct RpcLanes {
    conns: RefCell<BTreeMap<Option<ConnId>, LaneQueue>>,
    /// Batches a session refused (past its largest message, or closed): counted, never sent.
    dropped: Cell<u64>,
}

impl RpcLanes {
    /// Hands `session` the batches queued for `conn`, oldest first: an unreliable batch that fits the unreliable lane goes
    /// there, any other reliably, as many as the RPC lane's send queue takes; the rest wait here. Both a server's host and a
    /// client's connection feed their sessions with it.
    pub fn feed(&self, conn: Option<ConnId>, session: &mut Session) {
        let limits = LaneLimits::default();
        let mut queue: VecDeque<(Vec<u8>, Delivery)> = self.take_out(conn).into();
        while let Some((batch, asked)) = queue.pop_front() {
            let lane = if asked == Delivery::Unreliable && batch.len() <= limits.unreliable_message
            {
                LaneDelivery::Unreliable
            } else {
                LaneDelivery::Reliable
            };
            if lane == LaneDelivery::Reliable
                && session.unacked_reliable() + batch.len() > limits.rpc_send_queue
            {
                queue.push_front((batch, asked));
                break;
            }
            if session.send(batch, lane).is_err() {
                self.dropped.set(self.dropped.get() + 1);
            }
        }
        self.give_back(conn, queue.into());
        self.set_unacked(conn, session.unacked_reliable());
    }

    /// Batches a session refused so far (none from an honest link: its own caps are within the lanes').
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.get()
    }

    /// The batches queued for `conn`'s session, oldest first.
    pub fn take_out(&self, conn: Option<ConnId>) -> Vec<(Vec<u8>, Delivery)> {
        let mut conns = self.conns.borrow_mut();
        let Some(q) = conns.get_mut(&conn) else {
            return Vec::new();
        };
        q.out_bytes = 0;
        q.out.drain(..).collect()
    }

    /// Batches the session could not take now go back to the front, in order.
    pub fn give_back(&self, conn: Option<ConnId>, batches: Vec<(Vec<u8>, Delivery)>) {
        let mut conns = self.conns.borrow_mut();
        let q = conns.entry(conn).or_default();
        for batch in batches.into_iter().rev() {
            q.out_bytes += batch.0.len();
            q.out.push_front(batch);
        }
    }

    /// The reliable bytes `conn`'s session holds unacknowledged.
    pub fn set_unacked(&self, conn: Option<ConnId>, bytes: usize) {
        self.conns.borrow_mut().entry(conn).or_default().unacked = bytes;
    }

    /// A batch `conn`'s session received. False when it would pass the connection's share of what is held between two pumps
    /// ([`CONN_INBOX_CAP`]): it is not kept, and the host drops the connection.
    pub fn received(&self, conn: Option<ConnId>, batch: Vec<u8>) -> bool {
        let mut conns = self.conns.borrow_mut();
        let q = conns.entry(conn).or_default();
        if q.held.saturating_add(batch.len()) > CONN_INBOX_CAP {
            return false;
        }
        q.held += batch.len();
        q.inbox.push(batch);
        true
    }

    /// What every connection received since the last take, by connection in id order.
    pub fn take_received(&self) -> Vec<(Option<ConnId>, Vec<u8>)> {
        let mut conns = self.conns.borrow_mut();
        let mut out = Vec::new();
        for (conn, q) in conns.iter_mut() {
            q.held = 0;
            out.extend(q.inbox.drain(..).map(|batch| (*conn, batch)));
        }
        out
    }

    /// `conn` is gone: what it queued and received is dropped.
    pub fn close(&self, conn: Option<ConnId>) {
        self.conns.borrow_mut().remove(&conn);
    }

    fn unacked(&self, conn: Option<ConnId>) -> usize {
        self.conns
            .borrow()
            .get(&conn)
            .map_or(0, |q| q.unacked + q.out_bytes)
    }

    fn push(
        &self,
        conn: Option<ConnId>,
        batch: Vec<u8>,
        delivery: Delivery,
    ) -> Result<(), CarrierFull> {
        let mut conns = self.conns.borrow_mut();
        let q = conns.entry(conn).or_default();
        if q.unacked + q.out_bytes + batch.len() > DEFAULT_SEND_QUEUE {
            return Err(CarrierFull);
        }
        q.out_bytes += batch.len();
        q.out.push_back((batch, delivery));
        Ok(())
    }
}

/// The carrier a link of `conn` gets over `lanes`.
#[cfg(test)]
pub(crate) fn lane_carrier(lanes: &Rc<RpcLanes>, conn: Option<ConnId>) -> impl RpcCarrier {
    LaneCarrier {
        lanes: Rc::clone(lanes),
        conn,
    }
}

/// One connection's end of [`RpcLanes`].
struct LaneCarrier {
    lanes: Rc<RpcLanes>,
    conn: Option<ConnId>,
}

impl RpcCarrier for LaneCarrier {
    fn unacked(&self) -> usize {
        self.lanes.unacked(self.conn)
    }

    fn send(&self, batch: Vec<u8>, delivery: Delivery) -> Result<(), CarrierFull> {
        self.lanes.push(self.conn, batch, delivery)
    }
}

/// The server's RPCs: one [`RpcLink`] for each client whose channels are on, and the pump that drives them.
pub struct ServerRpc {
    table: RpcTable,
    hub: ChannelHub,
    host: Rc<dyn RpcServerHost>,
    perms: Rc<Permissions>,
    links: RefCell<HashMap<ConnId, RpcLink>>,
    /// Engine mode's carrier, in place of the reserved channel.
    lanes: Option<Rc<RpcLanes>>,
}

impl ServerRpc {
    /// The server's RPCs over `hub` (whose registry has the reserved channel) and the network host `host`.
    pub fn new(
        table: RpcTable,
        hub: ChannelHub,
        host: Rc<dyn RpcServerHost>,
        perms: Rc<Permissions>,
    ) -> ServerRpc {
        ServerRpc {
            table,
            hub,
            host,
            perms,
            links: RefCell::new(HashMap::new()),
            lanes: None,
        }
    }

    /// [`ServerRpc::new`] for a server in engine mode: the batches ride each session's RPC lanes ([`RpcLanes`]), which the
    /// network host moves; the hub still says which clients there are.
    pub fn over_lanes(
        table: RpcTable,
        hub: ChannelHub,
        host: Rc<dyn RpcServerHost>,
        perms: Rc<Permissions>,
        lanes: Rc<RpcLanes>,
    ) -> ServerRpc {
        ServerRpc {
            lanes: Some(lanes),
            ..ServerRpc::new(table, hub, host, perms)
        }
    }

    /// The RPC table.
    pub fn table(&self) -> &RpcTable {
        &self.table
    }

    /// The `GameConnection` of `conn` (what a native handler acts for).
    pub fn object_of(&self, conn: ConnId) -> Option<ObjId> {
        self.host.object_of(conn)
    }

    /// The connection a `GameConnection` object is.
    pub fn conn_of(&self, object: ObjId) -> Option<ConnId> {
        self.host.conn_of(object)
    }

    /// The connections that have a link now.
    pub fn connections(&self) -> Vec<ConnId> {
        let mut conns: Vec<ConnId> = self.links.borrow().keys().copied().collect();
        conns.sort_by_key(|c| c.0);
        conns
    }

    /// Runs `f` on `conn`'s link, if the client has one (its channels are on and the pump has seen it).
    pub fn with_link<R>(&self, conn: ConnId, f: impl FnOnce(&mut RpcLink) -> R) -> Option<R> {
        self.links.borrow_mut().get_mut(&conn).map(f)
    }

    /// What `conn`'s player holds now.
    pub fn permissions_of(&self, conn: ConnId) -> torque_engine::permissions::KeyView<'_> {
        self.perms.view(self.host.subject_key(conn))
    }

    /// Drops `conn` with `reason`, as a malformed batch does.
    pub fn drop_conn(&self, vm: &mut Vm, conn: ConnId, reason: &[u8]) {
        self.links.borrow_mut().remove(&conn);
        self.host.drop_conn(vm, conn, reason);
    }

    /// Sends the notification `rpc` to `conn`.
    pub fn notify<Req: torque_ghosts::plugin::NetMessage>(
        &self,
        conn: ConnId,
        rpc: &RpcRef<Req, ()>,
        message: Req,
    ) -> Result<(), RpcError> {
        self.with_link(conn, |link| link.notify(rpc, message))
            .unwrap_or(Err(RpcError::Disconnected))
    }

    /// Sends the notification `rpc` to every client that has a link.
    pub fn notify_all<Req: torque_ghosts::plugin::NetMessage + Clone>(
        &self,
        rpc: &RpcRef<Req, ()>,
        message: &Req,
    ) {
        for link in self.links.borrow_mut().values_mut() {
            let _ = link.notify(rpc, message.clone());
        }
    }

    /// How many calls of `rpc` from `conn` were dropped for going over a rate.
    pub fn drops(&self, conn: ConnId, rpc: &str) -> u64 {
        self.with_link(conn, |link| link.drops(rpc)).unwrap_or(0)
    }

    /// Gives `conn` a link if it has none.
    fn ensure_link(&self, conn: ConnId, tick: u64) {
        self.links.borrow_mut().entry(conn).or_insert_with(|| {
            let carrier: Rc<dyn RpcCarrier> = match &self.lanes {
                Some(lanes) => Rc::new(LaneCarrier {
                    lanes: Rc::clone(lanes),
                    conn: Some(conn),
                }),
                None => Rc::new(ServerCarrier {
                    hub: self.hub.clone(),
                    conn,
                }),
            };
            RpcLink::new(
                self.table.clone(),
                Side::Server,
                Some(conn),
                widths(self.host.ghost_bits(conn)),
                carrier,
                tick,
            )
        });
    }

    /// One tick: reads what clients sent, runs the calls, answers, and ends what must end. See the module docs.
    pub fn pump(&self, vm: &mut Vm, tick: u64) {
        let clients = self.hub.clients();
        let gone: Vec<ConnId> = self
            .links
            .borrow()
            .keys()
            .filter(|c| !clients.contains(c))
            .copied()
            .collect();
        for conn in gone {
            if let Some(mut link) = self.links.borrow_mut().remove(&conn) {
                link.close();
            }
            if let Some(lanes) = &self.lanes {
                lanes.close(Some(conn));
            }
        }
        let mut batches: HashMap<ConnId, Vec<Vec<u8>>> = HashMap::new();
        let taken = match &self.lanes {
            Some(lanes) => Received {
                batches: lanes.take_received(),
                over: Vec::new(),
            },
            None => self
                .hub
                .with_up_channel::<RpcChannel, _>(RPC_CHANNEL, RpcChannel::take)
                .unwrap_or_default(),
        };
        for (conn, batch) in taken.batches {
            if let Some(conn) = conn {
                batches.entry(conn).or_default().push(batch);
            }
        }
        // A client that went over its share of the channel's inbox is dropped; the others are untouched.
        let flooders: Vec<ConnId> = taken.over.into_iter().flatten().collect();
        for conn in flooders {
            batches.remove(&conn);
            self.links.borrow_mut().remove(&conn);
            self.host.drop_conn(vm, conn, b"Packet Error.");
        }
        let kick = vm
            .global("$Pref::Server::RpcKickDrops")
            .and_then(|v| String::from_utf8_lossy(&v).trim().parse::<u32>().ok())
            .unwrap_or(0);
        for conn in clients {
            self.ensure_link(conn, tick);
            let mut broken = false;
            let mut over = false;
            if let Some(link) = self.links.borrow_mut().get_mut(&conn) {
                link.set_kick_drops(kick);
                link.tick(tick);
                for batch in batches.remove(&conn).unwrap_or_default() {
                    if link.receive(&batch).is_err() {
                        broken = true;
                        break;
                    }
                }
                over = link.over_drop_limit();
            }
            if broken || over {
                self.links.borrow_mut().remove(&conn);
                let reason: &[u8] = if broken {
                    b"Packet Error."
                } else {
                    b"Too many calls."
                };
                self.host.drop_conn(vm, conn, reason);
                continue;
            }
            self.run(vm, conn, tick);
            // A client that does not take its replies is dropped rather than held for ever.
            let overloaded = self
                .links
                .borrow()
                .get(&conn)
                .is_some_and(RpcLink::is_overloaded);
            if overloaded {
                self.links.borrow_mut().remove(&conn);
                self.host.drop_conn(vm, conn, b"Too many calls.");
            }
        }
    }

    /// Runs `conn`'s waiting calls and sends its answers.
    fn run(&self, vm: &mut Vm, conn: ConnId, tick: u64) {
        let work = self
            .links
            .borrow_mut()
            .get_mut(&conn)
            .map(RpcLink::take_work)
            .unwrap_or_default();
        let view = self.perms.view(self.host.subject_key(conn));
        let ghosts = HostGhosts(self.host.as_ref());
        let mut hooks = ScriptHooks(self.host.as_ref());
        for work in work {
            let mut env = RpcEnv {
                vm,
                tick,
                conn: Some(conn),
                perms: &view,
                ghosts: &ghosts,
                hooks: &mut hooks,
            };
            let done = work.run(&mut env);
            if let Some(link) = self.links.borrow_mut().get_mut(&conn) {
                link.complete(done);
            }
        }
        let lines = {
            let mut links = self.links.borrow_mut();
            let Some(link) = links.get_mut(&conn) else {
                return;
            };
            link.flush();
            link.take_reports()
        };
        for line in lines {
            vm.print(line.as_bytes());
        }
        for line in self.perms.take_undeclared().lines() {
            vm.warn(line);
        }
    }
}

/// Registers the server's RPC natives on `vm`: `rpcToClient(client, name, args..)` and `rpcDrops(client, name)`. Held weakly: the
/// extension holds the script engine's state.
pub fn register_server_natives(vm: &mut Vm, rpc: &Rc<ServerRpc>) {
    let weak = Rc::downgrade(rpc);
    vm.register(
        "rpcToClient",
        3,
        0,
        "rpcToClient(client, name, fields..) - sends a client the server-to-client RPC `name`, its arguments the message's fields; answers the reason it was not queued, or nothing",
        {
            let weak = weak.clone();
            move |vm, call| {
                let Some(rpc) = weak.upgrade() else {
                    return Native::Str(b"no RPCs".to_vec());
                };
                let Some(conn) = find_net_connection(vm, arg(call, 1))
                    .and_then(|obj| rpc.host.conn_of(obj))
                else {
                    return Native::Str(b"no such client".to_vec());
                };
                let name = String::from_utf8_lossy(arg(call, 2)).into_owned();
                let args: Vec<Vec<u8>> = call.argv.iter().skip(3).cloned().collect();
                let ghosts = HostGhosts(rpc.host.as_ref());
                let sent = rpc.with_link(conn, |link| link.send_script(&name, &args, &ghosts));
                Native::Str(match sent {
                    Some(Ok(())) => Vec::new(),
                    Some(Err(e)) => e.to_string().into_bytes(),
                    None => b"that client has no RPC link".to_vec(),
                })
            }
        },
    );
    vm.register(
        "rpcDrops",
        3,
        3,
        "rpcDrops(client, name) - how many calls of an RPC the client sent that were dropped for going over its rate",
        move |vm, call| {
            let drops = (|| {
                let rpc = weak.upgrade()?;
                let conn = find_net_connection(vm, arg(call, 1)).and_then(|o| rpc.host.conn_of(o))?;
                Some(rpc.drops(conn, &String::from_utf8_lossy(arg(call, 2))))
            })()
            .unwrap_or(0);
            Native::Str(drops.to_string().into_bytes())
        },
    );
}

// ---- the client's side

/// The client's end of the reserved channel.
struct ClientCarrier(Rc<ClientChannels>);

impl RpcCarrier for ClientCarrier {
    fn unacked(&self) -> usize {
        self.0.upstream_backlog_of(RPC_CHANNEL)
    }

    fn send(&self, batch: Vec<u8>, _delivery: Delivery) -> Result<(), CarrierFull> {
        self.0
            .send(RPC_CHANNEL, Kind::Delta, batch)
            .map_err(|_| CarrierFull)
    }
}

/// What a client's RPCs need of the client host above them: its ghost table, and a way to end the connection.
pub trait ClientRpcHost {
    /// The object ghost `index` stands for in this client's world, if the server sent it such a ghost (a call that names
    /// another is refused before its handler runs).
    fn ghost_object(&self, index: u32) -> Option<ObjId>;
    /// The ghost index `object` has in this client's world, if it is a mirror of one.
    fn ghost_index(&self, object: ObjId) -> Option<u32>;
    /// Ends the connection because the server sent what no server of this engine sends, with `reason`.
    fn drop_malformed(&self, reason: &str);
}

/// The ghosts of a client's world, as a call is checked against them.
struct ClientGhosts<'a>(&'a dyn ClientRpcHost);

impl GhostResolver for ClientGhosts<'_> {
    fn knows(&self, _conn: Option<ConnId>, index: u32) -> bool {
        self.0.ghost_object(index).is_some()
    }

    fn resolve(&self, _conn: Option<ConnId>, index: u32) -> Option<ObjId> {
        self.0.ghost_object(index)
    }

    fn index_of(&self, _conn: Option<ConnId>, object: ObjId) -> Option<u32> {
        self.0.ghost_index(object)
    }
}

/// A client's RPCs in TGE mode: the link of the current connection and the pump that drives it ([module docs](self)).
pub struct ClientRpc {
    table: RpcTable,
    channels: Rc<ClientChannels>,
    link: RefCell<Option<RpcLink>>,
    epoch: Cell<u64>,
    broken: Cell<bool>,
    host: RefCell<Option<Rc<dyn ClientRpcHost>>>,
    /// Engine mode's carrier, in place of the reserved channel.
    lanes: Option<Rc<RpcLanes>>,
}

impl ClientRpc {
    /// A client's RPCs over `channels` (whose registry has the reserved channel).
    pub fn new(table: RpcTable, channels: Rc<ClientChannels>) -> ClientRpc {
        let epoch = channels.epoch();
        ClientRpc {
            table,
            channels,
            link: RefCell::new(None),
            epoch: Cell::new(epoch),
            broken: Cell::new(false),
            host: RefCell::new(None),
            lanes: None,
        }
    }

    /// [`ClientRpc::new`] for a client in engine mode: the batches ride the session's RPC lanes ([`RpcLanes`]), which the
    /// connection moves; the channels still say when a connection starts and ends, and its ghost-id width.
    pub fn over_lanes(
        table: RpcTable,
        channels: Rc<ClientChannels>,
        lanes: Rc<RpcLanes>,
    ) -> ClientRpc {
        ClientRpc {
            lanes: Some(lanes),
            ..ClientRpc::new(table, channels)
        }
    }

    /// Gives the RPCs the client host: its ghost table (without one every ghost reference is refused) and the way to end a
    /// connection whose server sent a malformed batch (without one the RPCs are only turned off for it, with a warning).
    pub fn set_host(&self, host: Rc<dyn ClientRpcHost>) {
        *self.host.borrow_mut() = Some(host);
    }

    /// The RPC table.
    pub fn table(&self) -> &RpcTable {
        &self.table
    }

    /// Runs `f` on the link of the current connection, if it has one (the server accepted and the pump has run).
    pub fn with_link<R>(&self, f: impl FnOnce(&mut RpcLink) -> R) -> Option<R> {
        self.link.borrow_mut().as_mut().map(f)
    }

    /// Which connection the client is on: a number that changes with every new connection (and only then), so a plugin that
    /// keeps state for a connection can tell a new one from the old even if it never saw the old one end.
    pub fn epoch(&self) -> u64 {
        self.epoch.get()
    }

    /// Whether the connection has a link that takes calls.
    pub fn is_ready(&self) -> bool {
        self.link.borrow().is_some()
    }

    /// One client tick (at `tick`, a count of 32 ms ticks): follows the connection (a link for each, closed when it ends),
    /// reads what the server sent, runs the notifications and replies, and sends what the handlers and callers queued.
    pub fn pump(&self, vm: &mut Vm, tick: u64) {
        let epoch = self.channels.epoch();
        if epoch != self.epoch.get() || !self.channels.is_active() {
            if let Some(link) = self.link.borrow_mut().as_mut() {
                link.close();
            }
            if let Some(lanes) = &self.lanes {
                lanes.close(None);
            }
            self.run(vm, tick);
            self.link.borrow_mut().take();
            self.broken.set(false);
            self.epoch.set(epoch);
        }
        if !self.channels.is_active() || self.broken.get() {
            return;
        }
        if self.link.borrow().is_none() {
            let Some(bits) = self.channels.ghost_bits() else {
                return;
            };
            let mut link = RpcLink::new(
                self.table.clone(),
                Side::Client,
                None,
                widths(bits),
                match &self.lanes {
                    Some(lanes) => Rc::new(LaneCarrier {
                        lanes: Rc::clone(lanes),
                        conn: None,
                    }),
                    None => Rc::new(ClientCarrier(Rc::clone(&self.channels))),
                },
                tick,
            );
            let seconds = vm
                .global("$Pref::Net::RpcTimeout")
                .and_then(|v| String::from_utf8_lossy(&v).trim().parse::<f64>().ok())
                .filter(|s| *s > 0.0);
            link.set_timeout(seconds.map_or(DEFAULT_TIMEOUT_TICKS, |s| (s * 1000.0 / 32.0) as u64));
            *self.link.borrow_mut() = Some(link);
        }
        let batches = match &self.lanes {
            Some(lanes) => Received {
                batches: lanes.take_received(),
                over: Vec::new(),
            },
            None => self
                .channels
                .with_channel::<RpcChannel, _>(RPC_CHANNEL, RpcChannel::take)
                .unwrap_or_default(),
        };
        let mut malformed = !batches.over.is_empty();
        let batches = batches.batches;
        if let Some(link) = self.link.borrow_mut().as_mut() {
            link.tick(tick);
            for (_, batch) in batches {
                if link.receive(&batch).is_err() {
                    malformed = true;
                    break;
                }
            }
        }
        if malformed {
            self.broken.set(true);
            let host = self.host.borrow().clone();
            match host {
                Some(host) => host.drop_malformed("the server sent a malformed RPC message"),
                None => vm.warn(
                    "the server sent a malformed RPC message; RPCs are off for this connection"
                        .to_string(),
                ),
            }
        }
        self.run(vm, tick);
    }

    /// Runs the link's work with the cell free (a handler or a callback may call through the extension, which is how a
    /// notification is answered with a call), then flushes it.
    fn run(&self, vm: &mut Vm, tick: u64) {
        let work = self
            .link
            .borrow_mut()
            .as_mut()
            .map(RpcLink::take_work)
            .unwrap_or_default();
        let mut hooks = ClientHooks;
        let host = self.host.borrow().clone();
        let ghosts = host.as_deref().map(ClientGhosts);
        for work in work {
            let mut env = RpcEnv {
                vm,
                tick,
                conn: None,
                perms: &NoPermissions,
                ghosts: ghosts.as_ref().map_or(&NoGhosts, |g| g),
                hooks: &mut hooks,
            };
            let done = work.run(&mut env);
            if let Some(link) = self.link.borrow_mut().as_mut() {
                link.complete(done);
            }
        }
        let lines = {
            let mut cell = self.link.borrow_mut();
            let Some(link) = cell.as_mut() else {
                return;
            };
            link.flush();
            link.take_reports()
        };
        for line in lines {
            vm.print(line.as_bytes());
        }
    }
}

/// The client's script hooks: a script handler is `rpcClient_<name>(..)`, and a call of an RPC marked for client scripts also
/// goes to the [`RealmInbox`] the host installed, when there is one.
struct ClientHooks;

impl RpcHooks for ClientHooks {
    fn realm(&mut self, vm: &mut Vm, rpc: &str, values: Vec<torque_ghosts::Value>) {
        if let Some(inbox) = vm.extension::<RealmInbox>() {
            inbox.push(RealmCall {
                rpc: rpc.to_string(),
                values,
            });
        }
    }

    fn script(
        &mut self,
        vm: &mut Vm,
        _caller: Option<ConnId>,
        function: &str,
        args: Vec<Vec<u8>>,
    ) -> Result<(), String> {
        if vm.global_function_origin(function.as_bytes()).is_none() {
            return Err(format!("{function} is not defined"));
        }
        vm.invoke(None, function.as_bytes(), &args)
            .map(drop)
            .map_err(|e| format!("{function}: {e:?}"))
    }
}

/// Registers the client's RPC native on `vm`: `rpcToServer(name, fields..)`.
pub fn register_client_natives(vm: &mut Vm, rpc: &Rc<ClientRpc>) {
    let weak = Rc::downgrade(rpc);
    vm.register(
        "rpcToServer",
        2,
        0,
        "rpcToServer(name, fields..) - calls the client-to-server RPC `name`, its arguments the message's fields; answers the reason it was not queued, or nothing",
        move |_vm, call| {
            let Some(rpc) = weak.upgrade() else {
                return Native::Str(b"no RPCs".to_vec());
            };
            let name = String::from_utf8_lossy(arg(call, 1)).into_owned();
            let args: Vec<Vec<u8>> = call.argv.iter().skip(2).cloned().collect();
            let host = rpc.host.borrow().clone();
            let ghosts = host.as_deref().map(ClientGhosts);
            let sent = rpc.with_link(|link| {
                link.send_script(&name, &args, ghosts.as_ref().map_or(&NoGhosts, |g| g))
            });
            Native::Str(match sent {
                Some(Ok(())) => Vec::new(),
                Some(Err(e)) => e.to_string().into_bytes(),
                None => b"not connected".to_vec(),
            })
        },
    );
}

#[cfg(test)]
#[path = "rpc_tests.rs"]
mod tests;
