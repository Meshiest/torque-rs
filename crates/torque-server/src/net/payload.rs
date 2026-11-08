//! The seam between the packet layer and what rides in the packets (the packet layer, then the event layer, and the rest).
//!
//! A [`PayloadLayer`] reads the payload of every new data packet a connection receives and writes the payload of every data
//! packet the server sends. The event layer, the NetString table, remote commands and the ghost manager all live behind it.
//!
//! **No script runs inside the layer, and no `RefCell` is borrowed across script.** The layer is called with the host's state
//! borrowed, so it must not run a script (it is given the `Vm` through [`ConnCtx::vm`] for the tag table and nothing else: a
//! script run from there would call a native that borrows the state again). What a packet asks for ("run `serverCmdX` for this
//! client", "drop this client") comes back as [`Followup`]s, which the host runs as outermost runs, one by one, after it has
//! let go of the state. Each followup draws on the network script budget, and a limit in one ends the rest of that packet's
//! work, not the server.

use std::rc::Rc;
use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};

use torque_net::handshake::ConnId;
use torque_net::huffman::Table;
use torque_net::protocol::{Notify, Protocol, Received};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

/// What the layer is given about one connection.
pub struct ConnCtx<'a> {
    pub conn: ConnId,
    /// The connection's `GameConnection` object (the script's `%client`).
    pub obj: Option<ObjId>,
    pub protocol: &'a mut Protocol,
    /// The simulation clock, in milliseconds.
    pub now: u32,
    /// The VM, for its tag table (`addTaggedString`, `removeString`) and nothing that runs script.
    pub vm: &'a mut Vm,
    /// The width of this connection's ghost ids (the accept's; `ConnectionMessage` counts are one bit wider).
    pub ghost_bits: u8,
    /// The string coder's code tree.
    pub table: Rc<Table>,
    /// Pays for work from the datagram's reservation: told how many units are about to be spent, it answers whether the
    /// reservation can pay. A `false` means the reservation was too small for what the datagram carried, which the bound should
    /// make unreachable; the layer then ends the read with a protocol error. [`write`](PayloadLayer::write) is given a callback
    /// that pays for everything (the send pass is charged by the packets it emits).
    pub charge: &'a mut dyn FnMut(u64) -> bool,
}

/// Work a packet asks the host to do once the state is released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Followup {
    /// Call the function `name` (a method of `object` when it is set, a global function otherwise) with `args`. An unknown
    /// function is silent.
    Call {
        object: Option<ObjId>,
        name: Vec<u8>,
        args: Vec<Vec<u8>>,
    },
    /// A client's remote command (TGE's `RemoteCommandEvent::process`): the global function `name` (`serverCmdX`) is called
    /// with the connection's object id as its first argument, then `args`. **The host may gate it when it runs**, per followup
    /// (an earlier command of the same packet may change what the gate answers); a command that fails the gate is dropped
    /// silently.
    ServerCmd {
        obj: ObjId,
        name: Vec<u8>,
        args: Vec<Vec<u8>>,
    },
    /// Print a console line (the `Mapping string:` line of a NetString event, in the order it ran among the commands).
    Print(Vec<u8>),
    /// Drop the connection of `obj` with this reason (`Packet Error.` for a protocol error).
    Drop { obj: ObjId, reason: Vec<u8> },
}

/// What reading one packet's payload came to: the followups decoded before an error, and the error. Each unordered event is
/// processed as it is read, so the work before a bad one has happened; the host runs `followups` and then disconnects with
/// `Packet Error.`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PacketOutcome {
    pub followups: Vec<Followup>,
    pub error: Option<NetError>,
}

impl PacketOutcome {
    /// Nothing to do.
    pub fn nothing() -> PacketOutcome {
        PacketOutcome::default()
    }
}

/// The payload of a connection's data packets.
pub trait PayloadLayer {
    /// A new data packet arrived: `payload` is positioned at the first bit after the rate prefix. An `error` in the outcome is
    /// a protocol error: the host disconnects the peer with `Packet Error.` after the followups.
    fn on_game_packet(
        &mut self,
        ctx: &mut ConnCtx<'_>,
        received: &Received,
        payload: &mut BitReader<'_>,
    ) -> PacketOutcome;

    /// One of our packets was acked (`delivered`) or lost.
    fn on_notify(&mut self, ctx: &mut ConnCtx<'_>, notify: Notify);

    /// A connection was accepted and joined `ClientGroup` (`obj` is its `GameConnection`, `ghost_bits` the ghost-id width its
    /// accept carried); called before the script's `onConnect`, so that what `onConnect` sends is queued ahead of the accept.
    fn on_accept(&mut self, _conn: ConnId, _obj: ObjId, _ghost_bits: u8) {}

    /// The host marks the connection accepted just now as one its layer treats apart (a peer of its own kind, by the host's
    /// handshake).
    fn mark_peer(&mut self, _conn: ConnId) {}

    /// The host marks the connection accepted just now as one whose client sent the bulk channels' names in its manifest, which
    /// the server's gate then matched against its own: the client reads the channels' section, so a layer that writes one may
    /// turn it on. Called after [`on_accept`](PayloadLayer::on_accept), before the first data packet.
    fn mark_channels(&mut self, _conn: ConnId) {}

    /// Appends the payload to a data packet whose header and rate prefix are written. An error is a protocol error of the
    /// connection (an event no packet can carry): the packet is not sent and the host drops the peer with `Packet Error.`.
    fn write(&mut self, ctx: &mut ConnCtx<'_>, w: &mut BitWriter) -> Result<(), NetError>;

    /// Whether the send pass may burst packets to the connection (the host's rule: back to back, up to a count of unacked ones,
    /// while the round trip is short). **True until the layer says otherwise**: a layer puts a connection in quiet mode, a
    /// ghost writer while it has nothing to send.
    fn busy(&self, _conn: ConnId) -> bool {
        true
    }

    /// The script calls the layer wants made for `conn` since the last call (a script call of the ghost layer's, such as a
    /// progress callback), which the host makes after the packet write, with the state released. Taking resets it.
    fn take_calls(&mut self, _conn: ConnId) -> Vec<Followup> {
        Vec::new()
    }

    /// The work units the layer spent on `conn` since the last call (the datablock renders of the packets it wrote), which the
    /// send pass adds to the tick's work like the bytes it emits. Taking resets it.
    fn take_work(&mut self, _conn: ConnId) -> u32 {
        0
    }

    /// The connection is gone. `vm` is for releasing what the layer holds in the VM (the NetString tag references the
    /// receiver's table keeps); it must not run script.
    fn on_drop(&mut self, _conn: ConnId, _vm: &mut Vm) {}
}

/// A layer that reads nothing and writes nothing. A connection under it is kept alive by the packet layer alone (data packets
/// with a header and a rate prefix, acks for pings, the timeouts).
#[derive(Debug, Default, Clone, Copy)]
pub struct Quiet;

impl PayloadLayer for Quiet {
    fn on_game_packet(
        &mut self,
        _ctx: &mut ConnCtx<'_>,
        _received: &Received,
        _payload: &mut BitReader<'_>,
    ) -> PacketOutcome {
        PacketOutcome::nothing()
    }

    fn on_notify(&mut self, _ctx: &mut ConnCtx<'_>, _notify: Notify) {}

    fn write(&mut self, _ctx: &mut ConnCtx<'_>, _w: &mut BitWriter) -> Result<(), NetError> {
        Ok(())
    }
}
