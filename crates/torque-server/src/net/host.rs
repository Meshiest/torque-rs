//! The network host of a server ([`NetShared`] over a game's parts, [`NetGame`]): the glue between `torque_net` (the packet
//! layer and the handshake, pure state machines) and the `Vm` (the scripts that decide who joins and what happens when they
//! leave).
//!
//! - the datagram transport, the bind policy: [`transport`](super::transport) (`NetInterface` sockets, `setNetPort`);
//! - the connections, their natives, the drop: [`conn`] (`GameConnection::onRemove`, the `NetConnection` and `GameConnection`
//!   console methods);
//! - the receive, send and timeout passes: [`pump`](super::pump) (the main loop, `processPacketReceiveEvent`, `checkTimeouts`);
//! - the server query's answers (the LAN list): [`query`](super::query) (`handleInfoPacket`, `handleGamePingRequest`,
//!   `handleGameInfoRequest`);
//! - what rides in the packets: [`payload`](super::payload) (the seam) and [`events`](super::events) (the layer: heads, events,
//!   NetStrings, `serverCmd*` followups; `GameConnection::readPacket`/`writePacket`, `eventReadPacket`/`eventWritePacket`);
//! - the remote-command natives and `buildTaggedString`: [`commands`].
//!
//! **Nothing changes for a server without a socket.** A server has a network host only when the caller attached one
//! ([`attach_host`]); the transport opens a socket only when the script's `setNetPort` binds. Without one, no state exists, the
//! tick loop does no network work, and the `GameConnection` methods are the class table's stubs.
//!
//! **A datagram's way.** The receive pass drains the transport (at most [`NET_DATAGRAMS_PER_PASS`] datagrams and
//! [`NET_WORK_PER_TICK`] units of work a pass; the rest waits), hands each to `torque_net::handshake::Server::handle_datagram`,
//! and acts on what comes back with the state released: a reply is sent; a connect request becomes a `GameConnection` object
//! and an outermost run of the script's `onConnectRequest` under the network's own step limits ([`NET_SCRIPT_RUN_STEPS`] a run,
//! [`NET_SCRIPT_WINDOW_STEPS`] per [`NET_SCRIPT_WINDOW_MS`]; not the run's lifetime budget, which these scripts do not draw on;
//! what they `schedule()` is a Sim event and does) after a global rate of [`NET_REQUESTS_PER_S`] (a request over a budget, or
//! whose script hits its limit, is dropped without a reply and counted, and an older connection from the same address is
//! dropped only once the request is admitted); an accepted one joins `ClientGroup`, runs `onConnect` (a limit there drops the
//! connection properly: the Disconnect datagram and `onDrop`, reason `Script limit.`), and only then gets its accept datagram;
//! a game packet goes to the payload layer. A client's commands draw on its own connection's share of the script window
//! ([`NET_CONN_SCRIPT_SHARE_STEPS`]) and every command is run (no per-second count unless a host sets
//! [`NetShared::commands_per_conn_per_s`]); a script's `delete` of the connection whose packet is being worked through waits
//! for the end of the packet.
//!
//! **Work reservation.** A game packet reserves its worst-case work ([`reserve_for`]) before it is handled; a tick that cannot
//! pay leaves the datagram at the head of the queue for the next pass ([`NetShared::stash`]), a source over
//! [`NET_WORK_PER_SOURCE`] of the tick has its datagrams skipped (a connection's game packet is counted for its address and
//! port, any other datagram for its IP: [`SourceKey`]; a skipped datagram costs the tick nothing, the pass still reads at most
//! [`NET_DATAGRAMS_PER_PASS`]), and the event layer pays from the reservation (a payment that fails means the bound was wrong
//! and is a `Packet Error.`). What the packet really used is charged to the tick afterwards.
//!
//! **Object ids.** The `GameConnection` is registered (it takes the next object id) before the connect request's fields are
//! read and `onConnectRequest` runs (`registerObject`, then `readConnectRequest`), and a rejected request deletes it again at
//! once (the id is spent). The host registers a candidate object for every request that gets past the digest and names
//! `GameConnection`.
//!
//! **One way out.** Every way a connection ends (the script's `delete`, the peer's Disconnect, a timeout, a packet error, a
//! replaced older connection, a rejected request) deletes the object; the class behaviour's `on_remove` then does what
//! `onRemove` does, in order: the Disconnect datagram, then `onDrop(%client, %reason)` with the client still in `ClientGroup`,
//! then the host's bookkeeping. `delete()` from a script goes through `deleteObject`, which posts the `DeleteThisEvent` 1 ms
//! later; the others destroy at once.
//!
//! **Caps.** [`NET_DATAGRAMS_PER_PASS`] 256, [`NET_WORK_PER_TICK`] 4,096, [`NET_TIMEOUT_SWEEP_MS`] 1,500,
//! [`NET_DROP_REASON_BYTES`] 255, [`NET_WORK_PER_SOURCE`] 2,048, [`NET_REQUESTS_PER_S`] 32,
//! [`NET_SCRIPT_RUN_STEPS`] 5,000,000, [`NET_SCRIPT_WINDOW_STEPS`] 10,000,000 per [`NET_SCRIPT_WINDOW_MS`] 1,000,
//! [`NET_CONN_SCRIPT_SHARE_STEPS`] 2,500,000.
//!
//! **Stack headroom.** The network entries recurse on the Rust stack only through the interpreter: a datagram's `serverCmd*`
//! followup, the datablocks-done message's `onDataBlocksDone` and the ghost layer's script calls start a script call at the
//! bottom of the receive pass, which adds 8 to 20 KiB to a plain script chain on a debug build.
//!
//! Credit: the host follows TGE as Torque3D, its MIT-licensed release, keeps it: `sim/netInterface.cpp` and
//! `sim/netConnection.cpp` (`Copyright (c) 2012 GarageGames, LLC`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::net::{IpAddr, SocketAddr};
use std::rc::Rc;
use web_time::Instant;

use torque_net::handshake::{ConnId, RateWindow, Server as Handshake};
use torque_net::huffman::Table;
use torque_net::serverquery::InfoLimiter;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::codecs::{NetCodecs, net_codecs};
use super::events::EventLayer;
use super::ghosts::{GhostGame, GhostHub};
use super::hostevent::GameEvents;
use super::payload::PayloadLayer;
use super::transport::SharedTransport;
use super::{commands, conn, datablocks};
use crate::state::{NetParts, START_GHOST_BITS, ServerState};

/// The most datagrams one receive pass reads; the rest stay in the transport for the next pass (the socket's own queue holds
/// the rest).
pub const NET_DATAGRAMS_PER_PASS: usize = 256;
/// The network work one pass may do: a unit for a datagram and one per 64 bytes of it, the same for a packet built, eight more
/// for a connect request (it runs script). A pass that has spent it stops and the rest waits for the next tick.
pub const NET_WORK_PER_TICK: u32 = 4_096;
/// The share of a tick's work one source address may use: a source over it is skipped for the rest of the tick, so one client
/// cannot hold the tick's budget.
pub const NET_WORK_PER_SOURCE: u32 = NET_WORK_PER_TICK / 2;
/// How often the timeout sweep acts (only when more than 1,500 ms passed since the last).
pub const NET_TIMEOUT_SWEEP_MS: u32 = 1_500;
/// The longest reason a drop keeps (the connection's 256-byte buffer less the NUL).
pub const NET_DROP_REASON_BYTES: usize = 255;
/// The verified connect requests the whole server takes a second (the handshake limits each source to 4; this is the total, so
/// many sources cannot run the script more than this often). A request over it is dropped without a reply.
pub const NET_REQUESTS_PER_S: u32 = 32;
/// The steps one script run the network starts may take (`onConnectRequest`, `onConnect`, `onDrop`, a client's command): the
/// caller is a stranger. Ordinary hooks take thousands.
pub const NET_SCRIPT_RUN_STEPS: u64 = 5_000_000;
/// The steps all the scripts the network starts may take in a window of [`NET_SCRIPT_WINDOW_MS`]. Once spent, requests and
/// commands are dropped (counted) until the window rolls; a drop's `onDrop` still runs. This budget is separate from the run's
/// lifetime budget.
pub const NET_SCRIPT_WINDOW_STEPS: u64 = 10_000_000;
/// The length of that window, in milliseconds.
pub const NET_SCRIPT_WINDOW_MS: u32 = 1_000;

/// The `onTrigger` calls the moves of one server tick may make in all (one for every trigger edge of every move read: a
/// connection with the full credit of 30 moves flipping five triggers each makes 150). Past it a call is dropped and counted
/// ([`Counters::triggers_dropped`]); the trigger state is kept either way, so an edge is never called late.
pub const NET_TRIGGER_CALLS_PER_TICK: u32 = 150;
/// The steps one connection's scripts (its commands and the calls its packets make) may take in a window: a quarter of
/// [`NET_SCRIPT_WINDOW_STEPS`], so that one client cannot starve joins or the other clients. Commands over it are dropped
/// and counted ([`Counters::over_share`]).
pub const NET_CONN_SCRIPT_SHARE_STEPS: u64 = NET_SCRIPT_WINDOW_STEPS / 4;

/// The most work a game datagram of `len` bytes can cost, reserved before it is handled (a datagram the tick's budget cannot
/// cover waits in the queue): the datagram's own cost, a unit per event (an event takes 11 bits at least, and a tag argument 7
/// more bits and a unit of its own, so one unit per 7 bits bounds both), and a unit per 64 bytes of event.
pub fn reserve_for(len: usize) -> u32 {
    let len = u32::try_from(len).unwrap_or(u32::MAX);
    super::pump::cost(len as usize)
        .saturating_add(len.saturating_mul(8) / 7)
        .saturating_add(len / 64)
        .saturating_add(1)
}

/// The phases of a tick, in the order the loop runs them (a test reads them from [`NetShared::trace`]).
pub use crate::server::Phase;

/// Whom a tick's work share is counted for: a game packet of a connection the handshake table holds for its address and port
/// (the table caps the connections), any other datagram (a challenge, a request, junk, a game packet from no connection:
/// nothing ties it to one) for its IP, so that a source cannot widen its share by varying its port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKey {
    Address(SocketAddr),
    Ip(IpAddr),
}

/// What the passes did, for tests and the report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counters {
    pub received: u64,
    /// Datagrams the handshake dropped without a reply.
    pub ignored: u64,
    pub requests: u64,
    pub accepted: u64,
    pub rejected: u64,
    /// Requests whose `onConnectRequest` hit a limit.
    pub dropped_requests: u64,
    pub packets_sent: u64,
    /// Data packets built past the datagram bound and not sent (counted lost; none expected).
    pub packets_withheld: u64,
    pub pings_sent: u64,
    pub drops: u64,
    /// Requests and commands dropped because the global request rate or the network script budget was spent.
    pub over_budget: u64,
    /// The script steps the network has used.
    pub script_steps: u64,
    /// Remote commands dropped because their connection had run [`NetShared::commands_per_conn_per_s`] in the second (none by
    /// default).
    pub commands_dropped: u64,
    /// Followups (commands and calls) dropped because their connection had used its share of the window's steps
    /// ([`NET_CONN_SCRIPT_SHARE_STEPS`]).
    pub over_share: u64,
    /// `onTrigger` calls dropped past [`NET_TRIGGER_CALLS_PER_TICK`] or because the connection's share of the script window was
    /// spent.
    pub triggers_dropped: u64,
    /// The work units the payload layer spent building packets beyond their bytes (the datablock renders,
    /// [`PayloadLayer::take_work`]).
    pub payload_work: u64,
    /// Server-query answers sent ([`query`](super::query): ping, info and the codecs' extra answers).
    pub info_replies: u64,
    /// Server-query requests the protocol would answer that the host's limits refused ([`NetShared::info`]).
    pub info_limited: u64,
    /// Info requests not answered because `onServerInfoQuery()` found no script budget or stopped at a limit.
    pub info_dropped: u64,
}

/// Where the network host reads the time from.
#[derive(Debug, Clone, Copy)]
pub enum NetClock {
    /// The simulation clock (`Vm::sim_time`): virtual servers and tests. Network time moves in ticks of 32 ms.
    Sim,
    /// The wall clock, in milliseconds since the instant: a realtime server, so that the replies and the round trips the
    /// receive passes stamp between two ticks are not rounded to the tick (a realtime server polls the socket while it waits
    /// for the next tick).
    Wall(Instant),
}

/// A send burst a game allows ([`NetGame::burst`]): while a connection's payload layer says it is busy and its round trip is
/// under `max_rtt_ms`, the send pass sends it more packets in the same tick, `packets` in all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burst {
    pub packets: u32,
    pub max_rtt_ms: f32,
}

/// What a game adds to the network host: its events, its ghost sources, its part of each connection and of the whole host, the
/// string coder's table, and the game's rules at a few points of the passes. Torque's answer is the default of each method.
pub trait NetGame: Sized + 'static {
    /// The game's events and their state ([`super::hostevent`]).
    type Events: GameEvents;
    /// The game's ghost sources ([`super::ghosts`]).
    type Ghosts: GhostGame;
    /// The game's part of one connection ([`ConnRec::game`]).
    type Conn: Clone + std::fmt::Debug + Default;
    /// The ghost-id width of a server before the ghost limit is set.
    const START_GHOST_BITS: u8 = START_GHOST_BITS;
    /// The `$Pref::Server::MaxPlayers` a server without the preference uses (TGE's `defaults.cs`: 64).
    const DEFAULT_MAX_PLAYERS: usize = 64;

    /// The string coder's table the packets' strings are coded with.
    fn string_table(&self) -> Rc<Table>;

    /// The game's part of a connection whose connect request carried `extra` (the handshake form's bytes past the arguments).
    fn conn_part(&self, extra: &[u8]) -> Self::Conn {
        let _ = extra;
        Self::Conn::default()
    }

    /// Whether a client's remote command may run (read when the command is about to run). Torque runs every one.
    fn command_allowed(&self, vm: &Vm, conn: &Self::Conn) -> bool {
        let _ = (vm, conn);
        true
    }

    /// The game's server preferences over the engine's (read for each datagram): the packets a second a client is sent, and
    /// the ghost tables wider than the server's that clients of the engine's own may hold.
    fn server_prefs(
        &self,
        vm: &Vm,
        engine: &ServerState<HostParts<Self>>,
        prefs: &mut super::codecs::ServerPrefs,
    ) {
        let _ = (vm, engine, prefs);
    }

    /// Whether an accepted request is let go without a word (the scripts accepted it, but the game will not serve the client).
    fn refuses_silently(&self, vm: &Vm, engine: &Engine<Self>, extension: bool) -> bool {
        let _ = (vm, engine, extension);
        false
    }

    /// Whether a scripted `delete(reason)` of a connection is ignored (the connection stays). Torque deletes on every reason.
    fn keeps_connection(&self, reason: &[u8]) -> bool {
        let _ = reason;
        false
    }

    /// The console text of a connection's protocol error (the drop's line: `Client N packet error: ...`).
    fn error_text(&self, error: &torque_net::NetError) -> String {
        error.to_string()
    }

    /// The send burst the game allows a busy connection this pass (none: one packet a tick, as Torque sends).
    fn burst(&self, vm: &Vm) -> Option<Burst> {
        let _ = vm;
        None
    }

    /// What the game does for connection `obj` while its round trip is above the codecs' lag limit
    /// ([`NetCodecs::infinite_lag_ms`]), at every send pass: nothing by default. A script it runs goes through
    /// [`pump::run`](super::pump::run).
    fn lagging(vm: &mut Vm, shared: &Rc<RefCell<NetShared<Self>>>, obj: ObjId) {
        let _ = (vm, shared, obj);
    }

    /// The game's work at the start of each send pass, after the ghost hub's; returns the connections to drop with a packet
    /// error and its text.
    fn begin_send_pass(
        vm: &mut Vm,
        shared: &Rc<RefCell<NetShared<Self>>>,
        now: u32,
    ) -> Vec<(ConnId, &'static str)> {
        let _ = (vm, shared, now);
        Vec::new()
    }
}

/// The network host's parts the server state holds for a server whose net game is `G`: its command host, datablock keys and
/// ghost hub.
pub struct HostParts<G>(std::marker::PhantomData<G>);

impl<G: NetGame> NetParts for HostParts<G> {
    type Commands = commands::CommandHost<G>;
    type DatablockKeys = datablocks::DatablockKeys;
    type Ghosts = GhostHub<G::Ghosts>;
    const START_GHOST_BITS: u8 = G::START_GHOST_BITS;
}

/// The server state of a host whose net game is `G`.
pub type Engine<G> = Rc<RefCell<ServerState<HostParts<G>>>>;

/// What the host knows about a `GameConnection` it made.
#[derive(Debug, Clone)]
pub struct ConnRec<C> {
    pub obj: ObjId,
    /// The handshake table's id once the connection was accepted.
    pub conn: Option<ConnId>,
    pub addr: SocketAddr,
    /// The client's connect sequence.
    pub sequence: u32,
    pub protocol_version: u32,
    /// The connect arguments `onConnect` is called with.
    pub args: Vec<Vec<u8>>,
    /// The reason the drop gives the Disconnect datagram and `onDrop` .
    pub reason: Vec<u8>,
    /// The packet loss `getPacketLoss` multiplies by 100: 0 from the constructor; nothing measures a live connection's loss.
    pub packet_loss: f32,
    /// The drop sends nothing and runs no script (a request dropped at a limit).
    pub silent: bool,
    /// `on_remove` ran.
    pub dropping: bool,
    /// The remote commands of the current second ([`NetShared::commands_per_conn_per_s`]).
    pub cmd_rate: RateWindow,
    /// The standing with the server commands plugins declared ([`NetShared::commands`]).
    pub plugin_cmds: super::cmdlimit::ConnCommands,
    /// The script window ([`NetShared::window_start`]) `share_used` counts in.
    pub share_window: u32,
    /// The steps this connection's scripts took in that window ([`NET_CONN_SCRIPT_SHARE_STEPS`]).
    pub share_used: u64,
    /// A script called `delete()` on the connection while its own packet's followups ran: the drop waits for the end of the
    /// packet.
    pub delete_pending: bool,
    /// `transmitPaths` / `clearPaths`: the connection has been sent the paths. The host has no `Path`s, so nothing is sent.
    pub paths_sent: bool,
    /// The server half of a local connection (`connectLocal`, [`local`](super::local)): the client's `ServerConnection` object,
    /// the remote-connection link (a `SimObjectPtr`: it reads as none once that object is deleted).
    pub local_peer: Option<ObjId>,
    /// The game's part of the connection ([`NetGame::Conn`]).
    pub game: C,
}

/// A connection held in a downloading state: its `onConnect` waits until its content is delivered.
#[derive(Debug, Clone)]
pub struct Awaiting {
    pub conn: ConnId,
    pub obj: ObjId,
    /// The connect arguments `onConnect` is called with.
    pub args: Vec<Vec<u8>>,
    /// The network time of the accept.
    pub since: u32,
    /// What the host holds the client to.
    pub limits: super::content::ContentLimits,
    /// The game's progress count when last looked at, and the network time it last changed.
    pub seen: u64,
    pub moved: u32,
}

/// The state the passes and the natives share (`Rc<RefCell<..>>`: never borrowed across script).
pub struct NetShared<G: NetGame> {
    pub transport: SharedTransport,
    /// The game's codecs (the `Vm`'s at attach).
    pub codecs: Rc<dyn NetCodecs>,
    pub hs: Handshake,
    /// The host's limits of the server-query answers ([`query`](super::query)).
    pub info: InfoLimiter,
    pub engine: Engine<G>,
    pub conns: HashMap<ObjId, ConnRec<G::Conn>>,
    pub by_conn: HashMap<ConnId, ObjId>,
    pub payload: Box<dyn PayloadLayer>,
    /// The event layer ([`payload`](Self::payload) starts as a clone of it): the commands and the tests post and read through
    /// it.
    pub events: EventLayer<G::Events, G::Ghosts>,
    /// The ghost hub, shared with the engine state and the event layer.
    pub ghosts: Option<GhostHub<G::Ghosts>>,
    /// A datagram the last pass could not pay the reservation for: the next pass reads it first (it stays at the head of the
    /// queue).
    pub stash: Option<(SocketAddr, Vec<u8>)>,
    /// Tests: the work a game datagram reserves instead of [`reserve_for`] (a forced tiny reservation shows the in-datagram
    /// failure).
    pub reservation_override: Option<u32>,
    /// What each source has been given of this tick's work ([`NET_WORK_PER_SOURCE`]).
    pub source_work: HashMap<SourceKey, u32>,
    pub counters: Counters,
    /// When set, every phase of the tick loop pushes itself here.
    pub trace: Option<Vec<Phase>>,
    /// The step limit of an outermost run outside the network host, which the passes put back after a run of theirs.
    pub step_limit: Option<u64>,
    /// The steps one script run the network starts may take ([`NET_SCRIPT_RUN_STEPS`]).
    pub run_limit: u64,
    /// The remote commands one connection may run a second; the rest of the second's are dropped and counted. None (the
    /// default): every command a packet carries runs (a client may send one a frame for a long time). A connection's work stays
    /// bounded by its share of the script window ([`NET_CONN_SCRIPT_SHARE_STEPS`]) and the tick's network work
    /// ([`NET_WORK_PER_SOURCE`]).
    pub commands_per_conn_per_s: Option<u32>,
    /// The server commands plugins declared, whose calls a client sends are held to their limits
    /// ([`set_commands`](Self::set_commands)); none by default.
    pub commands: Option<Rc<super::cmdlimit::CommandTable>>,
    /// The server ticks run so far: what the commands' token buckets count in (the host that runs the passes sets it before each
    /// receive, [`NetHost::set_tick`](crate::server::NetHost::set_tick)).
    pub tick: u64,
    /// The steps the scripts the network starts may take together in a window of [`NET_SCRIPT_WINDOW_MS`]
    /// ([`NET_SCRIPT_WINDOW_STEPS`]). These are not the run's lifetime budget (`Server::run`'s): a stranger's datagrams cannot
    /// spend that.
    pub window_budget: u64,
    /// The game's state of the host ([`NetGame`]).
    pub game: G,
    pub clock: NetClock,
    pub now: u32,
    pub window_start: u32,
    pub window_used: u64,
    /// The connection whose packet's followups are running (a script's `delete()` of it waits for the end of the packet).
    pub packet_obj: Option<ObjId>,
    pub requests: RateWindow,
    pub work_used: u32,
    pub last_sweep: u32,
    pub cursor: u32,
    /// Connections accepted with content to send, whose `onConnect` waits for it ([`content`](super::content)).
    pub awaiting: Vec<Awaiting>,
}

impl<G: NetGame> NetShared<G> {
    /// Holds the calls clients send to the declared server commands to their limits (none declared: nothing is held, and the
    /// commands run as TGE's do).
    pub fn set_commands(&mut self, table: super::cmdlimit::CommandTable) {
        self.commands = (!table.is_empty()).then(|| Rc::new(table));
    }

    /// The handshake this server speaks (what its connect gate's manifest is made for).
    pub fn handshake_profile(&self) -> torque_net::profile::HandshakeProfile {
        self.hs.config().profile.clone()
    }

    /// Judges every connect request that passes the standard checks with `gate`, before `onConnectRequest` runs
    /// ([`ServerConfig::gate`](torque_net::handshake::ServerConfig::gate)): a refused client gets the gate's reason and the
    /// scripts never see it.
    pub fn set_connect_gate(&mut self, gate: impl torque_net::connect::ConnectGate + 'static) {
        self.hs.config_mut().gate = Some(std::sync::Arc::new(gate));
    }

    /// Tells the server the address clients reach it by, which its auth provider binds tickets to
    /// ([`ServerConfig::address`](torque_net::handshake::ServerConfig::address)). A server behind a NAT, a relay or a socket
    /// bound to every interface needs its public address here; without one, a provider that binds to the address refuses every
    /// ticket.
    pub fn set_public_address(&mut self, address: Option<std::net::SocketAddr>) {
        self.hs.config_mut().address = address;
    }

    /// The string coder's table ([`NetGame::string_table`]).
    pub fn string_table(&self) -> Rc<Table> {
        self.game.string_table()
    }

    /// The time of the last pass (milliseconds on [`clock`](Self::clock)).
    pub fn now(&self) -> u32 {
        self.now
    }

    pub fn set_now(&mut self, now: u32) {
        self.now = now;
    }

    /// The network time for a simulation time of `sim`.
    pub fn clock_ms(&self, sim: u32) -> u32 {
        match self.clock {
            NetClock::Sim => sim,
            // Wraps every 49.7 days, as every comparison of two times does (`wrapping_sub`); never held at `u32::MAX`.
            NetClock::Wall(start) => start.elapsed().as_millis() as u32,
        }
    }

    /// When the timeout sweep last acted.
    pub fn last_sweep(&self) -> u32 {
        self.last_sweep
    }

    /// The steps the network's scripts used in the current window.
    pub fn window_used(&self) -> u64 {
        self.window_used
    }

    fn roll_window(&mut self) {
        if self.now.wrapping_sub(self.window_start) >= NET_SCRIPT_WINDOW_MS {
            self.window_start = self.now;
            self.window_used = 0;
        }
    }

    /// Whether the window has steps left for a script a stranger's datagram starts.
    pub fn script_available(&mut self) -> bool {
        self.roll_window();
        self.window_used < self.window_budget
    }

    /// Sets the VM's step limit for a script the network starts: [`NET_SCRIPT_RUN_STEPS`], or what is left of the window for a
    /// `strict` one (cleanup, a drop's `onDrop`, is not strict: it runs whatever the window says, within the run limit). False,
    /// and nothing set, when a strict run has no steps left. `cap` lowers the limit further (a connection's share,
    /// [`NET_CONN_SCRIPT_SHARE_STEPS`]).
    pub fn begin_run(&mut self, vm: &mut Vm, strict: bool, cap: Option<u64>) -> bool {
        self.roll_window();
        let left = self.window_budget.saturating_sub(self.window_used);
        if strict && left == 0 {
            return false;
        }
        let mut limit = if strict {
            self.run_limit.min(left)
        } else {
            self.run_limit
        };
        if let Some(cap) = cap {
            limit = limit.min(cap);
        }
        vm.set_step_limit(Some(limit));
        true
    }

    /// Ends a run [`begin_run`](Self::begin_run) started: its steps join the window and the counters, and the VM's limit is put
    /// back. Returns the steps the run took.
    pub fn end_run(&mut self, vm: &mut Vm) -> u64 {
        let used = vm.steps();
        self.window_used = self.window_used.saturating_add(used);
        self.counters.script_steps = self.counters.script_steps.saturating_add(used);
        vm.set_step_limit(self.step_limit);
        used
    }

    /// The steps connection `obj` may still take in this window ([`NET_CONN_SCRIPT_SHARE_STEPS`]); 0 for a connection the host
    /// does not know.
    pub fn share_left(&mut self, obj: ObjId) -> u64 {
        self.roll_window();
        let window = self.window_start;
        let Some(rec) = self.conns.get_mut(&obj) else {
            return 0;
        };
        if rec.share_window != window {
            rec.share_window = window;
            rec.share_used = 0;
        }
        NET_CONN_SCRIPT_SHARE_STEPS.saturating_sub(rec.share_used)
    }

    /// Counts `steps` against connection `obj`'s share of the window.
    pub fn charge_share(&mut self, obj: ObjId, steps: u64) {
        let _ = self.share_left(obj);
        if let Some(rec) = self.conns.get_mut(&obj) {
            rec.share_used = rec.share_used.saturating_add(steps);
        }
    }

    /// Replaces the payload layer (part 2 installs its event layer here).
    pub fn set_payload(&mut self, layer: Box<dyn PayloadLayer>) {
        self.payload = layer;
    }

    pub fn push_phase(&mut self, phase: Phase) {
        if let Some(trace) = self.trace.as_mut() {
            trace.push(phase);
        }
    }
}

/// Sixteen bytes for the challenge digest's key, from the operating system's randomness as the standard library's hasher seeds
/// draw it (a different seed per process and per call), mixed with the clock. The digest is keyed and verified server-side
/// only.
fn digest_key() -> [u8; 16] {
    let mut key = [0u8; 16];
    for (i, half) in key.chunks_mut(8).enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_usize(i);
        if let Ok(now) = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH) {
            hasher.write_u128(now.as_nanos());
        }
        half.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    key
}

/// What [`attach_host`] builds a network host from.
pub struct Host<G: NetGame> {
    pub transport: SharedTransport,
    /// The game's state of the host.
    pub game: G,
    pub engine: Engine<G>,
    /// The step limit of an outermost run outside the network host ([`NetShared::step_limit`]).
    pub step_limit: Option<u64>,
    pub clock: NetClock,
    /// The game's ghost sources: the hub's game.
    pub ghosts: G::Ghosts,
    /// The container whose bins the scope walks read.
    pub container: Rc<torque_game::scene::container::Container>,
    /// The game's rule for static writes to its own datablock classes ([`datablocks::install_keys_with`]).
    pub static_writes: Option<datablocks::StaticWriteRule>,
}

/// Attaches a network host to `vm`: the shared state, the ghost hub, the event layer, the `GameConnection` behaviour and the
/// connection natives, the `NetObject` behaviour and the ghosting natives ([`super::ghosting`]), the datablock keys, the
/// command host and the command natives ([`commands::register_always`]). The game installs its own natives on the result.
pub fn attach_host<G: NetGame>(vm: &mut Vm, host: Host<G>) -> Rc<RefCell<NetShared<G>>> {
    let Host {
        transport,
        game,
        engine,
        step_limit,
        clock,
        ghosts: sources,
        container,
        static_writes,
    } = host;
    // No connection is taken until a script allows them (`allowConnections`).
    engine.borrow_mut().allow_connections = false;
    let codecs = net_codecs(vm);
    let events = EventLayer::with_codecs(Rc::clone(&codecs));
    let shared = Rc::new(RefCell::new(NetShared {
        transport,
        codecs: Rc::clone(&codecs),
        hs: Handshake::new(codecs.server_config(G::DEFAULT_MAX_PLAYERS), digest_key()),
        info: InfoLimiter::default(),
        engine,
        conns: HashMap::new(),
        by_conn: HashMap::new(),
        payload: Box::new(events.clone()),
        events,
        ghosts: None,
        stash: None,
        reservation_override: None,
        source_work: HashMap::new(),
        counters: Counters::default(),
        trace: None,
        step_limit,
        run_limit: NET_SCRIPT_RUN_STEPS,
        commands_per_conn_per_s: None,
        commands: None,
        tick: 0,
        window_budget: NET_SCRIPT_WINDOW_STEPS,
        game,
        clock,
        now: 0,
        window_start: 0,
        window_used: 0,
        packet_obj: None,
        requests: RateWindow::default(),
        work_used: 0,
        last_sweep: 0,
        awaiting: Vec::new(),
        cursor: 0,
    }));
    conn::install(vm, &shared);
    let hub = GhostHub::new(
        sources,
        container,
        shared.borrow().engine.borrow().db_values.clone(),
        Rc::clone(&codecs),
    );
    shared.borrow().engine.borrow_mut().ghost = Some(hub.clone());
    shared.borrow_mut().ghosts = Some(hub.clone());
    shared.borrow().events.set_hub(hub.clone());
    super::ghosting::install(vm, &shared);
    let layer = shared.borrow().events.clone();
    let values = shared.borrow().engine.borrow().db_values.clone();
    layer.set_values(values.clone());
    datablocks::install_keys_with(vm, &layer.keys(), static_writes);
    shared.borrow().engine.borrow_mut().datablock_keys = layer.keys();
    shared.borrow().engine.borrow_mut().commands = commands::CommandHost::new(&shared, layer);
    let engine = Rc::clone(&shared.borrow().engine);
    commands::register_always(vm, &engine);
    shared
}
