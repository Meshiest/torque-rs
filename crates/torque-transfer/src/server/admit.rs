//! Taking an offer: the checks, the sink's rules, what is evicted to make room, and a resume.

use torque_net::handshake::ConnId;

use super::session::{Attachment, Phase, Room, Session, Slot};
use super::{Accepted, Ending, MAX_PLAYERS, MAX_RESUMES, MAX_SESSIONS, Millis, Offer, Subject};
use super::{UploadServer, millis};
use crate::blocks::block_count;
use crate::spec::Refusal;

impl UploadServer {
    /// A client offers an upload. `admit` is the kind's own rules (permission, naming) given the metadata: `Err` refuses with its
    /// reason. It runs after every limit has been checked and before anything is evicted or held for the offer.
    ///
    /// # Errors
    /// The [`Refusal`] to send the client. Nothing is held for a refused offer.
    pub fn begin(
        &mut self,
        now: Millis,
        conn: ConnId,
        subject: &Subject,
        offer: &Offer<'_>,
        admit: impl FnOnce(&[u8]) -> Result<(), String>,
    ) -> Result<Accepted, Refusal> {
        let result = self.try_begin(now, conn, subject, offer, admit);
        match &result {
            Ok(a) => {
                self.counters.accepted += 1;
                self.counters.resumed += u64::from(a.resumed);
            }
            Err(_) => self.counters.refused += 1,
        }
        result
    }

    fn try_begin(
        &mut self,
        now: Millis,
        conn: ConnId,
        subject: &Subject,
        offer: &Offer<'_>,
        admit: impl FnOnce(&[u8]) -> Result<(), String>,
    ) -> Result<Accepted, Refusal> {
        let kind = self.kind_index(offer.kind).ok_or(Refusal::UnknownKind)?;
        let spec = &self.kinds[kind].spec;
        let (max_bytes, meta_max, concurrent, in_flight_cap) = (
            spec.max_bytes,
            spec.meta_max,
            spec.concurrent,
            spec.server_in_flight,
        );
        if offer.size == 0 {
            return Err(Refusal::Empty);
        }
        if offer.size > max_bytes {
            return Err(Refusal::TooLarge {
                size: offer.size,
                max: max_bytes,
            });
        }
        if offer.meta.len() > meta_max {
            return Err(Refusal::MetaTooLarge {
                len: offer.meta.len(),
                max: meta_max,
            });
        }
        let known = self
            .players
            .get(subject)
            .and_then(|p| p.resumable.get(&(kind, offer.hash)))
            .copied();
        if let Some(id) = known {
            // An upload past its deadline is over, whether or not a poll has ended it yet; the offer then starts afresh.
            if self.sessions.get(&id).is_some_and(|s| now >= s.deadline) {
                self.end(now, id, &Ending::Told("The upload took too long."));
            } else {
                return self.resume(now, conn, id, offer, admit);
            }
        }
        self.check_quota(now, subject, kind, offer.size)?;
        if self.active_count(subject, kind) >= concurrent as usize {
            return Err(Refusal::Concurrent { max: concurrent });
        }
        let evict = self.evictions_for(subject, kind, concurrent, offer.size)?;
        if self.kinds[kind].in_flight.saturating_add(offer.size) > in_flight_cap {
            return Err(Refusal::ServerFull);
        }
        if self.sessions.len() >= MAX_SESSIONS
            || (!self.players.contains_key(subject) && self.players.len() >= MAX_PLAYERS)
        {
            return Err(Refusal::ServerBusy);
        }
        // The kind's own rules come before anything is given up or held for the offer.
        admit(offer.meta).map_err(Refusal::Rejected)?;
        // What had to give way goes now: only waiting uploads, the oldest first.
        for id in evict {
            self.end(now, id, &Ending::Silent);
        }
        let staged = self.pool.open(offer.size)?;
        let id = self.free_id();
        let blocks = block_count(offer.size) as usize;
        let spec = &self.kinds[kind].spec;
        let deadline = now.saturating_add(millis(spec.total));
        let alive_until = now.saturating_add(millis(spec.idle));
        self.kinds[kind].in_flight += offer.size;
        let player = self.player(subject);
        player.ledgers[kind].push((now, offer.size));
        player.resumable.insert((kind, offer.hash), id);
        self.sessions.insert(
            id,
            Session {
                kind,
                subject: subject.clone(),
                attachment: Attachment::Connected(conn),
                meta: offer.meta.to_vec(),
                size: offer.size,
                hash: offer.hash,
                list: None,
                slots: vec![Slot::Missing; blocks],
                held: 0,
                outstanding: Vec::new(),
                phase: Phase::Receiving,
                staged,
                alive_until,
                alive_before_request: None,
                deadline,
                resumes: 0,
                room: Room::Held,
            },
        );
        Ok(Accepted {
            id,
            has_list: false,
            have: 0,
            resumed: false,
        })
    }

    /// Re-attaches the waiting session `id` to `conn`.
    fn resume(
        &mut self,
        now: Millis,
        conn: ConnId,
        id: u32,
        offer: &Offer<'_>,
        admit: impl FnOnce(&[u8]) -> Result<(), String>,
    ) -> Result<Accepted, Refusal> {
        let Some(session) = self.sessions.get(&id) else {
            return Err(Refusal::Invalid("a lost upload"));
        };
        if session.size != offer.size {
            return Err(Refusal::Invalid("another size for the same content"));
        }
        if !matches!(session.phase, Phase::Receiving) {
            return Err(Refusal::Invalid("that upload is already complete"));
        }
        // What its connection has in flight would find the session changed under it, so an upload that has a connection is
        // not resumed: the player begins it again once the server has seen that connection end.
        if session.is_connected() {
            return Err(Refusal::Invalid("that upload is already going"));
        }
        let (kind, size) = (session.kind, session.size);
        if session.resumes >= MAX_RESUMES {
            self.end(now, id, &Ending::Silent);
            return Err(Refusal::Invalid(
                "that upload was resumed too many times; start it again",
            ));
        }
        let spec = &self.kinds[kind].spec;
        let (max, idle, in_flight_cap) =
            (spec.concurrent, millis(spec.idle), spec.server_in_flight);
        if self.active_count(&session.subject, kind) >= max as usize {
            return Err(Refusal::Concurrent { max });
        }
        let given = session.room == Room::Given;
        if given && self.kinds[kind].in_flight.saturating_add(size) > in_flight_cap {
            return Err(Refusal::ServerFull);
        }
        admit(offer.meta).map_err(Refusal::Rejected)?;
        let Some(session) = self.sessions.get_mut(&id) else {
            return Err(Refusal::Invalid("a lost upload"));
        };
        session.forget_requests();
        session.attachment = Attachment::Connected(conn);
        session.meta = offer.meta.to_vec();
        session.alive_until = now.saturating_add(idle);
        session.alive_before_request = None;
        session.resumes += 1;
        session.room = Room::Held;
        let accepted = Accepted {
            id,
            has_list: session.list.is_some(),
            have: session.held,
            resumed: true,
        };
        if given {
            self.kinds[kind].in_flight += size;
        }
        Ok(accepted)
    }

    /// The waiting uploads of `subject` a new upload of `size` bytes of kind `kind` must evict to fit: those that hold the player
    /// over `concurrent` uploads of the kind (they count whether or not their connection is there) and over the staging's
    /// per-player bytes, the oldest first. A refusal, with nothing evicted, when evicting all of them would not be enough.
    fn evictions_for(
        &self,
        subject: &Subject,
        kind: usize,
        concurrent: u32,
        size: u64,
    ) -> Result<Vec<u32>, Refusal> {
        let mut waiting: Vec<(Millis, u32, usize, u64)> = self
            .sessions
            .iter()
            .filter(|(_, s)| &s.subject == subject)
            .filter_map(|(&id, s)| match s.attachment {
                Attachment::Waiting { since, .. } => Some((since, id, s.kind, s.size)),
                Attachment::Connected(_) => None,
            })
            .collect();
        waiting.sort_unstable();
        let mine = || self.sessions.values().filter(|s| &s.subject == subject);
        let mut held: u64 = mine().map(|s| s.size).sum();
        let mut of_kind = mine().filter(|s| s.kind == kind).count();
        let limit = self.pool.config().per_subject;
        let mut evict = Vec::new();
        while of_kind >= concurrent as usize || held.saturating_add(size) > limit {
            let over_count = of_kind >= concurrent as usize;
            // Only a waiting upload of the same kind helps with the count; any helps with the bytes.
            let next = waiting
                .iter()
                .position(|&(_, _, k, _)| !over_count || k == kind);
            let Some((_, id, k, bytes)) = next.map(|at| waiting.remove(at)) else {
                return Err(if over_count {
                    Refusal::Concurrent { max: concurrent }
                } else {
                    Refusal::HoldingTooMuch { limit }
                });
            };
            evict.push(id);
            held -= bytes;
            of_kind -= usize::from(k == kind);
        }
        Ok(evict)
    }

    /// The player's quota records in the window, dropping the older ones; `Err` if `size` more would pass the quota.
    fn check_quota(
        &mut self,
        now: Millis,
        subject: &Subject,
        kind: usize,
        size: u64,
    ) -> Result<(), Refusal> {
        let quota = self.kinds[kind].spec.quota;
        let window = millis(quota.per);
        let Some(ledger) = self
            .players
            .get_mut(subject)
            .map(|player| &mut player.ledgers[kind])
        else {
            return if size > quota.bytes {
                Err(Refusal::QuotaBytes {
                    limit: quota.bytes,
                    left: quota.bytes,
                    per: quota.per,
                })
            } else {
                Ok(())
            };
        };
        ledger.retain(|&(at, _)| now.saturating_sub(at) < window);
        let used: u64 = ledger.iter().map(|&(_, bytes)| bytes).sum();
        if ledger.len() >= quota.files as usize {
            return Err(Refusal::QuotaFiles {
                limit: quota.files,
                per: quota.per,
            });
        }
        if used.saturating_add(size) > quota.bytes {
            return Err(Refusal::QuotaBytes {
                limit: quota.bytes,
                left: quota.bytes.saturating_sub(used),
                per: quota.per,
            });
        }
        Ok(())
    }
}
