//! The server query's info packets on the host (codecs in `torque_net::serverquery`): the answers that put a server in a
//! client's "Query LAN" list and its single-server queries (TGE's `handleInfoPacket`, `handleGamePingRequest`,
//! `handleGameInfoRequest`), and a game's own request types (the codecs' extra queries).
//!
//! **What is read, and when.** For each request, at the time it is handled: `allowConnections`, then for a ping or a game's
//! extra query `$Server::ServerType` (no answer for `SinglePlayer`, ignoring case), then the flags' offline bit. The ping
//! answer names `$Pref::Server::Name`. The info answer reads `$Server::GameType`, `$Server::MissionType`,
//! `$Server::MissionName`, `$Server::Dedicated` (`getBoolVariable`: empty is false, else `dAtob`), `$Pref::Server::Password`
//! (non-empty sets the bit), `$Server::PlayerCount`, `$Pref::Server::MaxPlayers`, `$Server::BotCount` (each `atoi` truncated to
//! a byte, empty 0) and `$Pref::Server::Info`, and only then runs `onServerInfoQuery();` the way `eval` does (the text is
//! compiled for every request) and sends what it returned as the long string.
//!
//! **Limits.** The CPU field is the codecs' unknown speed
//! ([`NetCodecs::unknown_cpu_mhz`](super::codecs::NetCodecs::unknown_cpu_mhz)). A request shorter than 6 bytes is silence.
//! Every answer passes [`torque_net::serverquery::InfoLimiter`] (per source address and network, and for every source, a connected
//! player's address included, the bound on what is reflected: an answer more than three times its request's size goes only
//! within a small budget in all, counted in [`Counters::info_limited`](super::Counters::info_limited) past it). The script's output is kept
//! for a second ([`InfoCache`](torque_net::serverquery::InfoCache)) and the answers in that time are made from it with no script run, so spoofed sources can
//! neither spend the script budget nor hide the server from browsers; when it does run,
//! `onServerInfoQuery()` runs as a stranger's script does: an outermost run under the network's step limits
//! ([`NET_SCRIPT_RUN_STEPS`](super::NET_SCRIPT_RUN_STEPS), the window's budget); with no budget left, or when the run stops at
//! a limit, no answer is sent (counted in [`Counters::info_dropped`](super::Counters::info_dropped)). The datagram's work is
//! charged by the receive pass like any stranger's; the answer's bytes and the script's run are charged on top.
//!
//! Credit: the answers follow Torque3D's MIT-licensed `app/net/serverQuery.cpp` (`Copyright (c) 2012 GarageGames, LLC`).

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use torque_net::cap::NET_MAX_DATAGRAM;
use torque_net::serverquery::{
    self, InfoAsk, InfoFields, Kind, PING_REPLY_MAX_BYTES, Parsed, Request, STATUS_DEDICATED,
    STATUS_PASSWORDED,
};

use super::codecs::net_codecs;
use torque_ts_vm::fields::atob;
use torque_ts_vm::text::atoi;
use torque_ts_vm::vm::Vm;

use super::NetGame;
use super::NetShared;
use super::pump::{cost, global_text, run, send};

type Shared<G> = Rc<RefCell<NetShared<G>>>;

/// The work units an info answer costs beyond its bytes when it runs the script (as a connect request's).
const SCRIPT_WORK: u32 = 8;

/// The text compiled and run for the info answer's last string.
pub const INFO_QUERY_CALL: &[u8] = b"onServerInfoQuery();";

fn text(vm: &Vm, name: &str) -> Vec<u8> {
    global_text(vm, name).unwrap_or_default()
}

/// `(U8)atoi(value)`, 0 for an empty value.
fn byte_of(vm: &Vm, name: &str) -> u8 {
    let t = text(vm, name);
    if t.is_empty() { 0 } else { atoi(&t) as u8 }
}

/// `Con::getBoolVariable(name, false)`.
fn bool_of(vm: &Vm, name: &str) -> bool {
    let t = text(vm, name);
    !t.is_empty() && atob(&t)
}

/// The info answer's fields but the script's string, read from the variables in the order the answer writes them.
pub fn info_fields(vm: &Vm) -> InfoFields {
    let mut status = 0;
    if bool_of(vm, "$Server::Dedicated") {
        status |= STATUS_DEDICATED;
    }
    if !text(vm, "$Pref::Server::Password").is_empty() {
        status |= STATUS_PASSWORDED;
    }
    InfoFields {
        game_type: text(vm, "$Server::GameType"),
        mission_type: text(vm, "$Server::MissionType"),
        mission_name: text(vm, "$Server::MissionName"),
        status,
        players: byte_of(vm, "$Server::PlayerCount"),
        max_players: byte_of(vm, "$Pref::Server::MaxPlayers"),
        bots: byte_of(vm, "$Server::BotCount"),
        cpu_mhz: net_codecs(vm).unknown_cpu_mhz(),
        info: text(vm, "$Pref::Server::Info"),
        content: Vec::new(),
    }
}

/// One info-type datagram (the caller checked its type). Returns the work it cost beyond the datagram's own.
pub fn handle<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    from: SocketAddr,
    data: &[u8],
    now: u32,
) -> u32 {
    let codecs = Rc::clone(&shared.borrow().codecs);
    let request = match serverquery::parse_request(data) {
        Parsed::Request(r) => Query::Torque(r),
        Parsed::Other { ty, flags, key } if codecs.extra_query(ty) => {
            Query::Extra { ty, flags, key }
        }
        Parsed::Other { .. } | Parsed::Short => {
            shared.borrow_mut().counters.ignored += 1;
            return 0;
        }
    };
    let allow = shared.borrow().engine.borrow().allow_connections;
    let single_player = text(vm, "$Server::ServerType").eq_ignore_ascii_case(b"SinglePlayer");
    let answers = match &request {
        Query::Torque(r) => serverquery::answers(r, allow, single_player),
        Query::Extra { ty, flags, .. } => {
            codecs.answers_extra_query(*ty, *flags, allow, single_player)
        }
    };
    if !answers {
        shared.borrow_mut().counters.ignored += 1;
        return 0;
    }
    let ask = InfoAsk {
        ip: super::pump::peer_ip(shared, from),
        request_len: data.len(),
        max_reply: match &request {
            Query::Torque(r) if r.kind == Kind::Ping => PING_REPLY_MAX_BYTES,
            _ => NET_MAX_DATAGRAM,
        },
    };
    let limited = shared.borrow_mut().info.allow(&ask, now).is_err();
    if limited {
        shared.borrow_mut().counters.info_limited += 1;
        return 0;
    }
    let Some((reply, script)) = build(vm, shared, &request, now) else {
        shared.borrow_mut().counters.info_dropped += 1;
        return SCRIPT_WORK;
    };
    let reflects = shared
        .borrow_mut()
        .info
        .allow_reply(&ask, reply.len(), now)
        .is_err();
    if reflects {
        shared.borrow_mut().counters.info_limited += 1;
    } else if send(shared, from, &reply) {
        shared.borrow_mut().counters.info_replies += 1;
    }
    let extra = cost(reply.len());
    if script {
        extra.saturating_add(SCRIPT_WORK)
    } else {
        extra
    }
}

/// A request the server answers: one of torque-net's, or one of the game's own types (the codecs' extra queries).
enum Query {
    Torque(Request),
    Extra { ty: u8, flags: u8, key: u32 },
}

/// The answer to `request` at `now` and whether a script ran for it; None when the script could not run or stopped at a
/// limit. The info answer's script output is taken from the cache while it is fresh ([`InfoCache`](torque_net::serverquery::InfoCache)): the script runs at most
/// once a [`NET_INFO_CACHE_MS`](torque_net::serverquery::NET_INFO_CACHE_MS), however many ask.
fn build<G: NetGame>(
    vm: &mut Vm,
    shared: &Shared<G>,
    request: &Query,
    now: u32,
) -> Option<(Vec<u8>, bool)> {
    let codecs = Rc::clone(&shared.borrow().codecs);
    let table = shared.borrow().string_table();
    let request = match request {
        Query::Torque(r) => r,
        Query::Extra { ty, flags, key } => {
            return Some((codecs.extra_query_response(*ty, *flags, *key), false));
        }
    };
    match request.kind {
        Kind::Ping => {
            let name = text(vm, "$Pref::Server::Name");
            Some((codecs.ping_response(&table, request, &name), false))
        }
        Kind::Info => {
            let mut fields = info_fields(vm);
            let cached = shared.borrow().info_cache.fresh(now).map(<[u8]>::to_vec);
            let ran = cached.is_none();
            fields.content = match cached {
                Some(content) => content,
                None => {
                    if !shared.borrow_mut().script_available() {
                        return None;
                    }
                    let out = run(vm, shared, true, |vm| vm.eval(INFO_QUERY_CALL))?.ok()?;
                    let content = torque_ts_vm::text::until_nul(&out).to_vec();
                    shared.borrow_mut().info_cache.store(now, content.clone());
                    content
                }
            };
            Some((serverquery::info_response(&table, request, &fields), ran))
        }
    }
}
