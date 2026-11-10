//! What a client sends: block lists, data, cancels, and the end of its connection.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use torque_net::handshake::ConnId;

use super::session::{Attachment, Phase, Room, Session, Slot};
use super::{Ending, Late, Millis, Out, SLOWEST_LINK, Subject, UploadServer, Violation, millis};
use crate::blocks::{BLOCK_BYTES, BLOCK_HASH_LEN, block_hash};
use crate::message::{self, ClientMessage};

impl UploadServer {
    /// A message a client sent on the upload channel.
    ///
    /// # Errors
    /// A [`Violation`] when the message is not one an honest client sends now. The host drops the connection, and what the
    /// connection was uploading ends with it.
    pub fn on_message(
        &mut self,
        now: Millis,
        conn: ConnId,
        bytes: &[u8],
    ) -> Result<Option<Out>, Violation> {
        let result = match message::decode(bytes) {
            Ok(ClientMessage::BlockList { id, hashes }) => {
                self.on_block_list(now, conn, id, hashes)
            }
            Ok(ClientMessage::Data { id, start, bytes }) => {
                self.on_data(now, conn, id, start, bytes)
            }
            Err(e) => Err(Violation::Malformed(e.0)),
        };
        result.inspect_err(|_| {
            self.counters.violations += 1;
            self.condemn(now, conn);
        })
    }

    /// The host saw `bytes` more of the upload channel's chunks arrive from `conn`, none of them seen before (not what is sent
    /// again, not what came on other channels): the connection's uploads that are waiting for the client live a little longer for
    /// them. The bytes are shared among those uploads, and an upload is given no more than the bytes it waits for (the runs it asked
    /// for, and its block list if that has not come), so the same bytes do not keep several alive and an excess keeps none alive.
    pub fn delivered(&mut self, now: Millis, conn: ConnId, bytes: u64) {
        let waiting = self
            .sessions
            .values()
            .filter(|s| s.connection() == Some(conn) && s.waits_on_client())
            .count() as u64;
        if waiting == 0 || bytes == 0 {
            return;
        }
        let share = bytes.div_ceil(waiting);
        for session in self.sessions.values_mut() {
            if session.connection() != Some(conn) || !session.waits_on_client() {
                continue;
            }
            let credit = share.min(session.wanted_bytes()).saturating_mul(1_000) / SLOWEST_LINK;
            let idle = millis(self.kinds[session.kind].spec.idle);
            let ceiling = now.saturating_add(idle);
            session.alive_until = session.alive_until.saturating_add(credit).min(ceiling);
        }
    }

    /// Whether a closed upload's late message is tolerated (and uses one of its allowances up).
    fn tolerate(&mut self, now: Millis, conn: ConnId, id: u32, late: Late) -> bool {
        let Some(tombs) = self.tombs.get_mut(&conn) else {
            return false;
        };
        tombs.retain(|t| t.until > now);
        let Some(tomb) = tombs.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        match late {
            Late::BlockList if tomb.list => {
                tomb.list = false;
                true
            }
            Late::Run if tomb.runs > 0 => {
                tomb.runs -= 1;
                true
            }
            _ => false,
        }
    }

    fn on_block_list(
        &mut self,
        now: Millis,
        conn: ConnId,
        id: u32,
        hashes: &[u8],
    ) -> Result<Option<Out>, Violation> {
        let Some(session) = owned(&mut self.sessions, conn, id) else {
            return if self.tolerate(now, conn, id, Late::BlockList) {
                Ok(None)
            } else {
                Err(Violation::UnknownUpload)
            };
        };
        if session.list.is_some() || hashes.len() != session.slots.len() * BLOCK_HASH_LEN {
            return Err(Violation::BadBlockList);
        }
        session.list = Some(hashes.as_chunks::<BLOCK_HASH_LEN>().0.to_vec());
        let idle = millis(self.kinds[session.kind].spec.idle);
        session.alive_until = now.saturating_add(idle);
        Ok(None)
    }

    fn on_data(
        &mut self,
        now: Millis,
        conn: ConnId,
        id: u32,
        start: u32,
        bytes: &[u8],
    ) -> Result<Option<Out>, Violation> {
        let Some(session) = owned(&mut self.sessions, conn, id) else {
            return if self.tolerate(now, conn, id, Late::Run) {
                Ok(None)
            } else {
                Err(Violation::UnknownUpload)
            };
        };
        let Some(at) = session.outstanding.iter().position(|&(s, _)| s == start) else {
            return Err(Violation::Unrequested);
        };
        let (_, count) = session.outstanding[at];
        let (kind, size, room) = (session.kind, session.size, session.room);
        if session.run_len(start, count) != bytes.len() as u64 {
            return Err(Violation::WrongLength);
        }
        let Some(list) = &session.list else {
            return Err(Violation::Unrequested);
        };
        for (k, block) in bytes.chunks(BLOCK_BYTES).enumerate() {
            if list.get(start as usize + k) != Some(&block_hash(block)) {
                return Err(Violation::BadBlockHash);
            }
        }
        session.outstanding.remove(at);
        let mut stored = Ok(());
        for (k, block) in bytes.chunks(BLOCK_BYTES).enumerate() {
            let index = start + k as u32;
            stored = session.staged.write_block(index, block);
            if stored.is_err() {
                break;
            }
            session.slots[index as usize] = Slot::Held;
            session.held += 1;
        }
        let idle = millis(self.kinds[kind].spec.idle);
        session.alive_until = now.saturating_add(idle);
        session.alive_before_request = None;
        let complete = session.held == session.blocks();
        if stored.is_err() {
            return Ok(self.end(
                now,
                id,
                &Ending::Told("The server could not store the upload."),
            ));
        }
        if room == Room::Given {
            // It went quiet and gave up its room; it takes it back if nobody else has.
            let cap = self.kinds[kind].spec.server_in_flight;
            if self.kinds[kind].in_flight.saturating_add(size) > cap {
                return Ok(self.end(
                    now,
                    id,
                    &Ending::Told("The server needed the room for other uploads."),
                ));
            }
            self.kinds[kind].in_flight += size;
        }
        if let Some(session) = self.sessions.get_mut(&id) {
            session.room = Room::Held;
            if complete {
                session.phase = Phase::Verifying {
                    next: 0,
                    hasher: Box::new(Sha256::new()),
                };
            }
        }
        Ok(None)
    }

    /// The client cancelled upload `id`. What it has sent is dropped; late data of it is tolerated for a moment.
    pub fn cancel(&mut self, now: Millis, conn: ConnId, id: u32) {
        if self
            .sessions
            .get(&id)
            .is_some_and(|s| s.connection() == Some(conn))
        {
            self.close(now, id);
        }
    }

    /// `conn` ended. Its uploads that are still missing blocks wait for a resume (when the player has an identity); the others
    /// are dropped.
    pub fn disconnected(&mut self, now: Millis, conn: ConnId) {
        self.tombs.remove(&conn);
        self.conn_buckets.retain(|&(c, _), _| c != conn);
        let ids: Vec<u32> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.connection() == Some(conn))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let Some(session) = self.sessions.get_mut(&id) else {
                continue;
            };
            let resumable = matches!(session.phase, Phase::Receiving)
                && matches!(session.subject, Subject::Identity(_));
            if resumable {
                let wait = millis(self.kinds[session.kind].spec.resume);
                session.attachment = Attachment::Waiting {
                    since: now,
                    until: now.saturating_add(wait),
                };
                session.forget_requests();
            } else {
                self.end(now, id, &Ending::Silent);
            }
        }
        // A player with no identity is the connection: its records go with it.
        self.players.remove(&Subject::Connection(conn));
    }
}

/// The session `id` if `conn` owns it and it is receiving.
fn owned(sessions: &mut BTreeMap<u32, Session>, conn: ConnId, id: u32) -> Option<&mut Session> {
    sessions
        .get_mut(&id)
        .filter(|s| s.connection() == Some(conn) && matches!(s.phase, Phase::Receiving))
}
