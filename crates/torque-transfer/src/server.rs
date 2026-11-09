//! The server's half: policy, scheduling, staging and resume ([`UploadServer`]).
//!
//! The server is a state machine with no sockets and no clock: the host passes the time (milliseconds, from any monotonic
//! source), hands over what clients send, and moves what comes out ([`Out`]). Every number a client sends is checked before it
//! is used, and a client that sends what an honest one cannot (data nobody asked for, a block whose hash does not match, a
//! malformed message) gets a [`Violation`] that the host answers by dropping the connection; the uploads of that connection end
//! with it and are not kept for a resume.
//!
//! **A session** is one accepted upload. It reserves its declared size against the kind's `server_in_flight` and the staging
//! pool, and takes one of the player's quota entries, when it is accepted. The server then *pulls*: [`UploadServer::poll`]
//! requests runs of at most [`MAX_RUN_BLOCKS`](crate::MAX_RUN_BLOCKS) blocks, at most
//! [`MAX_OUTSTANDING_RUNS`](crate::MAX_OUTSTANDING_RUNS) at a time, paced by a byte bucket per connection (`rate`) and one per kind
//! (`server_rate`). When every block is in, the whole content is hashed again, a little each poll, and only a content whose hash is
//! the declared one is handed to the sink ([`Out::Finish`]).
//!
//! **Quiet uploads.** An upload that waits for the client (for its block list, or for runs it asked for) is alive while bytes
//! keep coming: the host reports what the connection delivered ([`UploadServer::delivered`]), and each byte buys the time a link
//! of [`SLOWEST_LINK`] bytes a second needs for it, up to `idle` ahead. An upload alive until some time and not heard of since gives
//! up its room against `server_in_flight` (and goes last in line for runs); a further `idle` later it ends. An end leaves a
//! tomb, so data of the ended upload already on its way is ignored, not a violation.
//!
//! **Resume.** A session whose connection ends while blocks are still missing is kept for `resume`, holding its staged bytes
//! and its quota (but not its room once it has waited `idle`). A begin of the same content by the same player (auth identity; a
//! player with none resumes only within the same connection) re-attaches it, and only the blocks it lacks are requested again. A
//! session that still has a connection is not resumed. A resume never moves the deadline (`total` counts from the first accept)
//! and an upload is resumed at most [`MAX_RESUMES`] times. A waiting session counts toward the player's `concurrent` uploads of
//! its kind and toward the staging's per-player bytes; a new upload that would pass either **evicts the player's oldest waiting
//! sessions** (never one with a connection) and is refused only if that is not enough.

mod admit;
mod bucket;
mod receive;
mod schedule;
mod session;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::time::Duration;

use torque_net::handshake::ConnId;

use crate::blocks::Hash;
use crate::spec::{MAX_KIND_NAME, UploadSpec};
use crate::staging::{Pool, StagingConfig, UntrustedBytes};
use bucket::Bucket;
use session::{Phase, Player, Room, Session, Tomb};

/// Milliseconds on a clock the host keeps (it only has to go forward).
pub type Millis = u64;

/// The most sessions a server holds at once, whatever the kinds allow.
pub const MAX_SESSIONS: usize = 256;

/// The most players (auth identities, and connections with no identity) a server keeps quota records for.
pub const MAX_PLAYERS: usize = 16_384;

/// How many times one upload may be resumed. The deadline is set when it is first accepted and a resume does not move it; this
/// bounds the work of a client that disconnects and resumes in a loop.
pub const MAX_RESUMES: u32 = 8;

/// The slowest rate, in bytes a second, at which an honest connection is taken to deliver: what each byte it delivers buys in time
/// toward staying alive.
pub const SLOWEST_LINK: u64 = 1_024;

/// How many requests of a closed upload a client may still answer: its data may be in flight when the server closes it.
const MAX_TOMBS_PER_CONN: usize = 8;

/// How long a closed upload's late data is tolerated.
const TOMB_MS: Millis = 30_000;

/// Who an upload is counted for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Subject {
    /// The auth identity the server verified.
    Identity(Vec<u8>),
    /// A connection, for a player with no identity: it resumes only within that connection.
    Connection(ConnId),
}

/// What a client offers when it begins an upload.
#[derive(Clone, Copy, Debug)]
pub struct Offer<'a> {
    /// The kind's name.
    pub kind: &'a str,
    /// The metadata that describes the upload (the sink reads it).
    pub meta: &'a [u8],
    /// The content's declared size.
    pub size: u64,
    /// The content's declared SHA-256.
    pub hash: Hash,
}

/// The server's accept of an offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    /// The upload's id (what every later message names).
    pub id: u32,
    /// Whether the server holds the block list already, so the client does not send it.
    pub has_list: bool,
    /// The blocks the server holds.
    pub have: u32,
    /// Whether this resumed an earlier upload.
    pub resumed: bool,
}

/// What a client did that an honest client does not. The host drops the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Violation {
    /// A message that is not well formed.
    Malformed(&'static str),
    /// A message about an upload the connection does not have.
    UnknownUpload,
    /// Data nobody asked for.
    Unrequested,
    /// A run of the wrong size.
    WrongLength,
    /// A block whose hash is not the one in the block list.
    BadBlockHash,
    /// A second block list, or a block list of the wrong length.
    BadBlockList,
    /// A whole content that does not hash to what was declared.
    BadContent,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::Malformed(why) => write!(f, "malformed upload message ({why})"),
            Violation::UnknownUpload => f.write_str("message about an upload it does not have"),
            Violation::Unrequested => f.write_str("data that was not requested"),
            Violation::WrongLength => f.write_str("a run of the wrong size"),
            Violation::BadBlockHash => f.write_str("a block that does not match its hash"),
            Violation::BadBlockList => f.write_str("a bad block list"),
            Violation::BadContent => f.write_str("content that does not match its hash"),
        }
    }
}

impl std::error::Error for Violation {}

/// What the server wants done.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Out {
    /// Ask `conn` for blocks `start..start + count` of upload `id`.
    Request {
        /// Where to send it.
        conn: ConnId,
        /// The upload.
        id: u32,
        /// The first block.
        start: u32,
        /// How many.
        count: u32,
    },
    /// Every block is in and the content's hash matched: give the sink [`UploadServer::content`] and then call
    /// [`UploadServer::complete`] with its answer.
    Finish {
        /// The upload.
        id: u32,
    },
    /// Tell `conn` the upload ended: `Ok` once the sink took it, else the reason.
    End {
        /// Where to send it.
        conn: ConnId,
        /// The upload.
        id: u32,
        /// How it ended.
        result: Result<(), String>,
    },
    /// Drop `conn`: it lied about the content it sent.
    Drop {
        /// The connection.
        conn: ConnId,
    },
}

/// What a server counted since it started, for scripts and logs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Uploads accepted (resumes included).
    pub accepted: u64,
    /// Offers refused.
    pub refused: u64,
    /// Uploads resumed.
    pub resumed: u64,
    /// Uploads the sink took.
    pub completed: u64,
    /// Uploads ended before the sink took them (timeouts, the sink's refusal, a lost disk).
    pub aborted: u64,
    /// Clients that did what an honest client does not.
    pub violations: u64,
}

struct Kind {
    name: String,
    spec: UploadSpec,
    /// Declared bytes of this kind's sessions that hold their room.
    in_flight: u64,
    bucket: Bucket,
}

/// How a session ends.
enum Ending {
    /// The client is told why, and a tomb keeps its data still on the way from being a violation.
    Told(&'static str),
    /// The connection is dropped.
    DropConnection,
    /// Nobody is told: the session waited for a resume that did not come, or gave way to another.
    Silent,
}

/// Which late message of a closed upload is meant.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Late {
    BlockList,
    Run,
}

/// Everything a server holds for uploads. See the [module](self).
pub struct UploadServer {
    kinds: Vec<Kind>,
    pool: Pool,
    sessions: BTreeMap<u32, Session>,
    players: HashMap<Subject, Player>,
    conn_buckets: HashMap<(ConnId, usize), Bucket>,
    tombs: HashMap<ConnId, Vec<Tomb>>,
    next_id: u32,
    cursor: u32,
    counters: Counters,
}

impl UploadServer {
    /// A server for the kinds `kinds` names, staging under `staging`.
    ///
    /// # Errors
    /// The first kind whose spec is not valid or whose name is not 1 to 40 bytes or is used twice, with the reason.
    pub fn new(
        kinds: Vec<(String, UploadSpec)>,
        staging: StagingConfig,
    ) -> Result<UploadServer, String> {
        let mut made: Vec<Kind> = Vec::with_capacity(kinds.len());
        for (name, spec) in kinds {
            if name.is_empty() || name.len() > MAX_KIND_NAME {
                return Err(format!("upload {name:?}: the name is 1 to 40 bytes"));
            }
            if made.iter().any(|k| k.name == name) {
                return Err(format!("upload {name}: two uploads have this name"));
            }
            spec.validate().map_err(|e| format!("upload {name}: {e}"))?;
            made.push(Kind {
                bucket: Bucket::new(spec.server_rate, 0),
                in_flight: 0,
                name,
                spec,
            });
        }
        Ok(UploadServer {
            kinds: made,
            pool: Pool::new(staging),
            sessions: BTreeMap::new(),
            players: HashMap::new(),
            conn_buckets: HashMap::new(),
            tombs: HashMap::new(),
            next_id: 1,
            cursor: 0,
            counters: Counters::default(),
        })
    }

    /// The spec of kind `name`.
    pub fn spec(&self, name: &str) -> Option<&UploadSpec> {
        self.kinds.iter().find(|k| k.name == name).map(|k| &k.spec)
    }

    /// What was counted since the server started.
    pub fn counters(&self) -> Counters {
        self.counters
    }

    /// Why the staging folder was not used, if it was not.
    pub fn staging_warning(&self) -> Option<&str> {
        self.pool.warning.as_deref()
    }

    /// The bytes staged in memory now.
    pub fn staged_memory(&self) -> usize {
        self.pool.memory_used()
    }

    /// The bytes staged on disk now.
    pub fn staged_disk(&self) -> u64 {
        self.pool.disk_used()
    }

    /// The staging caps.
    pub fn staging(&self) -> &StagingConfig {
        self.pool.config()
    }

    /// The declared bytes of kind `name`'s sessions that hold their room.
    pub fn in_flight(&self, name: &str) -> u64 {
        self.kinds
            .iter()
            .find(|k| k.name == name)
            .map_or(0, |k| k.in_flight)
    }

    /// How many sessions there are, waiting to be resumed or not.
    pub fn sessions(&self) -> usize {
        self.sessions.len()
    }

    /// How many players the server keeps quota records or resumable uploads for.
    pub fn players(&self) -> usize {
        self.players.len()
    }

    /// The connections that have an upload waiting for them.
    pub fn connections(&self) -> Vec<ConnId> {
        let mut conns: Vec<ConnId> = self
            .sessions
            .values()
            .filter_map(Session::connection)
            .collect();
        conns.sort_unstable();
        conns.dedup();
        conns
    }

    /// How many uploads of kind `name` `subject` has going on a connection.
    pub fn active(&self, subject: &Subject, name: &str) -> usize {
        self.kind_index(name)
            .map_or(0, |kind| self.active_count(subject, kind))
    }

    /// The declared bytes of all of `subject`'s uploads, going or waiting to be resumed.
    pub fn staged_for(&self, subject: &Subject) -> u64 {
        self.sessions
            .values()
            .filter(|s| &s.subject == subject)
            .map(|s| s.size)
            .sum()
    }

    fn kind_index(&self, name: &str) -> Option<usize> {
        self.kinds.iter().position(|k| k.name == name)
    }

    fn active_count(&self, subject: &Subject, kind: usize) -> usize {
        self.sessions
            .values()
            .filter(|s| s.kind == kind && s.is_connected() && &s.subject == subject)
            .count()
    }

    fn free_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            if !self.sessions.contains_key(&id) {
                return id;
            }
        }
    }

    /// The player `subject`'s records, made when there are none.
    fn player(&mut self, subject: &Subject) -> &mut Player {
        if !self.players.contains_key(subject) {
            self.players
                .insert(subject.clone(), Player::new(self.kinds.len()));
        }
        self.players
            .get_mut(subject)
            .expect("the player was just made")
    }

    /// Takes the session `id` out, and releases what it held. Its quota entry stays for the window.
    fn remove(&mut self, id: u32) -> Option<Session> {
        let session = self.sessions.remove(&id)?;
        if let Some(player) = self.players.get_mut(&session.subject) {
            player.resumable.remove(&(session.kind, session.hash));
        }
        if session.room == Room::Held {
            let kind = &mut self.kinds[session.kind];
            kind.in_flight = kind.in_flight.saturating_sub(session.size);
        }
        Some(session)
    }

    /// Ends the session `id`, releases its staging, and returns what to tell its client.
    fn end(&mut self, now: Millis, id: u32, ending: &Ending) -> Option<Out> {
        let session = self.remove(id)?;
        self.counters.aborted += 1;
        let connected = session.connection();
        let out = match (ending, connected) {
            (Ending::Told(reason), Some(conn)) => {
                self.leave_tomb(now, conn, id, &session);
                Some(Out::End {
                    conn,
                    id,
                    result: Err((*reason).to_string()),
                })
            }
            (Ending::DropConnection, Some(conn)) => Some(Out::Drop { conn }),
            _ => None,
        };
        self.pool.release(session.staged);
        out
    }

    /// Removes the session `id` the client cancelled and releases its staging, leaving a tomb.
    fn close(&mut self, now: Millis, id: u32) {
        let Some(session) = self.remove(id) else {
            return;
        };
        if let Some(conn) = session.connection() {
            self.leave_tomb(now, conn, id, &session);
        }
        self.pool.release(session.staged);
    }

    /// What `conn` may still send of a closed upload: the runs it was asked for and the block list it had not sent.
    fn leave_tomb(&mut self, now: Millis, conn: ConnId, id: u32, session: &Session) {
        let tombs = self.tombs.entry(conn).or_default();
        tombs.retain(|t| t.until > now);
        if tombs.len() >= MAX_TOMBS_PER_CONN {
            tombs.remove(0);
        }
        tombs.push(Tomb {
            id,
            runs: session.outstanding.len(),
            list: session.list.is_none(),
            until: now.saturating_add(TOMB_MS),
        });
    }

    /// Ends every upload `conn` has, for good: it broke the rules, so what it sent is not kept for a resume.
    fn condemn(&mut self, now: Millis, conn: ConnId) {
        let ids: Vec<u32> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.connection() == Some(conn))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.end(now, id, &Ending::Silent);
        }
    }

    /// The finished content of upload `id`, once [`Out::Finish`] named it.
    pub fn content(&self, id: u32) -> Option<UntrustedBytes<'_>> {
        let session = self.sessions.get(&id)?;
        matches!(session.phase, Phase::Finishing).then(|| session.staged.view())
    }

    /// The metadata of upload `id`.
    pub fn meta(&self, id: u32) -> Option<&[u8]> {
        self.sessions.get(&id).map(|s| s.meta.as_slice())
    }

    /// The connection upload `id` belongs to, and the player it counts for.
    pub fn owner(&self, id: u32) -> Option<(ConnId, &Subject)> {
        let session = self.sessions.get(&id)?;
        Some((session.connection()?, &session.subject))
    }

    /// The name of the kind of upload `id`.
    pub fn kind_of(&self, id: u32) -> Option<&str> {
        self.sessions
            .get(&id)
            .map(|s| self.kinds[s.kind].name.as_str())
    }

    /// The sink's answer for upload `id` (after [`Out::Finish`]): the content is dropped either way. Returns the message for the
    /// client.
    pub fn complete(&mut self, id: u32, result: Result<(), String>) -> Option<Out> {
        if !self
            .sessions
            .get(&id)
            .is_some_and(|s| matches!(s.phase, Phase::Finishing))
        {
            return None;
        }
        let session = self.remove(id)?;
        if result.is_ok() {
            self.counters.completed += 1;
        } else {
            self.counters.aborted += 1;
        }
        let conn = session.connection();
        self.pool.release(session.staged);
        Some(Out::End {
            conn: conn?,
            id,
            result,
        })
    }

    /// Works for time `now`: ends uploads that ran out of time or went quiet, hashes finished ones, and asks for more blocks.
    pub fn poll(&mut self, now: Millis) -> Vec<Out> {
        let mut out = Vec::new();
        self.expire(now, &mut out);
        self.verify(now, &mut out);
        self.schedule(now, &mut out);
        let kinds = &self.kinds;
        self.players.retain(|_, player| {
            player.prune(now, |kind| millis(kinds[kind].spec.quota.per));
            !player.is_empty()
        });
        self.tombs.retain(|_, tombs| {
            tombs.retain(|t| t.until > now);
            !tombs.is_empty()
        });
        out
    }
}

fn millis(d: Duration) -> Millis {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
