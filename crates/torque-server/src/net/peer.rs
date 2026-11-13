//! What a connection's peer is, as far as the features only the engine's own clients get are concerned: the engine's own ghost
//! classes (and the plugins'), and the plugins' datablocks. [`PeerKind::of`] is the one place that decides it, so the ghost layer
//! and the event layer cannot disagree.
//!
//! In Torque's protocol a peer is a [`PeerKind::Tge`] client (Torque's classes and datablocks only: a real TGE client cannot read
//! the others' records), or an [`PeerKind::EnginePeer`] when the host marked it as the engine's own (the local connection, a client
//! granted the handshake's extension) or the server says every client is ([`ENGINE_CLASSES_GLOBAL`](super::tge_compat::ENGINE_CLASSES_GLOBAL)). On a server that speaks the
//! engine's own protocol every peer is the engine's: [`PeerKind::Engine`]. What marks a peer of Torque's protocol as the
//! engine's lives in [`super::tge_compat`]; nothing in it is read in the engine's protocol.

use torque_engine::ProtocolKind;
use torque_ts_vm::vm::Vm;

use super::tge_compat::engine_classes_global;

/// The protocol a server speaks, as an extension of its script engine. A server that has none speaks Torque's
/// ([`ProtocolKind::Tge`]); the host sets it from the engine's protocol slot when it makes the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolMode(pub ProtocolKind);

impl ProtocolMode {
    /// The protocol the server of `vm` speaks.
    pub fn of(vm: &Vm) -> ProtocolKind {
        vm.extension::<ProtocolMode>()
            .map_or(ProtocolKind::Tge, |mode| mode.0)
    }
}

/// What a connection's peer is (module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerKind {
    /// A client of Torque's own: it is sent Torque's classes and datablocks and nothing else.
    Tge,
    /// A client of the engine's own over Torque's protocol: it is marked as one, or the server says all its clients are.
    EnginePeer,
    /// Any peer of a server that speaks the engine's protocol.
    Engine,
}

impl PeerKind {
    /// The kind of the peer of a connection on the server of `vm`, where `marked` says whether the host marked the connection as
    /// the engine's own (`GhostHub::set_engine_peer`).
    pub fn of(vm: &Vm, marked: bool) -> PeerKind {
        if ProtocolMode::of(vm) == ProtocolKind::Engine {
            PeerKind::Engine
        } else if marked || engine_classes_global(vm) {
            PeerKind::EnginePeer
        } else {
            PeerKind::Tge
        }
    }

    /// Whether the peer is sent the engine's own ghost classes and the plugins' classes and datablocks.
    pub fn sees_engine_classes(self) -> bool {
        self != PeerKind::Tge
    }
}

#[cfg(test)]
#[path = "peer_tests.rs"]
mod tests;
