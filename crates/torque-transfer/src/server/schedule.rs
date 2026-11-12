//! What the server does each poll: ends uploads that ran out of time or went quiet, hashes finished ones, and asks for runs.

use super::bucket::bucket;
use super::session::{Attachment, Phase, Room, Session, Slot};
use super::{Ending, Millis, Out, UploadServer, millis};
use crate::blocks::{BLOCK_BYTES, Hash, MAX_OUTSTANDING_RUNS};
use crate::staging::UntrustedBytes;
use sha2::Digest;

/// The most blocks of the final hash a session does in one poll.
const VERIFY_BLOCKS_PER_POLL: u32 = 16;

/// How far a session's final hash got in one poll.
enum Step {
    Waiting,
    Unreadable,
    Matched,
    Mismatched,
}

impl UploadServer {
    pub(super) fn expire(&mut self, now: Millis, out: &mut Vec<Out>) {
        let mut ended: Vec<(u32, Ending)> = Vec::new();
        let mut give_up: Vec<u32> = Vec::new();
        for (&id, session) in &self.sessions {
            let spec = &self.kinds[session.kind].spec;
            let idle = millis(spec.idle);
            match session.attachment {
                Attachment::Waiting { since, until } => {
                    if now >= until || now >= session.deadline {
                        ended.push((id, Ending::Silent));
                    } else if session.room == Room::Held && now >= since.saturating_add(idle) {
                        give_up.push(id);
                    }
                }
                Attachment::Connected(_) => {
                    if now >= session.deadline {
                        ended.push((id, Ending::Told("The upload took too long.")));
                    } else if matches!(session.phase, Phase::Receiving) && session.waits_on_client()
                    {
                        if now >= session.alive_until.saturating_add(idle) {
                            ended.push((id, Ending::Told("The upload stalled.")));
                        } else if now >= session.alive_until && session.room == Room::Held {
                            give_up.push(id);
                        }
                    }
                }
            }
        }
        // An upload not heard of for `idle` gives up its room for others to use until data comes again, and goes last in line.
        for id in give_up {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.room = Room::Given;
                let kind = &mut self.kinds[session.kind];
                kind.in_flight = kind.in_flight.saturating_sub(session.size);
            }
        }
        for (id, ending) in ended {
            out.extend(self.end(now, id, &ending));
        }
    }

    /// Hashes the whole content of finished sessions, a few blocks each, and hands the good ones to the sink.
    pub(super) fn verify(&mut self, now: Millis, out: &mut Vec<Out>) {
        let ids: Vec<u32> = self
            .sessions
            .iter()
            .filter(|(_, s)| matches!(s.phase, Phase::Verifying { .. }))
            .map(|(&id, _)| id)
            .collect();
        let mut buf = vec![0u8; BLOCK_BYTES];
        for id in ids {
            let step = self
                .sessions
                .get_mut(&id)
                .map_or(Step::Waiting, |s| verify_step(s, &mut buf));
            match step {
                Step::Waiting => {}
                Step::Unreadable => {
                    let ending = Ending::Told("The server could not read the upload back.");
                    out.extend(self.end(now, id, &ending));
                }
                Step::Matched => {
                    if let Some(session) = self.sessions.get_mut(&id) {
                        session.phase = Phase::Finishing;
                        out.push(Out::Finish { id });
                    }
                }
                Step::Mismatched => {
                    // The content is not what the client declared, though every block matched its list: the client lied.
                    self.counters.violations += 1;
                    let dropped = self.end(now, id, &Ending::DropConnection);
                    if let Some(Out::Drop { conn }) = &dropped {
                        self.condemn(now, *conn);
                    }
                    out.extend(dropped);
                }
            }
        }
    }

    /// Asks for runs, round robin among sessions, within the buckets.
    pub(super) fn schedule(&mut self, now: Millis, out: &mut Vec<Out>) {
        for kind in &mut self.kinds {
            kind.bucket.refill(now);
        }
        for bucket in self.conn_buckets.values_mut() {
            bucket.refill(now);
        }
        let ids = self.grant_order();
        // One run for each session in turn, until nobody can be given another: sessions share the buckets evenly.
        loop {
            let before = out.len();
            for &id in &ids {
                out.extend(self.grant(id, now));
            }
            if out.len() == before {
                break;
            }
        }
    }

    /// The sessions in the order they are offered runs: round robin from the last one served, with those that gave up their room
    /// after all the others.
    pub(super) fn grant_order(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .sessions
            .range(self.cursor.saturating_add(1)..)
            .chain(self.sessions.range(..=self.cursor))
            .map(|(&id, _)| id)
            .collect();
        ids.sort_by_key(|id| self.sessions.get(id).is_some_and(|s| s.room == Room::Given));
        ids
    }

    /// Asks for one more run of session `id` if it may have one: it is attached, has its block list, has fewer than
    /// [`MAX_OUTSTANDING_RUNS`] runs out, and both buckets have the bytes.
    fn grant(&mut self, id: u32, now: Millis) -> Option<Out> {
        let session = self.sessions.get_mut(&id)?;
        let conn = session.connection()?;
        if !matches!(session.phase, Phase::Receiving)
            || session.list.is_none()
            || session.outstanding.len() >= MAX_OUTSTANDING_RUNS
        {
            return None;
        }
        let (start, count) = session.next_run()?;
        let cost = session.run_len(start, count);
        let kind = &mut self.kinds[session.kind];
        let rate = u64::from(kind.spec.rate);
        let conn_bucket = self
            .conn_buckets
            .entry((conn, session.kind))
            .or_insert_with(|| bucket(rate, now));
        if !conn_bucket.has(cost) || !kind.bucket.has(cost) {
            return None;
        }
        if session.outstanding.is_empty() {
            // The wait for the first run was the server's pacing, not the client's silence; the host may yet fail to send the
            // request, and then the clock goes back.
            session.alive_before_request = Some(session.alive_until);
            session.alive_until = now.saturating_add(millis(kind.spec.idle));
        }
        conn_bucket.spend(cost);
        kind.bucket.spend(cost);
        for slot in &mut session.slots[start as usize..(start + count) as usize] {
            *slot = Slot::Requested;
        }
        session.outstanding.push((start, count));
        self.cursor = id;
        Some(Out::Request {
            conn,
            id,
            start,
            count,
        })
    }

    /// The host could not send a request ([`Out::Request`]): forget it, so the blocks are asked for again. The upload's quiet
    /// time and the buckets are as they were before it.
    pub fn retract(&mut self, id: u32, start: u32, count: u32) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        let before = session.outstanding.len();
        session.outstanding.retain(|&run| run != (start, count));
        if session.outstanding.len() == before {
            return;
        }
        for slot in session
            .slots
            .iter_mut()
            .skip(start as usize)
            .take(count as usize)
        {
            *slot = Slot::Missing;
        }
        if session.outstanding.is_empty()
            && let Some(before) = session.alive_before_request.take()
        {
            session.alive_until = before;
        }
        let cost = session.run_len(start, count);
        let kind = session.kind;
        if let Some(conn) = session.connection()
            && let Some(bucket) = self.conn_buckets.get_mut(&(conn, kind))
        {
            bucket.refund(cost);
        }
        self.kinds[kind].bucket.refund(cost);
    }
}

/// Hashes up to [`VERIFY_BLOCKS_PER_POLL`] more blocks of a verifying session.
fn verify_step(session: &mut Session, buf: &mut [u8]) -> Step {
    let Phase::Verifying { next, hasher } = &mut session.phase else {
        return Step::Waiting;
    };
    let blocks = session.slots.len() as u32;
    let view = session.staged.view();
    for _ in 0..VERIFY_BLOCKS_PER_POLL {
        if *next >= blocks {
            break;
        }
        let (from, to) = crate::blocks::run_bytes(session.size, u64::from(*next), 1);
        let part = &mut buf[..(to - from) as usize];
        if read_exact_at(&view, from, part).is_err() {
            return Step::Unreadable;
        }
        hasher.update(&*part);
        *next += 1;
    }
    if *next < blocks {
        return Step::Waiting;
    }
    let digest: Hash = std::mem::take(hasher.as_mut()).finalize().into();
    if digest == session.hash {
        Step::Matched
    } else {
        Step::Mismatched
    }
}

fn read_exact_at(view: &UntrustedBytes<'_>, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let n = view.read_at(offset + done as u64, &mut buf[done..])?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        done += n;
    }
    Ok(())
}
