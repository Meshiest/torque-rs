//! What the upload code needs of the network host (two seams), and the server's half of the upload channel.

use std::rc::Rc;

use torque_net::chunk::registry::ChunkChannel;
use torque_net::chunk::{ChannelLimits, Kind};
use torque_net::handshake::ConnId;
use torque_transfer::MAX_MESSAGE;
use torque_ts_vm::vm::Vm;

use super::{BeginReply, BeginUpload, UploadEnd, UploadRequest};
use crate::permissions::PermissionView;
use crate::rpc::RpcError;

/// The bytes of a packet the upload channel's chunks may take.
const CHANNEL_BUDGET: usize = 1_100;
/// The most bytes of one connection's messages the server's upload channel holds between two ticks before it drops that
/// connection. There is no cap on all connections together: the channel layer already bounds what each connection delivers in a
/// tick, and a total that one connection could fill would get the next one blamed.
pub(super) const INBOX_PER_CONNECTION: usize = 2 << 20;

/// Why the network host could not take something now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// Not now: try again next tick.
    Busy,
    /// The connection is gone.
    Gone,
}

/// What the server's host received on the upload channel since the last tick.
#[derive(Debug, Default)]
pub struct UploadInbox {
    /// The messages, in the order they arrived, each with its connection.
    pub messages: Vec<(ConnId, Vec<u8>)>,
    /// The connections that ended.
    pub gone: Vec<ConnId>,
    /// The connections whose messages piled up past the cap: to be dropped.
    pub flooders: Vec<ConnId>,
}

impl UploadInbox {
    /// Adds what `later` holds after what this holds.
    pub fn absorb(&mut self, later: UploadInbox) {
        self.messages.extend(later.messages);
        self.gone.extend(later.gone);
        self.flooders.extend(later.flooders);
    }
}

/// The server's seam to the network host: what [`ServerUploads`](super::ServerUploads) needs of the connections.
pub trait ServerLinks {
    /// Takes what the upload channel received since the last call.
    fn take_inbox(&self) -> UploadInbox;
    /// The chunk bytes `conn` has delivered on the upload channel (and no other) so far that were not sent before (bytes sent again
    /// do not count), a count that only grows: how the server tells a slow connection that is still delivering from one that has stopped, or one
    /// that sends the same chunks over.
    fn received(&self, conn: ConnId) -> u64;
    /// Asks `conn` for a run.
    ///
    /// # Errors
    /// [`LinkError::Busy`] when the connection's send queue or rate cannot take it now.
    fn request(&self, conn: ConnId, request: UploadRequest) -> Result<(), LinkError>;
    /// Tells `conn` an upload ended.
    ///
    /// # Errors
    /// As [`request`](Self::request).
    fn end(&self, conn: ConnId, end: UploadEnd) -> Result<(), LinkError>;
    /// What `conn` holds now.
    fn permissions(&self, conn: ConnId) -> Box<dyn PermissionView + '_>;
    /// Drops `conn` with `reason`.
    fn drop_conn(&self, vm: &mut Vm, conn: ConnId, reason: &[u8]);
}

/// The server's links, as an extension of the server's script engine (the host sets it).
pub struct ServerLinksSlot(pub Rc<dyn ServerLinks>);

/// What runs with the server's answer to an offer.
pub type BeginDone = Box<dyn FnOnce(Result<BeginReply, RpcError>)>;

/// The client's seam to the network host.
pub trait ClientLinks {
    /// Whether the connection's channels are on, so a begin can go.
    fn ready(&self) -> bool;
    /// Counts the connections that started: a new number means the old connection's uploads are over.
    fn epoch(&self) -> u64;
    /// Calls the begin RPC; `done` runs with the reply in a later tick.
    ///
    /// # Errors
    /// The RPC layer's, before anything is sent.
    fn begin(&self, request: BeginUpload, done: BeginDone) -> Result<(), RpcError>;
    /// Tells the server upload `id` is cancelled (best effort).
    fn cancel(&self, id: u32);
    /// Queues a message on the upload channel.
    ///
    /// # Errors
    /// [`LinkError::Busy`] when the channel's backlog is full: try again next tick.
    fn send(&self, message: Vec<u8>) -> Result<(), LinkError>;
}

/// The client's links, as an extension of the client's script engine (the host sets it).
pub struct ClientLinksSlot(pub Rc<dyn ClientLinks>);

/// The server's half of the upload channel: it keeps what clients send until the tick takes it, and which connections ended.
#[derive(Default)]
pub struct UploadChannel {
    inbox: UploadInbox,
    bytes: std::collections::HashMap<ConnId, usize>,
}

impl UploadChannel {
    /// Takes what arrived since the last call.
    pub fn take(&mut self) -> UploadInbox {
        self.bytes.clear();
        std::mem::take(&mut self.inbox)
    }
}

impl ChunkChannel for UploadChannel {
    fn receive_from(&mut self, from: ConnId, _kind: Kind, data: &[u8]) {
        if self.inbox.flooders.contains(&from) {
            return;
        }
        let held = self.bytes.entry(from).or_default();
        if held.saturating_add(data.len()) > INBOX_PER_CONNECTION {
            self.inbox.flooders.push(from);
            return;
        }
        *held += data.len();
        self.inbox.messages.push((from, data.to_vec()));
    }

    fn disconnected(&mut self, conn: ConnId) {
        self.inbox.gone.push(conn);
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// The limits of the channel and its message.
pub(super) fn channel_limits() -> ChannelLimits {
    ChannelLimits::new(CHANNEL_BUDGET).with_max_message(MAX_MESSAGE)
}
