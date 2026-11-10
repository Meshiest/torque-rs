//! The server query's client half, TGE's `serverQuery.cc`: `queryLanServers`, `processPingsAndQueries` (a `ProcessPingEvent` 1 ms
//! of Sim time later while work is left), the ping answer (`handleGamePingResponse`), the info answer (`handleGameInfoResponse`),
//! the progress lines (`updatePingProgress`, the query's), `cancelServerQuery`, `stopServerQuery`, `getServerCount`,
//! `setServerInfo`, the favourites (`pushServerFavorites`) and the server records (find, add, remove). `querySingleServer` and
//! `queryMasterServer` do nothing here (no master list); a game's own pings are its own.
//!
//! **The query.** `queryLanServers(engine, flags, gameType, missionType, minPlayers, maxPlayers, maxBots, regionMask, maxPing, minCPU,
//! filterFlags)` starts a new session (the counter +1), clears the ping, query, finished and server lists, pushes the favourites,
//! sets the filter type to Offline (2: no filter applies to a LAN answer), pushes ONE ping to `255.255.255.255:port` (one try, the
//! broadcast mark), calls `onServerQueryStatus("start", "Querying LAN servers", "0")` and runs `processPingsAndQueries` at once.
//!
//! `processPingsAndQueries(session)` (nothing when the session is not the current one): the first 10 pings whose last send is more
//! than 800 ms old are sent again while they have tries (`Pinging Server %s (%d)...` with the tries left, or `LAN server ping: %s...`
//! for the broadcast), else time out (`Ping to server %s timed out.` but for the broadcast; a server record of that address gets the
//! status 0x80000000), the address goes to the finished list and the ping progress is shown; then, when no ping is left, the first 2
//! queries the same way (`Querying Server %s (%d)...`; `Query to server %s timed out.`, status 0x80000000; a query whose server
//! record is gone is dropped). Each request is `U8 type, U8 0, U32 (session << 16) | (key & 0xffff)` with the key counter +1 a send.
//! When both lists are empty: `onServerQueryStatus("done", "No servers found." | "One server found." | "%d servers found.", "1")`;
//! else the next pass 1 ms of Sim time later (the engine's pass runs before the next tick of the client host: a 32 ms grain).
//!
//! **Answers.** A ping answer when the ping list is empty is dropped. One from an address the list does not hold (the answer to the
//! broadcast) starts a unicast ping (4 tries, counted in the ping total) unless the address is finished. One from a listed address
//! must echo its key; then the game's version fields ([`QueryState::versions`]: `VER1`, protocol at least the profile's
//! `protocol_min` and the server's minimum at most its `protocol_current`; TGE's are 12 and 12) (else `Server %s is a different version.`,
//! `Protocol for server %s does not meet minimum protocol.`, `You do not meet the minimum protocol for server %s.`; the server record
//! gets 0x80000000), the ping time `now - sent`, the version number, the name (kept if the record had none); the address is finished
//! and moves to the query list (4 tries, the query total +1); the ping progress. An info answer from a queried address leaves the
//! query list (the query progress), and fills the record: game type, mission type, mission name (cut at `.mis`), the status byte with
//! the answered bit 0x40000000, players, most players, bots, the CPU, the info string, the long string as the record's state (at most
//! 2,047 bytes kept).
//!
//! **Progress.** `onServerQueryStatus("ping", "Waiting for lan servers..." | "Pinging servers: %d left...", "%g")` with `(total - left) /
//! (2 * total)` (0 while the total is 0), the broadcast not counted among those left; `("query", "Querying servers: %d left...",
//! "%g")` with `(total - left) / (2 * total) + 0.5` (0.5 while the total is 0), shown only when no ping is left.
//!
//! **Times** are the client host's Sim milliseconds. **Hostile answers**: every string is read through the
//! Huffman reader's caps, the lists and the server table are capped ([`MAX_SERVERS`], [`MAX_PINGS`]), an answer that does not parse
//! is dropped.
//!
//! Credit: follows TGE's server query as Torque3D carries it (MIT: `app/net/serverQuery.cpp`, Copyright (c) 2012 GarageGames,
//! LLC).

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::rc::Rc;

use net::bits::BitReader;
use net::huffman::Table;
use net::profile::QueryProfile;
use net::serverquery::{
    GAME_INFO_REQUEST, GAME_INFO_RESPONSE, GAME_PING_REQUEST, GAME_PING_RESPONSE,
};
use ts_vm::text::atoi;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::conn::ConnGame;
use crate::game::client_game;
use crate::net::{BROADCAST, ClientNet};
use crate::state::ClientState;

/// The ping and query time-out.
pub const TIMEOUT_MS: u32 = 800;
/// The most pings and queries in flight at once.
pub const PINGS_AT_ONCE: usize = 10;
pub const QUERIES_AT_ONCE: usize = 2;
/// A unicast ping's and a query's tries; the broadcast's.
pub const TRIES: u32 = 4;
pub const BROADCAST_TRIES: u32 = 1;
/// The bytes of a server name kept from a favourite (`strncpy(.., 24)`) and of the state string.
pub const FAVORITE_NAME_BYTES: usize = 24;
pub const MAX_STATE_BYTES: usize = 0x7ff;
/// The status of a server that did not answer, and the answered bit an info answer adds.
pub const STATUS_TIMED_OUT: u32 = 0x8000_0000;
pub const STATUS_RESPONDED: u32 = 0x4000_0000;
/// Engine caps on what hostile answers can grow: the server table, the ping and query lists, the finished list, the favourites read.
pub const MAX_SERVERS: usize = 1024;
pub const MAX_PINGS: usize = 1024;
pub const MAX_FINISHED: usize = 4096;
pub const MAX_FAVORITES: i32 = 256;

/// A ping or query in flight: the address, the session, the key, the last send, the tries, the broadcast mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub addr: SocketAddrV4,
    pub session: u32,
    pub key: u32,
    pub time: u32,
    pub tries: u32,
    pub broadcast: bool,
}

/// A server record (`setServerInfo` reads it into `$ServerInfo::*`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerInfo {
    pub players: u8,
    pub max_players: u8,
    pub bots: u8,
    pub name: Option<Vec<u8>>,
    pub game_type: Option<Vec<u8>>,
    pub mission_name: Option<Vec<u8>>,
    pub mission_type: Option<Vec<u8>>,
    pub state: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
    pub addr: Option<SocketAddrV4>,
    pub version: u32,
    pub ping: u32,
    pub cpu: u32,
    pub favorite: bool,
    pub status: u32,
}

/// The query's state.
#[derive(Debug, Default)]
pub struct QueryState {
    pub session: u32,
    pub key: u32,
    /// A query is running.
    pub active: bool,
    pub pings: Vec<Entry>,
    pub queries: Vec<Entry>,
    pub finished: Vec<SocketAddrV4>,
    pub servers: Vec<ServerInfo>,
    /// The unicast pings pushed and the queries of the session: the progress lines' totals.
    pub ping_total: u32,
    pub query_total: u32,
    /// The pass waiting: its session and the Sim time it is due.
    pub next_pass: Option<(u32, u32)>,
    /// The string coder (the game's Huffman table, [`crate::game::ClientGame::string_table`]), once the host has it.
    pub table: Option<Rc<Table>>,
    /// The version fields a ping answer is checked against: the version string, the oldest protocol the client takes and
    /// the newest it speaks (the game's codecs' [`QueryProfile`], set with the table).
    pub versions: QueryProfile,
}

/// What a step of the query makes the client do, in order, once no borrow is held: a console line or a script call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Print(String),
    Status(&'static str, String, String),
    /// A `Con::warnf` line (a game's own pings).
    Warn(String),
    /// A global script function called with these arguments (a game's own pings' callbacks).
    Call(&'static [u8], Vec<Vec<u8>>),
}

/// `%g` of a float as C's `printf` prints a `double` made from it (6 significant digits, no trailing zeros).
pub fn format_g(x: f32) -> String {
    ts_vm::text::format_g(f64::from(x))
}

fn address_text(a: SocketAddrV4) -> String {
    if *a.ip() == BROADCAST {
        format!("Broadcast:{}", a.port())
    } else {
        a.to_string()
    }
}

impl QueryState {
    fn server_index(&self, a: SocketAddrV4) -> Option<usize> {
        self.servers.iter().position(|s| s.addr == Some(a))
    }

    fn server_mut(&mut self, a: SocketAddrV4) -> Option<&mut ServerInfo> {
        self.servers.iter_mut().find(|s| s.addr == Some(a))
    }

    /// The record of `a`, made when there is none (`None` when the table is full: engine cap).
    fn add_server(&mut self, a: SocketAddrV4) -> Option<&mut ServerInfo> {
        if let Some(i) = self.server_index(a) {
            return self.servers.get_mut(i);
        }
        if self.servers.len() >= MAX_SERVERS {
            return None;
        }
        self.servers.push(ServerInfo {
            addr: Some(a),
            ..ServerInfo::default()
        });
        self.servers.last_mut()
    }

    fn is_finished(&self, a: SocketAddrV4) -> bool {
        self.finished.contains(&a)
    }

    fn finish(&mut self, a: SocketAddrV4) {
        if self.finished.len() < MAX_FINISHED {
            self.finished.push(a);
        }
    }

    /// A ping pushed (`broadcast` false, 4 tries, the ping total +1) or the broadcast (`broadcast`, 1 try): nothing for a finished
    /// address.
    ///
    /// **Engine-only:** nothing for an address the client may not send to ([`crate::net::is_lan_target`]; a unicast ping never goes to the
    /// broadcast address), and at most [`MAX_PINGS`] in the list.
    fn push_ping(&mut self, a: SocketAddrV4, broadcast: bool) {
        if self.is_finished(a)
            || self.pings.len() >= MAX_PINGS
            || !crate::net::is_lan_target(a)
            || (!broadcast && *a.ip() == BROADCAST)
        {
            return;
        }
        self.pings.push(Entry {
            addr: a,
            session: self.session,
            key: 0,
            time: 0,
            tries: if broadcast { BROADCAST_TRIES } else { TRIES },
            broadcast,
        });
        if !broadcast {
            self.ping_total = self.ping_total.wrapping_add(1);
        }
    }

    /// `updatePingProgress`: with no ping left, the query's progress.
    fn ping_progress(&self, out: &mut Vec<Out>) {
        if self.pings.is_empty() {
            self.query_progress(out);
            return;
        }
        let left = self.pings.iter().filter(|p| !p.broadcast).count() as u32;
        let msg = if left == 0 {
            "Waiting for lan servers...".to_string()
        } else {
            format!("Pinging servers: {left} left...")
        };
        let progress = if self.ping_total == 0 {
            0.0
        } else {
            // Both counts made floats, the quotient rounded to a float.
            (f64::from(self.ping_total.wrapping_sub(left) as f32)
                / f64::from(self.ping_total.wrapping_mul(2) as f32)) as f32
        };
        out.push(Out::Status("ping", msg, format_g(progress)));
    }

    /// The query's progress line (only with no ping left).
    fn query_progress(&self, out: &mut Vec<Out>) {
        if !self.pings.is_empty() {
            return;
        }
        let left = self.queries.len() as u32;
        let msg = format!("Querying servers: {left} left...");
        let progress = if self.query_total == 0 {
            0.5
        } else {
            // The quotient and the 0.5 (a double) added before the one rounding to a float.
            (f64::from(self.query_total.wrapping_sub(left) as f32)
                / f64::from(self.query_total.wrapping_mul(2) as f32)
                + 0.5) as f32
        };
        out.push(Out::Status("query", msg, format_g(progress)));
    }

    /// `processPingsAndQueries(session)` at `now`: what it sends goes through `net`.
    pub fn process(&mut self, session: u32, now: u32, net: &mut ClientNet, out: &mut Vec<Out>) {
        if session != self.session {
            return;
        }
        let mut i = 0usize;
        let mut slots = 0usize;
        while i < self.pings.len() && slots < PINGS_AT_ONCE {
            let p = self.pings[i];
            if p.time.wrapping_add(TIMEOUT_MS) < now {
                if p.tries != 0 {
                    let e = &mut self.pings[i];
                    e.time = now;
                    e.tries -= 1;
                    e.key = self.key;
                    self.key = self.key.wrapping_add(1);
                    let e = *e;
                    out.push(Out::Print(if e.broadcast {
                        format!("LAN server ping: {}...", address_text(e.addr))
                    } else {
                        format!("Pinging Server {} ({})...", address_text(e.addr), e.tries)
                    }));
                    net.send(e.addr, &request(&self.versions, GAME_PING_REQUEST, &e));
                    i += 1;
                    slots += 1;
                } else {
                    if !p.broadcast {
                        out.push(Out::Print(format!(
                            "Ping to server {} timed out.",
                            address_text(p.addr)
                        )));
                    }
                    if let Some(s) = self.server_mut(p.addr) {
                        s.status = STATUS_TIMED_OUT;
                    }
                    self.finish(p.addr);
                    self.pings.remove(i);
                    self.ping_progress(out);
                }
            } else {
                i += 1;
                slots += 1;
            }
        }
        if self.pings.is_empty() {
            let mut i = 0usize;
            let mut slots = 0usize;
            while i < self.queries.len() && slots < QUERIES_AT_ONCE {
                let q = self.queries[i];
                if q.time.wrapping_add(TIMEOUT_MS) < now {
                    if self.server_index(q.addr).is_none() {
                        self.queries.remove(i);
                        continue;
                    }
                    if q.tries != 0 {
                        let e = &mut self.queries[i];
                        e.time = now;
                        e.key = self.key;
                        self.key = self.key.wrapping_add(1);
                        e.tries -= 1;
                        let e = *e;
                        out.push(Out::Print(format!(
                            "Querying Server {} ({})...",
                            address_text(e.addr),
                            e.tries
                        )));
                        net.send(e.addr, &request(&self.versions, GAME_INFO_REQUEST, &e));
                        if let Some(s) = self.server_mut(e.addr)
                            && s.status & 0x1000_0000 == 0
                        {
                            s.status |= 0x1000_0000;
                        }
                        i += 1;
                        slots += 1;
                    } else {
                        out.push(Out::Print(format!(
                            "Query to server {} timed out.",
                            address_text(q.addr)
                        )));
                        if let Some(s) = self.server_mut(q.addr) {
                            s.status = STATUS_TIMED_OUT;
                        }
                        self.queries.remove(i);
                    }
                } else {
                    i += 1;
                    slots += 1;
                }
            }
        }
        if self.pings.is_empty() && self.queries.is_empty() {
            let msg = match self.servers.len() {
                0 => "No servers found.".to_string(),
                1 => "One server found.".to_string(),
                n => format!("{n} servers found."),
            };
            out.push(Out::Status("done", msg, "1".to_string()));
            self.next_pass = None;
        } else {
            self.next_pass = Some((self.session, now.wrapping_add(1)));
        }
    }

    /// `queryLanServers`' state work at `now`: the lists cleared, the favourites and the broadcast pushed.
    pub fn start_lan(&mut self, port: u16, favorites: &[(Vec<u8>, SocketAddrV4)]) {
        self.session = self.session.wrapping_add(1);
        self.active = true;
        self.servers.clear();
        self.finished.clear();
        self.pings.clear();
        self.queries.clear();
        self.query_total = 0;
        self.ping_total = 0;
        for (name, a) in favorites {
            if let Some(s) = self.add_server(*a) {
                if s.name.is_none() {
                    s.name = Some(name.clone());
                }
                s.favorite = true;
            }
            self.push_ping(*a, false);
        }
        self.push_ping(SocketAddrV4::new(BROADCAST, port), true);
    }

    /// `cancelServerQuery`: the records of every ping and query in flight not yet answered get 0x80000000, the lists are
    /// emptied, the query stops (`Server query canceled.` when one ran).
    pub fn cancel(&mut self, out: &mut Vec<Out>) {
        if !self.active {
            return;
        }
        out.push(Out::Print("Server query canceled.".to_string()));
        let pending: Vec<SocketAddrV4> = self
            .pings
            .iter()
            .chain(self.queries.iter())
            .map(|e| e.addr)
            .collect();
        for a in pending {
            if let Some(s) = self.server_mut(a)
                && s.status & STATUS_RESPONDED == 0
            {
                s.status = STATUS_TIMED_OUT;
            }
        }
        self.pings.clear();
        self.queries.clear();
        self.active = false;
    }

    /// `stopServerQuery`: with pings in flight, they all go to the finished list (the queries go on); else a cancel.
    pub fn stop(&mut self, out: &mut Vec<Out>) {
        if !self.active {
            return;
        }
        if self.pings.is_empty() {
            self.cancel(out);
            return;
        }
        let pings = std::mem::take(&mut self.pings);
        for p in pings {
            self.finish(p.addr);
        }
    }

    /// A ping answer (type 16) from `from` at `now`.
    pub fn ping_answer(&mut self, from: SocketAddrV4, data: &[u8], now: u32, out: &mut Vec<Out>) {
        if self.pings.is_empty() {
            return;
        }
        let Some(table) = self.table.clone() else {
            return;
        };
        let mut r = BitReader::new(data);
        let (Ok(_), Ok(_), Ok(key)) = (r.read_u8(), r.read_u8(), r.read_u32()) else {
            return;
        };
        let Some(index) = self.pings.iter().position(|p| p.addr == from) else {
            if !self.is_finished(from) {
                self.push_ping(from, false);
            }
            return;
        };
        let p = self.pings[index];
        if ((p.session << 16) | (p.key & 0xffff)) != key {
            return;
        }
        let failed = |s: &mut QueryState, msg: String, out: &mut Vec<Out>, timed: bool| {
            out.push(Out::Print(msg));
            s.finish(from);
            s.pings.remove(index);
            if timed && let Some(rec) = s.server_mut(from) {
                rec.status = STATUS_TIMED_OUT;
            }
            s.ping_progress(out);
        };
        let shown = address_text(from);
        let version = table.read(&mut r).unwrap_or_default();
        if version != self.versions.version_string {
            failed(
                self,
                format!("Server {shown} is a different version."),
                out,
                true,
            );
            return;
        }
        let Ok(protocol) = r.read_u32() else {
            return;
        };
        if protocol < self.versions.protocol_min {
            failed(
                self,
                format!("Protocol for server {shown} does not meet minimum protocol."),
                out,
                true,
            );
            return;
        }
        let Ok(min) = r.read_u32() else {
            return;
        };
        if min > self.versions.protocol_current {
            failed(
                self,
                format!("You do not meet the minimum protocol for server {shown}."),
                out,
                true,
            );
            return;
        }
        let ping = now.saturating_sub(p.time);
        let Ok(version_number) = r.read_u32() else {
            return;
        };
        let name = table.read(&mut r).unwrap_or_default();
        let Some(rec) = self.add_server(from) else {
            return;
        };
        rec.ping = ping;
        rec.version = version_number;
        if rec.name.is_none() {
            rec.name = Some(name);
        }
        self.finish(from);
        let mut q = p;
        q.key = 0;
        q.time = 0;
        q.tries = TRIES;
        if self.queries.len() < MAX_PINGS {
            self.queries.push(q);
        }
        self.query_total = self.query_total.wrapping_add(1);
        self.pings.remove(index);
        self.ping_progress(out);
    }

    /// An info answer (type 20) from `from`.
    pub fn info_answer(&mut self, from: SocketAddrV4, data: &[u8], out: &mut Vec<Out>) {
        if self.queries.is_empty() {
            return;
        }
        let Some(index) = self.queries.iter().position(|q| q.addr == from) else {
            return;
        };
        // Engine-only: the answer must echo the query's session and key, as a ping answer must (TGE's info handler matches the
        // address only).
        let q = self.queries[index];
        let mut head = BitReader::new(data);
        let (Ok(_), Ok(_), Ok(key)) = (head.read_u8(), head.read_u8(), head.read_u32()) else {
            return;
        };
        if key != ((q.session << 16) | (q.key & 0xffff)) {
            return;
        }
        self.queries.remove(index);
        self.query_progress(out);
        let Some(table) = self.table.clone() else {
            return;
        };
        let Some(rec) = self.server_mut(from) else {
            return;
        };
        let mut r = BitReader::new(data);
        let (Ok(_), Ok(_), Ok(_)) = (r.read_u8(), r.read_u8(), r.read_u32()) else {
            return;
        };
        let read = |r: &mut BitReader| table.read(r).ok();
        let Some(game_type) = read(&mut r) else {
            return;
        };
        rec.game_type = Some(game_type);
        let Some(mission_type) = read(&mut r) else {
            return;
        };
        rec.mission_type = Some(mission_type);
        let Some(mut mission) = read(&mut r) else {
            return;
        };
        if let Some(at) = mission.windows(4).position(|w| w == b".mis") {
            mission.truncate(at);
        }
        rec.mission_name = Some(mission);
        let Ok(status) = r.read_u8() else {
            return;
        };
        rec.status = u32::from(status) | STATUS_RESPONDED;
        let (Ok(players), Ok(max), Ok(bots)) = (r.read_u8(), r.read_u8(), r.read_u8()) else {
            return;
        };
        rec.players = players;
        rec.max_players = max;
        rec.bots = bots;
        let Ok(cpu) = r.read_bits(16) else {
            return;
        };
        rec.cpu = cpu;
        let Some(info) = read(&mut r) else {
            return;
        };
        rec.info = Some(info);
        let Ok(len) = r.read_bits(16) else {
            return;
        };
        let mut state = Vec::new();
        for _ in 0..len.min(MAX_STATE_BYTES as u32) {
            match r.read_u8() {
                Ok(b) => state.push(b),
                Err(_) => break,
            }
        }
        if let Some(nul) = state.iter().position(|&b| b == 0) {
            state.truncate(nul);
        }
        rec.state = Some(state);
    }
}

/// A request datagram (`sendPacket`): the type, flags 0, `(session << 16) | (key & 0xffff)`, padded as the game's query
/// profile says ([`QueryProfile::request_bytes`]: a server bounds what it sends a source it cannot prove at three times the
/// request, so this engine's clients pad to have the whole answer).
fn request(profile: &QueryProfile, ty: u8, e: &Entry) -> Vec<u8> {
    profile.request(ty, 0, e.session, e.key)
}

/// Takes one received datagram of an info type; `true` when it was an answer of the query's kinds.
pub fn take_datagram<G: ConnGame>(
    client: &Rc<RefCell<ClientState<G>>>,
    from: SocketAddrV4,
    data: &[u8],
    now: u32,
    out: &mut Vec<Out>,
) -> bool {
    match data.first() {
        Some(&GAME_PING_RESPONSE) => {
            client.borrow_mut().query.ping_answer(from, data, now, out);
            true
        }
        Some(&GAME_INFO_RESPONSE) => {
            client.borrow_mut().query.info_answer(from, data, out);
            true
        }
        _ => false,
    }
}

/// Runs what a step asked for, with no borrow held: the lines printed, `onServerQueryStatus` called. The first error stops it.
pub fn run_out(vm: &mut Vm, out: Vec<Out>) -> Result<(), VmError> {
    for o in out {
        match o {
            Out::Print(line) => vm.print(line.as_bytes()),
            Out::Status(status, msg, value) => {
                vm.invoke(
                    None,
                    b"onServerQueryStatus",
                    &[
                        status.as_bytes().to_vec(),
                        msg.into_bytes(),
                        value.into_bytes(),
                    ],
                )?;
            }
            Out::Warn(line) => vm.warn(line),
            Out::Call(name, args) => {
                vm.invoke(None, name, &args)?;
            }
        }
    }
    Ok(())
}

/// The waiting pass, when due at `now` (the host's tick).
pub fn tick<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    now: u32,
) -> Result<(), VmError> {
    let mut out = Vec::new();
    {
        let mut state = client.borrow_mut();
        let Some((session, due)) = state.query.next_pass else {
            return Ok(());
        };
        if now < due {
            return Ok(());
        }
        state.query.next_pass = None;
        let net = Rc::clone(&state.net);
        state
            .query
            .process(session, now, &mut net.borrow_mut(), &mut out);
    }
    run_out(vm, out)
}

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&[][..], Vec::as_slice)
}

/// The favourites (`$pref::Client::ServerFavoriteCount`, `$Pref::Client::ServerFavorite<N>` as `name TAB address`): a negative
/// count is set to 0 and none is pushed; an address that does not parse is skipped (engine-only: TGE pushes its unparsed copy).
fn favorites(vm: &mut Vm) -> Vec<(Vec<u8>, SocketAddrV4)> {
    let count = vm
        .global("$pref::Client::ServerFavoriteCount")
        .map_or(0, |v| if v.is_empty() { 0 } else { atoi(&v) });
    if count < 0 {
        vm.set_global("$pref::Client::ServerFavoriteCount", "0");
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..count.min(MAX_FAVORITES) {
        let Some(v) = vm.global(&format!("$Pref::Client::ServerFavorite{i}")) else {
            continue;
        };
        let cut = v.iter().position(|&b| b == b'\t').unwrap_or(v.len());
        if cut == 0 {
            continue;
        }
        let name = v[..cut.min(FAVORITE_NAME_BYTES)].to_vec();
        let rest = v.get(cut + 1..).unwrap_or_default();
        if let Some(a) = parse_address(rest) {
            out.push((name, a));
        }
    }
    out
}

/// `Net::stringToAddress` for the IP forms: `IP:a.b.c.d:port`, `a.b.c.d:port`, `a.b.c.d` (port 28000),
/// `localhost` and `BROADCAST`; anything else (IPX, a host name) is `None` (the engine resolves no names: LAN only).
pub fn parse_address(text: &[u8]) -> Option<SocketAddrV4> {
    let text = std::str::from_utf8(ts_vm::text::until_nul(text)).ok()?;
    let text = text
        .strip_prefix("IP:")
        .or_else(|| text.strip_prefix("ip:"))
        .unwrap_or(text);
    let (host, port) = match text.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (text, 28000),
    };
    let ip = if host.eq_ignore_ascii_case("broadcast") {
        BROADCAST
    } else if host.eq_ignore_ascii_case("localhost") {
        Ipv4Addr::LOCALHOST
    } else {
        host.parse().ok()?
    };
    Some(SocketAddrV4::new(ip, port))
}

/// Registers the query's natives.
pub fn register<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>) {
    // It broadcasts on the LAN (the master and single queries do nothing here).
    vm.add_native_effects("", "queryLanServers", ts_vm::effects::Effects::NETWORK);
    let state = Rc::clone(client);
    vm.register(
        "queryLanServers",
        12,
        12,
        "queryLanServers(...);",
        move |vm, c| {
            // The console wrapper parses every argument; only the engine reaches the query (the filters are never applied to a
            // LAN answer: the filter type is Offline).
            let port = atoi(arg(c, 1)) as u16;
            let favs = favorites(vm);
            let now = vm.sim_time();
            let mut out = Vec::new();
            {
                let mut s = state.borrow_mut();
                if s.query.table.is_none() {
                    // The game's string coder (read once; without it no answer is read).
                    let game = client_game(vm);
                    match game.string_table(&s.root) {
                        Ok(t) => {
                            s.query.table = Some(t);
                            s.query.versions = game.codecs().query_profile();
                        }
                        Err(e) => out.push(Out::Print(format!("queryLanServers: {e}"))),
                    }
                }
                s.query.start_lan(port, &favs);
                out.push(Out::Status(
                    "start",
                    "Querying LAN servers".to_string(),
                    "0".to_string(),
                ));
                let session = s.query.session;
                let net = Rc::clone(&s.net);
                // `processPingsAndQueries` runs after the `start` callback (TGE's order): it is queued behind it.
                let mut later = Vec::new();
                s.query
                    .process(session, now, &mut net.borrow_mut(), &mut later);
                out.extend(later);
            }
            match run_out(vm, out) {
                Ok(()) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        },
    );
    let state = Rc::clone(client);
    vm.register(
        "cancelServerQuery",
        1,
        1,
        "cancelServerQuery()",
        move |vm, _| {
            let mut out = Vec::new();
            state.borrow_mut().query.cancel(&mut out);
            match run_out(vm, out) {
                Ok(()) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        },
    );
    let state = Rc::clone(client);
    vm.register(
        "stopServerQuery",
        1,
        1,
        "stopServerQuery()",
        move |vm, _| {
            let mut out = Vec::new();
            state.borrow_mut().query.stop(&mut out);
            match run_out(vm, out) {
                Ok(()) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        },
    );
    let state = Rc::clone(client);
    vm.register("getServerCount", 1, 1, "getServerCount();", move |_, _| {
        Native::Int(state.borrow().query.servers.len() as i32)
    });
    let state = Rc::clone(client);
    vm.register(
        "setServerInfo",
        2,
        2,
        "setServerInfo(index);",
        move |vm, c| {
            let index = atoi(arg(c, 1));
            let rec = usize::try_from(index)
                .ok()
                .and_then(|i| state.borrow().query.servers.get(i).cloned());
            let Some(rec) = rec else {
                return Native::Bool(false);
            };
            set_server_info(vm, &rec);
            Native::Bool(true)
        },
    );
    // No master list: both do nothing.
    vm.register(
        "querySingleServer",
        3,
        3,
        "querySingleServer(address, flags);",
        |_, _| Native::Void,
    );
    vm.register(
        "queryMasterServer",
        11,
        11,
        "queryMasterServer(...);",
        |_, _| Native::Void,
    );
    // A game's own list's pings are the game's.
}

/// `setServerInfo`'s variables, in its order.
fn set_server_info(vm: &mut Vm, s: &ServerInfo) {
    let text = |v: &Option<Vec<u8>>| v.clone().unwrap_or_default();
    vm.set_global("$ServerInfo::Status", &(s.status as i32).to_string());
    vm.set_global(
        "$ServerInfo::Address",
        &s.addr.map(address_text).unwrap_or_default(),
    );
    vm.set_global_bytes("$ServerInfo::Name", &text(&s.name));
    vm.set_global_bytes("$ServerInfo::GameType", &text(&s.game_type));
    vm.set_global_bytes("$ServerInfo::MissionName", &text(&s.mission_name));
    vm.set_global_bytes("$ServerInfo::MissionType", &text(&s.mission_type));
    vm.set_global_bytes("$ServerInfo::State", &text(&s.state));
    vm.set_global_bytes("$ServerInfo::Info", &text(&s.info));
    vm.set_global("$ServerInfo::PlayerCount", &s.players.to_string());
    vm.set_global("$ServerInfo::MaxPlayers", &s.max_players.to_string());
    vm.set_global("$ServerInfo::BotCount", &s.bots.to_string());
    vm.set_global("$ServerInfo::Version", &(s.version as i32).to_string());
    vm.set_global("$ServerInfo::Ping", &(s.ping as i32).to_string());
    vm.set_global("$ServerInfo::CPUSpeed", &(s.cpu as i32).to_string());
    vm.set_global("$ServerInfo::Favorite", if s.favorite { "1" } else { "0" });
    vm.set_global(
        "$ServerInfo::Dedicated",
        if s.status & 1 != 0 { "1" } else { "0" },
    );
    vm.set_global(
        "$ServerInfo::Password",
        if s.status & 2 != 0 { "1" } else { "0" },
    );
}
