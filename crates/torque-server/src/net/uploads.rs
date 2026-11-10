//! The network host's side of client-to-server uploads: the links `torque-engine`'s uploads need (the RPCs, the upload channel and
//! the connections), over a server's [`ServerRpc`] and [`ChannelHub`] and a client's [`ClientRpc`] and [`ClientChannels`].
//!
//! A host that has both sets them as extensions of the script engine ([`install_server`], [`install_client`]); the engine's own
//! tick hooks then do the rest. Nothing here decides anything: the rules are `torque-transfer`'s and the engine's.

use std::rc::Rc;

use torque_engine::permissions::PermissionView;
use torque_engine::rpc::RpcError;
use torque_engine::upload::{
    BEGIN_RPC, BeginDone, BeginReply, BeginUpload, CANCEL_RPC, CHANNEL, CancelUpload, ClientLinks,
    ClientLinksSlot, END_RPC, LinkError, REQUEST_RPC, ServerLinks, ServerLinksSlot, UploadChannel,
    UploadEnd, UploadInbox, UploadRequest,
};
use torque_net::chunk::{Kind, SendError};
use torque_net::handshake::ConnId;
use torque_ts_vm::vm::Vm;

use super::channels::{ChannelHub, ClientChannels};
use super::rpc::{ClientRpc, ServerRpc};

/// A server's links: its RPC pump's connections and its channel hub.
pub struct ServerSideLinks {
    rpc: Rc<ServerRpc>,
    hub: ChannelHub,
}

/// Sets `vm`'s uploads links over `rpc` and `hub`.
pub fn install_server(vm: &mut Vm, rpc: &Rc<ServerRpc>, hub: &ChannelHub) {
    let links = ServerSideLinks {
        rpc: Rc::clone(rpc),
        hub: hub.clone(),
    };
    vm.set_extension(Rc::new(ServerLinksSlot(Rc::new(links))));
}

fn server_error(error: RpcError) -> LinkError {
    match error {
        RpcError::Busy | RpcError::RateLimited => LinkError::Busy,
        _ => LinkError::Gone,
    }
}

impl ServerSideLinks {
    fn notify<M: torque_engine::NetMessage>(
        &self,
        conn: ConnId,
        name: &str,
        message: M,
    ) -> Result<(), LinkError> {
        self.rpc
            .with_link(conn, |link| {
                let rpc = link.rpc::<M, ()>(name)?;
                link.notify(&rpc, message)
            })
            // A connection whose link is not made yet: the next tick.
            .unwrap_or(Err(RpcError::Busy))
            .map_err(server_error)
    }
}

impl ServerLinks for ServerSideLinks {
    fn take_inbox(&self) -> UploadInbox {
        self.hub
            .with_up_channel::<UploadChannel, _>(CHANNEL, UploadChannel::take)
            .unwrap_or_default()
    }

    fn received(&self, conn: ConnId) -> u64 {
        self.hub.upstream_fresh(conn, CHANNEL)
    }

    fn request(&self, conn: ConnId, request: UploadRequest) -> Result<(), LinkError> {
        self.notify(conn, REQUEST_RPC, request)
    }

    fn end(&self, conn: ConnId, end: UploadEnd) -> Result<(), LinkError> {
        self.notify(conn, END_RPC, end)
    }

    fn permissions(&self, conn: ConnId) -> Box<dyn PermissionView + '_> {
        Box::new(self.rpc.permissions_of(conn))
    }

    fn drop_conn(&self, vm: &mut Vm, conn: ConnId, reason: &[u8]) {
        self.rpc.drop_conn(vm, conn, reason);
    }
}

/// A client's links: its RPC link and its channels.
pub struct ClientSideLinks {
    rpc: Rc<ClientRpc>,
    channels: Rc<ClientChannels>,
}

/// Sets `vm`'s uploads links over `rpc` and `channels`.
pub fn install_client(vm: &mut Vm, rpc: &Rc<ClientRpc>, channels: &Rc<ClientChannels>) {
    let links = ClientSideLinks {
        rpc: Rc::clone(rpc),
        channels: Rc::clone(channels),
    };
    vm.set_extension(Rc::new(ClientLinksSlot(Rc::new(links))));
}

impl ClientLinks for ClientSideLinks {
    fn ready(&self) -> bool {
        self.channels.is_active() && self.rpc.is_ready()
    }

    fn epoch(&self) -> u64 {
        self.channels.epoch()
    }

    fn begin(&self, request: BeginUpload, done: BeginDone) -> Result<(), RpcError> {
        self.rpc
            .with_link(|link| {
                let rpc = link.rpc::<BeginUpload, BeginReply>(BEGIN_RPC)?;
                link.call(&rpc, request, done)
            })
            .unwrap_or(Err(RpcError::Disconnected))
    }

    fn cancel(&self, id: u32) {
        let _ = self.rpc.with_link(|link| {
            let rpc = link.rpc::<CancelUpload, ()>(CANCEL_RPC)?;
            link.notify(&rpc, CancelUpload { id })
        });
    }

    fn send(&self, message: Vec<u8>) -> Result<(), LinkError> {
        match self.channels.send(CHANNEL, Kind::Snapshot, message) {
            Ok(()) => Ok(()),
            Err(SendError::Backlog) => Err(LinkError::Busy),
            Err(_) => Err(LinkError::Gone),
        }
    }
}
