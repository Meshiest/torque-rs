//! A client and a server of uploads over a link that loses packets: the data goes through the real chunk layer (a `ChunkSender`
//! into an `UpstreamReader` under the server's caps), the server's requests and answers cross a reliable path with a delay, as
//! the engine's RPC lane does. A round is one server tick (32 ms).

#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use torque_net::bits::{BitReader, BitWriter};
use torque_net::chunk::upstream::{UpstreamCaps, UpstreamReader, UpstreamServer};
use torque_net::chunk::{ChannelLimits, ChunkSender, Delivered, Kind};
use torque_net::handshake::ConnId;
use torque_transfer::fuzz::Rng;
use torque_transfer::{
    Accepted, BLOCK_BYTES, ClientMessage, Handle, Hash, MAX_MESSAGE, Offer, Out, Refusal,
    StagingConfig, Status, Subject, UploadClient, UploadServer, UploadSpec, Violation, decode,
};

pub const KIND: &str = "build";
pub const ROUND_MS: u64 = 32;

/// A folder inside the build directory, removed when dropped.
pub struct TempFolder(pub PathBuf);

impl TempFolder {
    pub fn new(name: &str) -> TempFolder {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempFolder(dir)
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn content(len: usize, seed: u64) -> Vec<u8> {
    Rng::new(seed).bytes(len)
}

/// The bytes of block data a message carries (none for a block list).
fn payload(message: &[u8]) -> u64 {
    match decode(message) {
        Ok(ClientMessage::Data { bytes, .. }) => bytes.len() as u64,
        _ => 0,
    }
}

pub fn sha256(bytes: &[u8]) -> Hash {
    Sha256::digest(bytes).into()
}

/// What the server sent the client.
enum Control {
    Request { id: u32, start: u32, count: u32 },
    End { id: u32, result: Result<(), String> },
}

struct Fate {
    arrives_at: u64,
    notify_at: u64,
    seq: u32,
    lost: bool,
    bytes: Option<Vec<u8>>,
}

pub struct Sim {
    pub server: UploadServer,
    pub client: UploadClient,
    pub conn: ConnId,
    pub subject: Subject,
    pub round: u64,
    pub loss: usize,
    rng: Rng,
    limits: Vec<ChannelLimits>,
    upstream: UpstreamServer,
    tx: ChunkSender,
    rx: UpstreamReader,
    fates: Vec<Fate>,
    seq: u32,
    control: VecDeque<(u64, Control)>,
    waiting: VecDeque<Vec<u8>>,
    pub handle: Option<Handle>,
    /// What the client sent in data messages, and what the server took from them.
    pub data_sent: u64,
    pub data_received: u64,
    pub violations: Vec<Violation>,
    pub dropped: bool,
    /// The upload the sink took: its id, size and the hash of the bytes it read.
    pub finished: Vec<(u32, u64, Hash)>,
    pub ended: Vec<Result<(), String>>,
    /// The runs the server asked for, in order.
    pub requests: Vec<(u32, u32, u32)>,
}

impl Sim {
    pub fn new(spec: UploadSpec, staging: StagingConfig, loss: usize, seed: u64) -> Sim {
        let limits = vec![ChannelLimits::new(1_100).with_max_message(MAX_MESSAGE)];
        let caps = UpstreamCaps {
            bytes_per_second: 8 << 20,
            byte_burst: 1 << 20,
            messages_per_second: 1_000,
            message_burst: 1_000,
            ..UpstreamCaps::default()
        };
        let upstream = UpstreamServer::new(caps);
        let conn = ConnId(1);
        Sim {
            server: UploadServer::new(vec![(KIND.to_string(), spec)], staging).unwrap(),
            client: UploadClient::new(),
            conn,
            subject: Subject::Identity(b"player".to_vec()),
            round: 0,
            loss,
            rng: Rng::new(seed),
            tx: ChunkSender::new(&limits),
            rx: upstream.connection(&limits),
            upstream,
            limits,
            fates: Vec::new(),
            seq: 0,
            control: VecDeque::new(),
            waiting: VecDeque::new(),
            handle: None,
            data_sent: 0,
            data_received: 0,
            violations: Vec::new(),
            dropped: false,
            finished: Vec::new(),
            ended: Vec::new(),
            requests: Vec::new(),
        }
    }

    pub fn now(&self) -> u64 {
        self.round * ROUND_MS
    }

    /// Offers `bytes` and sends what an accept lets the client send. Returns what the server said.
    pub fn begin(&mut self, bytes: Vec<u8>) -> Result<Accepted, Refusal> {
        let begin = self
            .client
            .begin(KIND, b"meta".to_vec(), Box::new(bytes))
            .expect("the client takes it");
        let offer = Offer {
            kind: &begin.kind,
            meta: &begin.meta,
            size: begin.size,
            hash: begin.hash,
        };
        let now = self.now();
        match self
            .server
            .begin(now, self.conn, &self.subject, &offer, |_| Ok(()))
        {
            Ok(accepted) => {
                self.handle = Some(begin.handle);
                for message in self.client.accepted(begin.handle, accepted.into()).unwrap() {
                    self.waiting.push_back(message);
                }
                Ok(accepted)
            }
            Err(refusal) => {
                self.handle = Some(begin.handle);
                self.client.refused(begin.handle, refusal.to_string());
                Err(refusal)
            }
        }
    }

    /// One round: the server's answers reach the client, the client's packets cross the link, the server works.
    pub fn step(&mut self) {
        self.round += 1;
        let now = self.now();
        while self
            .control
            .front()
            .is_some_and(|(due, _)| *due <= self.round)
        {
            let Some((_, control)) = self.control.pop_front() else {
                break;
            };
            match control {
                Control::Request { id, start, count } => {
                    if let Ok(message) = self.client.request(id, start, count) {
                        self.data_sent += payload(&message);
                        self.waiting.push_back(message);
                    }
                }
                Control::End { id, result } => {
                    self.ended.push(result.clone());
                    self.client.ended(id, result);
                }
            }
        }
        if !self.dropped {
            self.client_packets(now);
        }
        for out in self.server.poll(now) {
            self.apply(now, out);
        }
    }

    fn client_packets(&mut self, now: u64) {
        while let Some(message) = self.waiting.front() {
            if self.tx.enqueue(0, Kind::Snapshot, message.clone()).is_err() {
                break;
            }
            self.waiting.pop_front();
        }
        for _ in 0..30 {
            if self.fates.len() >= 25 || !self.tx.has_work() {
                break;
            }
            self.seq += 1;
            let mut w = BitWriter::new();
            self.tx.write(&mut w, self.seq, 1_100 * 8);
            let lost = self.rng.chance(self.loss);
            let delay = self.rng.below(2) as u64;
            self.fates.push(Fate {
                arrives_at: self.round + delay,
                notify_at: self.round + delay + 1 + self.rng.below(3) as u64,
                seq: self.seq,
                lost,
                bytes: Some(w.into_bytes()),
            });
        }
        let mut got: Vec<Delivered> = Vec::new();
        let mut i = 0;
        while i < self.fates.len() {
            let fate = &mut self.fates[i];
            if !fate.lost
                && fate.arrives_at <= self.round
                && let Some(bytes) = fate.bytes.take()
            {
                self.rx
                    .read(&mut BitReader::new(&bytes), now as u32, &mut got)
                    .expect("a section the client wrote reads");
            }
            let fate = &self.fates[i];
            if fate.notify_at <= self.round && (fate.lost || fate.bytes.is_none()) {
                let (seq, lost) = (fate.seq, fate.lost);
                self.tx.on_notify(seq, !lost);
                self.fates.swap_remove(i);
            } else {
                i += 1;
            }
        }
        for message in got {
            self.data_received += payload(&message.data);
            let _ = self.inject(&message.data);
        }
    }

    /// Gives the server a message as if it had come off the link, and drops the connection if it was a violation.
    pub fn inject(&mut self, message: &[u8]) -> Result<(), Violation> {
        let now = self.now();
        match self.server.on_message(now, self.conn, message) {
            Ok(None) => Ok(()),
            Ok(Some(out)) => {
                self.apply(now, out);
                Ok(())
            }
            Err(violation) => {
                self.violations.push(violation);
                self.dropped = true;
                self.server.disconnected(now, self.conn);
                Err(violation)
            }
        }
    }

    fn apply(&mut self, now: u64, out: Out) {
        let due = self.round + 2;
        match out {
            Out::Request {
                id, start, count, ..
            } => {
                self.requests.push((id, start, count));
                self.control
                    .push_back((due, Control::Request { id, start, count }));
            }
            Out::Finish { id } => {
                let view = self
                    .server
                    .content(id)
                    .expect("a finished upload has content");
                let mut hasher = Sha256::new();
                let mut buf = vec![0u8; BLOCK_BYTES];
                let mut at = 0;
                while at < view.len() {
                    let n = view.read_at(at, &mut buf).unwrap();
                    assert!(n > 0);
                    hasher.update(&buf[..n]);
                    at += n as u64;
                }
                self.finished
                    .push((id, view.len(), hasher.finalize().into()));
                if let Some(Out::End { id, result, .. }) = self.server.complete(id, Ok(())) {
                    self.control.push_back((due, Control::End { id, result }));
                }
            }
            Out::End { id, result, .. } => {
                self.control.push_back((due, Control::End { id, result }));
            }
            Out::Drop { .. } => {
                self.dropped = true;
                self.server.disconnected(now, self.conn);
            }
            _ => {}
        }
    }

    /// Runs rounds until `done` or `max` rounds pass; returns whether `done` held.
    pub fn run_until(&mut self, max: u64, mut done: impl FnMut(&Sim) -> bool) -> bool {
        for _ in 0..max {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    pub fn status(&self) -> Option<&Status> {
        self.handle.and_then(|h| self.client.status(h))
    }

    /// The connection ends: the client and the server forget it, and the link starts over under a new connection id.
    pub fn reconnect(&mut self) {
        let now = self.now();
        self.client.disconnected();
        self.server.disconnected(now, self.conn);
        self.conn = ConnId(self.conn.0 + 1);
        self.tx = ChunkSender::new(&self.limits);
        self.rx = self.upstream.connection(&self.limits);
        self.fates.clear();
        self.control.clear();
        self.waiting.clear();
        self.dropped = false;
    }
}
