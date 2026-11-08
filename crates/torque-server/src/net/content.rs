//! Content a server sends a client before the client's game starts: the seam between the host and a game's mods.
//!
//! A connection accepted on the bulk channels is held in a downloading state. The host asks the game's [`ContentServer`] to
//! begin (it queues what the client reads first, its list of required mods), sends the accept, and calls the connection's `onConnect`
//! only once the game says the client is ready (it has what the server requires, mounted), so the datablocks and ghosts a script
//! starts sending in `onConnect` never reach a client that has not mounted the content they refer to. While it waits, the
//! connection is in a burst ([`torque_net::protocol::Protocol::set_burst`]): the host sends packets every tick, and the channels
//! take the whole packet. A client that makes no progress for [`ContentLimits::idle_ms`], or does not finish within
//! [`ContentLimits::total_ms`], is dropped; a server already holding [`ContentLimits::max_clients`] such connections refuses a
//! new one with `Server busy, try again.`.

use std::rc::Rc;

use torque_net::handshake::ConnId;
use torque_net::modsync::ModsSection;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::channels::ChannelHub;

/// How much a server holds of its joining clients.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContentLimits {
    /// The most connections held at once (past it a new client is refused with `Server busy, try again.`).
    pub max_clients: usize,
    /// How long a client may take, from the accept, in milliseconds (0: 300,000).
    pub total_ms: u32,
    /// How long a client may go without making progress, in milliseconds (0: 20,000).
    pub idle_ms: u32,
}

impl ContentLimits {
    /// The total time, 0 read as the default.
    pub fn total(&self) -> u32 {
        if self.total_ms == 0 {
            300_000
        } else {
            self.total_ms
        }
    }

    /// The idle time, 0 read as the default.
    pub fn idle(&self) -> u32 {
        if self.idle_ms == 0 {
            20_000
        } else {
            self.idle_ms
        }
    }
}

/// A game's way of sending required content.
pub trait ContentServer {
    /// The limits the host holds clients to.
    fn limits(&self, vm: &Vm) -> ContentLimits;

    /// Whether the server holds the clients it accepts at all: false when it has nothing to require (the client is sent its empty
    /// list and its join goes on at once, with no hold, no burst and no place in the count).
    fn holds(&self, vm: &Vm) -> bool;

    /// Whether the server takes one more held client, with `held` held now.
    fn admit(&self, vm: &Vm, held: usize) -> bool;

    /// The remote command `name` (`serverCmdX`) a held client may run: the content server's own; every other command of a held
    /// client is dropped.
    fn allows_command(&self, name: &[u8]) -> bool;

    /// A client was accepted: queues what it reads first through `hub`. `obj` is the client's connection object, `conn` its
    /// handshake id, `section` what it said about taking content. `Ok(true)` holds the client; `Ok(false)` sent what there is and
    /// holds nothing. An `Err` is the reason the client is dropped.
    fn begin(
        &self,
        vm: &mut Vm,
        hub: &ChannelHub,
        obj: ObjId,
        conn: ConnId,
        section: &ModsSection,
    ) -> Result<bool, String>;

    /// Whether the client has what the server requires (the host then runs `onConnect`).
    fn is_ready(&self, vm: &Vm, obj: ObjId) -> bool;

    /// A count that grows whenever the client does something the server asked of it (a request, the ready word): the host drops a
    /// client whose count stands still.
    fn progress(&self, vm: &Vm, obj: ObjId) -> u64;

    /// The client is gone or released: forget it.
    fn end(&self, vm: &Vm, obj: ObjId);
}

/// The server's script engine's [`ContentServer`], an extension of it.
#[derive(Clone)]
pub struct ContentServerSlot(pub Rc<dyn ContentServer>);
