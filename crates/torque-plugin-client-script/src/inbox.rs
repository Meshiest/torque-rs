//! What a client has been sent for its realm and has not handed over yet. An extension of the client's script engine, filled by
//! the handlers of the built-in RPCs and emptied by [`Realms`](crate::Realms) each frame.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use torque_client_script_format::Value;

use crate::messages::{MAX_INBOX, OfferedMod};

/// The server's values and its offer, waiting for the realm. What it holds is of one connection (`ClientRpc::epoch`): what
/// arrives on another connection first empties it, and a take for any other connection gets nothing.
#[derive(Default)]
pub struct ScriptInbox {
    epoch: Cell<u64>,
    values: RefCell<VecDeque<Value>>,
    offer: RefCell<Option<Vec<OfferedMod>>>,
    dropped: Cell<u64>,
    refused: Cell<u64>,
}

impl ScriptInbox {
    /// Queues a value; past [`MAX_INBOX`] the oldest is dropped (and counted), so a flood costs the realm its oldest messages
    /// and the client no more memory.
    pub fn push(&self, epoch: u64, value: Value) {
        self.enter(epoch);
        let mut values = self.values.borrow_mut();
        if values.len() >= MAX_INBOX {
            values.pop_front();
            self.dropped.set(self.dropped.get().saturating_add(1));
        }
        values.push_back(value);
    }

    /// Notes a message that was refused (bytes that are no value, a length past the cap).
    pub fn refuse(&self) {
        self.refused.set(self.refused.get().saturating_add(1));
    }

    /// Keeps the server's offer; a second one on the same connection is ignored.
    pub fn offer(&self, epoch: u64, mods: Vec<OfferedMod>) {
        self.enter(epoch);
        let mut held = self.offer.borrow_mut();
        if held.is_none() {
            *held = Some(mods);
        }
    }

    /// Takes the queued values of connection `epoch`, oldest first; those of another are dropped.
    pub fn take_values(&self, epoch: u64) -> Vec<Value> {
        let values: Vec<Value> = self.values.borrow_mut().drain(..).collect();
        if epoch == self.epoch.get() {
            values
        } else {
            Vec::new()
        }
    }

    /// Takes the offer of connection `epoch`, if one came; an offer of another is dropped.
    pub fn take_offer(&self, epoch: u64) -> Option<Vec<OfferedMod>> {
        let offer = self.offer.borrow_mut().take();
        offer.filter(|_| epoch == self.epoch.get())
    }

    /// Makes `epoch` the connection the inbox holds for, emptying it when that is a new one.
    fn enter(&self, epoch: u64) {
        if epoch != self.epoch.get() {
            self.epoch.set(epoch);
            self.clear();
        }
    }

    /// Forgets everything held (a connection ended).
    pub fn clear(&self) {
        self.values.borrow_mut().clear();
        self.offer.borrow_mut().take();
    }

    /// How many values were dropped for want of room.
    pub fn dropped(&self) -> u64 {
        self.dropped.get()
    }

    /// How many messages were refused.
    pub fn refused(&self) -> u64 {
        self.refused.get()
    }
}
