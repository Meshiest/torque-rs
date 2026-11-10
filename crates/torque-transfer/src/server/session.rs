//! What the server holds for one upload, and for one player.

use std::collections::HashMap;

use sha2::Sha256;
use torque_net::handshake::ConnId;

use super::{Millis, Subject};
use crate::blocks::{BLOCK_HASH_LEN, BlockHash, Hash, MAX_RUN_BLOCKS, run_bytes};

/// The bytes of a block list message besides its hashes.
const LIST_HEADER_BYTES: u64 = 16;
use crate::staging::Staged;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Slot {
    Missing,
    Requested,
    Held,
}

pub(super) enum Phase {
    Receiving,
    /// Hashing the whole content again, `next` blocks done.
    Verifying {
        next: u32,
        hasher: Box<Sha256>,
    },
    /// Handed to the sink; waiting for [`UploadServer::complete`](super::UploadServer::complete).
    Finishing,
}

/// Whether a session has a connection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Attachment {
    Connected(ConnId),
    /// The connection ended at `since`; the session is kept until `until` for a resume.
    Waiting {
        since: Millis,
        until: Millis,
    },
}

/// Whether a session's declared size counts against its kind's `server_in_flight`. A session quiet past its kind's `idle`, or
/// waiting that long for a resume, gives the room up until data comes (or the player resumes it) again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Room {
    Held,
    Given,
}

pub(super) struct Session {
    pub(super) kind: usize,
    pub(super) subject: Subject,
    pub(super) attachment: Attachment,
    pub(super) meta: Vec<u8>,
    pub(super) size: u64,
    pub(super) hash: Hash,
    pub(super) list: Option<Vec<BlockHash>>,
    pub(super) slots: Vec<Slot>,
    pub(super) held: u32,
    pub(super) outstanding: Vec<(u32, u32)>,
    pub(super) phase: Phase,
    pub(super) staged: Staged,
    /// Until when it is alive without a word from its connection while it waits for the client.
    pub(super) alive_until: Millis,
    /// What `alive_until` was before the last run was asked for, so a request the host could not send does not count.
    pub(super) alive_before_request: Option<Millis>,
    pub(super) deadline: Millis,
    /// How many times it was taken up again after its connection ended.
    pub(super) resumes: u32,
    pub(super) room: Room,
}

impl Session {
    pub(super) fn blocks(&self) -> u32 {
        self.slots.len() as u32
    }

    pub(super) fn connection(&self) -> Option<ConnId> {
        match self.attachment {
            Attachment::Connected(conn) => Some(conn),
            Attachment::Waiting { .. } => None,
        }
    }

    pub(super) fn is_connected(&self) -> bool {
        self.connection().is_some()
    }

    /// Whether the session is waiting for the client: for the block list, or for runs it was asked for.
    pub(super) fn waits_on_client(&self) -> bool {
        self.list.is_none() || !self.outstanding.is_empty()
    }

    /// The bytes of the run `start..start + count`.
    pub(super) fn run_len(&self, start: u32, count: u32) -> u64 {
        let (from, to) = run_bytes(self.size, u64::from(start), u64::from(count));
        to - from
    }

    /// The bytes the session waits for from its client: the runs it asked for, and its block list if that has not come.
    pub(super) fn wanted_bytes(&self) -> u64 {
        let runs: u64 = self
            .outstanding
            .iter()
            .map(|&(start, count)| self.run_len(start, count))
            .sum();
        let list = if self.list.is_none() {
            (self.slots.len() * BLOCK_HASH_LEN) as u64 + LIST_HEADER_BYTES
        } else {
            0
        };
        runs + list
    }

    /// The next run of up to [`MAX_RUN_BLOCKS`] blocks nobody has asked for.
    pub(super) fn next_run(&self) -> Option<(u32, u32)> {
        let start = self.slots.iter().position(|&s| s == Slot::Missing)?;
        let count = self.slots[start..]
            .iter()
            .take(MAX_RUN_BLOCKS as usize)
            .take_while(|&&s| s == Slot::Missing)
            .count();
        Some((start as u32, count as u32))
    }

    /// Forgets what was asked for and not yet sent (the connection ended).
    pub(super) fn forget_requests(&mut self) {
        for slot in &mut self.slots {
            if *slot == Slot::Requested {
                *slot = Slot::Missing;
            }
        }
        self.outstanding.clear();
    }
}

/// What the server still lets a connection send after it closed an upload.
pub(super) struct Tomb {
    pub(super) id: u32,
    pub(super) runs: usize,
    pub(super) list: bool,
    pub(super) until: Millis,
}

/// When each upload of a player was accepted, and its size.
pub(super) type Ledger = Vec<(Millis, u64)>;

/// What the server keeps for one player: the quota records of each kind, and the uploads that can be resumed.
pub(super) struct Player {
    pub(super) ledgers: Vec<Ledger>,
    /// (kind, content hash) to the session.
    pub(super) resumable: HashMap<(usize, Hash), u32>,
}

impl Player {
    pub(super) fn new(kinds: usize) -> Player {
        Player {
            ledgers: vec![Ledger::new(); kinds],
            resumable: HashMap::new(),
        }
    }

    /// Drops the quota records older than each kind's window (`window(kind)`, in milliseconds).
    pub(super) fn prune(&mut self, now: Millis, window: impl Fn(usize) -> Millis) {
        for (kind, ledger) in self.ledgers.iter_mut().enumerate() {
            let window = window(kind);
            ledger.retain(|&(at, _)| now.saturating_sub(at) < window);
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.resumable.is_empty() && self.ledgers.iter().all(Vec::is_empty)
    }
}
