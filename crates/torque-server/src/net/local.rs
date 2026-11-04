//! The server half of a local connection: `NetConnection::connectLocal`, the client that hosts the game joining its own server
//! in the same VM. The client host makes the client half (its `ServerConnection`) and drives the two calls here in order; the
//! packets between the halves are the protocol's own, carried in memory under the pseudo-address [`LOCAL_ADDR`] (no datagram
//! can come from `0.0.0.0:0`).
//!
//! **The steps** (TGE's `connectLocal`). A second object of the client's class is registered (the next object id) and flagged
//! the local connection, both sequences are 0 and each half is the other's remote connection; the client's connect request is
//! read by the server half (the protocol against the handshake profile's range, `CHR_PROTOCOL_LESS` / `CHR_PROTOCOL_GREATER`,
//! the join password against `$Pref::Server::Password` when that is set, `CHR_PASSWORD`, at most
//! [`NET_MAX_CONNECT_ARGS`](torque_net::cap::NET_MAX_CONNECT_ARGS) connect arguments, `CR_INVALID_ARGS`, then
//! `onConnectRequest(%client, address, args...)`, whose non-empty answer refuses); the accept is written and read (the protocol
//! `min(client, current)` and the ghost-id width); then the client's `onConnectionEstablished(true)`, then the server's
//! `onConnectionEstablished(false)` (`ClientGroup.add`, `onConnect(%client, args...)`), and the server object is named
//! `LocalClientConnection`. A failure deletes both objects (no `Issuing Disconnect packet.`: only a network connection sends
//! one; `onDrop` runs on each) and returns "Unknown Error".
//!
//! **The address.** `onConnectRequest` is given the codecs' local address text
//! ([`NetCodecs::local_address_text`](super::codecs::NetCodecs::local_address_text): by default the text of
//! [`LOCAL_ADDR`], `IP:0.0.0.0:0`). `getAddress` answers "local" while the remote connection lives; once the client's object
//! is deleted the link reads none and it falls back to that text.
//!
//! Credit: the steps follow Torque3D's MIT-licensed `sim/netConnection.cpp` (`Copyright (c) 2012 GarageGames, LLC`) and
//! `T3D/gameBase/gameConnection.cpp` (`Copyright (c) 2012 GarageGames, LLC`, `Copyright (C) 2015 Faust Logic, Inc.`).

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::text::until_nul;
use torque_ts_vm::vm::{Vm, VmError};

use super::NetGame;
use super::conn::{DeleteMode, delete_connection, join_client_group, register_candidate};
use super::pump::{blank_record, global_text};
use super::{ConnRec, NetShared};

/// The pseudo-address the local connection's packets are carried under (in memory: no socket sends to it or receives from it).
pub const LOCAL_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);

/// The most connect arguments `readConnectRequest` reads (more are `CR_INVALID_ARGS`).
pub const MAX_CONNECT_ARGS: usize = torque_net::cap::NET_MAX_CONNECT_ARGS;

/// What the client half's `writeConnectRequest` carries.
#[derive(Debug, Clone, Default)]
pub struct LocalRequest {
    /// `setConnectArgs`'s arguments.
    pub args: Vec<Vec<u8>>,
    /// `setJoinPassword`.
    pub password: Vec<u8>,
    /// The client's protocol version.
    pub protocol: u32,
}

/// The accept the server half wrote: its object, the handshake table's connection, the protocol and the ghost-id width.
#[derive(Debug, Clone, Copy)]
pub struct LocalAccept {
    pub obj: ObjId,
    pub protocol: u32,
    pub ghost_bits: u8,
}

/// Why the server half refused (the script is told "Unknown Error").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalRefusal {
    /// No object id is left, or the class is missing.
    NoObject,
    /// `readConnectRequest` refused: the reason it gives.
    Request(Vec<u8>),
}

/// Whether the record is the server half of a local connection whose client half still exists (the remote-connection link).
pub fn is_live_local<C>(vm: &Vm, rec: &ConnRec<C>) -> bool {
    rec.local_peer
        .is_some_and(|peer| vm.objects().contains(peer))
}

/// `connectLocal`'s server half up to the accept: the object registered (the next id), `readConnectRequest`'s checks and
/// `onConnectRequest` run (inside the caller's script run, as `connectLocal` runs them), the established connection entered
/// in the handshake table. On a refusal the object is deleted (its `onDrop` runs) and the caller deletes the client half.
pub fn request<G: NetGame>(
    vm: &mut Vm,
    shared: &Rc<RefCell<NetShared<G>>>,
    client: ObjId,
    req: &LocalRequest,
) -> Result<LocalAccept, (LocalRefusal, Result<(), VmError>)> {
    let (protocol_min, protocol_current) = {
        let sh = shared.borrow();
        let profile = &sh.hs.config().profile;
        (profile.protocol_min, profile.protocol_current)
    };
    let rec = ConnRec {
        protocol_version: req.protocol.min(protocol_current),
        args: req.args.clone(),
        local_peer: Some(client),
        ..blank_record(LOCAL_ADDR, 0)
    };
    let Some(obj) = register_candidate(vm, shared, rec) else {
        return Err((LocalRefusal::NoObject, Ok(())));
    };
    let refuse = |vm: &mut Vm, reason: &[u8]| {
        let deleted = delete_connection(vm, shared, obj, None, DeleteMode::Immediate);
        Err((LocalRefusal::Request(reason.to_vec()), deleted))
    };
    if req.protocol < protocol_min {
        return refuse(vm, b"CHR_PROTOCOL_LESS");
    }
    if req.protocol > protocol_current {
        return refuse(vm, b"CHR_PROTOCOL_GREATER");
    }
    let password = global_text(vm, "$Pref::Server::Password").unwrap_or_default();
    if !password.is_empty() && until_nul(&req.password) != password.as_slice() {
        return refuse(vm, b"CHR_PASSWORD");
    }
    if req.args.len() > MAX_CONNECT_ARGS {
        return refuse(vm, b"CR_INVALID_ARGS");
    }
    let address = shared.borrow().codecs.local_address_text();
    let mut argv = vec![address.into_bytes()];
    argv.extend(req.args.iter().cloned());
    let answer = vm.call_method(obj, b"onConnectRequest", &argv);
    let reason = match answer {
        Ok(reason) => until_nul(&reason).to_vec(),
        Err(e) => {
            let _ = delete_connection(vm, shared, obj, None, DeleteMode::Immediate);
            return Err((LocalRefusal::Request(Vec::new()), Err(e)));
        }
    };
    if !vm.objects().contains(obj) {
        return Err((LocalRefusal::Request(reason), Ok(())));
    }
    if !reason.is_empty() {
        return refuse(vm, &reason);
    }
    let now = shared.borrow().now();
    let mut sh = shared.borrow_mut();
    let sh = &mut *sh;
    // The handshake's ghost limit as the ghost limit a host set left it (TGE's `GhostIdBitSize` wide until a host sets one): a
    // local connection's ghost ids take its width, as a datagram's request would (no datagram refreshes it on this path).
    super::pump::refresh_config(vm, sh);
    let conn = sh.hs.insert_local(LOCAL_ADDR, req.protocol, now);
    let (protocol, ghost_bits) = sh
        .hs
        .connection(conn)
        .map_or((req.protocol, 0), |c| (c.protocol_version, c.ghost_bits));
    if let Some(rec) = sh.conns.get_mut(&obj) {
        rec.conn = Some(conn);
        rec.protocol_version = protocol;
    }
    sh.by_conn.insert(conn, obj);
    sh.counters.accepted += 1;
    Ok(LocalAccept {
        obj,
        protocol,
        ghost_bits,
    })
}

/// The server half's `onConnectionEstablished(false)`, after the client half's: the payload layer told, `ClientGroup.add`,
/// `onConnect(%client, args...)`, then the name `LocalClientConnection`. Runs inside the caller's script run.
pub fn establish<G: NetGame>(
    vm: &mut Vm,
    shared: &Rc<RefCell<NetShared<G>>>,
    obj: ObjId,
) -> Result<(), VmError> {
    let (conn, args) = {
        let sh = shared.borrow();
        let Some(rec) = sh.conns.get(&obj) else {
            return Ok(());
        };
        (rec.conn, rec.args.clone())
    };
    if let Some(conn) = conn {
        let bits = shared
            .borrow()
            .hs
            .connection(conn)
            .map_or(G::START_GHOST_BITS, |c| c.ghost_bits);
        shared.borrow_mut().payload.on_accept(conn, obj, bits);
        // Both halves are the engine's: the ghost layer scopes new objects as they come (`GhostManager::set_fresh`).
        let hub = shared.borrow().ghosts.clone();
        if let Some(hub) = hub {
            hub.set_fresh(conn, true);
            hub.set_engine_peer(conn);
        }
    }
    join_client_group(vm, obj);
    let result = vm.call_method(obj, b"onConnect", &args).map(|_| ());
    if vm.objects().contains(obj) {
        vm.objects_mut().set_name(obj, b"LocalClientConnection");
    }
    result
}
