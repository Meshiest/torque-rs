//! The three passes of the network host and what each does with a datagram (`Server::run_ticks` calls them in the main loop's
//! order: receive, the server tick, send, the Sim events, timeouts).
//!
//! **Discipline.** Every step is "borrow the shared state, compute an owned action, let go, act". Scripts run only from actions
//! with nothing borrowed; the natives they call borrow the state briefly. Each script call a pass makes is an outermost run
//! (the host is not inside one) under the network's own step limits ([`NET_SCRIPT_RUN_STEPS`](super::NET_SCRIPT_RUN_STEPS) a
//! run, [`NET_SCRIPT_WINDOW_STEPS`](super::NET_SCRIPT_WINDOW_STEPS) a window), not the run's lifetime budget. (What those
//! scripts `schedule()` is a Sim event and draws on the run's budget; `Server::run_restarting` is the run of a server with a
//! socket.)
//!
//! **Receive** ([`receive`]). At most [`NET_DATAGRAMS_PER_PASS`] datagrams of connections a call and [`NET_WORK_PER_TICK`]
//! units of work a tick (the polls of a realtime server between two ticks share the tick's units); strangers' datagrams draw on
//! a pool of their own and past it are read and dropped (the host's module docs). A datagram from the server's own address
//! (`127.0.0.1` with its own port, whatever address the socket is bound to) is skipped. A datagram of one of the codecs' info
//! types is the server query's ([`super::query`]: the LAN list's ping and info requests), answered there. A challenge or accept
//! reply is sent. A connect request that passes the global rate ([`NET_REQUESTS_PER_S`](super::NET_REQUESTS_PER_S)) and finds script budget is a
//! `GameConnection` candidate and the script's `onConnectRequest(%client, address, args...)` (the codecs' address text,
//! [`NetCodecs::address_text`](super::codecs::NetCodecs::address_text); non-empty return =
//! reject); a limit or error there drops the request silently (counted). An accepted one joins `ClientGroup`,
//! `onConnect(%client, args...)` runs (hitting a limit there drops the connection with the Disconnect datagram and `onDrop`),
//! and only then is the accept datagram sent (what `onConnect` sends is queued before the accept leaves). A reject the
//! handshake made itself (wrong password, protocol, argument count, server full) still takes an object id and runs the drop, as
//! `readConnectRequest` fails after `registerObject`. The allow flag, the password and the player count are read from the
//! variables for each datagram.
//!
//! **Send** ([`send_pass`]). Per connection in rotating order: the packet layer's gate (`may_send`) and a data packet (header,
//! rate prefix, the payload layer's bits) whether or not the client has spoken; then, while the layer is busy and the game
//! allows a burst ([`NetGame::burst`]: none by default, one packet a tick) whose round trip limit the connection is under,
//! forced packets until the burst's count is sent. A connection whose round trip is above the codecs' lag limit is handed to
//! the game ([`NetGame::lagging`]).
//!
//! **Timeouts** ([`timeouts`]). Acts when more than [`NET_TIMEOUT_SWEEP_MS`] passed since the last sweep: pings for idle
//! connections; 15 unanswered pings drop the connection with `TimedOut`; a connection never heard from expires.
//!
//! Credit: the passes follow TGE as Torque3D, its MIT-licensed release, keeps them: `sim/netInterface.cpp`
//! (`processPacketReceiveEvent`, `checkTimeouts`), `sim/netConnection.cpp` and the own-address skip of
//! `platform/platformNet.cpp` (`Copyright (c) 2012 GarageGames, LLC`), and the
//! move handling of `T3D/gameBase/gameConnection.cpp` (`Copyright (c) 2012 GarageGames, LLC`, `Copyright (C) 2015 Faust Logic,
//! Inc.`).

use std::borrow::Cow;
use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::rc::Rc;

use super::codecs::ServerPrefs;
use torque_net::NetError;
use torque_net::bits::BitReader;
use torque_net::cap::NET_MAX_DATAGRAM;
use torque_net::connect::Identity;
use torque_net::handshake::{
    ConnId, Decision, Finished, Handled, PendingRequest, RateWindow, TimeoutEvent, peek_type,
};
use torque_net::protocol::{Received, reader_at};
use torque_ts_vm::fields::atob;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::text::{atoi, until_nul};
use torque_ts_vm::vm::{CommandSource, Vm, VmError};

use super::NetGame;
use super::channels::ChannelHub;
use super::cmdlimit::Admission;
use super::conn::{
    DeleteMode, connection_of, delete_connection, join_client_group, register_candidate,
};
use super::content::ContentServerSlot;
use super::events::EventLayer;
use super::ghosts::GhostHub;
use super::payload::{ConnCtx, Followup, PacketOutcome};
use super::reserve_for;
use super::transport::{RECV_BUFFER_BYTES, Recv};
use super::{
    Awaiting, Burst, ConnRec, NET_DATAGRAMS_PER_PASS, NET_READS_PER_PASS,
    NET_STRANGER_DATAGRAMS_PER_PASS, NET_STRANGER_WORK_PER_SOURCE, NET_STRANGER_WORK_PER_TICK,
    NET_TIMEOUT_SWEEP_MS, NET_TRIGGER_CALLS_PER_TICK, NET_UNKNOWN_COMMAND_STEPS,
    NET_WORK_PER_SOURCE, NET_WORK_PER_TICK, NetShared, SourceKey,
};
use torque_game::scene::camera_tick::{TriggerCalls, call_on_trigger};
use torque_game::sim::adapters;
use torque_game::sim::pass::ControlSide;
use torque_game::sim::{TickCx, Ticker};
use torque_net::fair::NetworkKey;

/// The packets a second the server sends a client unless the game says otherwise ([`NetGame::server_prefs`]).
const DEFAULT_PACKET_RATE: u32 = 10;
/// The reason a drop gives when `onConnect` hit a limit of the host's.
const ON_CONNECT_LIMIT_REASON: &[u8] = b"Script limit.";
/// The work units a connect request costs beyond its bytes (it runs script).
const REQUEST_WORK: u32 = 8;
/// The most sources whose work a tick remembers (a source past it is not limited more).
const SOURCES_TRACKED: usize = 4_096;

type Shared<G> = Rc<RefCell<NetShared<G>>>;

/// Runs `f` as a script the network starts: under the network's step limits, its steps counted to the window. `strict` runs (a
/// stranger's request or command) are refused, with `None`, when the window has no steps left; the others (a drop's `onDrop`)
/// run within the run limit.
pub fn run<T, G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    strict: bool,
    f: impl FnOnce(&mut Vm) -> T,
) -> Option<T> {
    run_for(vm, shared, strict, None, f)
}

/// [`run`] on behalf of connection `owner`: the run is capped at what is left of the connection's share of the window
/// ([`NET_CONN_SCRIPT_SHARE_STEPS`](super::NET_CONN_SCRIPT_SHARE_STEPS)) and its steps are counted to it.
fn run_for<T, G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    strict: bool,
    owner: Option<ObjId>,
    f: impl FnOnce(&mut Vm) -> T,
) -> Option<T> {
    let run = {
        let mut sh = shared.borrow_mut();
        let cap = owner.map(|obj| sh.share_left(obj));
        sh.begin_run(vm, strict, cap)?
    };
    let out = f(vm);
    {
        let mut sh = shared.borrow_mut();
        let used = sh.end_run(vm, run);
        if let Some(obj) = owner {
            sh.charge_share(obj, used);
        }
    }
    Some(out)
}

/// Deletes a connection's object with its `onDrop`. A `strict` drop (a rejected stranger) that finds no script budget deletes
/// silently.
fn delete_running<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    obj: ObjId,
    reason: Option<&[u8]>,
    mode: DeleteMode,
    strict: bool,
) {
    let ran = run(vm, shared, strict, |vm| {
        delete_connection(vm, shared, obj, reason, mode)
    });
    if ran.is_none() {
        let _ = delete_connection(vm, shared, obj, reason, DeleteMode::Silent);
    }
}

/// The work a datagram or packet of `len` bytes costs.
pub const fn cost(len: usize) -> u32 {
    let units = len / 64;
    let units = if units > u32::MAX as usize {
        u32::MAX
    } else {
        units as u32
    };
    1u32.saturating_add(units)
}

pub fn global_text(vm: &Vm, name: &str) -> Option<Vec<u8>> {
    vm.global(name).map(|v| until_nul(&v).to_vec())
}

/// The most players a server takes, whatever `$Pref::Server::MaxPlayers` says: a pref is script's to write (and a
/// prefs file's to hold), so the limit it sets is held under a cap of the engine's own.
pub const MAX_PLAYERS: usize = 1024;

/// The players `$Pref::Server::MaxPlayers` (`text`, `None` or empty when unset) lets in: `default` when unset, never
/// more than [`MAX_PLAYERS`].
fn max_players(text: Option<&[u8]>, default: usize) -> usize {
    text.filter(|t| !t.is_empty())
        .map_or(default, |t| usize::try_from(atoi(t)).unwrap_or(0))
        .min(MAX_PLAYERS)
}

/// Copies what the handshake checks a request against from the scripts' variables and the engine natives' state (read for each
/// datagram: a script that runs in the middle of a pass may change them).
pub fn refresh_config<G: NetGame>(vm: &Vm, sh: &mut NetShared<G>) {
    let (allow, size) = {
        let engine = sh.engine.borrow();
        (engine.allow_connections, engine.ghost_size)
    };
    let max_players = max_players(
        global_text(vm, "$Pref::Server::MaxPlayers").as_deref(),
        G::DEFAULT_MAX_PLAYERS,
    );
    let password = global_text(vm, "$Pref::Server::Password").unwrap_or_default();
    let lan = global_text(vm, "$Server::LAN").is_some_and(|t| atob(&t));
    let codecs = Rc::clone(&sh.codecs);
    let (wide_clients, wide_slots) = codecs.wide_defaults();
    // `checkMaxRate`: the packets a second to each client, `$pref::Net::PacketRateToClient` when set (clamped by the rate
    // config).
    let to_client_rate = global_text(vm, "$pref::Net::PacketRateToClient")
        .filter(|t| !t.trim_ascii().is_empty())
        .map_or(DEFAULT_PACKET_RATE, |t| {
            u32::try_from(atoi(&t)).unwrap_or(1)
        });
    let mut prefs = ServerPrefs {
        lan,
        to_client_rate,
        engine_ghost_bits: None,
        wide_clients,
        wide_slots,
    };
    {
        let engine = sh.engine.borrow();
        sh.game.server_prefs(vm, &engine, &mut prefs);
    }
    let config = sh.hs.config_mut();
    config.packet_bytes = None;
    config.allow_connections = allow;
    config.ghost_limit = size;
    config.max_players = max_players;
    config.password = password;
    codecs.configure_server(config, &prefs);
}

pub fn send<G: NetGame>(shared: &Shared<G>, to: SocketAddr, data: &[u8]) -> bool {
    let transport = Rc::clone(&shared.borrow().transport);
    let sent = transport.borrow_mut().send_to(to, data);
    if sent {
        shared.borrow_mut().counters.packets_sent += 1;
    }
    sent
}

/// Sends a handshake reply; an engine session's replies are its own, so there an empty one sends nothing.
fn send_reply<G: NetGame>(shared: &Shared<G>, to: SocketAddr, reply: &[u8]) {
    if !reply.is_empty() {
        send(shared, to, reply);
    }
}

/// The IP address the per-address and per-network limits count `from` by: the transport's real one behind a stand-in
/// ([`Transport::peer_ip`](super::transport::Transport::peer_ip)).
pub(super) fn peer_ip<G: NetGame>(shared: &Shared<G>, from: SocketAddr) -> IpAddr {
    let transport = Rc::clone(&shared.borrow().transport);
    transport.borrow().peer_ip(from).to_canonical()
}

/// Whether `from` is the server itself: `127.0.0.1` with the socket's own port, also when the socket is bound to every
/// address.
pub fn is_own_address(local: Option<SocketAddr>, from: SocketAddr) -> bool {
    local.is_some_and(|local| from.ip() == Ipv4Addr::LOCALHOST && from.port() == local.port())
}

// ---- receive
// -----------------------------------------------------------------------------------------------------------------

/// The receive pass at network time `now`. `new_tick` starts a tick's work budget; a poll between two ticks (a realtime
/// server's wait) draws on what the tick has left.
pub fn receive<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, now: u32, new_tick: bool) {
    let transport = Rc::clone(&shared.borrow().transport);
    {
        let mut sh = shared.borrow_mut();
        sh.set_now(now);
        if new_tick {
            sh.work_used = 0;
            sh.stranger_work = 0;
            sh.source_work.clear();
        }
    }
    // Links the transport saw close (a web transport's game sockets): their connections go now.
    let gone = transport.borrow_mut().take_gone();
    for addr in gone {
        let conn = shared.borrow().hs.lookup(addr);
        if let Some(conn) = conn {
            drop_conn(
                vm,
                shared,
                conn,
                Some(b"Disconnected."),
                DeleteMode::Immediate,
            );
        }
        super::engine::forget(vm, shared, addr);
    }
    let local = transport.borrow().local_addr();
    let engine_mode = super::peer::ProtocolMode::of(vm) == torque_engine::ProtocolKind::Engine;
    let mut buf = vec![0u8; RECV_BUFFER_BYTES];
    let (mut reads, mut players, mut strangers) = (0usize, 0usize, 0usize);
    while reads < NET_READS_PER_PASS
        && players < NET_DATAGRAMS_PER_PASS
        && shared.borrow().work_used < NET_WORK_PER_TICK
    {
        // A datagram the last pass could not pay for is still at the head of the queue.
        let stashed = shared.borrow_mut().stash.take();
        reads += 1;
        // The datagram is looked at where it was read: only one that waits for the next pass is copied, so a dropped one costs
        // its read alone.
        let (from, data): (SocketAddr, Cow<'_, [u8]>) = match stashed {
            Some((from, held)) => (from, Cow::Owned(held)),
            None => {
                let got = transport.borrow_mut().recv(&mut buf);
                let (len, from) = match got {
                    Recv::Empty => break,
                    Recv::Skipped => continue,
                    Recv::Datagram { len, from } => (len.min(buf.len()), from),
                };
                if is_own_address(local, from) {
                    shared.borrow_mut().work_used += cost(len);
                    continue;
                }
                (from, Cow::Borrowed(buf.get(..len).unwrap_or_default()))
            }
        };
        // A client of the engine's protocol sends no datagram above what a source's share of work can take, so a longer one is no
        // honest peer's (the reserved work of one above about 1,700 bytes could never fit, and a reliable lane would send it again
        // for ever).
        if engine_mode && data.len() > NET_MAX_DATAGRAM {
            shared.borrow_mut().counters.oversize += 1;
            continue;
        }
        // A connection's game packet (a player's) reserves its worst case before it is handled; what the tick cannot pay
        // waits (and ends the pass). Every other datagram is a stranger's: it costs its bytes, from the strangers' pool, and one
        // past the pool is read and dropped at no cost, so a flood cannot take the players' work. Any datagram counts against
        // its source's share of the tick ([`SourceKey`]: a player's by address and port, a stranger's by IP, a smaller share),
        // and a source over it is skipped for the rest of the tick without costing the tick anything.
        let game = data.first().is_some_and(|b| b & 1 != 0);
        // Only a connection the table holds is counted by its address and port: a datagram that merely looks like a game
        // packet is a stranger's, counted for its IP, so a source cannot take a fresh share by varying its port.
        let player = game && shared.borrow().hs.lookup(from).is_some();
        let key = if player {
            SourceKey::Address(from)
        } else {
            SourceKey::Ip(transport.borrow().peer_ip(from).to_canonical())
        };
        {
            let mut sh = shared.borrow_mut();
            let bytes = cost(data.len());
            let need = if player {
                sh.reservation_override
                    .map_or_else(|| reserve_for(data.len()), |r| r.saturating_add(bytes))
            } else {
                bytes
            };
            let over_tick = sh.work_used.saturating_add(need) > NET_WORK_PER_TICK;
            if player && sh.work_used > 0 && over_tick {
                sh.stash = Some((from, data.into_owned()));
                break;
            }
            if !player
                && (strangers >= NET_STRANGER_DATAGRAMS_PER_PASS
                    || over_tick
                    || sh.stranger_work.saturating_add(need) > NET_STRANGER_WORK_PER_TICK)
            {
                sh.counters.strangers_dropped += 1;
                continue;
            }
            let share = if player {
                NET_WORK_PER_SOURCE
            } else {
                NET_STRANGER_WORK_PER_SOURCE
            };
            let used = sh.source_work.get(&key).copied().unwrap_or(0);
            if used.saturating_add(need) > share {
                sh.counters.over_budget += 1;
                continue;
            }
            sh.work_used += bytes;
            if player {
                players += 1;
            } else {
                strangers += 1;
                sh.stranger_work += bytes;
            }
        }
        {
            let mut sh = shared.borrow_mut();
            sh.counters.received += 1;
            refresh_config(vm, &mut sh);
        }
        let extra = handle_datagram(vm, shared, from, &data, now);
        let mut sh = shared.borrow_mut();
        sh.work_used = sh.work_used.saturating_add(extra);
        if !player {
            sh.stranger_work = sh.stranger_work.saturating_add(extra);
        }
        let charge = cost(data.len()).saturating_add(extra);
        if let Some(spent) = sh.source_work.get_mut(&key) {
            *spent = spent.saturating_add(charge);
        } else if sh.source_work.len() < SOURCES_TRACKED {
            sh.source_work.insert(key, charge);
        }
    }
}

/// One datagram. Returns extra work units (a connect request runs script).
fn handle_datagram<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    from: SocketAddr,
    data: &[u8],
    now: u32,
) -> u32 {
    // A datagram of one of the codecs' info types goes to the info handler before the handshake looks.
    let codecs = Rc::clone(&shared.borrow().codecs);
    if data.len() <= NET_MAX_DATAGRAM && data.first().is_some_and(|&ty| codecs.is_info_type(ty)) {
        return super::query::handle(vm, shared, from, data, now);
    }
    // In engine mode every other datagram is the sessions' (TGE's handshake and packets are not answered).
    if super::engine::ensure_host(vm, &mut shared.borrow_mut()).is_some() {
        return super::engine::receive(vm, shared, from, data, now);
    }
    // Every per-address and per-network count keys by the peer's own address: a web peer behind a stand-in counts for its
    // real one.
    let ip = peer_ip(shared, from);
    let handled = {
        let mut sh = shared.borrow_mut();
        let table = sh.string_table();
        sh.hs.handle_datagram_from(&table, from, ip, data, now)
    };
    match handled {
        Handled::Ignored(_) => {
            shared.borrow_mut().counters.ignored += 1;
            0
        }
        Handled::Reply(reply) => {
            let types = codecs.handshake_types();
            let is_reject = data.first() == Some(&types.connect_request)
                && reply.first() == Some(&types.connect_reject);
            if is_reject {
                // A reject the handshake made takes an object and runs `onDrop`: it draws on a budget of its own, shared out
                // per network as the requests' is.
                if shared.borrow_mut().rejects.allow(ip, now).is_ok() {
                    reject_candidate(vm, shared, from, data, &reply);
                } else {
                    shared.borrow_mut().counters.over_budget += 1;
                }
                REQUEST_WORK
            } else {
                send(shared, from, &reply);
                0
            }
        }
        Handled::Request(request) => {
            handle_request(vm, shared, *request, now);
            REQUEST_WORK
        }
        Handled::Game {
            conn,
            received,
            payload_bit,
        } => handle_game(vm, shared, conn, &received, payload_bit, data, now),
        Handled::GameError { conn, error } => {
            let why = shared.borrow().game.error_text(&error);
            packet_error(vm, shared, conn, &why);
            0
        }
        Handled::Disconnected { conn, reason } => {
            peer_disconnected(vm, shared, conn, &reason);
            0
        }
    }
}

pub fn blank_record<C: Default>(addr: SocketAddr, sequence: u32) -> ConnRec<C> {
    ConnRec {
        obj: 0,
        conn: None,
        addr,
        sequence,
        protocol_version: 0,
        args: Vec::new(),
        reason: Vec::new(),
        packet_loss: 0.0,
        silent: false,
        dropping: false,
        cmd_rate: RateWindow::default(),
        plugin_cmds: Default::default(),
        share_window: 0,
        share_used: 0,
        delete_pending: false,
        paths_sent: false,
        local_peer: None,
        game: Default::default(),
    }
}

/// A request the handshake refused itself after the connection was registered: the object id is spent, the reject goes out,
/// and the drop runs (`Disconnect` datagram, `onDrop` with an empty reason).
fn reject_candidate<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    from: SocketAddr,
    data: &[u8],
    reply: &[u8],
) {
    let sequence = peek_type(data).map_or(0, |(_, seq)| seq);
    let Some(obj) = register_candidate(vm, shared, blank_record(from, sequence)) else {
        send(shared, from, reply);
        return;
    };
    shared.borrow_mut().counters.rejected += 1;
    send(shared, from, reply);
    delete_running(vm, shared, obj, None, DeleteMode::Immediate, true);
}

/// A connect request that passed the handshake's checks.
pub(super) fn handle_request<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    request: PendingRequest,
    now: u32,
) {
    shared.borrow_mut().counters.requests += 1;
    // The global rate and the script budget: a request over either is dropped, never the server, and a reconnect dropped here
    // keeps its old session.
    let admitted = {
        let mut sh = shared.borrow_mut();
        sh.script_available() && sh.requests.allow(request.ip, now).is_ok()
    };
    if !admitted {
        shared.borrow_mut().counters.over_budget += 1;
        return;
    }
    // An older connection from the same address is deleted before the new one is registered; it keeps no reason. The host does
    // it only once the request is admitted.
    if let Some(old) = request.replaced {
        drop_conn(vm, shared, old, None, DeleteMode::Immediate);
    }
    let rec = ConnRec {
        protocol_version: request.protocol_version,
        game: shared.borrow().game.conn_part(&request.fields.extra),
        args: request.fields.args.clone(),
        ..blank_record(request.addr, request.seq)
    };
    let Some(obj) = register_candidate(vm, shared, rec) else {
        return;
    };
    let address = shared.borrow().codecs.address_text(request.addr);
    let mut args = vec![address.into_bytes()];
    args.extend(request.fields.args.iter().cloned());
    let answer = run(vm, shared, true, |vm| {
        vm.call_method(obj, b"onConnectRequest", &args)
    });
    // A request the game will not serve ([`NetGame::refuses_silently`]) gets no answer at all, as from a server that is down;
    // the scripts' answer still comes first, and the connection is let go without a word.
    if matches!(&answer, Some(Ok(reason)) if reason.is_empty()) && {
        let sh = shared.borrow();
        sh.game.refuses_silently(vm, &sh.engine, request.extension)
    } {
        shared.borrow_mut().counters.dropped_requests += 1;
        let _ = delete_connection(vm, shared, obj, None, DeleteMode::Silent);
        return;
    }
    let decision = match answer {
        Some(Ok(reason)) if reason.is_empty() => Decision::Accept,
        Some(Ok(reason)) => Decision::Reject(until_nul(&reason).to_vec()),
        Some(Err(_)) | None => {
            // A script that hit a limit answering a stranger: no reply, no more work for this request.
            shared.borrow_mut().counters.dropped_requests += 1;
            let _ = delete_connection(vm, shared, obj, None, DeleteMode::Silent);
            return;
        }
    };
    // The script deleted the connection itself: nothing is left to answer.
    if !vm.objects().contains(obj) {
        return;
    }
    // A client that would be held while the server already holds as many as it allows is turned away in the reject.
    let decision = if matches!(decision, Decision::Accept) && content_busy(vm, shared, &request) {
        Decision::Reject(b"Server busy, try again.".to_vec())
    } else {
        decision
    };
    let finished = {
        let mut sh = shared.borrow_mut();
        if sh.engine_host.is_some() {
            super::engine::finish(&mut sh, &request, decision, now)
        } else {
            let table = sh.string_table();
            sh.hs.finish_request(&table, request.clone(), decision, now)
        }
    };
    match finished {
        Finished::Rejected { reply } => {
            shared.borrow_mut().counters.rejected += 1;
            send_reply(shared, request.addr, &reply);
            delete_running(vm, shared, obj, None, DeleteMode::Immediate, true);
        }
        Finished::Refused => {
            let _ = delete_connection(vm, shared, obj, None, DeleteMode::Silent);
        }
        Finished::Accepted {
            conn,
            reply,
            replaced,
            evicted,
        } => {
            {
                let mut sh = shared.borrow_mut();
                sh.counters.accepted += 1;
                if let Some(rec) = sh.conns.get_mut(&obj) {
                    rec.conn = Some(conn);
                }
                sh.by_conn.insert(conn, obj);
                let bits = sh.hs.connection(conn).map_or_else(
                    || sh.codecs.ghost_id_bits(sh.hs.config().ghost_limit),
                    |c| c.ghost_bits,
                );
                sh.payload.on_accept(conn, obj, bits);
                // The client's channels matched the gate's: it reads the channels' section from the first data packet.
                if sh.hs.connection(conn).is_some_and(|c| c.channels) {
                    sh.payload.mark_channels(conn);
                }
                // A client of the engine's own (the handshake's extension): new objects are scoped as they come and its
                // datablocks go with the codecs' window for it.
                if request.extension {
                    sh.payload.mark_peer(conn);
                    if let Some(hub) = sh.ghosts.clone() {
                        hub.set_fresh(conn, true);
                        hub.set_engine_peer(conn);
                    }
                }
            }
            if let Some(old) = replaced {
                drop_conn(vm, shared, old, None, DeleteMode::Immediate);
            }
            if let Some(identity) = &request.identity {
                end_older_sessions(vm, shared, conn, identity);
            }
            // The oldest pending connection made room for this one: it goes as one whose pending time ran out does.
            if let Some(old) = evicted {
                drop_conn(vm, shared, old, Some(b"TimedOut"), DeleteMode::Immediate);
            }
            let mods = shared
                .borrow()
                .hs
                .connection(conn)
                .and_then(|c| c.mods.clone());
            if let Some(section) = mods
                && let (Some(server), Some(hub)) = (
                    vm.extension::<ContentServerSlot>(),
                    vm.extension::<ChannelHub>(),
                )
            {
                // The accept goes out now and `onConnect` waits for the client to have what the server requires.
                match server.0.begin(vm, &hub, obj, conn, &section) {
                    // Nothing to require: the client got its (empty) list and nothing holds the join.
                    Ok(false) => {}
                    Ok(true) => {
                        let limits = server.0.limits(vm);
                        {
                            let mut sh = shared.borrow_mut();
                            // An engine connection's burst is as many packets a pass as its byte credit allows.
                            if let Some(c) = sh.hs.connection_mut(conn) {
                                c.protocol.set_burst(true);
                            }
                            sh.awaiting.push(Awaiting::joining(
                                conn,
                                obj,
                                request.fields.args.clone(),
                                now,
                                limits,
                            ));
                        }
                        send_reply(shared, request.addr, &reply);
                        return;
                    }
                    Err(reason) => {
                        server.0.end(vm, obj);
                        delete_running(
                            vm,
                            shared,
                            obj,
                            Some(reason.as_bytes()),
                            DeleteMode::Immediate,
                            false,
                        );
                        return;
                    }
                }
            }
            if !run_on_connect(vm, shared, obj, &request.fields.args) {
                return;
            }
            // `onConnect` may have kicked the client already.
            if vm.objects().contains(obj) {
                send_reply(shared, request.addr, &reply);
            }
        }
    }
}

/// The reason an account's older connection is dropped with when the same account connects again.
pub const SIGNED_IN_ELSEWHERE: &[u8] = b"You signed in from another place.";

/// Whether an account may have one connection at a time: `$Pref::Server::OneSessionPerAccount`, on when unset.
fn one_session_per_account(vm: &Vm) -> bool {
    global_text(vm, "$Pref::Server::OneSessionPerAccount").is_none_or(|text| atob(&text))
}

/// The same account connected again as `newest`: its older connections go, with [`SIGNED_IN_ELSEWHERE`], before `onConnect` of
/// the new one runs, so the roles, props and uploads the account holds follow the live session. A game that allows several
/// sessions per account turns this off with `$Pref::Server::OneSessionPerAccount = 0`.
fn end_older_sessions<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    newest: ConnId,
    identity: &Identity,
) {
    if !one_session_per_account(vm) {
        return;
    }
    let mut older: Vec<ConnId> = {
        let sh = shared.borrow();
        sh.by_conn
            .keys()
            .copied()
            .filter(|&c| c != newest)
            .filter(|&c| {
                sh.hs
                    .connection(c)
                    .is_some_and(|k| k.identity.as_ref() == Some(identity))
            })
            .collect()
    };
    older.sort_unstable_by_key(|c| c.0);
    for old in older {
        drop_conn(
            vm,
            shared,
            old,
            Some(SIGNED_IN_ELSEWHERE),
            DeleteMode::Immediate,
        );
    }
}

/// Calls `onConnect` of `obj` with the connect arguments; false when it hit a limit and the client was dropped for it.
fn run_on_connect<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    obj: ObjId,
    args: &[Vec<u8>],
) -> bool {
    // A connection is a member of `ClientGroup` from its `onConnect` on: while it is held it is in nothing a script walks.
    join_client_group(vm, obj);
    let connected = run(vm, shared, false, |vm| {
        vm.call_method(obj, b"onConnect", args)
    });
    if !matches!(connected, Some(Ok(_))) {
        // The client is in `ClientGroup` already: a proper drop (the Disconnect datagram, then `onDrop`), not a silent
        // delete.
        delete_running(
            vm,
            shared,
            obj,
            Some(ON_CONNECT_LIMIT_REASON),
            DeleteMode::Immediate,
            false,
        );
        return false;
    }
    true
}

/// Whether `obj` is held before its join and `name` is not a command the content server takes from a held client. A client
/// held for late content is playing: every command of it runs.
fn held_command_refused<G: NetGame>(vm: &Vm, shared: &Shared<G>, obj: ObjId, name: &[u8]) -> bool {
    if !shared
        .borrow()
        .awaiting
        .iter()
        .any(|a| a.obj == obj && !a.late)
    {
        return false;
    }
    vm.extension::<ContentServerSlot>()
        .is_none_or(|server| !server.0.allows_command(name))
}

/// Whether a request that would be held cannot be taken now: the server already holds as many clients as it allows, or as many
/// from the request's network ([`ContentLimits::per_network_clients`](super::content::ContentLimits::per_network_clients)).
fn content_busy<G: NetGame>(vm: &Vm, shared: &Shared<G>, request: &PendingRequest) -> bool {
    let Some(server) = vm.extension::<ContentServerSlot>() else {
        return false;
    };
    let network = NetworkKey::of(request.ip);
    let (held, from_network, wants) = {
        let sh = shared.borrow();
        let wants = sh.hs.config().gate.as_ref().is_some_and(|gate| {
            gate.matches_channels(&request.fields) && gate.mods_support(&request.fields).is_some()
        });
        // The places are the joining clients': a player sent content mid-game holds none.
        let joining = sh.awaiting.iter().filter(|a| !a.late);
        let from_network = joining
            .clone()
            .filter_map(|a| sh.hs.connection(a.conn))
            .filter(|c| NetworkKey::of(c.ip) == network)
            .count();
        (joining.count(), from_network, wants)
    };
    wants
        && server.0.holds(vm)
        && (!server.0.admit(vm, held)
            || from_network >= server.0.limits(vm).per_network_clients.get())
}

/// Holds the connections the content server sent content to mid-game ([`ContentServer::take_late`]): each goes into the
/// waiting list as a late entry, in a burst, with its live datablock rounds held until it is ready. A connection the host does
/// not know, or one held already, is left as it is.
///
/// [`ContentServer::take_late`]: super::content::ContentServer::take_late
fn begin_late<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, server: &ContentServerSlot, now: u32) {
    let started = server.0.take_late(vm);
    if started.is_empty() {
        return;
    }
    let limits = server.0.limits(vm);
    let mut sh = shared.borrow_mut();
    for obj in started {
        let Some(conn) = connection_of(&sh, obj) else {
            continue;
        };
        if sh.awaiting.iter().any(|a| a.obj == obj) {
            continue;
        }
        if let Some(c) = sh.hs.connection_mut(conn) {
            c.protocol.set_burst(true);
        }
        sh.events.hold_datablocks(conn);
        sh.awaiting.push(Awaiting::late(conn, obj, now, limits));
    }
}

/// Lets a held connection go on: `onConnect` for a joining client; for a client sent content mid-game only the burst ends
/// (the live datablock rounds that waited start in this tick's [`EventLayer::poll_datablock_holds`]).
fn release_held<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, entry: &Awaiting) {
    if let Some(c) = shared.borrow_mut().hs.connection_mut(entry.conn) {
        c.protocol.set_burst(false);
    }
    if !entry.late {
        run_on_connect(vm, shared, entry.obj, &entry.args);
    }
}

/// Runs `onConnect` of the connections whose client says it has what the server requires (or, for a client sent content
/// mid-game, lets its live datablock rounds go), and drops the ones that took too long, stood still, or are gone.
fn poll_content<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, now: u32) {
    let server = vm.extension::<ContentServerSlot>();
    if let Some(server) = &server {
        begin_late(vm, shared, server, now);
    }
    if shared.borrow().awaiting.is_empty() {
        return;
    }
    let Some(server) = server else {
        // The game took its content server away: nobody waits for it.
        let waiting = std::mem::take(&mut shared.borrow_mut().awaiting);
        for entry in waiting {
            if vm.objects().contains(entry.obj) {
                release_held(vm, shared, &entry);
            }
        }
        return;
    };
    let waiting = std::mem::take(&mut shared.borrow_mut().awaiting);
    let mut still = Vec::new();
    for mut entry in waiting {
        if !vm.objects().contains(entry.obj) {
            server.0.end(vm, entry.obj);
            continue;
        }
        if server.0.is_ready(vm, entry.obj) {
            server.0.end(vm, entry.obj);
            release_held(vm, shared, &entry);
            continue;
        }
        // Progress is what was served since the last mark, at the least rate the limits ask for: a client that takes a
        // block now and then to look busy stalls.
        let seen = server.0.progress(vm, entry.obj);
        if entry.limits.counts_as_progress(
            seen.saturating_sub(entry.seen),
            now.wrapping_sub(entry.moved),
        ) {
            entry.seen = seen;
            entry.moved = now;
        }
        let reason: Option<&[u8]> = if now.wrapping_sub(entry.moved) > entry.limits.idle() {
            Some(b"Mod download stalled.")
        } else if now.wrapping_sub(entry.since) > entry.limits.total() {
            Some(b"Mod download took too long.")
        } else {
            None
        };
        match reason {
            Some(reason) => {
                server.0.end(vm, entry.obj);
                delete_running(
                    vm,
                    shared,
                    entry.obj,
                    Some(reason),
                    DeleteMode::Immediate,
                    false,
                );
            }
            None => still.push(entry),
        }
    }
    shared.borrow_mut().awaiting.extend(still);
}

/// Deletes the object of handshake connection `conn`, if the host has one; else just forgets the connection.
pub(super) fn drop_conn<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    conn: ConnId,
    reason: Option<&[u8]>,
    mode: DeleteMode,
) {
    let obj = shared.borrow().by_conn.get(&conn).copied();
    match obj {
        Some(obj) => delete_running(vm, shared, obj, reason, mode, false),
        None => {
            shared.borrow_mut().hs.remove(conn);
        }
    }
}

/// A new data packet: acks and drops, a ping's answer, and the payload.
pub(super) fn handle_game<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    conn: ConnId,
    received: &Received,
    payload_bit: usize,
    data: &[u8],
    now: u32,
) -> u32 {
    // The work this datagram may spend was reserved before it was read ([`reserve_for`]); the layer pays from it. A payment
    // that fails means the bound was too small for what the datagram carried, and the connection is dropped with `Packet
    // Error.`.
    let mut left = u64::from(
        shared
            .borrow()
            .reservation_override
            .unwrap_or_else(|| reserve_for(data.len())),
    );
    let mut used = 0u32;
    let mut pay = |units: u64| {
        if units > left {
            return false;
        }
        left -= units;
        used = used.saturating_add(u32::try_from(units).unwrap_or(u32::MAX));
        true
    };
    let outcome = {
        let mut sh = shared.borrow_mut();
        let sh = &mut *sh;
        let obj = sh.by_conn.get(&conn).copied();
        let table = sh.string_table();
        let Some(c) = sh.hs.connection_mut(conn) else {
            return 0;
        };
        let addr = c.addr;
        let ghost_bits = c.ghost_bits;
        let answer = received.answer_ping.then(|| c.protocol.ack_packet(now));
        let mut ctx = ConnCtx {
            conn,
            obj,
            protocol: &mut c.protocol,
            now,
            vm: &mut *vm,
            ghost_bits,
            table,
            charge: &mut pay,
        };
        for notify in &received.notifies {
            sh.payload.on_notify(&mut ctx, *notify);
        }
        let outcome = if received.payload {
            let mut reader: BitReader<'_> = reader_at(data, payload_bit);
            sh.payload.on_game_packet(&mut ctx, received, &mut reader)
        } else {
            PacketOutcome::nothing()
        };
        (addr, answer, outcome, obj)
    };
    let (addr, answer, outcome, obj) = outcome;
    if let Some(ack) = answer {
        send(shared, addr, &ack);
    }
    // What was read before an error has happened (each unordered event is processed as it is read).
    run_packet_followups(vm, shared, obj, outcome.followups);
    if let Some(error) = outcome.error {
        let why = shared.borrow().game.error_text(&error);
        packet_error(vm, shared, conn, &why);
    }
    used
}

/// Runs what a packet (or a batch of script commands in the engine's protocol) of connection `obj` asked for. A script's
/// `delete()` of the connection waits until the followups are done: the later commands run with a live `%client`, then the drop
/// happens.
fn run_packet_followups<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    obj: Option<ObjId>,
    followups: Vec<Followup>,
) {
    shared.borrow_mut().packet_obj = obj;
    run_followups(vm, shared, obj, followups);
    let deferred = {
        let mut sh = shared.borrow_mut();
        sh.packet_obj = None;
        obj.filter(|obj| {
            sh.conns
                .get_mut(obj)
                .is_some_and(|rec| std::mem::take(&mut rec.delete_pending))
        })
    };
    if let Some(obj) = deferred {
        delete_running(vm, shared, obj, None, DeleteMode::Scheduled, false);
    }
}

/// The steps a tag argument of a script command costs its connection's share of the script window: the expansion copies up to
/// 1,023 bytes, and a command of the engine's protocol is not paid for from a datagram's reservation as a packet's is.
const COMMAND_TAG_STEPS: u64 = 64;

/// Runs the script commands `conn`'s client sent in the engine's protocol, each a body ([`command_rpc`](super::command_rpc)),
/// as the commands of a packet run: the same gates and limits ([`run_followups`]). A body that is not one a sender of this
/// engine makes ends the connection with `Packet Error.`, after the commands before it ran.
pub(super) fn run_command_bodies<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    conn: ConnId,
    bodies: Vec<Vec<u8>>,
) {
    let (events, obj) = {
        let sh = shared.borrow();
        (sh.events.clone(), sh.by_conn.get(&conn).copied())
    };
    let Some(obj) = obj else { return };
    let mut followups = Vec::with_capacity(bodies.len());
    let mut failed = None;
    for body in &bodies {
        match events.decode_command(vm, conn, body) {
            Ok(Some(followup)) => {
                if let Followup::ServerCmd { args, .. } = &followup {
                    let tags = args.iter().filter(|a| a.first() == Some(&1)).count();
                    shared
                        .borrow_mut()
                        .charge_share(obj, COMMAND_TAG_STEPS.saturating_mul(tags as u64));
                }
                followups.push(followup);
            }
            Ok(None) => {}
            Err(error) => {
                failed = Some(error);
                break;
            }
        }
    }
    run_packet_followups(vm, shared, Some(obj), followups);
    if let Some(error) = failed {
        let why = shared.borrow().game.error_text(&error);
        packet_error(vm, shared, conn, &why);
    }
}

/// Whether a client's remote command may run ([`NetGame::command_allowed`]). Read when the command is about to run, so an
/// earlier command of the same packet may have set it.
fn command_allowed<G: NetGame>(vm: &Vm, shared: &Shared<G>, obj: ObjId) -> bool {
    let sh = shared.borrow();
    sh.conns
        .get(&obj)
        .is_some_and(|rec| sh.game.command_allowed(vm, &rec.game))
}

/// Runs what a packet asked for, each as a script run of its own under the network's limits. Only a real error ends the rest of
/// the packet's work: an unknown function and a command that fails its gate are silent.
///
/// `conn_obj` is the connection the packet came from. Its commands and calls draw on that connection's share of the script
/// window ([`NET_CONN_SCRIPT_SHARE_STEPS`](super::NET_CONN_SCRIPT_SHARE_STEPS)): a run is capped at what is left of the share,
/// and once the share is spent the rest of the packet is dropped (counted in `over_share`); a host that sets
/// [`NetShared::commands_per_conn_per_s`](super::NetShared::commands_per_conn_per_s) also caps the commands a second (the rest
/// are dropped, counted in `commands_dropped`; none is set by default). A window or share that is spent ends the packet's work
/// (`break`), never a later followup's.
fn run_followups<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    conn_obj: Option<ObjId>,
    followups: Vec<Followup>,
) {
    let total = followups.len();
    for (index, followup) in followups.into_iter().enumerate() {
        // The calls and commands a connection makes: its share of the window must have steps left.
        if matches!(followup, Followup::Call { .. } | Followup::ServerCmd { .. })
            && let Some(obj) = conn_obj
            && shared.borrow_mut().share_left(obj) == 0
        {
            shared.borrow_mut().counters.over_share += (total - index) as u64;
            break;
        }
        let result: Option<Result<(), VmError>> = match followup {
            Followup::Call { object, name, args } => {
                run_for(vm, shared, true, conn_obj, |vm| match object {
                    Some(object) => vm.call_method(object, &name, &args).map(|_| ()),
                    None => vm.invoke(None, &name, &args).map(|_| ()),
                })
            }
            Followup::ServerCmd { obj, name, args } => {
                if !command_allowed(vm, shared, obj) {
                    continue;
                }
                // A connection that is held (its `onConnect` has not run) may speak only to the content server: any other
                // command is dropped, as if it had not been sent.
                if held_command_refused(vm, shared, obj, &name) {
                    shared.borrow_mut().counters.commands_dropped += 1;
                    continue;
                }
                // A command a plugin declared is held to its spec (the connection's bucket, the count, size and types of its
                // arguments) before its function runs; any other runs as TGE's do.
                let perms = vm.extension::<torque_engine::permissions::Permissions>();
                let admission = {
                    let mut sh = shared.borrow_mut();
                    let table = sh.commands.clone();
                    let tick = sh.tick;
                    let key = super::permissions::subject_key(&sh, obj);
                    let view = match (&perms, key) {
                        (Some(perms), Some(key)) => Some(perms.view(key)),
                        _ => None,
                    };
                    let view: &dyn torque_engine::permissions::PermissionView = match &view {
                        Some(view) => view,
                        None => &torque_engine::permissions::NoPermissions,
                    };
                    match (table, sh.conns.get_mut(&obj)) {
                        (Some(table), Some(rec)) => rec
                            .plugin_cmds
                            .admit_for(&table, obj, &name, &args, tick, view),
                        _ => Admission::Unlimited,
                    }
                };
                if let Admission::Refused { report, .. } = admission {
                    shared.borrow_mut().counters.commands_dropped += 1;
                    if let Some(line) = report {
                        vm.print(line.as_bytes());
                    }
                    continue;
                }
                let within_rate = {
                    let mut sh = shared.borrow_mut();
                    match sh.commands_per_conn_per_s {
                        None => true,
                        Some(limit) => {
                            let now = sh.now();
                            sh.conns
                                .get_mut(&obj)
                                .is_none_or(|rec| rec.cmd_rate.allow(now, limit))
                        }
                    }
                };
                if !within_rate {
                    shared.borrow_mut().counters.commands_dropped += 1;
                    continue;
                }
                // A name with no function costs the connection's share as a short run would, so a flood of them runs out of
                // share as real commands do; its warning is the script engine's, whose lines are limited per source
                // (`Vm::invoke_from`), so the flood does not fill the log either.
                if !vm.is_function(None, &name) {
                    charge_unknown_command(shared, obj, &name);
                }
                let mut argv = vec![obj.to_string().into_bytes()];
                argv.extend(args);
                run_for(vm, shared, true, conn_obj, |vm| {
                    vm.invoke_from(CommandSource(u64::from(obj)), None, &name, &argv)
                        .map(|_| ())
                })
            }
            Followup::Print(line) => {
                vm.print(&line);
                Some(Ok(()))
            }
            Followup::Drop { obj, reason } => {
                delete_running(vm, shared, obj, Some(&reason), DeleteMode::Immediate, false);
                Some(Ok(()))
            }
        };
        match result {
            Some(Ok(())) => {}
            Some(Err(_)) => break,
            None => {
                shared.borrow_mut().counters.over_budget += 1;
                break;
            }
        }
    }
}

/// A remote command of connection `obj` that names no function: charged to the connection's share
/// ([`NET_UNKNOWN_COMMAND_STEPS`] and a step per byte of the name) and counted.
fn charge_unknown_command<G: NetGame>(shared: &Shared<G>, obj: ObjId, name: &[u8]) {
    let mut sh = shared.borrow_mut();
    sh.counters.unknown_commands += 1;
    sh.charge_share(
        obj,
        NET_UNKNOWN_COMMAND_STEPS.saturating_add(u64::try_from(name.len()).unwrap_or(u64::MAX)),
    );
}

/// A protocol error from a connection: the error is printed, and the connection is given the reason `Packet Error.` and
/// destroyed at once.
pub(super) fn packet_error<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, conn: ConnId, why: &str) {
    let obj = shared.borrow().by_conn.get(&conn).copied();
    // A connection with an object is removed from the handshake table as the object goes, after `onDrop` ran: until then the
    // connection still has its identity (what its permissions are kept under).
    let Some(obj) = obj else {
        shared.borrow_mut().hs.remove(conn);
        return;
    };
    vm.print(format!("Client {obj} packet error: {why}.").as_bytes());
    delete_running(
        vm,
        shared,
        obj,
        Some(b"Packet Error."),
        DeleteMode::Immediate,
        false,
    );
}

/// The peer's own Disconnect: its text is the reason (`onDisconnect`). The connection is destroyed at once.
pub(super) fn peer_disconnected<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    conn: ConnId,
    reason: &[u8],
) {
    let obj = shared.borrow().by_conn.get(&conn).copied();
    let Some(obj) = obj else { return };
    vm.print(format!("Client {obj} disconnected.").as_bytes());
    delete_running(vm, shared, obj, Some(reason), DeleteMode::Immediate, false);
}

// ---- tick
// --------------------------------------------------------------------------------------------------------------------

/// `onTrigger` calls from the moves of a tick: each is a script run on behalf of the connection that sent the move (under its
/// share of the network's script window), and at most [`NET_TRIGGER_CALLS_PER_TICK`] are made in a tick for all connections. A
/// call past either limit is dropped and counted; the camera's trigger state has already moved on, so the edge is not called
/// late.
struct TickCalls<'a, G: NetGame> {
    shared: &'a Shared<G>,
    owner: Option<ObjId>,
    left: &'a mut u32,
}

impl<G: NetGame> TriggerCalls for TickCalls<'_, G> {
    fn on_trigger(
        &mut self,
        vm: &mut Vm,
        datablock: ObjId,
        obj: ObjId,
        slot: usize,
        state: bool,
    ) -> Result<(), VmError> {
        let spent = *self.left == 0
            || self
                .owner
                .is_some_and(|o| self.shared.borrow_mut().share_left(o) == 0);
        if spent {
            self.shared.borrow_mut().counters.triggers_dropped += 1;
            return Ok(());
        }
        *self.left -= 1;
        let ran = run_for(vm, self.shared, true, self.owner, |vm| {
            call_on_trigger(vm, datablock, obj, slot, state)
        });
        if ran.is_none() {
            // The window is spent: no more calls this tick.
            *self.left = 0;
            let mut sh = self.shared.borrow_mut();
            sh.counters.over_budget += 1;
            sh.counters.triggers_dropped += 1;
        }
        // A script that fails has printed its error; the move goes on.
        Ok(())
    }
}

/// The control branch of the pass (`processServerTick` and the move credit pass after it), the part that belongs to the
/// connections. `Server::run_ticks` hands it to [`advance_objects`](torque_game::sim::pass::advance_objects), which visits the
/// objects in process-list order: for a connection's control object, `getMoveList` takes `min(size, credit)` of its moves, its
/// ticker runs on each of them while the object is still the connection's control object, and `clearMoves` drops the ones that
/// ran. With no move there is no call at all. After the batch the credit rises (capped at 30) and a connection with no control
/// object has its list emptied. Runs before the send pass, so the ack the next head carries is `lastMoveAck - size` after the
/// tick. A control object whose class has no real tick has its moves consumed and nothing run on them.
pub struct NetControl<'a, G: NetGame> {
    shared: &'a Shared<G>,
    layer: EventLayer<G::Events, G::Ghosts>,
    hub: Option<GhostHub<G::Ghosts>>,
    ids: Vec<ConnId>,
    left: u32,
    by_obj: std::collections::HashMap<ObjId, ConnId>,
}

/// The control branch for the connections of `shared`.
pub fn control_side<G: NetGame>(shared: &Shared<G>) -> NetControl<'_, G> {
    let sh = shared.borrow();
    NetControl {
        shared,
        layer: sh.events.clone(),
        hub: sh.ghosts.clone(),
        ids: sh.hs.ids(),
        left: NET_TRIGGER_CALLS_PER_TICK,
        by_obj: std::collections::HashMap::new(),
    }
}

impl<G: NetGame> NetControl<'_, G> {
    fn control_of(&self, id: ConnId) -> Option<ObjId> {
        self.hub.as_ref().and_then(|h| h.control_of(id))
    }
}

impl<G: NetGame> ControlSide for NetControl<'_, G> {
    fn begin_step(&mut self) {
        // The map is made once for the step; each object's turn checks it against the connection's control object as it is then
        // (another connection's script may have taken the object since).
        self.by_obj = self
            .ids
            .iter()
            .filter_map(|id| self.control_of(*id).map(|obj| (obj, *id)))
            .collect();
    }

    fn run_control(&mut self, vm: &mut Vm, obj: ObjId, ticker: &mut Ticker) -> bool {
        let Some(&conn) = self.by_obj.get(&obj) else {
            return false;
        };
        if self.control_of(conn) != Some(obj) {
            return false;
        }
        let moves = self.layer.take_moves(conn);
        let owner = self.shared.borrow().by_conn.get(&conn).copied();
        let mut calls = TickCalls {
            shared: self.shared,
            owner,
            left: &mut self.left,
        };
        let mut ran = 0;
        for mv in &moves {
            if self.hub.as_ref().and_then(|h| h.control_of(conn)) != Some(obj)
                || !vm.objects().contains(obj)
            {
                break;
            }
            let mut cx = TickCx {
                vm: &mut *vm,
                calls: &mut calls,
            };
            ticker.process_tick(&mut cx, Some(&adapters::sim_move(mv)));
            ran += 1;
        }
        self.layer.finish_moves(conn, ran);
        true
    }

    fn end_batch(&mut self, steps: u32) {
        for id in std::mem::take(&mut self.ids) {
            let has = self.control_of(id).is_some();
            self.layer.end_batch(id, steps, has);
        }
    }
}

// ---- send
// --------------------------------------------------------------------------------------------------------------------

/// The send pass at network time `now`.
pub fn send_pass<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, now: u32) {
    shared.borrow_mut().set_now(now);
    // The tick pass starts with `collapseDirtyList`: the objects marked since the last pass reach the connections' ghosts, most
    // recently first-marked first. Every connection's ghost work credit is full again.
    let hub = shared.borrow().ghosts.clone();
    // What plugins keep out of connections' ghosts goes with the connections that are gone.
    if let Some(scope) = torque_engine::scope::ScopeControl::of(vm) {
        scope.sweep(vm);
    }
    torque_game::plugin_collision::report_overflow(vm);
    if let Some(hub) = hub.as_ref() {
        hub.begin_pass();
        hub.collapse();
    }
    // The game's work of the pass ([`NetGame::begin_send_pass`]); the connections it gives up are dropped.
    for (conn, why) in G::begin_send_pass(vm, shared, now) {
        packet_error(vm, shared, conn, why);
    }
    let burst = shared.borrow().game.burst(vm);
    let order = {
        let sh = shared.borrow();
        let mut ids = sh.hs.ids();
        // Rotate so that a pass that ran out of work starts where the last one stopped.
        let start = ids.iter().position(|id| id.0 >= sh.cursor).unwrap_or(0);
        ids.rotate_left(start);
        ids
    };
    let transport = Rc::clone(&shared.borrow().transport);
    let mut work = 0u32;
    let mut lagging: Vec<ConnId> = Vec::new();
    let mut failed: Vec<(ConnId, NetError)> = Vec::new();
    // The preference the engine connections' byte rate is held under, read once a pass and only when one is sent to.
    let mut byte_rate = None;
    let emit = |packet: Option<(SocketAddr, Vec<u8>)>, work: &mut u32| match packet {
        Some((to, bytes)) => {
            *work = work.saturating_add(cost(bytes.len()));
            if transport.borrow_mut().send_to(to, &bytes) {
                shared.borrow_mut().counters.packets_sent += 1;
            }
            true
        }
        None => false,
    };
    for id in order {
        if work >= NET_WORK_PER_TICK {
            shared.borrow_mut().cursor = id.0;
            break;
        }
        shared.borrow_mut().cursor = id.0.wrapping_add(1);
        if shared.borrow().hs.connection(id).is_some_and(|c| c.engine) {
            let cap = *byte_rate.get_or_insert_with(|| super::engine::byte_rate_pref(vm));
            work = work.saturating_add(super::engine::send_conn(
                vm,
                shared,
                id,
                now,
                cap,
                &mut failed,
            ));
            work = work.saturating_add(take_payload_work(shared, id));
            run_payload_calls(vm, shared, id);
            continue;
        }
        // The gated packet, then forced ones until the window is full while the connection is busy.
        let first = build_packet(vm, shared, id, now, false, &mut lagging, &mut failed);
        work = work.saturating_add(take_payload_work(shared, id));
        run_payload_calls(vm, shared, id);
        let mut sent = u32::from(emit(first, &mut work));
        while let Some(b) = burst
            && sent < b.packets
            && can_burst(shared, id, b)
        {
            let built = build_packet(vm, shared, id, now, true, &mut lagging, &mut failed);
            work = work.saturating_add(take_payload_work(shared, id));
            run_payload_calls(vm, shared, id);
            if !emit(built, &mut work) {
                break;
            }
            sent += 1;
        }
    }
    // The engine sessions not accepted yet: their cookies, accepts, rejects and goodbyes.
    super::engine::poll_pending(vm, shared, now);
    // The pass's end, after the connections have been written: the ghost game's hook and the feed of new objects.
    if let Some(hub) = hub.as_ref() {
        hub.end_pass(vm.sim_time());
    }
    // An event no packet can carry: the peer is dropped (it would stall for ever).
    for (conn, error) in failed {
        let why = shared.borrow().game.error_text(&error);
        packet_error(vm, shared, conn, &why);
    }
    for conn in lagging {
        let obj = shared.borrow().by_conn.get(&conn).copied();
        if let Some(obj) = obj {
            G::lagging(vm, shared, obj);
        }
    }
}

/// Runs the script calls the payload layer asked for while it built a packet of `id` (the ghost layer's script calls), now
/// that the state is released. They are the server's own calls: under the network window, not on the connection's share.
fn run_payload_calls<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, id: ConnId) {
    let calls = shared.borrow_mut().payload.take_calls(id);
    if !calls.is_empty() {
        run_followups(vm, shared, None, calls);
    }
}

/// The work the payload layer spent on `id`'s last packet beyond its bytes (taken, and counted).
fn take_payload_work<G: NetGame>(shared: &Shared<G>, id: ConnId) -> u32 {
    let mut sh = shared.borrow_mut();
    let units = sh.payload.take_work(id);
    sh.counters.payload_work = sh.counters.payload_work.saturating_add(u64::from(units));
    units
}

/// Whether the send pass may go on bursting to `id`: the layer says busy and the round trip is under the burst's limit.
fn can_burst<G: NetGame>(shared: &Shared<G>, id: ConnId, burst: Burst) -> bool {
    let sh = shared.borrow();
    sh.payload.busy(id)
        && sh
            .hs
            .connection(id)
            .is_some_and(|c| c.protocol.round_trip() < burst.max_rtt_ms)
}

/// One data packet for `id` if the gate allows it: where to and the bytes.
fn build_packet<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    id: ConnId,
    now: u32,
    force: bool,
    lagging: &mut Vec<ConnId>,
    failed: &mut Vec<(ConnId, NetError)>,
) -> Option<(SocketAddr, Vec<u8>)> {
    let mut sh = shared.borrow_mut();
    let sh = &mut *sh;
    let obj = sh.by_conn.get(&id).copied();
    let table = sh.string_table();
    let lag_ms = sh.codecs.infinite_lag_ms();
    let c = sh.hs.connection_mut(id)?;
    // The game hears of the lag at every gate call while the round trip is above the codecs' limit.
    let lag = lag_ms.is_some_and(|ms| c.protocol.round_trip() > ms);
    let allowed = c.protocol.may_send(now, force);
    if lag && !lagging.contains(&id) {
        lagging.push(id);
    }
    if !allowed {
        return None;
    }
    let addr = c.addr;
    let ghost_bits = c.ghost_bits;
    let mut w = c.protocol.begin_data_packet(now);
    // The send pass is charged by the packets it emits.
    let mut free = |_: u64| true;
    let mut ctx = ConnCtx {
        conn: id,
        obj,
        protocol: &mut c.protocol,
        now,
        vm,
        ghost_bits,
        table,
        charge: &mut free,
    };
    if let Err(e) = sh.payload.write(&mut ctx, &mut w) {
        failed.push((id, e));
        return None;
    }
    // A packet past the datagram bound is not sent and the client is not dropped for it. Its sequence
    // number is spent, so the client's next ack reports it lost and its events and ghost updates are sent again.
    if w.is_overflowed() {
        sh.counters.packets_withheld += 1;
        let who = obj.map_or_else(|| format!("{}", id.0), |o| o.to_string());
        vm.print(
            format!("Client {who}: a packet past the datagram bound was not sent (counted lost)")
                .as_bytes(),
        );
        return None;
    }
    Some((addr, w.into_bytes()))
}

// ---- timeouts
// ----------------------------------------------------------------------------------------------------------------

/// The timeout sweep at network time `now`.
pub fn timeouts<G: NetGame>(vm: &mut Vm, shared: &Shared<G>, now: u32) {
    shared.borrow_mut().set_now(now);
    poll_content(vm, shared, now);
    // The live rounds held for a connection's content start once its content server says it is ready.
    let layer = shared.borrow().events.clone();
    layer.poll_datablock_holds(vm);
    let events = {
        let mut sh = shared.borrow_mut();
        sh.set_now(now);
        if now.wrapping_sub(sh.last_sweep) <= NET_TIMEOUT_SWEEP_MS {
            return;
        }
        sh.last_sweep = now;
        sh.hs.poll_timeouts(now)
    };
    for event in events {
        match event {
            TimeoutEvent::Ping { to, datagram, .. } => {
                shared.borrow_mut().counters.pings_sent += 1;
                send(shared, to, &datagram);
            }
            TimeoutEvent::TimedOut { conn } => {
                let obj = shared.borrow().by_conn.get(&conn).copied();
                if let Some(obj) = obj {
                    vm.print(format!("Client {obj} timed out.").as_bytes());
                }
                drop_conn(vm, shared, conn, Some(b"TimedOut"), DeleteMode::Immediate);
            }
            TimeoutEvent::PendingExpired { conn } => {
                drop_conn(vm, shared, conn, Some(b"TimedOut"), DeleteMode::Immediate);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_players_is_held_under_the_engines_cap() {
        assert_eq!(max_players(None, 64), 64);
        assert_eq!(max_players(Some(b""), 64), 64);
        assert_eq!(max_players(Some(b"12"), 64), 12);
        assert_eq!(max_players(Some(b"-5"), 64), 0);
        assert_eq!(max_players(Some(b"99999999"), 64), MAX_PLAYERS);
    }
}
