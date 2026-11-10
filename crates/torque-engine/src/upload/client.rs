//! The client's uploads: what a game calls, and what the tick hook does.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use torque_transfer::{Accept, ClientError, Handle, MAX_META_BYTES, Source, Status, UploadClient};
use torque_ts_vm::vm::Vm;

use super::links::LinkError;
use super::{BeginReply, BeginUpload, ClientLinks, ClientLinksSlot, UploadEnd, UploadRequest};
use crate::rpc::RpcError;

/// Why a client could not begin an upload.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum UploadError {
    /// The client is not connected to a server that takes uploads.
    NotConnected,
    /// The content or the request cannot be uploaded.
    Client(ClientError),
    /// The RPC layer refused the offer.
    Rpc(RpcError),
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadError::NotConnected => f.write_str("You are not connected to a server."),
            UploadError::Client(e) => e.fmt(f),
            UploadError::Rpc(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for UploadError {}

/// The client's uploads: an extension of its script engine, made by the engine's recipes. A game reaches it with [`uploads`].
#[derive(Default)]
pub struct ClientUploads {
    client: RefCell<UploadClient>,
    outbox: RefCell<VecDeque<Vec<u8>>>,
    cancels: RefCell<Vec<u32>>,
    epoch: Cell<Option<u64>>,
}

impl ClientUploads {
    fn on_reply(&self, handle: Handle, result: Result<BeginReply, RpcError>) {
        let mut client = self.client.borrow_mut();
        match result {
            Ok(reply) if reply.accepted => {
                let accept = Accept {
                    id: reply.id,
                    has_list: reply.has_list,
                    have: reply.have,
                };
                match client.accepted(handle, accept) {
                    Ok(messages) => self.outbox.borrow_mut().extend(messages),
                    Err(_) => self.cancels.borrow_mut().push(reply.id),
                }
            }
            Ok(reply) => client.refused(handle, reply.reason),
            Err(RpcError::Refused) => {
                client.refused(handle, "The server did not allow that upload.".to_string());
            }
            Err(RpcError::Timeout) => {
                client.refused(handle, "The server did not answer.".to_string());
            }
            Err(e) => client.refused(handle, e.to_string()),
        }
    }

    pub(super) fn on_request(&self, request: &UploadRequest) {
        let result = self
            .client
            .borrow_mut()
            .request(request.id, request.start, request.count);
        match result {
            Ok(message) => self.outbox.borrow_mut().push_back(message),
            Err(ClientError::Unknown) => {}
            Err(_) => self.cancels.borrow_mut().push(request.id),
        }
    }

    pub(super) fn on_end(&self, end: &UploadEnd) {
        let result = if end.ok {
            Ok(())
        } else {
            Err(end.reason.clone())
        };
        self.client.borrow_mut().ended(end.id, result);
    }

    /// One client tick: uploads of a connection that ended fail, cancels and queued messages go out.
    pub(super) fn pump(&self, links: &dyn ClientLinks) {
        let epoch = links.epoch();
        if self
            .epoch
            .replace(Some(epoch))
            .is_some_and(|last| last != epoch)
        {
            self.client.borrow_mut().disconnected();
            self.outbox.borrow_mut().clear();
            self.cancels.borrow_mut().clear();
        }
        for id in std::mem::take(&mut *self.cancels.borrow_mut()) {
            links.cancel(id);
        }
        let mut outbox = self.outbox.borrow_mut();
        while let Some(message) = outbox.front() {
            match links.send(message.clone()) {
                Err(LinkError::Busy) => break,
                Ok(()) | Err(LinkError::Gone) => {
                    outbox.pop_front();
                }
            }
        }
    }
}

/// A client's way to upload ([`uploads`]).
pub struct Uploads {
    state: Rc<ClientUploads>,
    links: Rc<dyn ClientLinks>,
}

/// The client's uploads, if its engine has any kind registered and a network host set the links.
pub fn uploads(vm: &Vm) -> Option<Uploads> {
    Some(Uploads {
        state: vm.extension::<ClientUploads>()?,
        links: Rc::clone(&vm.extension::<ClientLinksSlot>()?.0),
    })
}

impl Uploads {
    /// Offers `source` to the server as an upload of kind `kind` described by `meta`. The content is read once to hash it (do
    /// it where a pause is fine), and then only as the server asks. The upload goes on in the client's ticks: read its progress
    /// with [`status`](Self::status).
    ///
    /// # Errors
    /// [`UploadError::NotConnected`] when the connection is not up, [`UploadError::Client`] for content that cannot be uploaded
    /// (empty, too large, unreadable), [`UploadError::Rpc`] when the offer could not be sent.
    pub fn begin(
        &self,
        kind: &str,
        meta: Vec<u8>,
        source: Box<dyn Source>,
    ) -> Result<Handle, UploadError> {
        if !self.links.ready() {
            return Err(UploadError::NotConnected);
        }
        if meta.len() > MAX_META_BYTES {
            return Err(UploadError::Client(ClientError::BadRequest(
                "the description is too long",
            )));
        }
        let request = self
            .state
            .client
            .borrow_mut()
            .begin(kind, meta, source)
            .map_err(UploadError::Client)?;
        let handle = request.handle;
        let message = BeginUpload {
            kind: request.kind,
            meta: request.meta,
            size_low: request.size as u32,
            size_high: (request.size >> 32) as u32,
            hash: request.hash,
        };
        let state = Rc::clone(&self.state);
        let sent = self.links.begin(
            message,
            Box::new(move |result| state.on_reply(handle, result)),
        );
        if let Err(e) = sent {
            self.state
                .client
                .borrow_mut()
                .refused(handle, e.to_string());
            return Err(UploadError::Rpc(e));
        }
        Ok(handle)
    }

    /// Where an upload is.
    pub fn status(&self, handle: Handle) -> Option<Status> {
        self.state.client.borrow().status(handle).cloned()
    }

    /// Whether the connection takes uploads now (an offer would be sent).
    pub fn ready(&self) -> bool {
        self.links.ready()
    }

    /// Cancels an upload that has not ended: the server drops what it has of it. The cancel goes ahead of any offer made after it,
    /// so one that follows finds the room free.
    pub fn cancel(&self, handle: Handle) {
        let id = self.state.client.borrow_mut().cancel(handle);
        if let Some(id) = id {
            self.links.cancel(id);
        }
    }
}
