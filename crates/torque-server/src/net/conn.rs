//! The connections of the network host: `GameConnection` objects, the natives scripts call on them, and the one way out.
//!
//! **Natives.** On `NetConnection`: `getAddress` (the codecs' address text: TGE's `IP:a.b.c.d:port` by default), `getPing`
//! (the round trip truncated), `getPacketLoss` (`(int)(loss * 100.0)`; nothing measures a live connection's loss: 0). On
//! `GameConnection`: `isAIControlled`, `delete` and `transmitDataBlocks` (the chain of [`super::datablocks`]; the connection
//! stays alive). Called on an object the host did not make (a script's `new GameConnection()`), each answers the neutral
//! value of its kind and `delete` just deletes. The function `updateDataBlocks()` ([`update_datablocks`]) starts a live round
//! of the chain on every connection past its mission's datablocks. A game registers its own natives on the connections with [`register`] and
//! posts its events with [`post_events`].
//!
//! **`delete(reason)`.** A reason the game keeps the connection for ([`NetGame::keeps_connection`]) does nothing at all. Any
//! other is copied ([`NET_DROP_REASON_BYTES`]) and the object is deleted with `deleteObject`: `onRemove` runs inside the call,
//! the object is destroyed 1 ms of Sim time later. **While the connection's own packet is being worked through** (its commands
//! run as followups), the delete waits for the end of the packet: the later commands of that packet run with a live `%client`,
//! then the drop happens ([`ConnRec::delete_pending`]). With no reason the connection keeps the one it had (empty).
//!
//! **`onRemove`**, as the `GameConnection` class behaviour: (1) `Issuing Disconnect packet.` and the Disconnect datagram (the
//! client's connect sequence, the reason as a plain string); (2) the script's `onDrop(%client, %reason)`, the client still in
//! `ClientGroup`; (3) the host's bookkeeping (the handshake table, the payload layer). The reason is never null: it is the
//! script's, `Packet Error.`, `TimedOut`, the peer's own text, or empty. A drop with [`ConnRec::silent`] does only (3).
//!
//! Credit: the natives and the drop follow TGE as Torque3D, its MIT-licensed release, keeps them: `sim/netConnection.cpp`
//! (`Copyright (c) 2012 GarageGames, LLC`) and `T3D/gameBase/gameConnection.cpp` (`Copyright (c) 2012 GarageGames, LLC`,
//! `Copyright (C) 2015 Faust Logic, Inc.`).

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use torque_net::handshake::ConnId;
use torque_net::remote::SendError;
use torque_ts_vm::classes::EngineClass;
use torque_ts_vm::sim::{ObjId, SimObject};
use torque_ts_vm::text::{atoi, until_nul};
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

use super::NetGame;
use super::datablocks::NET_DATABLOCK_SCAN_PER_UNIT;
use super::events::{NET_QUEUE_FULL_REASON, queue_full_line};
use super::hostevent::{GameEvents, HostEvent};
use super::{ConnRec, NET_DROP_REASON_BYTES, NetShared};
use torque_game::methods::register_method;

/// How a connection's object is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// `deleteObject` (the script's `delete`): `onRemove` now, the destruction 1 ms of Sim time later.
    Scheduled,
    /// A direct delete (a peer's disconnect, a timeout, a packet error, a replaced or rejected connection).
    Immediate,
    /// Immediate, and the drop sends no datagram and runs no script.
    Silent,
}

/// Registers the `GameConnection` candidate for a request that passed the handshake's checks: a normal object id
/// (`registerObject`, which runs before `readConnectRequest`), not placed in any group. None when no id is left.
pub fn register_candidate<G: NetGame>(
    vm: &mut Vm,
    shared: &Rc<RefCell<NetShared<G>>>,
    rec: ConnRec<G::Conn>,
) -> Option<ObjId> {
    let class = vm.objects().classes.find(b"GameConnection")?;
    let object = SimObject::new(class, &vm.objects().classes);
    let registered = vm.objects_mut().register(object)?;
    if registered.wrapped {
        vm.warn("Wrapping SimId numbers...".to_string());
    }
    let obj = registered.id;
    shared
        .borrow_mut()
        .conns
        .insert(obj, ConnRec { obj, ..rec });
    Some(obj)
}

/// Adds the connection to `ClientGroup` (`onConnectionEstablished`).
pub fn join_client_group(vm: &mut Vm, obj: ObjId) {
    if let Some(group) = vm.objects().find(b"ClientGroup") {
        vm.objects_mut().add(group, obj);
    }
}

/// Deletes the connection's object. `reason` (cut at [`NET_DROP_REASON_BYTES`]) becomes what the drop gives the peer and
/// `onDrop`; None keeps the reason the connection has. The script callbacks run inside; a limit they hit is returned.
pub fn delete_connection<G: NetGame>(
    vm: &mut Vm,
    shared: &Rc<RefCell<NetShared<G>>>,
    obj: ObjId,
    reason: Option<&[u8]>,
    mode: DeleteMode,
) -> Result<(), VmError> {
    {
        let mut sh = shared.borrow_mut();
        let in_packet = sh.packet_obj == Some(obj);
        if let Some(rec) = sh.conns.get_mut(&obj) {
            if let Some(reason) = reason {
                let reason = until_nul(reason);
                rec.reason = reason
                    .get(..NET_DROP_REASON_BYTES)
                    .unwrap_or(reason)
                    .to_vec();
            }
            rec.silent |= mode == DeleteMode::Silent;
            // A script's `delete()` while the connection's own packet is still being worked through waits for the end of the
            // packet (the host drops it then): the later commands of that packet run with a live `%client`.
            if mode == DeleteMode::Scheduled && in_packet {
                rec.delete_pending = true;
                return Ok(());
            }
        }
    }
    match mode {
        DeleteMode::Scheduled => vm.delete_object(obj),
        DeleteMode::Immediate | DeleteMode::Silent => vm.delete_object_quiet(obj),
    }
}

/// The `GameConnection` class behaviour: the drop.
struct GameConnectionClass<G: NetGame> {
    shared: Rc<RefCell<NetShared<G>>>,
}

impl<G: NetGame> EngineClass for GameConnectionClass<G> {
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        let plan = {
            let mut sh = self.shared.borrow_mut();
            let table = sh.string_table();
            let Some(rec) = sh.conns.get_mut(&id) else {
                return Ok(());
            };
            if std::mem::replace(&mut rec.dropping, true) {
                return Ok(());
            }
            (
                rec.addr,
                rec.sequence,
                rec.reason.clone(),
                rec.silent,
                table,
                rec.local_peer.is_some(),
                rec.conn,
            )
        };
        let (addr, sequence, reason, silent, table, local, conn) = plan;
        let engine = self.shared.borrow().engine_host.is_some();
        // Only a network connection sends the Disconnect packet; a local one never has it. In engine mode it is the session's
        // goodbye, and a connection never accepted has no session of its own to say it on (its reject is the session's).
        if !silent && !local && (conn.is_some() || !engine) {
            vm.print(b"Issuing Disconnect packet.");
        }
        if engine {
            if conn.is_some() {
                super::engine::goodbye(&self.shared, addr, &reason, !silent && !local);
                if let (Some(conn), Some(lanes)) = (conn, vm.extension::<super::rpc::RpcLanes>()) {
                    lanes.close(Some(conn));
                }
            }
        } else if !silent && !local {
            let types = self.shared.borrow().codecs.handshake_types();
            let datagram = types.disconnect(&table, sequence, &reason);
            let transport = Rc::clone(&self.shared.borrow().transport);
            transport.borrow_mut().send_to(addr, &datagram);
        }
        let result = if silent {
            Ok(Vec::new())
        } else {
            vm.call_method(id, b"onDrop", std::slice::from_ref(&reason))
        };
        let mut ended = None;
        {
            let mut sh = self.shared.borrow_mut();
            if let Some(rec) = sh.conns.remove(&id)
                && let Some(conn) = rec.conn
            {
                ended = Some(conn);
                sh.by_conn.remove(&conn);
                sh.hs.remove(conn);
                sh.payload.on_drop(conn, vm);
            }
            sh.counters.drops += 1;
        }
        // What a connection held under its own key goes with it (an identity's is the provider's).
        if let (Some(conn), Some(perms)) = (
            ended,
            vm.extension::<torque_engine::permissions::Permissions>(),
        ) {
            perms.forget_connection(conn.0);
        }
        result.map(|_| ())
    }
}

/// What a native needs to know about the connection it was called on.
pub struct ConnView {
    pub addr: SocketAddr,
    /// The server half of a local connection whose client half lives (the remote-connection link).
    pub local: bool,
    /// The server half of a local connection (live or not): its address prints as the codecs' local address text.
    pub local_kind: bool,
    pub protocol_version: u32,
    pub ping: i32,
    pub packet_loss: f32,
}

pub fn conn_view<G: NetGame>(
    vm: &Vm,
    shared: &Rc<RefCell<NetShared<G>>>,
    c: &NativeCall,
) -> Option<ConnView> {
    let obj = c.this_object?;
    let sh = shared.borrow();
    let rec = sh.conns.get(&obj)?;
    let local = super::local::is_live_local(vm, rec);
    let local_kind = rec.local_peer.is_some();
    let ping = rec
        .conn
        .and_then(|conn| sh.hs.connection(conn))
        .map_or(0, |conn| conn.protocol.round_trip() as i32);
    Some(ConnView {
        addr: rec.addr,
        local,
        local_kind,
        protocol_version: rec.protocol_version,
        ping,
        packet_loss: rec.packet_loss,
    })
}

/// Makes `f` the connection method `class::method` with the host's state. A class table without the method (its specs do not
/// name it) is left as it is: the method is not registered. Returns whether it was.
pub fn register<G: NetGame>(
    vm: &mut Vm,
    class: &str,
    method: &str,
    shared: &Rc<RefCell<NetShared<G>>>,
    f: impl Fn(&mut Vm, &NativeCall, &Rc<RefCell<NetShared<G>>>) -> Native + 'static,
) -> bool {
    let shared = Rc::clone(shared);
    register_method(vm, class, method, move |vm, c| f(vm, c, &shared))
}

/// Posts `events` on the connection of `obj` (all or none, in order). A target the host did not make is silent; a full ordered
/// queue drops the connection like the command natives do (`Overflow.`).
pub fn post_events<G: NetGame>(
    vm: &mut Vm,
    shared: &Rc<RefCell<NetShared<G>>>,
    obj: ObjId,
    events: Vec<HostEvent<<G::Events as GameEvents>::Event>>,
) -> Native {
    let (conn, layer) = {
        let sh = shared.borrow();
        (
            sh.conns.get(&obj).and_then(|rec| rec.conn),
            sh.events.clone(),
        )
    };
    let Some(conn) = conn else {
        return Native::Void;
    };
    match layer.post_events(conn, events) {
        Err(SendError::Backpressure) => {
            vm.print(queue_full_line(obj).as_bytes());
            match delete_connection(
                vm,
                shared,
                obj,
                Some(NET_QUEUE_FULL_REASON),
                DeleteMode::Scheduled,
            ) {
                Ok(()) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        }
        _ => Native::Void,
    }
}

/// Makes the `GameConnection` behaviour and replaces the connection natives' stubs.
pub fn install<G: NetGame>(vm: &mut Vm, shared: &Rc<RefCell<NetShared<G>>>) {
    super::permissions::install(vm, shared);
    vm.set_class_behaviour(
        "GameConnection",
        Rc::new(GameConnectionClass {
            shared: Rc::clone(shared),
        }),
    );
    // A local connection's server half ([`super::local`]) answers "local" while its client half lives.
    register(vm, "NetConnection", "getAddress", shared, |vm, c, s| {
        let codecs = Rc::clone(&s.borrow().codecs);
        Native::Str(conn_view(vm, s, c).map_or_else(Vec::new, |v| {
            if v.local {
                b"local".to_vec()
            } else if v.local_kind {
                codecs.local_address_text().into_bytes()
            } else {
                codecs.address_text(v.addr).into_bytes()
            }
        }))
    });
    register(vm, "NetConnection", "getPing", shared, |vm, c, s| {
        Native::Int(conn_view(vm, s, c).map_or(0, |v| v.ping))
    });
    register(vm, "NetConnection", "getPacketLoss", shared, |vm, c, s| {
        Native::Int(conn_view(vm, s, c).map_or(0, |v| (v.packet_loss * 100.0) as i32))
    });
    register(vm, "GameConnection", "isAIControlled", shared, |_, _, _| {
        Native::Bool(false)
    });
    register(vm, "GameConnection", "delete", shared, |vm, c, s| {
        let Some(obj) = c.this_object else {
            return Native::Void;
        };
        let reason = c.argv.get(2).map(|r| until_nul(r));
        // A reason the game keeps the connection for does nothing at all ([`NetGame::keeps_connection`]).
        if reason.is_some_and(|r| s.borrow().game.keeps_connection(r)) {
            return Native::Void;
        }
        match delete_connection(vm, s, obj, reason, DeleteMode::Scheduled) {
            Ok(()) => Native::Void,
            Err(e) => Native::Fail(e),
        }
    });
    register(
        vm,
        "GameConnection",
        "transmitDataBlocks",
        shared,
        |vm, c, s| {
            // The sequence is `atoi` of the argument (NULL: 0); the chain is [`super::datablocks`].
            let Some(obj) = c.this_object else {
                return Native::Void;
            };
            let seq = c.argv.get(2).map_or(0, |t| atoi(until_nul(t)));
            let (conn, layer) = {
                let sh = s.borrow();
                (
                    sh.conns.get(&obj).and_then(|rec| rec.conn),
                    sh.events.clone(),
                )
            };
            let Some(conn) = conn else {
                return Native::Void;
            };
            match layer.transmit_datablocks(vm, conn, seq) {
                Ok((_, scanned)) => {
                    let units = 1u64.saturating_add((scanned / NET_DATABLOCK_SCAN_PER_UNIT) as u64);
                    if vm.charge_work(units) {
                        Native::Void
                    } else {
                        Native::Fail(VmError::StepLimit)
                    }
                }
                Err(SendError::Backpressure) => {
                    vm.print(queue_full_line(obj).as_bytes());
                    match delete_connection(
                        vm,
                        s,
                        obj,
                        Some(NET_QUEUE_FULL_REASON),
                        DeleteMode::Scheduled,
                    ) {
                        Ok(()) => Native::Void,
                        Err(e) => Native::Fail(e),
                    }
                }
                Err(_) => Native::Void,
            }
        },
    );
    let s = Rc::clone(shared);
    vm.register(
        "updateDataBlocks",
        1,
        1,
        "updateDataBlocks() - sends every client past its mission's datablocks the datablocks added or changed since it last got them, one round per client (a client still being sent its datablocks gets the round after them; each client's GameConnection::onDataBlocksUpdated(%client, %count) runs when its round is in); returns the number of clients reached",
        move |vm, _| match update_datablocks(vm, &s) {
            Ok(reached) => Native::Int(i32::try_from(reached).unwrap_or(i32::MAX)),
            Err(e) => Native::Fail(e),
        },
    );
}

/// `updateDataBlocks()`: a live round on every connection past its mission chain ([`EventLayer::update_datablocks`]). Returns
/// the connections reached; the group positions looked at are charged as `transmitDataBlocks` charges them.
///
/// # Errors
/// The script's work budget is spent ([`VmError::StepLimit`]).
///
/// [`EventLayer::update_datablocks`]: super::events::EventLayer::update_datablocks
pub fn update_datablocks<G: NetGame>(
    vm: &mut Vm,
    shared: &Rc<RefCell<NetShared<G>>>,
) -> Result<usize, VmError> {
    let layer = shared.borrow().events.clone();
    let (reached, scanned) = layer.update_datablocks(vm);
    let units = 1u64.saturating_add((scanned / NET_DATABLOCK_SCAN_PER_UNIT) as u64);
    if vm.charge_work(units) {
        Ok(reached)
    } else {
        Err(VmError::StepLimit)
    }
}

/// The connection of an object, if the host made it.
pub fn connection_of<G: NetGame>(shared: &NetShared<G>, obj: ObjId) -> Option<ConnId> {
    shared.conns.get(&obj).and_then(|rec| rec.conn)
}
