//! What only a server of Torque's own protocol does to tell its peers apart. A real Torque client cannot read the engine's own
//! ghost classes, the plugins' classes and datablocks, or the bulk channels' section, so on Torque's protocol the server sends
//! them only to a peer it knows is the engine's: the local connection, a client that said so in its connect request (the
//! handshake's extension, [`torque_net::tge`]), or any client when the game says all of them are
//! ([`ENGINE_CLASSES_GLOBAL`]). The engine's own protocol has no such peers to tell apart: every peer is the engine's
//! ([`PeerKind::Engine`]) and nothing here is read.
//!
//! [`PeerKind::of`] is the one query the ghost and event layers ask. This module holds what answers it for Torque's protocol, the
//! marking of an accepted connection (`mark_accepted`).

use torque_ts_vm::text::atof;
use torque_ts_vm::vm::Vm;

use super::NetGame;
use super::NetShared;
use super::ghosts::{GhostGame, GhostHub};
#[cfg(doc)]
use super::peer::PeerKind;
use torque_net::handshake::{ConnId, PendingRequest};

/// The global that makes a server ghost the engine's own classes (`torque_ghosts::light`'s, and the plugins') to every
/// client, not only to the engine's own (the local connection, a client granted the handshake's extension): set it to 1 when every client is the
/// engine's (a TGE client cannot read their records).
pub const ENGINE_CLASSES_GLOBAL: &str = "$Server::EngineClasses";

/// Whether the server says all its clients are the engine's ([`ENGINE_CLASSES_GLOBAL`] true).
pub(super) fn engine_classes_global(vm: &Vm) -> bool {
    vm.global(ENGINE_CLASSES_GLOBAL)
        .is_some_and(|v| atof(&v) != 0.0)
}

/// Tells the layers about the connection just accepted, as the handshake described it, before its first data packet: a request
/// whose manifest's channels the gate matched has its channels' section turned on, and one that ended with the handshake's
/// extension argument is a peer of the engine's own, whose new objects are scoped as they come and whose datablocks go with the
/// codecs' window for it.
pub(super) fn mark_accepted<G: NetGame>(
    sh: &mut NetShared<G>,
    conn: ConnId,
    request: &PendingRequest,
) {
    // The client's channels matched the gate's: it reads the channels' section from the first data packet.
    if sh.hs.connection(conn).is_some_and(|c| c.channels) {
        sh.payload.mark_channels(conn);
    }
    if request.extension {
        sh.payload.mark_peer(conn);
        if let Some(hub) = sh.ghosts.clone() {
            hub.set_fresh(conn, true);
            hub.set_engine_peer(conn);
        }
    }
}

impl<G: GhostGame> GhostHub<G> {
    /// Marks `conn`'s peer as the engine's own client (the local connection, or a client that said so when it connected): the
    /// game may ghost it classes that other clients do not know.
    pub fn set_engine_peer(&self, conn: ConnId) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.engine_peer = true;
        }
    }
}

#[cfg(test)]
#[path = "tge_compat_tests.rs"]
mod tests;
