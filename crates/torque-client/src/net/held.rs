//! Datagrams a page hands the client: a browser's link the page holds, the host player's own (from the server in the
//! page's worker) or a joiner's data channel to a player's host. They are hostile as on UDP: [`HeldDatagrams`] refuses
//! one over [`NET_MAX_DATAGRAM`] bytes and holds at most [`MAX_INBOX`] datagrams and [`MAX_HELD_BYTES`] bytes until the
//! client reads them.

use std::collections::VecDeque;

use net::cap::NET_MAX_DATAGRAM;

use super::MAX_INBOX;

/// The most bytes held in all.
pub const MAX_HELD_BYTES: usize = 1 << 20;

/// Datagrams waiting for the client (module docs).
#[derive(Debug, Default)]
pub struct HeldDatagrams {
    held: VecDeque<Vec<u8>>,
    bytes: usize,
    /// Datagrams refused: too long, or with the queue full.
    pub dropped: u64,
}

impl HeldDatagrams {
    /// Holds `data`; false (dropped and counted) when it is over [`NET_MAX_DATAGRAM`] bytes or the queue is full.
    pub fn push(&mut self, data: &[u8]) -> bool {
        if data.len() > NET_MAX_DATAGRAM
            || self.held.len() >= MAX_INBOX
            || self.bytes + data.len() > MAX_HELD_BYTES
        {
            self.dropped += 1;
            return false;
        }
        self.bytes += data.len();
        self.held.push_back(data.to_vec());
        true
    }

    /// The oldest datagram held.
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let data = self.held.pop_front()?;
        self.bytes -= data.len();
        Some(data)
    }

    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
#[path = "held_tests.rs"]
mod tests;
